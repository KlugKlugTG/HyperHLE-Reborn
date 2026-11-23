/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The implementation of layer compositing.
//!
//! This is completely original; I don't think Apple document how this works and
//! I haven't attempted to reverse-engineer the details. As such, it probably
//! diverges wildly from what the real iPhone OS does.
#![allow(clippy::zero_ptr)] // alas, as you know, opengl

use super::ca_eagl_layer::find_fullscreen_eagl_layer;
use super::ca_layer::CALayerHostObject;
use crate::frameworks::core_animation::animation;
use crate::frameworks::core_graphics::cg_color::CGColorHostObject;
use crate::frameworks::core_graphics::{cg_bitmap_context, cg_image, CGFloat, CGRect, CGSize};
use crate::gles::gles11_raw as gles11; // constants only
use crate::gles::gles11_raw::types::*;
use crate::gles::present::{present_frame, FpsCounter};
use crate::gles::GLES; // constants only
use crate::image::Image;
use crate::matrix::Matrix;
use crate::mem::SafeWrite;
use crate::objc::{id, msg, msg_class, nil, ObjC};
use crate::Environment;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct State {
    /// (texture, framebuffer, width, height). Keeping the dimensions here is
    /// important because Android can recreate/resize the SDL surface while the
    /// compositor state survives; reusing the old-sized texture otherwise
    /// makes the compositor sample undefined texels (often a black screen).
    texture_framebuffer: Option<(GLuint, GLuint, u32, u32)>,
    recomposite_next: Option<Instant>,
    fps_counter: Option<FpsCounter>,
    misc_gl_objects: Option<MiscGlObjects>,
}

struct MiscGlObjects {
    /// Texture containing a single rounded corner.
    rounded_corner_texture: GLuint,
    /// [BASIC_SQUARE_POINTS], used as both vertex and texture co-ords for
    /// drawing simple textured quads.
    basic_square_buffer: GLuint,
    /// [FLIPPED_SQUARE_POINTS], used as texture co-ords for some textured
    /// quads.
    flipped_square_buffer: GLuint,
    /// 9-patch rounded corner texture co-ords (always the same).
    rounded_vertex_buffer: GLuint,
    /// 9-patch rounded corner vertex co-ords (varies with ratio of corner
    /// radius to overall rectangle size).
    rounded_tex_coord_buffer: GLuint,
    /// Index buffer for 9-patch (first 6 elements can be used for square).
    index_buffer: GLuint,
}

unsafe fn load_matrix(gles: &mut dyn GLES, matrix: Matrix<4>) {
    gles.LoadMatrixf(matrix.columns().as_ptr() as *const _);
}

// Core Animation presents at refresh boundaries and drops missed frames.
// https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreAnimation_guide/Introduction/Introduction.html
const COMPOSITION_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 60);

fn composition_deadline(previous: Option<Instant>, finished: Instant) -> Instant {
    let Some(previous) = previous else {
        return finished + COMPOSITION_INTERVAL;
    };
    if previous > finished {
        return previous;
    }
    let intervals =
        finished.duration_since(previous).as_nanos() / COMPOSITION_INTERVAL.as_nanos() + 1;
    u32::try_from(intervals)
        .ok()
        .and_then(|n| COMPOSITION_INTERVAL.checked_mul(n))
        .and_then(|advance| previous.checked_add(advance))
        .unwrap_or(finished + COMPOSITION_INTERVAL)
}

#[cfg(test)]
#[test]
fn composition_drops_missed_refreshes_without_immediate_redraw() {
    let now = Instant::now();
    assert_eq!(composition_deadline(None, now), now + COMPOSITION_INTERVAL);
    let due = now + COMPOSITION_INTERVAL;
    assert_eq!(composition_deadline(Some(due), now), due);
    assert_eq!(
        composition_deadline(Some(due), due),
        due + COMPOSITION_INTERVAL
    );
    let finished = due + COMPOSITION_INTERVAL * 4 + Duration::from_millis(2);
    let next = composition_deadline(Some(due), finished);
    assert_eq!(next, due + COMPOSITION_INTERVAL * 5);
    assert!(next > finished);
    assert!(next.duration_since(finished) <= COMPOSITION_INTERVAL);
}

/// For use by `NSRunLoop`: call this 60 times per second. Composites the app's
/// visible layers (i.e. UI) and presents it to the screen. Does nothing if
/// composition isn't in use or it's too soon (the latter check is skipped if
/// `force` is set to [true]).
///
/// Returns the time a recomposite is due, if any.
pub fn recomposite_if_necessary(env: &mut Environment, force: bool) -> Option<Instant> {
    // Compositing requires a window/render target. In headless mode there is
    // none, so there is nothing to composite; skip rather than touching the
    // absent window further down.
    if env.window.is_none() {
        return None;
    }

    // Run loops may poll much faster than the display refresh rate. Avoid
    // guest isHidden messages, image animation updates and a layer-tree
    // search when this iteration cannot present anything yet. Forced draws
    // still process changes immediately.
    // https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreAnimation_guide/Introduction/Introduction.html
    if !force {
        if let Some(next) = env
            .framework_state
            .core_animation
            .composition
            .recomposite_next
        {
            if next > Instant::now() {
                return Some(next);
            }
        }
    }

    let mut animation_state = animation::State::default();
    let window_count = env.framework_state.uikit.ui_view.ui_window.windows.len();
    let mut has_visible_window = false;
    for index in 0..window_count {
        let window = env.framework_state.uikit.ui_view.ui_window.windows[index];
        if !msg![env; window isHidden] {
            has_visible_window = true;
            break;
        }
    }
    if !has_visible_window {
        log_dbg!("No visible windows, skipping composition");
        return None;
    }

    // Advance any UIImageView frame animations before compositing.
    crate::frameworks::uikit::ui_view::ui_image_view::update_animations(env);

    let fullscreen_eagl_layer = find_fullscreen_eagl_layer(env);
    if fullscreen_eagl_layer != nil {
        let has_presented = {
            let host_obj = env.objc.borrow::<CALayerHostObject>(fullscreen_eagl_layer);
            host_obj.presented_pixels.is_some() || host_obj.presented_directly
        };
        if has_presented && !force && !crate::frameworks::media_player::has_active_video(env) {
            // EAGLContext normally presents the fullscreen drawable directly.
            // Native ES1 readback and movie overlays are the exceptions that
            // need the Core Animation compositor.
            //
            // The fast path is only valid once the EAGL layer has actually
            // presented at least one frame. Some apps (e.g. Doodle Jump) sit
            // on a UIKit screen — menus, cross-promo/"prize" popups — while the
            // fullscreen EAGL layer stays idle and never presents. In that
            // state the fast path would skip composition and leave the screen
            // blank, so fall through to the Core Animation compositor and draw
            // the UIKit view/layer tree instead. Once the app starts driving
            // GL (presentRenderbuffer sets presented_pixels on the readback
            // path, or presented_directly on the GPU path), the fast path
            // takes over again. Checking only presented_pixels made the
            // compositor keep swapping its own frame (with a black EAGL
            // layer) in between every GPU-presented game frame, which shows
            // up as constant black flicker.
            log_dbg!("Using CAEAGLLayer fast path, skipping composition");
            // Recheck UIKit overlays at the display rate instead of walking
            // the entire layer tree on every run-loop poll during gameplay.
            let next = Instant::now() + Duration::from_secs_f64(1.0 / 60.0);
            env.framework_state
                .core_animation
                .composition
                .recomposite_next = Some(next);
            return Some(next);
        }
    }

    if env.options.print_fps
        || cfg!(target_os = "ios")
        || crate::gles::present::onscreen_fps_enabled()
    {
        env.framework_state
            .core_animation
            .composition
            .fps_counter
            .get_or_insert_with(FpsCounter::start)
            .count_frame(
                format_args!("Core Animation compositor"),
                env.options.print_fps,
            );
    }

    let windows = env.framework_state.uikit.ui_view.ui_window.windows.clone();
    let window_layers: Vec<id> = windows
        .into_iter()
        .map(|window| {
            let layer: id = msg![env; window layer];
            // Ensure layer bitmaps are up to date.
            display_layers(env, layer);
            layer
        })
        .collect();

    let screen_bounds: CGRect = {
        let screen: id = msg_class![env; UIScreen mainScreen];
        msg![env; screen bounds]
    };
    let scale_hack: u32 = env.options.scale_hack.get();
    let ui_scale: u32 = env.options.ui_scale.get();
    let fb_width = screen_bounds.size.width as u32 * scale_hack * ui_scale;
    let fb_height = screen_bounds.size.height as u32 * scale_hack * ui_scale;
    let present_frame_args = (
        env.window().viewport(),
        env.window().rotation_matrix(),
        env.window().virtual_cursor_visible_at(),
        env.window().perf_context_token(),
    );
    let host_framebuffer = env.window().host_framebuffer();

    // TODO: draw status bar if it's not hidden

    // Initial state for layer tree traversal (see composite_layer_recursive)
    let cumulative_transform = Matrix::<4>::identity();
    let opacity = 1.0;

    let window = env.window.as_mut().unwrap();
    let mut gles = window.make_internal_gl_ctx_current();

    // The compositor shares the GL context with the guest app, but on real
    // iOS Core Animation composites on its own render queue: the app's GL
    // state (buffer bindings in particular) is never disturbed. The guest
    // relies on that — cocos2d-style engines bind a vertex/index VBO once
    // during initialisation and then pass pointer *offsets* every frame
    // without re-binding. Saving and restoring the bindings here keeps that
    // assumption valid, and keeps the guest-visible GLShadowState mirror
    // (see eagl::GLShadowState) consistent with the driver: leaving both
    // bindings at 0 (the old behaviour) made the next gl*Pointer(offset)
    // call be classified as a client pointer while the app meant it as an
    // offset into its still-bound VBO — sprite geometry then read garbage
    // and 2D games rendered only their clear colour.
    let mut saved_array_buffer: GLint = 0;
    let mut saved_element_array_buffer: GLint = 0;
    unsafe {
        gles.GetIntegerv(gles11::ARRAY_BUFFER_BINDING, &mut saved_array_buffer);
        // Swallow errors: some strict drivers reject individual binding
        // queries, and a wrong "0" only degrades to the old behaviour.
        let _ = gles.GetError();
        gles.GetIntegerv(
            gles11::ELEMENT_ARRAY_BUFFER_BINDING,
            &mut saved_element_array_buffer,
        );
        let _ = gles.GetError();
    }

    // On iOS, SDL2 draws into a framebuffer object of its own rather than 0,
    // and it recreates that object whenever the window leaves and re-enters
    // fullscreen — which is exactly what rotating the device does (see
    // `Window::rotate_device`). The value cached at window creation can
    // therefore be stale by now, and compositing into a stale framebuffer
    // produces a black screen. Ask for the current one while it is still bound,
    // before the render-to-texture setup below replaces the binding.
    let host_framebuffer = if cfg!(target_os = "ios") {
        let mut current = 0;
        unsafe {
            gles.GetIntegerv(gles11::FRAMEBUFFER_BINDING_OES, &mut current);
        }
        let current = current as u32;
        if current != host_framebuffer {
            log!(
                "Note: the host framebuffer changed since window creation ({} -> {}); \
                 compositing into the current one.",
                host_framebuffer,
                current
            );
        }
        current
    } else {
        host_framebuffer
    };

    // Set up GL objects needed for render-to-texture. We could draw directly
    // to the screen instead, but this way we can reuse the code for scaling and
    // rotating the screen and drawing the virtual cursor.
    let cached_texture_framebuffer = env
        .framework_state
        .core_animation
        .composition
        .texture_framebuffer;
    let cached_target_was_resized = cached_texture_framebuffer
        .map(|(_, _, old_width, old_height)| (old_width, old_height) != (fb_width, fb_height))
        .unwrap_or(false);
    let texture = if let Some((texture, framebuffer, old_width, old_height)) =
        cached_texture_framebuffer
    {
        unsafe {
            gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, framebuffer);
            if (old_width, old_height) != (fb_width, fb_height) {
                // Reallocate the compositor target when the Android surface or
                // scale changes. Sampling an old-sized texture is undefined on
                // strict native GLES1 drivers and commonly presents as black.
                gles.BindTexture(gles11::TEXTURE_2D, texture);
                gles.TexImage2D(
                    gles11::TEXTURE_2D,
                    0,
                    gles11::RGBA as _,
                    fb_width as _,
                    fb_height as _,
                    0,
                    gles11::RGBA,
                    gles11::UNSIGNED_BYTE,
                    std::ptr::null(),
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_WRAP_S,
                    gles11::CLAMP_TO_EDGE as _,
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_WRAP_T,
                    gles11::CLAMP_TO_EDGE as _,
                );
            }
        };
        texture
    } else {
        let mut texture = 0;
        let mut framebuffer = 0;
        unsafe {
            gles.GenTextures(1, &mut texture);
            gles.BindTexture(gles11::TEXTURE_2D, texture);
            gles.TexImage2D(
                gles11::TEXTURE_2D,
                0,
                gles11::RGBA as _,
                fb_width as _,
                fb_height as _,
                0,
                gles11::RGBA,
                gles11::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MIN_FILTER,
                gles11::LINEAR as _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MAG_FILTER,
                gles11::LINEAR as _,
            );
            // This texture is the compositor's final frame. It is frequently
            // NPOT on phones, and GL_REPEAT makes strict GLES1 drivers mark it
            // incomplete and sample black.
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_WRAP_S,
                gles11::CLAMP_TO_EDGE as _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_WRAP_T,
                gles11::CLAMP_TO_EDGE as _,
            );

            gles.GenFramebuffersOES(1, &mut framebuffer);
            gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, framebuffer);
            gles.FramebufferTexture2DOES(
                gles11::FRAMEBUFFER_OES,
                gles11::COLOR_ATTACHMENT0_OES,
                gles11::TEXTURE_2D,
                texture,
                0,
            );

            // ХАК: Убраны вызовы assert_eq!, которые убивали приложение
            // при ошибках GL (типа GL_OUT_OF_MEMORY = 1285)
            let _ = gles.GetError(); // Просто сбрасываем флаг текущей ошибки, чтобы он не висел
            let status = gles.CheckFramebufferStatusOES(gles11::FRAMEBUFFER_OES);
            if status != gles11::FRAMEBUFFER_COMPLETE_OES {
                log!(
                    "Warning: Core Animation compositor framebuffer is incomplete: {status:#x} ({fb_width}x{fb_height})"
                );
            }
        }
        env.framework_state
            .core_animation
            .composition
            .texture_framebuffer = Some((texture, framebuffer, fb_width, fb_height));
        texture
    };
    if cached_target_was_resized {
        env.framework_state
            .core_animation
            .composition
            .texture_framebuffer = Some((
            texture,
            cached_texture_framebuffer.unwrap().1,
            fb_width,
            fb_height,
        ));
    }

    // Set up various other GL objects that will be reused on every frame.
    let misc_gl_objects = env
        .framework_state
        .core_animation
        .composition
        .misc_gl_objects
        .get_or_insert_with(|| {
            let dimension = 512usize; // way larger than any reasonable corner
            let mut image = Image::from_pixel_vec(
                vec![255u8; dimension * dimension * 4],
                (dimension as _, dimension as _),
            );
            image.round_corners(dimension as _, /* four_corners: */ false, /* add_sheen: */ false);

            let mut rounded_corner_texture = 0;
            unsafe {
                gles.GenTextures(1, &mut rounded_corner_texture);
                gles.BindTexture(gles11::TEXTURE_2D, rounded_corner_texture);
                // GENERATE_MIPMAP must be set before the texture upload.
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::GENERATE_MIPMAP,
                    gles11::TRUE as _,
                );
                upload_rgba8_pixels(
                    gles.as_mut(),
                    image.pixels(),
                    (dimension as _, dimension as _),
                    None,
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_MIN_FILTER,
                    gles11::LINEAR_MIPMAP_LINEAR as _,
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_WRAP_S,
                    gles11::CLAMP_TO_EDGE as _,
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_WRAP_T,
                    gles11::CLAMP_TO_EDGE as _,
                );
            }

            let [basic_square_buffer, flipped_square_buffer, rounded_vertex_buffer, rounded_tex_coord_buffer, index_buffer] = unsafe {
                let mut array_buffers = [0; 5];
                gles.GenBuffers(5, array_buffers.as_mut_ptr());
                array_buffers
            };
            unsafe {
                gles.BindBuffer(gles11::ARRAY_BUFFER, basic_square_buffer);
                upload_slice(gles.as_mut(), gles11::ARRAY_BUFFER, &BASIC_SQUARE_POINTS, gles11::STATIC_DRAW);
                gles.BindBuffer(gles11::ARRAY_BUFFER, flipped_square_buffer);
                upload_slice(gles.as_mut(), gles11::ARRAY_BUFFER, &FLIPPED_SQUARE_POINTS, gles11::STATIC_DRAW);
                gles.BindBuffer(gles11::ARRAY_BUFFER, rounded_vertex_buffer);
                upload_slice(gles.as_mut(), gles11::ARRAY_BUFFER, &[0f32; FLOATS_PER_9PATCH], gles11::DYNAMIC_DRAW);
                gles.BindBuffer(gles11::ARRAY_BUFFER, rounded_tex_coord_buffer);
                upload_slice(
                    gles.as_mut(),
                    gles11::ARRAY_BUFFER,
                    &make_9patch_coords([0.0, 1.0, 1.0, 0.0], [0.0, 1.0, 1.0, 0.0]),
                    gles11::STATIC_DRAW,
                );
                // Prevent accidental subsequent use.
                gles.BindBuffer(gles11::ARRAY_BUFFER, 0);

                gles.BindBuffer(gles11::ELEMENT_ARRAY_BUFFER, index_buffer);
                upload_slice(gles.as_mut(), gles11::ELEMENT_ARRAY_BUFFER, &make_9patch_indices(), gles11::STATIC_DRAW);
                // Prevent accidental subsequent use.
                gles.BindBuffer(gles11::ELEMENT_ARRAY_BUFFER, 0);
            }

            MiscGlObjects {
                rounded_corner_texture,
                basic_square_buffer,
                flipped_square_buffer,
                rounded_vertex_buffer,
                rounded_tex_coord_buffer,
                index_buffer,
            }
        });

    // Clear the framebuffer and set up state to prepare for rendering
    unsafe {
        gles.Viewport(0, 0, fb_width as _, fb_height as _);
        // The compositor owns this internal context, but its state persists
        // across frames. Reset the tests/masks that can make every fragment
        // fail or every color channel unwritable after a previous layer or
        // presentation pass. Native Adreno GLES1 is particularly strict here.
        gles.Disable(gles11::DEPTH_TEST);
        gles.Disable(gles11::STENCIL_TEST);
        gles.Disable(gles11::SCISSOR_TEST);
        gles.Disable(gles11::CULL_FACE);
        gles.ColorMask(gles11::TRUE, gles11::TRUE, gles11::TRUE, gles11::TRUE);
        gles.DepthMask(gles11::TRUE);
        gles.StencilMask(!0);
        gles.ClearColor(0.0, 0.0, 0.0, 1.0);
        gles.Clear(gles11::COLOR_BUFFER_BIT);
        gles.Color4f(1.0, 1.0, 1.0, 1.0);

        gles.MatrixMode(gles11::PROJECTION);
        // Scale down screen-space to normalized device co-ordinates, shift the
        // origin to be at the top-left rather than the center, and flip the
        // Y axis (OpenGL's points up, Core Animation's points down).
        // Using the projection matrix for this is more convenient than adding
        // an extra multiply to composite_layer_recursive.
        load_matrix(
            gles.as_mut(),
            Matrix::from(&Matrix::scale_2d(
                2.0 / screen_bounds.size.width,
                -2.0 / screen_bounds.size.height,
            ))
            .multiply(&Matrix::translate_3d(-1.0, 1.0, 0.0)),
        );
        gles.MatrixMode(gles11::MODELVIEW);
        gles.LoadIdentity();

        // One index buffer to rule them all
        gles.BindBuffer(gles11::ELEMENT_ARRAY_BUFFER, misc_gl_objects.index_buffer);
    }
    std::mem::drop(gles);

    if crate::env_flag_cached!("TOUCHHLE_DUMP_LAYER_TREE") {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        if N.fetch_add(1, Ordering::Relaxed) % 600 == 0 {
            log!(
                "COMPOSITE-GEOM screen={:?} framebuffer={}x{} viewport={:?} orientation={:?}",
                screen_bounds,
                fb_width,
                fb_height,
                env.window().viewport(),
                env.window().current_rotation()
            );
            fn dump(env: &Environment, layer: id, depth: usize) {
                let h = env.objc.borrow::<CALayerHostObject>(layer);
                let delegate: id = h.delegate_for_debug();
                let cls = |o: id| -> String {
                    if o == nil {
                        "nil".into()
                    } else {
                        let c = crate::objc::ObjC::read_isa(o, &env.mem);
                        env.objc.get_class_name(c).to_string()
                    }
                };
                log!(
                    "LAYER {:indent$}{} ({:?}) delegate={} bounds={:?} pos={:?} transform={:?} hidden={} opaque={} opacity={} bg={:?} contents={} pixels={} ctx={}",
                    "",
                    cls(layer),
                    layer,
                    cls(delegate),
                    h.bounds,
                    h.position,
                    h.affine_transform,
                    h.hidden,
                    h.opaque,
                    h.opacity,
                    h.background_color.map(|c| (c.r, c.g, c.b, c.a)),
                    h.contents != nil,
                    h.presented_pixels.as_ref().map(|p| (p.1, p.2)).is_some(),
                    h.cg_context.is_some(),
                    indent = depth * 2
                );
                for &s in &h.sublayers {
                    dump(env, s, depth + 1);
                }
            }
            for &root in &window_layers {
                dump(env, root, 0);
            }
            for (i, t) in env.threads.iter().enumerate() {
                log!("THREAD {} current={} {:?}", i, i == env.current_thread, t);
            }
        }
    }

    // Assumes the windows in the list are ordered back-to-front.
    // TODO: this may not be correct once we support windowLevel.
    for root_layer in window_layers {
        // Here's where the actual drawing happens
        unsafe {
            composite_layer_recursive(
                env,
                &mut animation_state,
                root_layer,
                cumulative_transform,
                opacity,
            );
        }
    }

    // Re-borrow
    let window = env.window.as_mut().unwrap();
    let mut gles = window.make_internal_gl_ctx_current();

    // Clean up some GL state
    unsafe {
        gles.Viewport(0, 0, fb_width as _, fb_height as _);
        gles.Color4f(1.0, 1.0, 1.0, 1.0);
        gles.Disable(gles11::BLEND);
        gles.MatrixMode(gles11::PROJECTION);
        gles.LoadIdentity();
        gles.MatrixMode(gles11::MODELVIEW);
        gles.LoadIdentity();
        gles.BindBuffer(gles11::ARRAY_BUFFER, 0);
        gles.BindBuffer(gles11::ELEMENT_ARRAY_BUFFER, 0);

        // ХАК: Снова убираем assert_eq!(gles.GetError(), 0);
        // Мы прощаем OpenGL за потерю фокуса!
        let _ = gles.GetError();
    }

    // Present our rendered frame (bound to TEXTURE_2D). This copies it to the
    // host window framebuffer, so we need to unbind our internal framebuffer.
    unsafe {
        gles.BindTexture(gles11::TEXTURE_2D, texture);
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, host_framebuffer);
        present_frame(
            gles.as_mut(),
            present_frame_args.0,
            present_frame_args.1,
            present_frame_args.2,
            present_frame_args.3,
        );
        // Hand the context back to the guest with the buffer bindings it
        // left behind (see the save at the top of this function).
        gles.BindBuffer(gles11::ARRAY_BUFFER, saved_array_buffer as _);
        gles.BindBuffer(
            gles11::ELEMENT_ARRAY_BUFFER,
            saved_element_array_buffer as _,
        );
        let _ = gles.GetError();
    }
    std::mem::drop(gles);
    window.swap_window();

    animation_state.update_started_and_finished_animations(env);

    // A readback/composition can exceed one display interval. Schedule
    // against completion so NSRunLoop does not immediately redraw the same
    // frame and starve the next guest timer or input event.
    let state = &mut env.framework_state.core_animation.composition;
    let next = composition_deadline(state.recomposite_next, Instant::now());
    state.recomposite_next = Some(next);
    Some(next)
}

/// Call `displayIfNeeded` on all relevant layers in the tree, so their bitmaps
/// are up to date before compositing.
fn display_layers(env: &mut Environment, root_layer: id) {
    // Tell layers to redraw themselves if needed.

    fn traverse(objc: &ObjC, layer: id, layers_needing_display: &mut Vec<id>) {
        let host_obj = objc.borrow::<CALayerHostObject>(layer);
        if host_obj.hidden {
            return;
        }
        if host_obj.needs_display {
            layers_needing_display.push(layer);
        }
        for &layer in &host_obj.sublayers {
            traverse(objc, layer, layers_needing_display);
        }
    }

    let mut layers_needing_display = Vec::new();
    traverse(&env.objc, root_layer, &mut layers_needing_display);

    for layer in layers_needing_display {
        () = msg![env; layer displayIfNeeded];
    }
}

/// Traverses the layer tree and draws each layer.
unsafe fn composite_layer_recursive(
    env: &mut Environment,
    animation_state: &mut animation::State,
    layer: id,
    cumulative_transform: Matrix<4>,
    opacity: CGFloat,
) {
    // TODO: this can't handle zPosition among other things, but it is not
    //       supported yet :)
    // TODO: back-to-front drawing is not efficient, could we use front-to-back?

    // Fully transparent subtrees cannot contribute pixels. Avoid cloning
    // their presentation layers and issuing GL work, including for children.
    let model = env.objc.borrow::<CALayerHostObject>(layer);
    if model.hidden
        || (model.opacity <= 0.0
            && model.animations.is_empty()
            && model.anonymous_animations.is_empty())
        || opacity <= 0.0
    {
        return;
    }

    // This is both acting as the presentationLayer and the private render layer
    // It might need to be reworked in the future into a guest presentationLayer
    //
    // PERF: the presentation layer is a clone of the layer's host object.
    // For a CAEAGLLayer presented through the compositor that would clone a
    // full frame of pixels (hundreds of KB to several MB) every time it is
    // composited, so take the pixels out for the duration of the clone and
    // leave only a placeholder (with the real dimensions) in the copy — the
    // upload below reads the pixels from the original layer anyway.
    let presented_pixels = env
        .objc
        .borrow_mut::<CALayerHostObject>(layer)
        .presented_pixels
        .take();
    let mut host_obj = animation_state.create_presentation_layer(env, layer);
    if let Some((pixels, width, height)) = presented_pixels {
        host_obj.presented_pixels = Some((Vec::new(), width, height));
        env.objc
            .borrow_mut::<CALayerHostObject>(layer)
            .presented_pixels = Some((pixels, width, height));
    }

    if host_obj.hidden || host_obj.opacity <= 0.0 {
        return;
    }

    let window = env.window.as_mut().unwrap();
    let mut gles = window.make_internal_gl_ctx_current();

    let opacity = opacity * host_obj.opacity;
    let cumulative_transform = {
        let CALayerHostObject { bounds, .. } = host_obj;

        // Update the transform to match this layer's co-ordinate space.
        let cumulative_transform =
            <Matrix<4> as From<_>>::from(host_obj.superlayer_to_layer_transform())
                .multiply(&cumulative_transform);

        // Reposition and scale the unit quad (see ARRAY_BUFFER binding)
        // so it will have the right size in this layer's co-ordinate space.
        gles.MatrixMode(gles11::MODELVIEW);
        load_matrix(
            gles.as_mut(),
            Matrix::<4>::from(&Matrix::scale_2d(bounds.size.width, bounds.size.height))
                .multiply(&Matrix::translate_3d(bounds.origin.x, bounds.origin.y, 0.0))
                .multiply(&cumulative_transform),
        );

        cumulative_transform
    };

    // Draw background color, if any
    let have_background = if let Some(background_color) = host_obj.background_color {
        let misc = env
            .framework_state
            .core_animation
            .composition
            .misc_gl_objects
            .as_ref()
            .unwrap();

        let CGColorHostObject { r, g, b, a, .. } = background_color;
        gles.Color4f(
            r * a * opacity,
            g * a * opacity,
            b * a * opacity,
            a * opacity,
        );
        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);

        let radius = host_obj.corner_radius;
        if radius == 0.0 {
            gles.Disable(gles11::TEXTURE_2D);
            gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);

            gles.EnableClientState(gles11::VERTEX_ARRAY);
            gles.BindBuffer(gles11::ARRAY_BUFFER, misc.basic_square_buffer);
            gles.VertexPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);

            gles.DrawElements(
                gles11::TRIANGLES,
                SQUARE_INDICES.len() as _,
                gles11::UNSIGNED_BYTE,
                0 as *const GLvoid,
            );
        } else {
            gles.Enable(gles11::TEXTURE_2D);
            gles.BindTexture(gles11::TEXTURE_2D, misc.rounded_corner_texture);
            gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
            gles.BindBuffer(gles11::ARRAY_BUFFER, misc.rounded_tex_coord_buffer);
            gles.TexCoordPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);

            gles.EnableClientState(gles11::VERTEX_ARRAY);
            gles.BindBuffer(gles11::ARRAY_BUFFER, misc.rounded_vertex_buffer);
            upload_slice(
                gles.as_mut(),
                gles11::ARRAY_BUFFER,
                &make_9patch_coords(
                    [
                        0.0,
                        (radius / host_obj.bounds.size.width).min(0.5),
                        (1.0 - radius / host_obj.bounds.size.width).max(0.5),
                        1.0,
                    ],
                    [
                        0.0,
                        (radius / host_obj.bounds.size.height).min(0.5),
                        (1.0 - radius / host_obj.bounds.size.height).max(0.5),
                        1.0,
                    ],
                ),
                gles11::DYNAMIC_DRAW,
            );
            gles.VertexPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);

            gles.DrawElements(
                gles11::TRIANGLES,
                INDICES_PER_9PATCH as _,
                gles11::UNSIGNED_BYTE,
                0 as *const GLvoid,
            );
        };

        true
    } else {
        false
    };

    // Draw background pattern image (tiled), if any
    let have_background = if host_obj.background_pattern_cg_image != nil {
        let pattern_cg = host_obj.background_pattern_cg_image;
        let image = cg_image::borrow_image(&env.objc, pattern_cg);
        let (img_w, img_h) = image.dimensions();

        // Get or create the cached GL texture for this pattern
        let pattern_tex = {
            let orig = env.objc.borrow::<CALayerHostObject>(layer);
            orig.background_pattern_gles_texture
        };
        let tex = if let Some(t) = pattern_tex {
            t
        } else {
            let mut t: GLuint = 0;
            gles.GenTextures(1, &mut t);
            gles.BindTexture(gles11::TEXTURE_2D, t);
            let pixels = image.pixels();
            upload_rgba8_pixels(gles.as_mut(), pixels, (img_w, img_h), None);
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_WRAP_S,
                gles11::REPEAT as _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_WRAP_T,
                gles11::REPEAT as _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MIN_FILTER,
                gles11::NEAREST as _,
            );
            gles.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MAG_FILTER,
                gles11::NEAREST as _,
            );
            env.objc
                .borrow_mut::<CALayerHostObject>(layer)
                .background_pattern_gles_texture = Some(t);
            t
        };
        gles.BindTexture(gles11::TEXTURE_2D, tex);

        // Compute tiled texture coordinates
        let tile_x = host_obj.bounds.size.width / img_w as f32;
        let tile_y = host_obj.bounds.size.height / img_h as f32;
        let tiled_coords: [f32; 8] = [0.0, tile_y, 0.0, 0.0, tile_x, tile_y, tile_x, 0.0];

        let misc = env
            .framework_state
            .core_animation
            .composition
            .misc_gl_objects
            .as_ref()
            .unwrap();

        gles.Color4f(opacity, opacity, opacity, opacity);
        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
        gles.Enable(gles11::TEXTURE_2D);

        gles.EnableClientState(gles11::VERTEX_ARRAY);
        gles.BindBuffer(gles11::ARRAY_BUFFER, misc.basic_square_buffer);
        gles.VertexPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);

        gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.BindBuffer(gles11::ARRAY_BUFFER, 0);
        gles.TexCoordPointer(2, gles11::FLOAT, 0, tiled_coords.as_ptr() as *const GLvoid);

        gles.DrawElements(
            gles11::TRIANGLES,
            SQUARE_INDICES.len() as _,
            gles11::UNSIGNED_BYTE,
            0 as *const GLvoid,
        );

        true
    } else {
        have_background
    };

    let need_texture = host_obj.presented_pixels.is_some()
        || host_obj.contents != nil
        || host_obj.cg_context.is_some();
    let need_update = need_texture && !host_obj.gles_texture_is_up_to_date;

    if need_texture {
        if let Some(texture) = host_obj.gles_texture {
            gles.BindTexture(gles11::TEXTURE_2D, texture);
        } else {
            assert!(!host_obj.gles_texture_is_up_to_date);
            let mut texture = 0;
            gles.GenTextures(1, &mut texture);
            gles.BindTexture(gles11::TEXTURE_2D, texture);
            // Update original layer texture
            env.objc.borrow_mut::<CALayerHostObject>(layer).gles_texture = Some(texture);
        }
    }

    // Dimensions of the texture storage that already exists (if any), so the
    // uploads below can update it in place.
    let mut texture_size = host_obj.gles_texture_size;

    // Update original layer texture with CAEAGLLayer pixels (slow path), if any
    if need_update {
        let original_host_obj = env.objc.borrow_mut::<CALayerHostObject>(layer);
        if let Some((ref mut pixels, width, height)) = original_host_obj.presented_pixels {
            // The pixels are always RGBA, but if the layer is opaque then the
            // alpha channel is meant to be ignored. glTexImage2D() has no
            // option to ignore it, so let's manually set them to 255.
            if original_host_obj.opaque {
                let mut i = 3;
                while i < pixels.len() {
                    pixels[i] = 255;
                    i += 4;
                }
            }

            texture_size = Some(upload_rgba8_pixels(
                gles.as_mut(),
                pixels,
                (width, height),
                texture_size,
            ));
        }
    }

    // Update texture with CGImageRef or CGContextRef pixels, if any
    if need_update {
        if host_obj.contents != nil {
            let image = cg_image::borrow_image(&env.objc, host_obj.contents);

            // No special handling for opacity is needed here: the alpha channel
            // on an image is meaningful and won't be ignored.
            texture_size = Some(upload_rgba8_pixels(
                gles.as_mut(),
                image.pixels(),
                image.dimensions(),
                texture_size,
            ));
        } else if let Some(cg_context) = host_obj.cg_context {
            // Make sure this is in sync with the code in ca_layer.rs that
            // sets up the context!
            let (width, height, data) = cg_bitmap_context::get_data(&env.objc, cg_context);
            let size = width * height * 4;
            let pixels = env.mem.bytes_at(data.cast(), size);
            texture_size = Some(upload_rgba8_pixels(
                gles.as_mut(),
                pixels,
                (width, height),
                texture_size,
            ));
        }
    }

    if need_update {
        // Update original layer fields
        let original_host_obj = env.objc.borrow_mut::<CALayerHostObject>(layer);
        original_host_obj.gles_texture_is_up_to_date = true;
        original_host_obj.gles_texture_size = texture_size;
    }

    // Draw texture, if any
    if need_texture {
        let misc = env
            .framework_state
            .core_animation
            .composition
            .misc_gl_objects
            .as_ref()
            .unwrap();

        // Position `contents` images by `contentsGravity`; the unit quad is
        // otherwise scaled to the whole bounds (kCAGravityResize).
        if host_obj.contents != nil
            && host_obj.contents_gravity != super::ca_layer::kCAGravityResize
        {
            let (width, height) = cg_image::borrow_image(&env.objc, host_obj.contents).dimensions();
            let scale = host_obj.contents_scale;
            let size = CGSize {
                width: width as CGFloat / scale,
                height: height as CGFloat / scale,
            };
            let rect = super::ca_layer::contents_rect_for_gravity(
                &host_obj.contents_gravity,
                host_obj.bounds,
                size,
            );
            gles.MatrixMode(gles11::MODELVIEW);
            load_matrix(
                gles.as_mut(),
                Matrix::<4>::from(&Matrix::scale_2d(rect.size.width, rect.size.height))
                    .multiply(&Matrix::translate_3d(rect.origin.x, rect.origin.y, 0.0))
                    .multiply(&cumulative_transform),
            );
        }

        gles.Color4f(opacity, opacity, opacity, opacity);
        // Apple: `opaque` "affects only the backing store managed by Core
        // Animation. If you assign an image with an alpha channel to the
        // layer's contents property, that image retains its alpha channel
        // regardless of the value of this property."
        // https://developer.apple.com/documentation/quartzcore/calayer/1410763-isopaque
        if opacity == 1.0 && host_obj.opaque && !have_background && host_obj.contents == nil {
            gles.Disable(gles11::BLEND);
        } else {
            gles.Enable(gles11::BLEND);
            gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
        }

        gles.EnableClientState(gles11::VERTEX_ARRAY);
        gles.BindBuffer(gles11::ARRAY_BUFFER, misc.basic_square_buffer);
        gles.VertexPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);

        gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
        // Normal images will have top-to-bottom row order, but OpenGL ES
        // expects bottom-to-top, so flip the UVs in that case.
        gles.BindBuffer(
            gles11::ARRAY_BUFFER,
            if host_obj.contents != nil {
                misc.basic_square_buffer
            } else {
                misc.flipped_square_buffer
            },
        );
        gles.TexCoordPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);
        gles.Enable(gles11::TEXTURE_2D);
        gles.DrawElements(
            gles11::TRIANGLES,
            SQUARE_INDICES.len() as _,
            gles11::UNSIGNED_BYTE,
            0 as *const GLvoid,
        );
    }
    std::mem::drop(gles);

    // Sort sublayers by zPosition for correct back-to-front compositing.
    // Per Apple's CALayer documentation (iOS 2.0+):
    //   "Sublayers that share the same value in their zPosition property
    //    are rendered in the order they appear in the sublayers array."
    // We clone the sublayer list, sort by zPosition (stable sort preserves
    // order of equal elements), then composite each child.
    let sorted_sublayers = {
        let original_host_obj = env.objc.borrow::<CALayerHostObject>(layer);
        let mut sublayers_copy = original_host_obj.sublayers.clone();
        sublayers_copy.sort_by(|&a, &b| {
            let z_a = env.objc.borrow::<CALayerHostObject>(a).z_position;
            let z_b = env.objc.borrow::<CALayerHostObject>(b).z_position;
            z_a.partial_cmp(&z_b).unwrap_or(std::cmp::Ordering::Equal)
        });
        sublayers_copy
    };

    for child_layer in sorted_sublayers {
        composite_layer_recursive(
            env,
            animation_state,
            child_layer,
            cumulative_transform,
            opacity,
        )
    }
}

const FLOATS_PER_POINT: usize = 2;
const BASIC_SQUARE_POINTS: [f32; 4 * FLOATS_PER_POINT] = [0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0];
const SQUARE_INDICES: [u8; 6] = [0, 1, 2, 2, 1, 3];
const FLIPPED_SQUARE_POINTS: [f32; 4 * FLOATS_PER_POINT] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0];
const FLOATS_PER_9PATCH: usize = BASIC_SQUARE_POINTS.len() * 3 * 3;
const INDICES_PER_9PATCH: usize = SQUARE_INDICES.len() * 3 * 3;

fn make_9patch_coords(x_edges: [f32; 4], y_edges: [f32; 4]) -> [f32; FLOATS_PER_9PATCH] {
    let mut out_points = [0.0; FLOATS_PER_9PATCH];
    for (i, out_points_chunk) in out_points
        .chunks_exact_mut(BASIC_SQUARE_POINTS.len())
        .enumerate()
    {
        let (x, y) = (i % 3, i / 3);

        for (dst_xy, src_xy) in out_points_chunk
            .chunks_exact_mut(2)
            .zip(BASIC_SQUARE_POINTS.chunks_exact(2))
        {
            let (x1, x2) = (x_edges[x], x_edges[x + 1]);
            let (y1, y2) = (y_edges[y], y_edges[y + 1]);
            dst_xy[0] = x1 + src_xy[0] * (x2 - x1);
            dst_xy[1] = y1 + src_xy[1] * (y2 - y1);
        }
    }
    out_points
}

fn make_9patch_indices() -> [u8; INDICES_PER_9PATCH] {
    let mut out_indices = [0; SQUARE_INDICES.len() * 3 * 3];
    for (i, out_indices_chunk) in out_indices
        .chunks_exact_mut(SQUARE_INDICES.len())
        .enumerate()
    {
        for (out_index, in_index) in out_indices_chunk
            .iter_mut()
            .zip(SQUARE_INDICES.iter().copied())
        {
            *out_index = in_index + i as u8 * (BASIC_SQUARE_POINTS.len() / FLOATS_PER_POINT) as u8;
        }
    }
    out_indices
}

unsafe fn upload_slice<T: SafeWrite>(
    gles: &mut dyn GLES,
    target: GLenum,
    data: &[T],
    usage: GLenum,
) {
    gles.BufferData(
        target,
        std::mem::size_of_val(data) as _,
        data.as_ptr() as *const _,
        usage,
    )
}

/// Upload RGBA8 `pixels` to the texture bound to `GL_TEXTURE_2D`.
///
/// `current_size` is the size of the storage the texture already has (if it
/// was uploaded to before); when it matches, the contents are replaced in
/// place with `glTexSubImage2D`, which lets the driver update the existing
/// allocation instead of orphaning it and allocating a new one on every
/// change (a measurable per-frame cost for layers that update continuously,
/// e.g. a CAEAGLLayer presented through the compositor). Returns the size of
/// the texture storage afterwards.
unsafe fn upload_rgba8_pixels(
    gles: &mut dyn GLES,
    pixels: &[u8],
    dimensions: (u32, u32),
    current_size: Option<(u32, u32)>,
) -> (u32, u32) {
    if current_size == Some(dimensions) {
        gles.TexSubImage2D(
            gles11::TEXTURE_2D,
            0,
            0,
            0,
            dimensions.0 as _,
            dimensions.1 as _,
            gles11::RGBA,
            gles11::UNSIGNED_BYTE,
            pixels.as_ptr() as *const _,
        );
        // The parameters below were already applied when the storage was
        // first created.
        return dimensions;
    }
    gles.TexImage2D(
        gles11::TEXTURE_2D,
        0,
        gles11::RGBA as _,
        dimensions.0 as _,
        dimensions.1 as _,
        0,
        gles11::RGBA,
        gles11::UNSIGNED_BYTE,
        pixels.as_ptr() as *const _,
    );
    gles.TexParameteri(
        gles11::TEXTURE_2D,
        gles11::TEXTURE_MIN_FILTER,
        gles11::LINEAR as _,
    );
    gles.TexParameteri(
        gles11::TEXTURE_2D,
        gles11::TEXTURE_MAG_FILTER,
        gles11::LINEAR as _,
    );
    // Force CLAMP_TO_EDGE on both axes. The CAEAGLLayer pixel buffers we
    // upload here are virtually always NPOT (e.g. 480x320, 320x480), and
    // strict ES 1.1 drivers without GL_OES_texture_npot only support NPOT
    // textures with CLAMP_TO_EDGE wrapping; with GL_REPEAT the texture is
    // "incomplete" and samples as (0,0,0,1), making the whole composited
    // window appear black on Mali and similar tile-based GPUs even though
    // more permissive drivers (Mesa Zink, desktop GL) silently accept it.
    gles.TexParameteri(
        gles11::TEXTURE_2D,
        gles11::TEXTURE_WRAP_S,
        gles11::CLAMP_TO_EDGE as _,
    );
    gles.TexParameteri(
        gles11::TEXTURE_2D,
        gles11::TEXTURE_WRAP_T,
        gles11::CLAMP_TO_EDGE as _,
    );
    dimensions
}

/*
 * Implementation of UIImage for iOS 2.0 - 4.3.5 targeting TouchHLE.
 */

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_graphics::cg_context::CGContextDrawImage;
use crate::frameworks::core_graphics::cg_image::{
    self, CGImageGetHeight, CGImageGetWidth, CGImageRef, CGImageRelease, CGImageRetain,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{ns_data, ns_string, NSInteger};
use crate::frameworks::uikit::ui_graphics::UIGraphicsGetCurrentContext;
use crate::fs::GuestPath;
use crate::image::Image;
use crate::mem::MutVoidPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send, nil, objc_classes, release, retain, ClassExports,
    HostObject, NSZonePtr, SEL,
};
use crate::Environment;
use std::collections::HashMap;

const CACHE_SIZE: usize = 60;

#[derive(Default)]
pub struct State {
    cached_images: HashMap<String, id>,
}

impl State {
    fn get(env: &Environment) -> &Self {
        &env.framework_state.uikit.ui_image
    }
    fn get_mut(env: &mut Environment) -> &mut Self {
        &mut env.framework_state.uikit.ui_image
    }
}

// В iOS 2-4 stretchableImage хранило параметры leftCapWidth и topCapHeight
// прямо в объекте.
#[derive(Default)]
struct UIImageHostObject {
    cg_image: CGImageRef,
    orientation: NSInteger, // UIImageOrientation
    left_cap_width: NSInteger,
    top_cap_height: NSInteger,
    /// Image scale factor (`1.0` for standard, `2.0` for @2x, `3.0` for @3x).
    /// UIKit reports `-[UIImage size]` in points (pixels / scale); honouring
    /// this keeps layout and hit-testing correct when a Retina-only app's
    /// @2x/@3x asset is loaded on touchHLE's 1x device.
    scale: CGFloat,
}
impl HostObject for UIImageHostObject {}

fn get_dummy_cg_image(env: &mut Environment) -> CGImageRef {
    const DUMMY_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 11, 73, 68, 65, 84, 8, 215, 99, 96, 0, 2, 0, 0, 5, 0,
        1, 226, 38, 5, 155, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let image = Image::from_bytes(DUMMY_PNG).unwrap();
    cg_image::from_image(env, image)
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIImage: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIImageHostObject {
        cg_image: nil,
        orientation: 0, // UIImageOrientationUp
        left_cap_width: 0,
        top_cap_height: 0,
        scale: 1.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)imageNamed:(id)name { // NSString*
    if name == nil {
        return nil;
    }
    let bundle: id = msg_class![env; NSBundle mainBundle];
    let mut path: id = msg![env; bundle pathForResource:name ofType:nil];
    let name_str = ns_string::to_rust_string(env, name).to_string();
    // Like UIKit, accept names without an extension (".png" is implied) and
    // fall back to the "~iphone" and @2x/@3x variants.
    //
    // The @2x/@3x fallback is important even though touchHLE emulates a 1x
    // device: modern iPhone-only apps (iOS 7+) frequently ship ONLY the
    // Retina (@2x/@3x) assets and no 1x copy at all, because every device
    // they support is Retina. Without this fallback those apps fail to load
    // almost all of their artwork (buttons, popups, HUD) and appear broken.
    // The @2x suffix must be inserted before the file extension, i.e.
    // "news-close.png" -> "news-close@2x.png", not "news-close.png@2x".
    if path == nil {
        // Split "foo.png" into stem "foo" and extension "png"; a name with no
        // extension implies "png" like UIKit does.
        let (stem, ext) = match name_str.rfind('.') {
            Some(dot) => (
                name_str[..dot].to_string(),
                name_str[dot + 1..].to_string(),
            ),
            None => (name_str.clone(), "png".to_string()),
        };
        let ext_ns = ns_string::from_rust_string(env, ext);
        let is_ipad = env
            .options
            .device_family
            .map_or(false, |family| family.is_ipad());
        let candidates: Vec<String> = if is_ipad {
            vec![
                stem.clone(),
                format!("{}~ipad", stem),
                format!("{}@2x~ipad", stem),
                format!("{}@2x", stem),
                format!("{}~iphone", stem),
                format!("{}@2x~iphone", stem),
                format!("{}@3x~iphone", stem),
                format!("{}@3x", stem),
            ]
        } else {
            vec![
                stem.clone(),
                format!("{}~iphone", stem),
                format!("{}@2x~iphone", stem),
                format!("{}@3x~iphone", stem),
                format!("{}@2x", stem),
                format!("{}@3x", stem),
                format!("{}~ipad", stem),
                format!("{}@2x~ipad", stem),
            ]
        };
        for candidate in candidates {
            let candidate = ns_string::from_rust_string(env, candidate);
            path = msg![env; bundle pathForResource:candidate ofType:ext_ns];
            release(env, candidate);
            if path != nil {
                break;
            }
        }
        release(env, ext_ns);
    }
    if path == nil {
        log!("UIImage imageNamed: {:?} not found in bundle", name_str);
    }

    if State::get(env).cached_images.len() > CACHE_SIZE {
        let cache = std::mem::take(&mut State::get_mut(env).cached_images);
        for (_, img) in cache {
            release(env, img);
        }
    }

    if !State::get(env).cached_images.contains_key(&name_str) {
        let mut img: id = nil;
        if path != nil {
            img = msg![env; this imageWithContentsOfFile:path];
        }

        let final_img = if img != nil {
            retain(env, img);
            img
        } else {
            let cg_image = get_dummy_cg_image(env);
            let new_img: id = msg![env; this alloc];
            msg![env; new_img initWithCGImage:cg_image]
        };

        State::get_mut(env).cached_images.insert(name_str.clone(), final_img);
    }
    *State::get(env).cached_images.get(&name_str).unwrap()
}

+ (id)imageWithContentsOfFile:(id)path { // NSString*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithContentsOfFile:path];
    autorelease(env, new)
}

+ (id)imageWithData:(id)data { // NSData*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithData:data];
    autorelease(env, new)
}

+ (id)imageWithCGImage:(CGImageRef)cg_image {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithCGImage:cg_image];
    autorelease(env, new)
}

// iOS 4.0+
+ (id)imageWithCGImage:(CGImageRef)cg_image scale:(CGFloat)scale orientation:(NSInteger)orientation {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithCGImage:cg_image scale:scale orientation:orientation];
    autorelease(env, new)
}

// iOS 4.0+
+ (id)imageWithCGImage:(CGImageRef)cg_image scale:(CGFloat)scale {
    msg![env; this imageWithCGImage:cg_image scale:scale orientation:(0i32)]
}

+ (id)imageWithData:(id)data scale:(CGFloat)scale {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithData:data scale:scale];
    autorelease(env, new)
}

// MARK: - Initializers

- (id)initWithCGImage:(CGImageRef)cg_image {
    msg![env; this initWithCGImage:cg_image scale:1.0 orientation:(0i32)]
}

- (id)initWithCGImage:(CGImageRef)cg_image scale:(CGFloat)scale orientation:(NSInteger)orientation {
    CGImageRetain(env, cg_image);
    let host = env.objc.borrow_mut::<UIImageHostObject>(this);
    host.cg_image = cg_image;
    host.scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    host.orientation = orientation;
    host.left_cap_width = 0;
    host.top_cap_height = 0;
    this
}

- (id)initWithContentsOfFile:(id)path { // NSString*
    if path == nil {
        env.objc.borrow_mut::<UIImageHostObject>(this).cg_image = get_dummy_cg_image(env);
        return this;
    }
    let path_str = ns_string::to_rust_string(env, path);
    let bytes = env.fs.read(GuestPath::new(&path_str));
    let lower_path = path_str.to_ascii_lowercase();
    if crate::env_flag_cached!("TOUCHHLE_TRACE_RESOURCES")
        && [".fnt", ".png", ".plist", ".jpg", ".jpeg"]
            .iter()
            .any(|extension| lower_path.ends_with(extension))
    {
        static RESOURCE_TRACE_COUNT: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        if RESOURCE_TRACE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 300 {
            log!(
                "[RESOURCE-TRACE] UIImage path={:?} bytes={:?}",
                path_str,
                bytes.as_ref().map(Vec::len)
            );
        }
    }
    let Ok(bytes) = bytes else {
        env.objc.borrow_mut::<UIImageHostObject>(this).cg_image = get_dummy_cg_image(env);
        return this;
    };
    let Ok(image) = Image::from_bytes(&bytes) else {
        env.objc.borrow_mut::<UIImageHostObject>(this).cg_image = get_dummy_cg_image(env);
        return this;
    };

    // Infer the scale factor from the Retina filename suffix so that
    // `-[UIImage size]` reports points, not pixels. This matters for
    // Retina-only apps (see the `@2x`/`@3x` fallback in `imageNamed:`):
    // without it a 60x60-pixel `foo@2x.png` would report a 60x60-point
    // size and lay out (and hit-test) at twice its intended size.
    let scale: CGFloat = if lower_path.contains("@3x.") || lower_path.contains("@3x~") {
        3.0
    } else if lower_path.contains("@2x.") || lower_path.contains("@2x~") {
        2.0
    } else {
        1.0
    };
    let cg = cg_image::from_image(env, image);
    let host = env.objc.borrow_mut::<UIImageHostObject>(this);
    host.cg_image = cg;
    host.scale = scale;
    this
}

- (id)initWithData:(id)data { // NSData*
    msg![env; this initWithData:data scale:1.0]
}

- (id)initWithData:(id)data scale:(CGFloat)scale { // NSData*
    let normalized_scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    if data == nil {
        let dummy = get_dummy_cg_image(env);
        let host = env.objc.borrow_mut::<UIImageHostObject>(this);
        host.cg_image = dummy;
        host.scale = normalized_scale;
        return this;
    }
    let slice = ns_data::to_rust_slice(env, data);
    let Ok(image) = Image::from_bytes(slice) else {
        let dummy = get_dummy_cg_image(env);
        let host = env.objc.borrow_mut::<UIImageHostObject>(this);
        host.cg_image = dummy;
        host.scale = normalized_scale;
        return this;
    };
    let cg = cg_image::from_image(env, image);
    let host = env.objc.borrow_mut::<UIImageHostObject>(this);
    host.cg_image = cg;
    host.scale = normalized_scale;
    this
}

// MARK: - Properties

- (CGSize)size {
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    // An image created via bare -alloc/-init has no CGImage; Apple returns
    // a zero size rather than crashing.
    if image.is_null() {
        return CGSize { width: 0.0, height: 0.0 };
    }
    let (width, height) = cg_image::borrow_image(&env.objc, image).dimensions();
    // UIKit reports the size in points: pixels divided by the image scale.
    // For a @2x asset loaded on this 1x-emulated device the CGImage is twice
    // the point size, so a view that sizes itself to `image.size` (and the
    // close buttons that depend on it for hit-testing) must see points.
    let scale = env.objc.borrow::<UIImageHostObject>(this).scale.max(1.0);
    CGSize {
        width: width as CGFloat / scale,
        height: height as CGFloat / scale,
    }
}

- (CGFloat)scale {
    env.objc.borrow::<UIImageHostObject>(this).scale.max(1.0)
}

- (CGImageRef)CGImage {
    env.objc.borrow::<UIImageHostObject>(this).cg_image
}

- (NSInteger)imageOrientation {
    env.objc.borrow::<UIImageHostObject>(this).orientation
}

// MARK: - Stretchable Images (iOS 2.0 - 4.x legacy method)

- (id)stretchableImageWithLeftCapWidth:(NSInteger)leftCapWidth topCapHeight:(NSInteger)topCapHeight {
    let &UIImageHostObject {
        cg_image,
        scale,
        orientation,
        ..
    } = env.objc.borrow::<UIImageHostObject>(this);

    let new_img: id = msg_class![env; UIImage alloc];
    let new_img: id = msg![env; new_img initWithCGImage:cg_image scale:scale orientation:orientation];

    let host = env.objc.borrow_mut::<UIImageHostObject>(new_img);
    host.left_cap_width = leftCapWidth;
    host.top_cap_height = topCapHeight;

    autorelease(env, new_img)
}

- (NSInteger)leftCapWidth {
    env.objc.borrow::<UIImageHostObject>(this).left_cap_width
}

- (NSInteger)topCapHeight {
    env.objc.borrow::<UIImageHostObject>(this).top_cap_height
}

// MARK: - Drawing

- (())drawAtPoint:(CGPoint)point {
    let context = UIGraphicsGetCurrentContext(env);
    if context == nil { return; }
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    // Drawing a nil image is a no-op, not a crash.
    if image == nil { return; }
    // Draw at the image's point size (pixels / scale) so @2x assets are not
    // drawn at double size.
    let scale = env.objc.borrow::<UIImageHostObject>(this).scale.max(1.0);
    let rect = CGRect {
        origin: point,
        size: CGSize {
            width: CGImageGetWidth(env, image) as CGFloat / scale,
            height: CGImageGetHeight(env, image) as CGFloat / scale,
        }
    };
    CGContextDrawImage(env, context, rect, image);
}

- (())drawInRect:(CGRect)rect {
    let context = UIGraphicsGetCurrentContext(env);
    if context == nil { return; }
    // TODO: Здесь должна быть логика отрисовки с учетом leftCapWidth и
    // topCapHeight
    // Если left_cap_width > 0 || top_cap_height > 0, нужно делить картинку на 9
    // частей (nine-patch)
    // и отрисовывать через CGContextDrawImage кусками. Пока рисуем целиком.
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    // Drawing a nil image is a no-op, not a crash.
    if image == nil { return; }
    CGContextDrawImage(env, context, rect, image);
}

// `blendMode` and `alpha` arguments are accepted and validated, but applying
// them to the bitmap blit path is not yet implemented. The standard, opaque
// draw still happens so the image is not invisible (which is what blocked
// FarmFrenzy from progressing past its loading screen).
// See https://developer.apple.com/documentation/uikit/uiimage/1624155-drawinrect
- (())drawInRect:(CGRect)rect blendMode:(i32)_blend_mode alpha:(CGFloat)_alpha {
    let context = UIGraphicsGetCurrentContext(env);
    if context == nil { return; }
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    // Drawing a nil image is a no-op, not a crash.
    if image == nil { return; }
    CGContextDrawImage(env, context, rect, image);
}

- (())drawAtPoint:(CGPoint)point blendMode:(i32)_blend_mode alpha:(CGFloat)_alpha {
    let context = UIGraphicsGetCurrentContext(env);
    if context == nil { return; }
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    // Drawing a nil image is a no-op, not a crash.
    if image == nil { return; }
    let scale = env.objc.borrow::<UIImageHostObject>(this).scale.max(1.0);
    let rect = CGRect {
        origin: point,
        size: CGSize {
            width: CGImageGetWidth(env, image) as CGFloat / scale,
            height: CGImageGetHeight(env, image) as CGFloat / scale,
        }
    };
    CGContextDrawImage(env, context, rect, image);
}

// Apple: "Draws the image, tiled, in a rectangle." The receiver's CGImage is
// repeated horizontally and vertically to fill `rect` in the current graphics
// context, anchored at the rect's top-left.
// https://developer.apple.com/documentation/uikit/uiimage/1624157-drawaspatterninrect
- (())drawAsPatternInRect:(CGRect)rect {
    let context = UIGraphicsGetCurrentContext(env);
    if context == nil { return; }
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    if image == nil { return; }
    let scale = env.objc.borrow::<UIImageHostObject>(this).scale.max(1.0);
    let tile_w = CGImageGetWidth(env, image) as CGFloat / scale;
    let tile_h = CGImageGetHeight(env, image) as CGFloat / scale;
    if tile_w <= 0.0 || tile_h <= 0.0 || rect.size.width <= 0.0 || rect.size.height <= 0.0 {
        return;
    }
    let mut y = rect.origin.y;
    let y_end = rect.origin.y + rect.size.height;
    let x_end = rect.origin.x + rect.size.width;
    while y < y_end {
        let mut x = rect.origin.x;
        while x < x_end {
            let tile_rect = CGRect {
                origin: CGPoint { x, y },
                size: CGSize { width: tile_w, height: tile_h },
            };
            CGContextDrawImage(env, context, tile_rect, image);
            x += tile_w;
        }
        y += tile_h;
    }
}

// MARK: - Memory Management

- (())dealloc {
    let &UIImageHostObject { cg_image, .. } = env.objc.borrow(this);
    CGImageRelease(env, cg_image);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation UIImageNibPlaceholder: NSObject

// `UIImageNibPlaceholder` is UIKit's private stand-in for an image stored in
// a nib. Real nibs usually decode the resource name through `-initWithCoder:`,
// but storyboards and hand-written archives may instead invoke the regular
// `UIImage` data/file initializers on the placeholder. Forward those to a
// real `UIImage` and transfer ownership, exactly like `-initWithCoder:` below.
// <https://developer.apple.com/documentation/uikit/uiimage>

- (id)initWithData:(id)data { // NSData*
    () = msg![env; this dealloc];
    let image: id = msg_class![env; UIImage imageWithData:data];
    if image != nil {
        retain(env, image);
    }
    image
}

- (id)initWithData:(id)data scale:(CGFloat)scale { // NSData*
    () = msg![env; this dealloc];
    let image: id = msg_class![env; UIImage imageWithData:data scale:scale];
    if image != nil {
        retain(env, image);
    }
    image
}

- (id)initWithContentsOfFile:(id)path { // NSString*
    () = msg![env; this dealloc];
    let image: id = msg_class![env; UIImage imageWithContentsOfFile:path];
    if image != nil {
        retain(env, image);
    }
    image
}

- (id)initWithCoder:(id)coder {
    // В NIB-файлах плейсхолдеры картинок хранят имя ресурса под ключом
    // UIResourceName
    let key = get_static_str(env, "UIResourceName");
    let name: id = msg![env; coder decodeObjectForKey:key];

    // Плейсхолдер был выделен через alloc, но мы не будем его использовать.
    // Сразу деаллоцируем этот временный объект, чтобы не было утечек памяти.
    () = msg![env; this dealloc];

    if name != nil {
        // Запрашиваем настоящую картинку (твой метод imageNamed: сам проверит
        // кэш или загрузит)
        let image: id = msg_class![env; UIImage imageNamed:name];

        // Важный момент (без заглушек и утечек):
        // Вызов [UIImageNibPlaceholder alloc] initWithCoder:] подразумевает,
        // что возвращенный объект будет иметь retain count +1 (владение).
        // Но [UIImage imageNamed:] возвращает закэшированный/autorelease
        // объект!
        // Поэтому мы ОБЯЗАНЫ сделать retain возвращаемой картинке, иначе она
        // удалится раньше времени.
        if image != nil {
            retain(env, image);
        }
        return image;
    }

    nil
}

@end

};

// MARK: - C Functions (Exporting formats like mentioned in the doc)

/// `void UIImageWriteToSavedPhotosAlbum(UIImage *image, id completionTarget,
///     SEL completionSelector, void *contextInfo);`
///
/// Apple documentation:
/// <https://developer.apple.com/documentation/uikit/1619125-uiimagewritetosavedphotosalbum>
///
/// Adds the specified image to the user's Camera Roll album. In the emulator
/// we persist the PNG data into the app's sandboxed Documents directory (since
/// there is no real photo library). After saving, the optional completion
/// callback is invoked with a nil error to signal success.
///
/// The completion selector signature expected by Apple is:
/// `- (void)image:(UIImage *)image didFinishSavingWithError:(NSError *)error
///               contextInfo:(void *)contextInfo;`
fn UIImageWriteToSavedPhotosAlbum(
    env: &mut Environment,
    image: id,
    completion_target: id,
    completion_selector: SEL,
    context_info: MutVoidPtr,
) {
    if image == nil {
        log!("UIImageWriteToSavedPhotosAlbum: nil image, ignoring.");
        // Still invoke callback if requested, with nil error
        invoke_save_completion(
            env,
            completion_target,
            completion_selector,
            image,
            nil,
            context_info,
        );
        return;
    }

    // Get the CGImage backing the UIImage
    let cg_image_ref: CGImageRef = msg![env; image CGImage];

    let save_success = if !cg_image_ref.is_null() {
        let img = cg_image::borrow_image(&env.objc, cg_image_ref);
        let (w, h) = img.dimensions();
        let rgba = img.pixels();
        let stride = w as usize * 4;

        let mut png_data: Vec<u8> = Vec::new();
        let ctx_ptr: *mut std::ffi::c_void = (&mut png_data as *mut Vec<u8>).cast();
        let ok = img.write_png_image(ctx_ptr, w as i32, h as i32, rgba, stride as i32);

        if ok != 0 && !png_data.is_empty() {
            // Generate a unique filename based on timestamp
            let filename = format!(
                "SavedPhoto_{}.png",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            );

            // Save to the app's Documents directory
            let docs_path = format!("/Documents/{}", filename);
            let guest_path = GuestPath::new(&docs_path);
            match env.fs.write(guest_path, &png_data) {
                Ok(()) => {
                    log!(
                        "UIImageWriteToSavedPhotosAlbum: saved {}x{} image to {}",
                        w,
                        h,
                        docs_path
                    );
                    true
                }
                Err(e) => {
                    log!(
                        "UIImageWriteToSavedPhotosAlbum: failed to write {}: {:?}",
                        docs_path,
                        e
                    );
                    false
                }
            }
        } else {
            log!("UIImageWriteToSavedPhotosAlbum: PNG encoding failed.");
            false
        }
    } else {
        log!("UIImageWriteToSavedPhotosAlbum: image has nil CGImage.");
        false
    };

    // Invoke the completion callback if provided.
    // We pass nil for the error on success and a generic NSError on failure
    // (most apps only check whether the error is nil).
    let error: id = if save_success {
        nil
    } else {
        let domain = ns_string::get_static_str(env, "UIImageWriteToSavedPhotosAlbumErrorDomain");
        let error: id = msg_class![env; NSError errorWithDomain:domain
                                                           code:(-1 as NSInteger)
                                                       userInfo:nil];
        error
    };
    invoke_save_completion(
        env,
        completion_target,
        completion_selector,
        image,
        error,
        context_info,
    );
}

/// Invokes the `UIImageWriteToSavedPhotosAlbum` completion callback. Per
/// Apple's documentation the selector must have this signature:
/// `- (void)image:(UIImage *)image didFinishSavingWithError:(NSError *)error
///               contextInfo:(void *)contextInfo;`
fn invoke_save_completion(
    env: &mut Environment,
    target: id,
    selector: SEL,
    image: id,
    error: id,
    context_info: MutVoidPtr,
) {
    if target == nil || selector.is_null() {
        return;
    }
    let _: () = msg_send(env, (target, selector, image, error, context_info));
}

fn UIImagePNGRepresentation(env: &mut Environment, image: id) -> id {
    if image == nil {
        return nil;
    }
    let cg_image: CGImageRef = msg![env; image CGImage];
    if cg_image.is_null() {
        return nil;
    }

    let img = cg_image::borrow_image(&env.objc, cg_image);
    let (w, h) = img.dimensions();
    let rgba = img.pixels();
    let stride = w as usize * 4;

    let mut png_data: Vec<u8> = Vec::new();
    let ctx_ptr: *mut std::ffi::c_void = (&mut png_data as *mut Vec<u8>).cast();
    let ok = img.write_png_image(ctx_ptr, w as i32, h as i32, rgba, stride as i32);
    if ok == 0 {
        return nil;
    }

    let len = png_data.len() as crate::frameworks::foundation::NSUInteger;
    let buf: crate::mem::MutVoidPtr = env.mem.alloc(len as u32).cast();
    env.mem
        .bytes_at_mut(buf.cast(), len as u32)
        .copy_from_slice(&png_data);
    msg_class![env; NSData dataWithBytesNoCopy:buf length:len]
}

fn UIImageJPEGRepresentation(
    env: &mut Environment,
    image: id,
    _compression_quality: CGFloat,
) -> id {
    // В эмуляторе пока фоллбек на PNG, если нет JPEG энкодера
    UIImagePNGRepresentation(env, image)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(UIImagePNGRepresentation(_)),
    export_c_func!(UIImageJPEGRepresentation(_, _)),
    export_c_func!(UIImageWriteToSavedPhotosAlbum(_, _, _, _)),
];

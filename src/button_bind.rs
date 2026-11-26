/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! "Bind buttons": a BlueStacks-style gamepad-to-touch overlay.
//!
//! While the option is on, a button appears at the top centre of the emulated
//! screen. It opens a menu with "Add button", "Clear all" and "Close"; adding
//! a marker drops a control on the screen, which can be dragged into place.
//! Tapping a marker and then pressing a gamepad button binds the two, and
//! while the game runs that gamepad button holds a finger down on the marker
//! (so a marker placed over a game's "Fire" control fires the weapon).
//! Tapping the marker that is waiting for a button again removes it.
//!
//! Bindings are per app and are saved in `HyperHLE_button_bindings.txt`, so
//! they survive a restart, unlike the quick menu's session-only settings.
//!
//! Drawing and input follow `crate::quick_menu`: the overlay is drawn
//! host-side on top of the presented frame and receives raw window-space
//! touch coordinates from `crate::window` before they reach the guest.

use crate::gles::GLES;
use crate::options::Button;
use std::collections::VecDeque;
use std::sync::Mutex;

const COL_PANEL: (f32, f32, f32, f32) = (0.05, 0.05, 0.06, 0.94);
const COL_BUTTON: (f32, f32, f32, f32) = (0.08, 0.08, 0.10, 0.82);
const COL_ROW: (f32, f32, f32, f32) = (0.16, 0.16, 0.18, 1.0);
const COL_TEXT: (f32, f32, f32, f32) = (0.95, 0.95, 0.95, 1.0);
const COL_TEXT_DIM: (f32, f32, f32, f32) = (0.62, 0.62, 0.66, 1.0);
const COL_ACCENT: (f32, f32, f32, f32) = (0.00, 0.75, 0.35, 1.0);
const COL_MARKER: (f32, f32, f32, f32) = (0.10, 0.12, 0.18, 0.55);
const COL_MARKER_BORDER: (f32, f32, f32, f32) = (0.85, 0.85, 0.90, 0.85);
const COL_BINDING: (f32, f32, f32, f32) = (0.55, 0.35, 0.00, 0.75);
const COL_BINDING_BORDER: (f32, f32, f32, f32) = (1.00, 0.80, 0.10, 1.00);
const COL_HELD: (f32, f32, f32, f32) = (0.55, 0.10, 0.10, 0.80);
const COL_HELD_BORDER: (f32, f32, f32, f32) = (1.00, 0.30, 0.25, 1.00);

/// The most markers one app can have. Also keeps the marker ids that travel
/// through [`crate::window::FingerId`] and the widget ids well within range.
const MAX_MARKERS: usize = 32;
/// Distance, in unscaled pixels, a press has to travel before it counts as a
/// drag rather than a tap.
const DRAG_THRESHOLD: f32 = 10.0;
/// Markers never get smaller than this on tiny windows.
const MIN_MARKER_SIZE: f32 = 18.0;

// Widget ids of the overlay chrome. Everything the overlay draws is also a
// touch target; the id travels through `Ui::pending` between down and up.
const W_NONE: u16 = 0;
const W_BUTTON: u16 = 1;
/// Inside the menu but not on a row: swallow the gesture.
const W_MENU: u16 = 2;
const W_ADD: u16 = 10;
const W_CLEAR: u16 = 11;
const W_CLOSE: u16 = 12;
/// Marker ids start here, so `W_MARKER_BASE + index` identifies a marker.
const W_MARKER_BASE: u16 = 100;

/// One on-screen control.
#[derive(Clone, Copy, Debug)]
struct Marker {
    /// Stable id, used as the finger id of the touch this marker sends. It
    /// never changes while the marker exists, so deleting or reordering other
    /// markers cannot make a held touch lose its Up.
    id: u8,
    /// Position in normalized viewport coordinates: 0..1 across the emulated
    /// screen, so the marker stays over the same game control on any window.
    x: f32,
    y: f32,
    /// The gamepad button that holds a finger down here.
    button: Option<Button>,
    /// Whether the game is currently being sent a touch for this marker.
    held: bool,
}

impl Marker {
    fn new(id: u8, x: f32, y: f32) -> Self {
        Self {
            id,
            x,
            y,
            button: None,
            held: false,
        }
    }

    /// The text on the marker. Unbound markers ask for a binding.
    fn label(self) -> String {
        match self.button {
            Some(button) => button.marker_label().to_string(),
            None => "?".to_string(),
        }
    }
}

struct Ui {
    enabled: bool,
    menu_open: bool,
    markers: Vec<Marker>,
    /// The widget the current touch started on, if it started on us.
    pending: u16,
    /// Where the press that started on a marker landed, in viewport pixels.
    press_start: (f32, f32),
    /// Whether that press has moved far enough to be a drag.
    press_moved: bool,
    /// The marker waiting for the next gamepad button, if any.
    bind_target: Option<usize>,
    /// The app the markers belong to (the bindings file key).
    app_id: String,
    /// The id the next added marker gets. Ids are not reused, so a stale
    /// touch can never hit a different marker.
    next_id: u8,
    /// Set when a gamepad press just bound a marker; [gamepad_button] logs
    /// it after dropping the lock.
    just_bound: Option<Button>,
}

impl Ui {
    const fn new() -> Self {
        Self {
            enabled: false,
            menu_open: false,
            markers: Vec::new(),
            pending: W_NONE,
            press_start: (0.0, 0.0),
            press_moved: false,
            bind_target: None,
            app_id: String::new(),
            next_id: 0,
            just_bound: None,
        }
    }
}

static UI: Mutex<Ui> = Mutex::new(Ui::new());

/// Changes the guest still has to be told about: a marker that went away or
/// moved while its gamepad button was held, and whose Up/Move has not been
/// delivered yet. `crate::window` drains this in order.
static PENDING: Mutex<VecDeque<MarkerTouch>> = Mutex::new(VecDeque::new());

/// What a marker wants the guest touch to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerAction {
    /// A gamepad button put a finger down on the marker.
    Down,
    /// The marker was dragged while the finger was down.
    Move,
    /// The button was released, or the marker went away.
    Up,
}

/// A touch change an on-screen marker needs, in normalized coordinates.
#[derive(Clone, Copy, Debug)]
pub struct MarkerTouch {
    /// The marker's id, used as the finger id so each marker is its own
    /// simulated finger.
    pub marker: u8,
    /// Where to touch, normalized to the emulated screen.
    pub position: (f32, f32),
    pub action: MarkerAction,
}

/// Adds a change to the queue. A pending [MarkerAction::Move] for the same
/// marker is replaced instead of stacking up, since the guest only needs the
/// latest position.
fn queue_change(change: MarkerTouch) {
    let Ok(mut pending) = PENDING.lock() else {
        return;
    };
    if change.action == MarkerAction::Move {
        if let Some(last) = pending.back_mut() {
            if last.marker == change.marker && last.action == MarkerAction::Move {
                *last = change;
                return;
            }
        }
    }
    if pending.len() < MAX_MARKERS * 4 {
        pending.push_back(change);
    }
}

/// Starts the overlay for an app, loading the app's saved bindings.
///
/// `enabled` comes from the "Bind buttons" setting (`--bind-buttons`).
pub fn init(enabled: bool, app_id: Option<&str>) {
    let Ok(mut ui) = UI.lock() else { return };
    ui.enabled = enabled;
    ui.menu_open = false;
    ui.pending = W_NONE;
    ui.bind_target = None;
    ui.press_moved = false;
    ui.press_start = (0.0, 0.0);
    ui.markers.clear();
    ui.app_id.clear();
    if let Some(app_id) = app_id {
        ui.app_id.push_str(app_id);
        if enabled {
            ui.markers = load_for_app(app_id);
        }
    }
    ui.next_id = ui
        .markers
        .iter()
        .map(|marker| marker.id)
        .max()
        .map_or(0, |id| id.wrapping_add(1));
    if let Ok(mut pending) = PENDING.lock() {
        pending.clear();
    }
}

/// Hides the overlay. Used by the app picker, which is an `Environment` of
/// its own while the static state outlives a session.
pub fn disable() {
    init(false, None);
}

fn bindings_path() -> std::path::PathBuf {
    bindings_path_in(crate::paths::user_data_base_path().as_ref())
}

fn bindings_path_in(base: &std::path::Path) -> std::path::PathBuf {
    base.join(crate::paths::BUTTON_BINDINGS_FILE)
}

/// `x,y,Button` entries joined with `;`, using the same button names as
/// `--button-to-touch=`. An unbound marker is written as `none`.
fn format_markers(markers: &[Marker]) -> String {
    markers
        .iter()
        .map(|marker| {
            format!(
                "{:.3},{:.3},{}",
                marker.x,
                marker.y,
                marker.button.map_or("none", |button| button.short_name())
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// The inverse of [`format_markers`]. Unknown or malformed entries are
/// skipped rather than failing the whole list, so a hand-edited file or a
/// build that no longer knows a button name still loads what it can.
fn parse_markers(text: &str) -> Vec<Marker> {
    let mut markers = Vec::new();
    for entry in text.split(';') {
        if markers.len() >= MAX_MARKERS {
            break;
        }
        let mut fields = entry.split(',');
        let (Some(x), Some(y), Some(button)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(x), Ok(y)) = (x.trim().parse::<f32>(), y.trim().parse::<f32>()) else {
            continue;
        };
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        let button = match button.trim() {
            "none" | "" => None,
            name => match Button::from_short_name(name) {
                Some(button) => Some(button),
                None => continue,
            },
        };
        let mut marker = Marker::new(
            markers.len() as u8,
            x.clamp(0.0, 1.0),
            y.clamp(0.0, 1.0),
        );
        marker.button = button;
        markers.push(marker);
    }
    markers
}

fn load_for_app(app_id: &str) -> Vec<Marker> {
    load_for_app_in(&bindings_path(), app_id)
}

fn load_for_app_in(path: &std::path::Path, app_id: &str) -> Vec<Marker> {
    if app_id.is_empty() {
        return Vec::new();
    }
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    for line in contents.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == app_id {
            return parse_markers(value);
        }
    }
    Vec::new()
}

/// Rewrites the bindings file, keeping the other apps' lines untouched. An
/// app with no markers simply loses its line.
fn save(ui: &Ui) {
    if ui.app_id.is_empty() {
        return;
    }
    save_to(ui, &bindings_path());
}

fn save_to(ui: &Ui, path: &std::path::Path) {
    let mut others: Vec<String> = Vec::new();
    if let Ok(contents) = std::fs::read_to_string(path) {
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            match line.split_once(':') {
                Some((key, _)) if key.trim() == ui.app_id => {}
                _ => others.push(line.to_string()),
            }
        }
    }
    let mut contents = String::from("# Gamepad button bindings saved by \"Bind buttons\".\n");
    for line in &others {
        contents.push_str(line);
        contents.push('\n');
    }
    if !ui.markers.is_empty() {
        contents.push_str(&ui.app_id);
        contents.push_str(": ");
        contents.push_str(&format_markers(&ui.markers));
        contents.push('\n');
    }
    if let Err(error) = std::fs::write(path, contents) {
        log!("Warning: couldn't save gamepad button bindings: {}", error);
    }
}

/// Forgets that the marker at `index` is holding a touch and asks
/// `crate::window` to send the release.
fn release_marker(ui: &mut Ui, index: usize) {
    if !ui.markers[index].held {
        return;
    }
    let marker = &mut ui.markers[index];
    marker.held = false;
    queue_change(MarkerTouch {
        marker: marker.id,
        position: (marker.x, marker.y),
        action: MarkerAction::Up,
    });
}

fn add_marker(ui: &mut Ui) {
    if ui.markers.len() >= MAX_MARKERS {
        return;
    }
    // Slightly offset successive markers so they do not stack exactly.
    let step = ui.markers.len() as f32;
    let x = (0.5 + 0.05 * (step % 5.0)).clamp(0.05, 0.95);
    let y = (0.55 + 0.05 * (step % 4.0)).clamp(0.05, 0.95);
    let mut id = ui.next_id;
    while ui.markers.iter().any(|marker| marker.id == id) {
        id = id.wrapping_add(1);
    }
    ui.markers.push(Marker::new(id, x, y));
    ui.next_id = id.wrapping_add(1);
    save(ui);
}

fn clear_markers(ui: &mut Ui) {
    let mut index = 0;
    while index < ui.markers.len() {
        release_marker(ui, index);
        index += 1;
    }
    ui.markers.clear();
    ui.bind_target = None;
    save(ui);
}

/// Runs a gamepad button event through the overlay. `Some(..)` means the
/// event belongs to the overlay (the returned changes may be empty, e.g.
/// when the press was consumed by binding a marker), `None` means the normal
/// controller mapping should handle it.
fn apply_gamepad(ui: &mut Ui, button: Button, pressed: bool) -> Option<Vec<MarkerTouch>> {
    if !ui.enabled {
        return None;
    }
    if pressed {
        if let Some(index) = ui.bind_target.take() {
            if let Some(marker) = ui.markers.get_mut(index) {
                marker.button = Some(button);
            }
            ui.just_bound = Some(button);
            save(ui);
            return Some(Vec::new());
        }
    }
    let mut changes = Vec::new();
    let mut matched = false;
    for marker in &mut ui.markers {
        if marker.button != Some(button) {
            continue;
        }
        matched = true;
        if pressed && !marker.held {
            marker.held = true;
            changes.push(MarkerTouch {
                marker: marker.id,
                position: (marker.x, marker.y),
                action: MarkerAction::Down,
            });
        } else if !pressed && marker.held {
            marker.held = false;
            changes.push(MarkerTouch {
                marker: marker.id,
                position: (marker.x, marker.y),
                action: MarkerAction::Up,
            });
        }
    }
    matched.then_some(changes)
}

/// Takes the oldest change (from a deletion or a drag) the guest has not
/// been told about yet.
pub fn take_pending_change() -> Option<MarkerTouch> {
    let Ok(mut pending) = PENDING.lock() else {
        return None;
    };
    pending.pop_front()
}

/// Passes a gamepad button press or release to the overlay. See
/// [`apply_gamepad`].
pub fn gamepad_button(button: Button, pressed: bool) -> Option<Vec<MarkerTouch>> {
    let Ok(mut ui) = UI.lock() else { return None };
    let result = apply_gamepad(&mut ui, button, pressed);
    let bound_now = ui.just_bound.take();
    drop(ui);
    if let Some(bound) = bound_now {
        log!("Bind buttons: a marker is now bound to {:?}.", bound);
    }
    result
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

struct MenuLayout {
    rect: Rect,
    rows: Vec<(u16, Rect)>,
    /// Where the two lines of usage hint go.
    hint: Rect,
}

struct Layout {
    scale: f32,
    button: Rect,
    menu: Option<MenuLayout>,
    markers: Vec<(usize, Rect)>,
}

impl Layout {
    /// The rect of a non-marker widget, for "did the tap end where it began".
    fn row_at(&self, id: u16) -> Option<Rect> {
        if id == W_BUTTON {
            return Some(self.button);
        }
        self.menu
            .as_ref()
            .and_then(|menu| menu.rows.iter().find(|(row_id, _)| *row_id == id))
            .map(|(_, rect)| *rect)
    }
}

fn scale_for(viewport: (u32, u32, u32, u32)) -> f32 {
    let (vw, vh) = (viewport.2 as f32, viewport.3 as f32);
    if vw <= 0.0 || vh <= 0.0 {
        return 1.0;
    }
    (vh / 480.0).min(vw / 320.0).clamp(0.35, 3.0)
}

/// Where everything is, in viewport pixel space. Shared by the draw and the
/// touch paths so the targets line up with what is drawn; coordinates are
/// viewport-relative because the GL viewport already offsets drawing and the
/// touch handlers subtract the origin.
fn compute_layout(ui: &Ui, viewport: (u32, u32, u32, u32)) -> Layout {
    let (vw, vh) = (viewport.2 as f32, viewport.3 as f32);
    let s = scale_for(viewport);
    let button_size = 34.0 * s;
    let button = Rect {
        x: (vw - button_size) / 2.0,
        y: 6.0 * s,
        w: button_size,
        h: button_size,
    };
    let menu = if ui.menu_open {
        let width = (vw - 12.0 * s).clamp(80.0, (240.0 * s).max(80.0));
        let x = (vw - width) / 2.0;
        let y = button.y + button.h + 6.0 * s;
        let header_h = 22.0 * s;
        let row_h = 28.0 * s;
        let hint_h = 37.0 * s;
        let height = header_h + row_h * 3.0 + hint_h + 6.0 * s;
        let ids = [W_ADD, W_CLEAR, W_CLOSE];
        let mut rows = Vec::with_capacity(ids.len());
        for (index, id) in ids.iter().enumerate() {
            rows.push((
                *id,
                Rect {
                    x: x + 6.0 * s,
                    y: y + header_h + index as f32 * row_h + 2.0 * s,
                    w: width - 12.0 * s,
                    h: row_h - 4.0 * s,
                },
            ));
        }
        let hint = Rect {
            x: x + 8.0 * s,
            y: y + header_h + 3.0 * row_h + 2.0 * s,
            w: width - 16.0 * s,
            h: hint_h,
        };
        Some(MenuLayout {
            rect: Rect {
                x,
                y,
                w: width,
                h: height,
            },
            rows,
            hint,
        })
    } else {
        None
    };
    let marker_size = (44.0 * s).max(MIN_MARKER_SIZE);
    let markers = ui
        .markers
        .iter()
        .enumerate()
        .map(|(index, marker)| {
            (
                index,
                Rect {
                    x: marker.x * vw - marker_size / 2.0,
                    y: marker.y * vh - marker_size / 2.0,
                    w: marker_size,
                    h: marker_size,
                },
            )
        })
        .collect();
    Layout {
        scale: s,
        button,
        menu,
        markers,
    }
}

/// Builds the overlay scene for the current state, or `None` when there is
/// nothing to draw.
unsafe fn build_scene(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
) -> Option<crate::trainer_ui::OverlayScene> {
    let ui = UI.lock().ok()?;
    if !ui.enabled {
        return None;
    }
    let layout = compute_layout(&ui, viewport);
    let s = layout.scale;
    let mut scene = crate::trainer_ui::OverlayScene::new();

    // The button at the top centre: a plus sign, or an X while the menu is up.
    let button = layout.button;
    scene.rect(button.x, button.y, button.w, button.h, COL_BUTTON);
    let cx = button.x + button.w / 2.0;
    let cy = button.y + button.h / 2.0;
    if ui.menu_open {
        scene.text(gles, "X", cx - 4.0 * s, cy - 6.0 * s, 13.0 * s, COL_TEXT);
    } else {
        let arm = (button.w * 0.5).max(6.0);
        let thick = (button.w * 0.1).max(2.0);
        scene.rect(cx - arm / 2.0, cy - thick / 2.0, arm, thick, COL_TEXT);
        scene.rect(cx - thick / 2.0, cy - arm / 2.0, thick, arm, COL_TEXT);
    }

    if let Some(menu) = &layout.menu {
        scene.rect(menu.rect.x, menu.rect.y, menu.rect.w, menu.rect.h, COL_PANEL);
        scene.text(
            gles,
            "BIND BUTTONS",
            menu.rect.x + 8.0 * s,
            menu.rect.y + 6.0 * s,
            11.0 * s,
            COL_TEXT_DIM,
        );
        for (id, row) in &menu.rows {
            let (label, color) = match *id {
                W_ADD => ("ADD BUTTON", COL_ACCENT),
                W_CLEAR => ("CLEAR ALL", COL_ROW),
                _ => ("CLOSE", COL_ROW),
            };
            scene.rect(row.x, row.y, row.w, row.h, color);
            scene.text(gles, label, row.x + 6.0 * s, row.y + 5.0 * s, 11.0 * s, COL_TEXT);
        }
        let hint = menu.hint;
        for (line, text) in [
            "TAP: PICK PAD BTN",
            "DRAG: MOVE",
            "TAP ARMED: DELETE",
        ]
        .iter()
        .enumerate()
        {
            scene.text(
                gles,
                text,
                hint.x,
                hint.y + line as f32 * 11.0 * s,
                9.0 * s,
                COL_TEXT_DIM,
            );
        }
    }

    for (index, rect) in &layout.markers {
        let Some(marker) = ui.markers.get(*index) else {
            continue;
        };
        let armed = ui.bind_target == Some(*index);
        let held = marker.held;
        let (fill, border) = if armed {
            (COL_BINDING, COL_BINDING_BORDER)
        } else if held {
            (COL_HELD, COL_HELD_BORDER)
        } else {
            (COL_MARKER, COL_MARKER_BORDER)
        };
        scene.rect(rect.x, rect.y, rect.w, rect.h, border);
        let inset = (rect.w * 0.08).max(1.0);
        scene.rect(
            rect.x + inset,
            rect.y + inset,
            rect.w - 2.0 * inset,
            rect.h - 2.0 * inset,
            fill,
        );
        let label = if armed {
            "?".to_string()
        } else {
            marker.label()
        };
        // Centred by eye: the atlas has no side bearings to measure with.
        let label_size = 11.0 * s;
        let label_width = (label.chars().count() as f32) * label_size * 0.6;
        scene.text(
            gles,
            &label,
            rect.x + (rect.w - label_width) / 2.0,
            rect.y + rect.h * 0.5 - label_size / 2.0,
            label_size,
            COL_TEXT,
        );
    }
    Some(scene)
}

/// Draws the overlay on a fixed-function (GLES 1.x) present path.
pub unsafe fn draw(gles: &mut dyn GLES, viewport: (u32, u32, u32, u32)) {
    let Some(scene) = build_scene(gles, viewport) else {
        return;
    };
    crate::trainer_ui::draw_scene(gles, viewport, &scene);
}

/// Draws the overlay on a native OpenGL ES 2.0 present path.
pub unsafe fn draw_es2(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    context_token: usize,
) {
    crate::trainer_ui::prepare_overlay_context(context_token);
    let Some(scene) = build_scene(gles, viewport) else {
        return;
    };
    crate::trainer_ui::draw_scene_es2(gles, viewport, context_token, &scene);
}

fn touch_down_inner_abs(
    ui: &mut Ui,
    abs: (f32, f32),
    viewport: (u32, u32, u32, u32),
) -> bool {
    if !ui.enabled {
        return false;
    }
    let layout = compute_layout(ui, viewport);
    let p = (abs.0 - viewport.0 as f32, abs.1 - viewport.1 as f32);
    if layout.button.contains(p.0, p.1) {
        ui.pending = W_BUTTON;
        return true;
    }
    if let Some(menu) = &layout.menu {
        // The menu is tapped through the bottom of the screen on a short
        // window, so it wins over a marker that happens to share a spot.
        if menu.rect.contains(p.0, p.1) {
            for (id, rect) in &menu.rows {
                if rect.contains(p.0, p.1) {
                    ui.pending = *id;
                    return true;
                }
            }
            ui.pending = W_MENU;
            return true;
        }
    }
    // Newest markers are on top, so hit-test them in reverse order.
    for (index, rect) in layout.markers.iter().rev() {
        if rect.contains(p.0, p.1) {
            ui.pending = W_MARKER_BASE + *index as u16;
            ui.press_start = p;
            ui.press_moved = false;
            return true;
        }
    }
    if ui.menu_open {
        // A tap on the game behind an open menu closes it instead of
        // reaching the guest; W_MENU keeps swallowing the rest of the
        // gesture so the guest never sees a lone TouchesUp.
        ui.menu_open = false;
        ui.pending = W_MENU;
        return true;
    }
    false
}

fn touch_motion_inner_abs(
    ui: &mut Ui,
    abs: (f32, f32),
    viewport: (u32, u32, u32, u32),
) -> bool {
    if !ui.enabled || ui.pending == W_NONE {
        return false;
    }
    if ui.pending < W_MARKER_BASE {
        return true;
    }
    let index = (ui.pending - W_MARKER_BASE) as usize;
    if index >= ui.markers.len() {
        ui.pending = W_NONE;
        return false;
    }
    let p = (abs.0 - viewport.0 as f32, abs.1 - viewport.1 as f32);
    if !ui.press_moved {
        let distance = (p.0 - ui.press_start.0).hypot(p.1 - ui.press_start.1);
        if distance < DRAG_THRESHOLD * scale_for(viewport) {
            return true;
        }
        ui.press_moved = true;
    }
    let (vw, vh) = (viewport.2.max(1) as f32, viewport.3.max(1) as f32);
    let marker = &mut ui.markers[index];
    marker.x = (p.0 / vw).clamp(0.02, 0.98);
    marker.y = (p.1 / vh).clamp(0.02, 0.98);
    if marker.held {
        // The finger follows the marker, like a real finger being dragged.
        queue_change(MarkerTouch {
            marker: marker.id,
            position: (marker.x, marker.y),
            action: MarkerAction::Move,
        });
    }
    true
}

fn touch_up_inner_abs(ui: &mut Ui, abs: (f32, f32), viewport: (u32, u32, u32, u32)) -> bool {
    if !ui.enabled {
        return false;
    }
    let pending = std::mem::replace(&mut ui.pending, W_NONE);
    if pending == W_NONE {
        return false;
    }
    let layout = compute_layout(ui, viewport);
    let p = (abs.0 - viewport.0 as f32, abs.1 - viewport.1 as f32);
    if pending >= W_MARKER_BASE {
        let index = (pending - W_MARKER_BASE) as usize;
        let moved = std::mem::replace(&mut ui.press_moved, false);
        if index >= ui.markers.len() {
            return true;
        }
        if moved {
            save(ui);
        } else if ui.bind_target == Some(index) {
            // Tapping the marker that is waiting for a button deletes it.
            ui.bind_target = None;
            release_marker(ui, index);
            ui.markers.remove(index);
            save(ui);
        } else {
            ui.bind_target = Some(index);
            ui.menu_open = false;
        }
        return true;
    }
    let hit = layout
        .row_at(pending)
        .is_some_and(|rect| rect.contains(p.0, p.1));
    match pending {
        W_BUTTON => {
            if hit {
                ui.menu_open = !ui.menu_open;
            }
        }
        W_ADD => {
            if hit {
                add_marker(ui);
            }
        }
        W_CLEAR => {
            if hit {
                clear_markers(ui);
            }
        }
        W_CLOSE => {
            if hit {
                ui.menu_open = false;
            }
        }
        _ => {}
    }
    true
}

/// Takes a touch that lands on the overlay's button, menu or a marker.
///
/// `abs` is in window pixels, like the quick menu's handlers; the internal
/// versions work in viewport pixels, which is what the layout uses.
pub fn touch_down(abs: (f32, f32), viewport: (u32, u32, u32, u32)) -> bool {
    let Ok(mut ui) = UI.lock() else { return false };
    touch_down_inner_abs(&mut ui, abs, viewport)
}

/// Swallows motion while a press that started on the overlay is in progress.
pub fn touch_motion(abs: (f32, f32), viewport: (u32, u32, u32, u32)) -> bool {
    let Ok(mut ui) = UI.lock() else { return false };
    touch_motion_inner_abs(&mut ui, abs, viewport)
}

/// Finishes the gesture: taps act, releases elsewhere do nothing.
pub fn touch_up(abs: (f32, f32), viewport: (u32, u32, u32, u32)) -> bool {
    let Ok(mut ui) = UI.lock() else { return false };
    touch_up_inner_abs(&mut ui, abs, viewport)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: (u32, u32, u32, u32) = (0, 0, 480, 320);

    fn ui_with_marker(x: f32, y: f32) -> Ui {
        let mut ui = Ui::new();
        ui.enabled = true;
        ui.markers.push(Marker::new(ui.next_id, x, y));
        ui.next_id = ui.next_id.wrapping_add(1);
        ui
    }

    fn tap(ui: &mut Ui, at: (f32, f32), viewport: (u32, u32, u32, u32)) {
        assert!(touch_down_inner_abs(ui, at, viewport));
        assert!(touch_up_inner_abs(ui, at, viewport));
    }

    #[test]
    fn the_overlay_button_sits_at_the_top_centre() {
        let ui = Ui::new();
        let layout = compute_layout(&ui, (12, 20, 480, 320));
        let center = layout.button.x + layout.button.w / 2.0;
        assert!((center - 240.0).abs() < 1.0);
        assert!(layout.button.y < 16.0);
        assert!(layout.menu.is_none());
    }

    #[test]
    fn the_open_menu_fits_inside_the_viewport() {
        let mut ui = Ui::new();
        ui.menu_open = true;
        for viewport in [
            (0, 0, 480, 320),
            (0, 0, 320, 480),
            (0, 0, 1024, 768),
            (0, 0, 120, 90),
        ] {
            let layout = compute_layout(&ui, viewport);
            let menu = layout.menu.expect("an open menu has a layout");
            assert!(menu.rect.x >= -0.5);
            assert!(menu.rect.y >= 0.0);
            assert!(menu.rect.x + menu.rect.w <= viewport.2 as f32 + 0.5);
            assert!(menu.rect.y + menu.rect.h <= viewport.3 as f32 + 0.5);
            for (_, row) in &menu.rows {
                assert!(row.y >= menu.rect.y);
                assert!(row.y + row.h <= menu.rect.y + menu.rect.h);
            }
            assert!(menu.hint.y + menu.hint.h <= menu.rect.y + menu.rect.h + 0.5);
        }
    }

    #[test]
    fn markers_are_centred_on_their_normalized_position() {
        let ui = ui_with_marker(0.5, 0.5);
        let layout = compute_layout(&ui, VIEWPORT);
        let (_, rect) = layout.markers[0];
        assert!(((rect.x + rect.w / 2.0) - 240.0).abs() < 0.5);
        assert!(((rect.y + rect.h / 2.0) - 160.0).abs() < 0.5);
    }

    #[test]
    fn marker_serialization_round_trips() {
        let mut first = Marker::new(0, 0.25, 0.75);
        first.button = Some(Button::X);
        let markers = [first, Marker::new(1, 0.5, 0.1)];
        let text = format_markers(&markers);
        let parsed = parse_markers(&text);
        assert_eq!(parsed.len(), 2);
        assert!((parsed[0].x - 0.25).abs() < 0.001);
        assert!((parsed[0].y - 0.75).abs() < 0.001);
        assert_eq!(parsed[0].button, Some(Button::X));
        assert_eq!(parsed[1].button, None);
    }

    #[test]
    fn the_bindings_file_keeps_other_apps_and_drops_emptied_ones() {
        let dir = std::env::temp_dir().join(format!(
            "hyperhle_bind_buttons_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = bindings_path_in(&dir);
        std::fs::write(
            &path,
            "# comment\ncom.other.game: 0.100,0.200,A\ncom.test.game: 0.500,0.500,B\n",
        )
        .unwrap();
        let mut ui = Ui::new();
        ui.app_id = "com.test.game".to_string();
        ui.markers.push(Marker::new(0, 0.25, 0.30));
        ui.markers[0].button = Some(Button::Y);
        save_to(&ui, &path);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("com.other.game: 0.100,0.200,A"));
        assert!(saved.contains("com.test.game: 0.250,0.300,Y"));
        let loaded = load_for_app_in(&path, "com.test.game");
        assert_eq!(loaded.len(), 1);
        assert!((loaded[0].y - 0.30).abs() < 0.001);
        assert_eq!(loaded[0].button, Some(Button::Y));
        // Clearing every marker removes the app's line but nobody else's.
        ui.markers.clear();
        save_to(&ui, &path);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("com.test.game"));
        assert!(saved.contains("com.other.game"));
        assert!(load_for_app_in(&path, "com.test.game").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parsing_skips_entries_it_does_not_understand() {
        let parsed = parse_markers("0.1,0.2,X; nonsense ;0.3,0.4,NoSuchButton;0.6,0.7,A");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].button, Some(Button::X));
        assert_eq!(parsed[1].button, Some(Button::A));
    }

    #[test]
    fn tapping_a_marker_arms_it_and_the_next_pad_button_binds_it() {
        let mut ui = ui_with_marker(0.5, 0.5);
        tap(&mut ui, (240.0, 160.0), VIEWPORT);
        assert_eq!(ui.bind_target, Some(0));
        let changes = apply_gamepad(&mut ui, Button::X, true).expect("the press is ours");
        assert!(changes.is_empty());
        assert_eq!(ui.markers[0].button, Some(Button::X));
        assert_eq!(ui.bind_target, None);
    }

    #[test]
    fn a_bound_button_holds_a_touch_at_the_marker() {
        let mut ui = ui_with_marker(0.25, 0.75);
        ui.markers[0].button = Some(Button::A);
        let changes = apply_gamepad(&mut ui, Button::A, true).expect("the press is ours");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Down);
        assert!((changes[0].position.0 - 0.25).abs() < 0.001);
        assert!(ui.markers[0].held);
        // A second Down without an Up must not produce a duplicate touch.
        let again = apply_gamepad(&mut ui, Button::A, true).expect("still ours");
        assert!(again.is_empty());
        let changes = apply_gamepad(&mut ui, Button::A, false).expect("the release is ours");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Up);
        assert!(!ui.markers[0].held);
        assert!(apply_gamepad(&mut ui, Button::B, false).is_none());
    }

    #[test]
    fn an_up_always_follows_its_down_even_if_the_marker_was_deleted() {
        PENDING.lock().unwrap().clear();
        let mut ui = ui_with_marker(0.5, 0.5);
        ui.markers[0].button = Some(Button::X);
        let down = apply_gamepad(&mut ui, Button::X, true).expect("the press is ours");
        assert_eq!(down[0].action, MarkerAction::Down);
        // "CLEAR ALL" while the button is still held.
        clear_markers(&mut ui);
        assert!(ui.markers.is_empty());
        let owed = take_pending_change().expect("the Up is still owed");
        assert_eq!(owed.action, MarkerAction::Up);
        assert_eq!(owed.marker, down[0].marker);
        assert!(take_pending_change().is_none());
    }

    #[test]
    fn dragging_a_held_marker_queues_a_single_move_with_the_latest_position() {
        PENDING.lock().unwrap().clear();
        let mut ui = ui_with_marker(0.5, 0.5);
        ui.markers[0].button = Some(Button::B);
        apply_gamepad(&mut ui, Button::B, true);
        assert!(touch_down_inner_abs(&mut ui, (240.0, 160.0), VIEWPORT));
        assert!(touch_motion_inner_abs(&mut ui, (260.0, 170.0), VIEWPORT));
        assert!(touch_motion_inner_abs(&mut ui, (300.0, 200.0), VIEWPORT));
        assert!(touch_up_inner_abs(&mut ui, (300.0, 200.0), VIEWPORT));
        let owed = take_pending_change().expect("a Move is owed");
        assert_eq!(owed.action, MarkerAction::Move);
        assert!((owed.position.0 - 300.0 / 480.0).abs() < 0.01);
        assert!(take_pending_change().is_none());
        // The finger is still down, so a release is still owed later.
        let up = apply_gamepad(&mut ui, Button::B, false).expect("the release is ours");
        assert_eq!(up[0].action, MarkerAction::Up);
    }

    #[test]
    fn dragging_a_marker_moves_it() {
        let mut ui = ui_with_marker(0.5, 0.5);
        assert!(touch_down_inner_abs(&mut ui, (240.0, 160.0), VIEWPORT));
        assert!(touch_motion_inner_abs(&mut ui, (300.0, 200.0), VIEWPORT));
        assert!(touch_up_inner_abs(&mut ui, (300.0, 200.0), VIEWPORT));
        assert!((ui.markers[0].x - 300.0 / 480.0).abs() < 0.01);
        assert!((ui.markers[0].y - 200.0 / 320.0).abs() < 0.01);
        // A drag must not arm the marker for binding.
        assert_eq!(ui.bind_target, None);
    }

    #[test]
    fn tapping_an_armed_marker_again_removes_it() {
        let mut ui = ui_with_marker(0.5, 0.5);
        tap(&mut ui, (240.0, 160.0), VIEWPORT);
        assert_eq!(ui.bind_target, Some(0));
        tap(&mut ui, (240.0, 160.0), VIEWPORT);
        assert!(ui.markers.is_empty());
        assert_eq!(ui.bind_target, None);
    }

    #[test]
    fn the_menu_adds_and_clears_markers() {
        let mut ui = Ui::new();
        ui.enabled = true;
        tap(&mut ui, (240.0, 12.0), VIEWPORT);
        assert!(ui.menu_open);
        // "ADD BUTTON" is the first row; tapping it adds one marker.
        let layout = compute_layout(&ui, VIEWPORT);
        let menu = layout.menu.expect("the menu is open");
        let add = menu.rows[0].1;
        let at = (add.x + add.w / 2.0, add.y + add.h / 2.0);
        tap(&mut ui, at, VIEWPORT);
        assert_eq!(ui.markers.len(), 1);
        // "CLEAR ALL" removes it again.
        let clear = menu.rows[1].1;
        let at = (clear.x + clear.w / 2.0, clear.y + clear.h / 2.0);
        tap(&mut ui, at, VIEWPORT);
        assert!(ui.markers.is_empty());
    }

    #[test]
    fn a_tap_outside_an_open_menu_closes_it_without_reaching_the_game() {
        let mut ui = Ui::new();
        ui.enabled = true;
        ui.menu_open = true;
        assert!(touch_down_inner_abs(&mut ui, (240.0, 300.0), VIEWPORT));
        assert!(!ui.menu_open);
        assert!(touch_motion_inner_abs(&mut ui, (240.0, 301.0), VIEWPORT));
    }

    #[test]
    fn a_disabled_overlay_ignores_touches_and_pad_buttons() {
        let mut ui = ui_with_marker(0.5, 0.5);
        ui.enabled = false;
        ui.markers[0].button = Some(Button::Y);
        assert!(!touch_down_inner_abs(&mut ui, (240.0, 160.0), VIEWPORT));
        assert!(apply_gamepad(&mut ui, Button::Y, true).is_none());
    }
}

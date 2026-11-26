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
const COL_PAD: (f32, f32, f32, f32) = (0.90, 0.92, 0.98, 0.62);
const COL_PAD_KNOB: (f32, f32, f32, f32) = (0.35, 0.55, 0.85, 0.55);
const COL_PAD_KNOB_BORDER: (f32, f32, f32, f32) = (0.92, 0.95, 1.00, 0.90);
const COL_PAD_ARROW: (f32, f32, f32, f32) = (0.35, 0.95, 0.55, 1.00);
const COL_PAD_ARROW_IDLE: (f32, f32, f32, f32) = (0.85, 0.88, 0.95, 0.85);
const COL_MARKER: (f32, f32, f32, f32) = (0.10, 0.12, 0.18, 0.55);
const COL_MARKER_BORDER: (f32, f32, f32, f32) = (0.85, 0.85, 0.90, 0.85);
const COL_BINDING: (f32, f32, f32, f32) = (0.55, 0.35, 0.00, 0.75);
const COL_BINDING_BORDER: (f32, f32, f32, f32) = (1.00, 0.80, 0.10, 1.00);
const COL_HELD: (f32, f32, f32, f32) = (0.55, 0.10, 0.10, 0.80);
const COL_HELD_BORDER: (f32, f32, f32, f32) = (1.00, 0.30, 0.25, 1.00);

/// The most markers one app can have. Also keeps the marker ids that travel
/// through [`crate::window::FingerId`] and the widget ids well within range.
const MAX_MARKERS: usize = 32;
/// Shown in the menu's header. Bump it whenever the pad gestures change: the
/// difference between a drag that starts at the rim and one that starts in
/// the middle is invisible in a screenshot but decides whether walking works,
/// so "is this the build with the fix" has to be answerable without guessing.
const OVERLAY_REVISION: &str = "PADS R7 SWIPE";
/// Distance, in unscaled pixels, a press has to travel before it counts as a
/// drag rather than a tap.
const DRAG_THRESHOLD: f32 = 10.0;
/// Markers never get smaller than this on tiny windows.
const MIN_MARKER_SIZE: f32 = 18.0;
/// The marker size in unscaled pixels. It grows with the screen, so it stays
/// big enough for a two-character label like "RT" or "<LS" at any scale.
const MARKER_SIZE: f32 = 46.0;

/// Where the pads sit by default: bottom-left and bottom-right, which is
/// where thumbs already rest in a two-hands-on-a-pad game.
const PAD_DEFAULT_CENTERS: [(f32, f32); PAD_COUNT] = [(0.20, 0.70), (0.80, 0.70)];
/// Pad radius as a fraction of the shorter viewport side.
const PAD_DEFAULT_RADIUS: f32 = 0.15;
const PAD_MIN_RADIUS: f32 = 16.0;
/// Two pads cannot overlap more than this, or their knobs would fight.
const PAD_MIN_GAP: f32 = 6.0;
/// How far the knob travels from the centre of its pad.
const KNOB_TRAVEL: f32 = 0.62;
/// Distance a press must travel before it counts as a size change rather than
/// a tap, in unscaled pixels.
const RESIZE_THRESHOLD: f32 = 6.0;
/// The pad counts as pushed once the knob is this far out (0..1 of travel).
/// Below it the pad is idle, so a wobbly thumb does not make the character
/// creep. It is a scalar per axis, so diagonals come out even.
const PAD_ACTIVATION: f32 = 0.18;
/// A push has to change by this much before the game is sent another Move.
const MOVE_EPSILON: f32 = 0.02;
/// How long the stick has to stay at rest before the pad lifts its finger.
/// Sweeping from forward to backward goes through the middle, and a real
/// thumb never leaves the glass doing that; lifting the finger there makes the
/// game stop the character until the stick is released.
const PAD_RELEASE_DELAY: f32 = 0.12;

// Camera ("look") mode.
/// How fast a fully pushed right stick drags the look finger, in fractions of
/// the viewport per second.
const LOOK_SPEED: f32 = 0.55;

/// Where the camera's drag box sits, and how big it is. It is deliberately not
/// the middle of the screen: on a phone shooter the middle of the screen is
/// where the player is aiming, and the left of it is the game's own movement
/// joystick. A small box on the right-hand third is the area a player's thumb
/// already owns for turning, and it keeps the camera's finger well away from
/// the game's joystick so moving the camera cannot nudge the character.
const LOOK_BOX: (f32, f32, f32) = (0.72, 0.50, 0.12);
/// Where the look finger starts: the middle of that box.
const LOOK_CENTRE: (f32, f32) = (LOOK_BOX.0, LOOK_BOX.1);
/// A look drag only counts while the stick is past this.
const LOOK_DEADZONE: f32 = 0.15;

/// How visible markers and pads are while "hide buttons" is on: faint enough
/// to stop covering the game, strong enough to see where they are.
const HIDDEN_ALPHA: f32 = 0.18;
const PAD_COUNT: usize = 2;

// Widget ids of the overlay chrome. Everything the overlay draws is also a
// touch target; the id travels through `Ui::pending` between down and up.
const W_NONE: u16 = 0;
const W_BUTTON: u16 = 1;
/// Inside the menu but not on a row: swallow the gesture.
const W_MENU: u16 = 2;
const W_ADD: u16 = 10;
const W_CLEAR: u16 = 11;
const W_CLOSE: u16 = 12;
const W_LOOK: u16 = 13;
const W_HIDE: u16 = 14;
const W_ADD_CIRCLE: u16 = 15;
const W_ADD_CROSS: u16 = 16;
const W_REACH_CIRCLE: u16 = 17;
const W_REACH_CROSS: u16 = 18;
/// Marker ids start here, so `W_MARKER_BASE + index` identifies a marker.
const W_MARKER_BASE: u16 = 100;
/// Pad ids start here: `W_PAD_BASE + index` is a pad's knob, plus `PAD_COUNT`
/// is its gauge / size handle.
const W_PAD_BASE: u16 = 200;

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
            Some(button) => button.display_name().to_string(),
            None => "?".to_string(),
        }
    }
}

/// One of the two movement pads: a circle or a cross driven by a stick, which
/// sends one analogue touch to the game.
///
/// A pad is not a marker: markers inject a finger at a fixed spot, while a pad
/// is a virtual analogue stick. The finger goes down in the middle of the pad
/// and is dragged in the direction the stick is pushed, as far as it is
/// pushed: a light push is a short drag, a full push reaches the rim. Because
/// there is exactly one finger for the whole pad, it can slide from up to
/// down-left without ever being lifted, which is what a game's own stick does
/// and what walking needs.
#[derive(Clone, Copy, Debug)]
struct Pad {
    /// Which stick drives this pad, and which glyph is drawn.
    kind: PadKind,
    /// Where the middle of the pad is, normalized to the viewport. The touch
    /// it sends and the knob both hang off this, so resizing or moving the
    /// pad keeps sending the same shape of gesture.
    cx: f32,
    cy: f32,
    /// The radius, as a fraction of the shorter viewport side.
    radius: f32,
    /// The stick's push, -1..1 on each axis, with the inner threshold applied.
    /// Only the circle uses this: its finger is analogue.
    axis: (f32, f32),
    /// How far the stick is pushed in total, 0..1. Only the circle uses this.
    push: f32,
    /// Which of the eight directions is held, or `None` at rest. Only the cross
    /// uses this: it is a D-pad, and each direction is a swipe outwards from
    /// the middle of the pad.
    held: Option<usize>,
    /// The cross's swipe: where the finger is right now, and how long the
    /// finger has been travelling. A swipe is put down at the middle, moved
    /// outwards and lifted, and its speed is what a wheel's flick measures, so
    /// the pad needs to know how far through the travel it is.
    swipe: (f32, f32),
    swipe_elapsed: f32,
    /// The direction the current (or last) swipe travels towards. Remembered
    /// separately from `held` because the button can be let go before the
    /// finger has finished travelling, and the travel still has to be played
    /// out: a tap that lifts halfway selects nothing.
    swipe_dir: usize,
    /// Whether the pad's finger is down right now.
    touching: bool,
    /// How long the stick has been at rest. The finger is only lifted once
    /// this passes [PAD_RELEASE_DELAY], so sweeping through the middle to
    /// reverse direction does not count as letting go of the stick.
    rest: f32,
    /// Whether the pad exists. Pads are added from the menu like markers, so
    /// an app that does not need them keeps the screen clear.
    enabled: bool,
    /// How far a full push moves the finger, as a fraction of the normal
    /// travel. Games differ in how big their own stick's zone is; if a full
    /// push on the gamepad overshoots it, the finger can leave the game's
    /// stick and the character stops. Lowering this keeps the finger inside.
    reach: f32,
}

impl Pad {
    const fn new(kind: PadKind, cx: f32, cy: f32) -> Self {
        Self {
            kind,
            cx,
            cy,
            radius: PAD_DEFAULT_RADIUS,
            axis: (0.0, 0.0),
            push: 0.0,
            held: None,
            swipe: (cx + CROSS_START, cy),
            swipe_elapsed: 0.0,
            swipe_dir: 0,
            touching: false,
            rest: 0.0,
            enabled: false,
            reach: 1.0,
        }
    }

    /// The pad's radius in viewport pixels.
    fn radius_px(self, viewport: (u32, u32, u32, u32)) -> f32 {
        let shortest = viewport.2.min(viewport.3) as f32;
        (self.radius * shortest).max(PAD_MIN_RADIUS)
    }

    /// Where the arrow of `direction` sits, as a fraction of the pad radius
    /// from the centre.
    fn point(self, direction: usize) -> (f32, f32) {
        let (dx, dy) = PAD_DIRECTIONS[direction];
        (self.cx + dx * self.radius * PAD_RIM, self.cy + dy * self.radius * PAD_RIM)
    }

    /// The analogue finger's position for the current push: on the stick's own
    /// angle, as far out as the stick is pushed. Used by the circle.
    fn finger_analogue(self) -> (f32, f32) {
        let (x, y) = normalize_push(self.axis.0, self.axis.1);
        let reach = self.push.clamp(0.0, 1.0) * self.reach.clamp(0.0, 1.0);
        (
            self.cx + x * reach * self.radius * PAD_RIM,
            self.cy + y * reach * self.radius * PAD_RIM,
        )
    }

    /// Where the pad's touch sits for a held direction: a fixed spot on that
    /// direction's arrow, on the rim. It never moves while the direction is
    /// held, which is the whole point of a D-pad touch: the game's own
    /// joystick sees a finger resting in one place.
    /// Where the cross's finger starts: just off the middle, so the game sees
    /// a finger that has room to travel outwards in any direction.
    fn swipe_start(self) -> (f32, f32) {
        let reach = self.reach.clamp(0.0, 1.0);
        (
            self.cx + CROSS_START * reach * self.radius,
            self.cy + CROSS_START * reach * self.radius,
        )
    }

    /// Where the cross's finger has got to on the way out: `t` is how far
    /// through the swipe it is, 0..1.
    fn swipe_at(self, direction: usize, t: f32) -> (f32, f32) {
        let (x, y) = PAD_DIRECTIONS_8[direction];
        let reach = self.reach.clamp(0.0, 1.0);
        let distance = (CROSS_START + (CROSS_SWIPE - CROSS_START) * t.clamp(0.0, 1.0))
            * reach
            * self.radius;
        (self.cx + x * distance, self.cy + y * distance)
    }
}

/// The two pads. They are driven by different things on purpose: the circle is
/// the left stick, and the cross is the D-pad. Nothing has to be shared between
/// them that way, so neither can take the other's input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PadKind {
    /// Left stick, drawn as a ring. Analogue: the finger travels with the push.
    Circle,
    /// D-pad, drawn as a cross. Four buttons, one fixed touch each, for games
    /// with a weapon wheel or any other four-way on-screen choice.
    Cross,
}

impl PadKind {
    fn pad(self) -> usize {
        match self {
            PadKind::Circle => 0,
            PadKind::Cross => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            PadKind::Circle => "CIRCLE",
            PadKind::Cross => "CROSS",
        }
    }
}

/// Up, down, left, right: the order of [Pad::dir], [Pad::down] and
/// [Pad::point], and of the button each direction maps to.
const PAD_DIRECTIONS: [(f32, f32); 4] = [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)];
/// The circle's own direction buttons, in the order of [PAD_DIRECTIONS]. A
/// marker bound to one of these takes the left stick from the circle; the
/// cross does not use them, because it reads the D-pad instead.
const CIRCLE_DIRECTION_BUTTONS: [Button; 4] = [
    Button::LeftStickUp,
    Button::LeftStickDown,
    Button::LeftStickLeft,
    Button::LeftStickRight,
];
/// The four D-pad buttons and the bit each one sets on the cross pad.
///
/// The cross reads these and nothing else. That is the whole reason it can
/// exist next to camera mode: the camera owns the right stick, the cross owns
/// the D-pad, and neither can disturb the other.
const DPAD_BUTTONS: [(Button, u8); 4] = [
    (Button::DPadUp, 1),
    (Button::DPadRight, 2),
    (Button::DPadDown, 4),
    (Button::DPadLeft, 8),
];
/// How far out the direction arrows sit, as a fraction of the pad radius.
const PAD_RIM: f32 = 0.78;
/// How far out from the middle of the cross its finger starts, as a fraction
/// of the radius. A weapon wheel is a wheel: it reads the *direction a finger
/// travels from its middle*, not where the finger ends up. Starting on the rim
/// would make every press look like a completed swipe with no travel at all.
const CROSS_START: f32 = 0.05;
/// How far a D-pad swipe travels, as a fraction of the radius. Long enough to
/// clear a wheel's dead zone and reach the weapon ring.
const CROSS_SWIPE: f32 = PAD_RIM;

/// The eight directions a cross pad can send, going clockwise from up. Each is
/// a unit vector in screen space (Y is down). A D-pad has to offer diagonals
/// as their own directions, or pushing up-right would have to pick one of the
/// two and the character would not walk diagonally.
const PAD_DIRECTIONS_8: [(f32, f32); 8] = [
    (0.0, -1.0),
    (std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2),
    (1.0, 0.0),
    (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2),
    (0.0, 1.0),
    (-std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2),
    (-1.0, 0.0),
    (-std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2),
];

/// Picks the nearest of the eight directions for a stick vector, or `None` when
/// the stick is inside [PAD_ACTIVATION].
fn dir8(x: f32, y: f32) -> Option<usize> {
    if x.hypot(y) <= PAD_ACTIVATION {
        return None;
    }
    let angle = y.atan2(x);
    // Each sector is 45 degrees wide, centred on a direction. The directions
    // start at up (-90 degrees) and go clockwise, matching screen coordinates
    // where positive Y is down.
    let sector = (angle + std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_8)
        .rem_euclid(std::f32::consts::TAU);
    Some(((sector / std::f32::consts::FRAC_PI_4).floor() as usize).min(7))
}

/// The direction a D-pad bitmask means, as a [PAD_DIRECTIONS_8] index.
///
/// Two buttons at once is a diagonal, which is what a weapon wheel wants when
/// the player rolls the D-pad round. Opposite buttons cancel: holding left and
/// right is not a direction, and saying so keeps the finger from sitting on a
/// diagonal nobody asked for.
fn dpad_dir(mask: u8) -> Option<usize> {
    let up = mask & 1 != 0;
    let right = mask & 2 != 0;
    let down = mask & 4 != 0;
    let left = mask & 8 != 0;
    let y = down as i8 - up as i8;
    let x = right as i8 - left as i8;
    match (x, y) {
        (0, 0) => None,
        (0, -1) => Some(0),
        (1, -1) => Some(1),
        (1, 0) => Some(2),
        (1, 1) => Some(3),
        (0, 1) => Some(4),
        (-1, 1) => Some(5),
        (-1, 0) => Some(6),
        (-1, -1) => Some(7),
        _ => None,
    }
}

/// The D-pad bits a marker has claimed, so the cross leaves those buttons to it.
fn dpad_claimed(ui: &Ui) -> u8 {
    let mut mask = 0;
    for marker in &ui.markers {
        let Some(button) = marker.button else {
            continue;
        };
        for (dpad_button, bit) in DPAD_BUTTONS {
            if dpad_button == button {
                mask |= bit;
            }
        }
    }
    mask
}

/// Applies one D-pad button edge to the cross pad and returns the touches it
/// owes, or `None` when the button is not the cross's to handle.
///
/// Called straight from [apply_gamepad], on the edge itself. The alternative --
/// setting a bit and letting the next pad poll notice -- loses a tap that goes
/// down and up inside one event poll, which is exactly how a weapon wheel is
/// tapped. `None` means "carry on and treat this as an ordinary button", which
/// is what lets a marker bind the button and take it over.
fn dpad_edge(ui: &mut Ui, bit: u8, pressed: bool) -> Option<Vec<MarkerTouch>> {
    // Keep the held state accurate either way: it is what the menu and the pad
    // draw from, and a stale bit would show a direction nobody is pressing.
    if pressed {
        ui.dpad |= bit;
    } else {
        ui.dpad &= !bit;
    }
    let cross = PadKind::Cross.pad();
    if !ui.pads[cross].enabled {
        // The cross is off, so nothing here is the cross's business: leave the
        // button to the markers and the rest of the controller.
        return None;
    }
    // A marker that has this button gets it instead: it was bound on purpose.
    let claimed = dpad_claimed(ui) & bit != 0;
    if claimed {
        return None;
    }
    let direction = dpad_dir(ui.dpad & !dpad_claimed(ui));
    let mut changes = Vec::new();
    // The press lands on the edge, so a tap is never missed. The release is
    // left to the pad's own grace period, which the next polls run out: a
    // finger that goes down and up between two polls still stays down long
    // enough for the game to read it as a tap.
    apply_pad_direction(&mut ui.pads[cross], direction, 0.0, &mut changes);
    Some(changes)
}

struct Ui {
    enabled: bool,
    menu_open: bool,
    markers: Vec<Marker>,
    /// The circle (left stick) and cross (right stick) movement pads.
    pads: [Pad; PAD_COUNT],
    /// Where the press that is resizing a pad, or dragging it, started.
    pad_press: (f32, f32),
    pad_press_moved: bool,
    /// Which D-pad buttons are down right now, as the bitmask [DPAD_BUTTONS]
    /// sets. The cross pad follows this and ignores the sticks entirely.
    dpad: u8,
    /// Camera mode: the right stick drags a finger across the middle of the
    /// screen instead of driving the cross pad. This is the look-around
    /// gesture mobile games use, and it needs a finger that keeps travelling,
    /// which a pad's short swipe cannot do.
    look: bool,
    /// Where the look finger is now, in normalized viewport coordinates.
    look_at: (f32, f32),
    /// Whether the look finger is down.
    look_touching: bool,
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
    /// "Hide buttons": draws markers and pads almost invisibly, so they stop
    /// covering the game, while still working. The overlay's own button stays
    /// fully visible so it can be undone.
    hidden: bool,
}

impl Ui {
    const fn new() -> Self {
        Self {
            enabled: false,
            menu_open: false,
            markers: Vec::new(),
            pads: [
                Pad::new(PadKind::Circle, PAD_DEFAULT_CENTERS[0].0, PAD_DEFAULT_CENTERS[0].1),
                Pad::new(
                    PadKind::Cross,
                    PAD_DEFAULT_CENTERS[1].0,
                    PAD_DEFAULT_CENTERS[1].1,
                ),
            ],
            pad_press: (0.0, 0.0),
            pad_press_moved: false,
            dpad: 0,
            look: false,
            look_at: LOOK_CENTRE,
            look_touching: false,
            pending: W_NONE,
            press_start: (0.0, 0.0),
            press_moved: false,
            bind_target: None,
            app_id: String::new(),
            next_id: 0,
            just_bound: None,
            hidden: false,
        }
    }
}

static UI: Mutex<Ui> = Mutex::new(Ui::new());

/// Which axis each stick direction comes from, and whether it is the
/// positive end of the axis.
const STICK_DIRECTIONS: [(Button, usize, bool); 8] = [
    (Button::LeftStickLeft, 0, false),
    (Button::LeftStickRight, 0, true),
    (Button::LeftStickUp, 1, false),
    (Button::LeftStickDown, 1, true),
    (Button::RightStickLeft, 2, false),
    (Button::RightStickRight, 2, true),
    (Button::RightStickUp, 3, false),
    (Button::RightStickDown, 3, true),
];

/// The analog state `crate::window` reads from the controllers. The stick
/// values are already deadzone-corrected, so 0.0 means "centred" and any
/// other sign means the stick is pushed that way.
#[derive(Clone, Copy, Debug)]
pub struct AxisInputs {
    /// Left stick X, left stick Y, right stick X, right stick Y.
    pub sticks: [f32; 4],
    pub left_trigger: bool,
    pub right_trigger: bool,
    /// Raw trigger axis value at which the trigger counts as pressed. SDL
    /// reports triggers as either 0..32767 or -32768..32767 depending on the
    /// mapping, so half-travel works for both.
    pub trigger_point: i16,
}

impl Default for AxisInputs {
    fn default() -> Self {
        Self {
            sticks: [0.0; 4],
            left_trigger: false,
            right_trigger: false,
            trigger_point: 16_384,
        }
    }
}

/// The four directions of each stick as 0..1 pushes, which is what the pads
/// consume. Unlike a button, a direction does not snap: the pad shows how far
/// the stick is pushed, and a diagonal pushes two directions at once.
///
/// A direction only leaves 0 once the stick is past [PAD_ACTIVATION], and
/// reaches 1.0 when the stick is fully pushed.
fn stick_axis(sticks: [f32; 4], stick: usize) -> ((f32, f32), f32) {
    let x = sticks[stick * 2];
    let y = sticks[stick * 2 + 1];
    // The deadzone is applied to the whole vector, not to each axis, so a
    // diagonal push is not tilted towards whichever axis happens to clear
    // the threshold first. The stick's angle is kept exactly as it is.
    let (nx, ny) = normalize_to_threshold(x, y, PAD_ACTIVATION);
    let push = (nx.hypot(ny)).min(1.0);
    ((nx, ny), push)
}

/// Applies a radial deadzone: the vector keeps its direction, and its length
/// goes from 0 at `threshold` to 1 at full push, never past 1.
fn normalize_to_threshold(x: f32, y: f32, threshold: f32) -> (f32, f32) {
    let length = x.hypot(y);
    if length <= threshold || length <= f32::EPSILON {
        return (0.0, 0.0);
    }
    let scale = ((length - threshold) / (1.0 - threshold)).min(1.0) / length;
    (x * scale, y * scale)
}

/// Turns a sequence of [AxisInputs] samples into button edges.
///
/// Each stick direction is a button that is pressed while the stick is past
/// the deadzone on that side (45 degrees or more), so e.g. pushing
/// down-left presses both `LeftStickLeft` and `LeftStickDown`, exactly like
/// holding a gamepad button pair.
#[derive(Clone, Copy, Debug, Default)]
pub struct AxisTracker {
    last: AxisInputs,
    initialized: bool,
}

impl AxisTracker {
    /// The edges between the previous sample and `inputs`: `(button, pressed)`.
    /// The first sample never produces edges, so turning the overlay on with
    /// a stick already pushed does not fire a phantom press.
    pub fn edges(&mut self, inputs: AxisInputs) -> Vec<(Button, bool)> {
        let mut edges = Vec::new();
        if self.initialized {
            for (button, axis, positive) in STICK_DIRECTIONS {
                let now = direction_pressed(inputs.sticks[axis], positive);
                let before = direction_pressed(self.last.sticks[axis], positive);
                if now != before {
                    edges.push((button, now));
                }
            }
            for (button, now, before) in [
                (Button::LeftTrigger, inputs.left_trigger, self.last.left_trigger),
                (Button::RightTrigger, inputs.right_trigger, self.last.right_trigger),
            ] {
                if now != before {
                    edges.push((button, now));
                }
            }
        }
        self.last = inputs;
        self.initialized = true;
        edges
    }
}

/// Whether a stick is pushed towards the positive (or negative) end of its
/// axis far enough to count as that direction.
fn direction_pressed(value: f32, positive: bool) -> bool {
    if positive {
        value > 0.0
    } else {
        value < 0.0
    }
}

/// Feeds the sticks to the pads and returns the touches the pads owe the
/// game. `crate::window` calls this on every poll, before it drains the
/// individual button edges.
///
/// Pulling a stick moves that pad's knob, and each direction that crosses
/// [PAD_ACTIVATION] starts a touch at the centre of the pad; the finger then
/// slides outwards as the stick goes further, so how far it travels follows
/// how hard the stick is pushed. Directions are independent, so a diagonal
/// presses two of them at once.
pub fn pads_move(sticks: [f32; 4], dt: f32) -> Vec<MarkerTouch> {
    let Ok(mut ui) = UI.lock() else {
        return Vec::new();
    };
    if !ui.enabled {
        return Vec::new();
    }
    pads_move_inner(&mut ui, sticks, dt)
}

/// [pads_move] without the global lock, so the tests drive exactly the code
/// the game runs.
fn pads_move_inner(ui: &mut Ui, sticks: [f32; 4], dt: f32) -> Vec<MarkerTouch> {
    let mut changes = Vec::new();

    // The circle is the left stick: an analogue drag that follows the push. A
    // stick either walks or presses markers, not both, so if the player bound
    // one of the left stick's directions to a marker the pad leaves it alone --
    // and must let go of a finger it still holds rather than leaving it down.
    let index = PadKind::Circle.pad();
    if ui.pads[index].enabled {
        let taken = ui.markers.iter().any(|marker| {
            marker
                .button
                .is_some_and(|button| CIRCLE_DIRECTION_BUTTONS.contains(&button))
        });
        let (axis, push) = stick_axis(sticks, 0);
        if taken {
            ui.pads[index].axis = axis;
            ui.pads[index].push = push;
            if std::mem::replace(&mut ui.pads[index].touching, false) {
                changes.push(pad_release(&mut ui.pads[index]));
            }
            ui.pads[index].rest = 0.0;
        } else {
            apply_pad_push(&mut ui.pads[index], axis, push, dt, &mut changes);
        }
    }

    // The cross is the D-pad, and it reads the D-pad buttons -- never the right
    // stick. That is what lets camera mode and the cross be on at the same
    // time: the camera drags the right stick, this looks at four buttons
    // nobody else wants, and turning the camera cannot move the finger that is
    // choosing a weapon.
    let index = PadKind::Cross.pad();
    if ui.pads[index].enabled {
        // A button a marker has claimed belongs to that marker, but only that
        // button: binding D-pad up must not take away left and right.
        let held = ui.dpad & !dpad_claimed(ui);
        if held != ui.dpad && std::mem::replace(&mut ui.pads[index].touching, false) {
            // The directions that are left do not make one: let go rather than
            // leave a finger on a spot the player did not choose.
            changes.push(pad_release(&mut ui.pads[index]));
            ui.pads[index].rest = 0.0;
        }
        // The D-pad edges press and move the cross; this poll is what runs the
        // release grace period out afterwards, and what lets go when the button
        // is already up (a tap plays out even with no further edges).
        let direction = dpad_dir(held);
        apply_pad_direction(&mut ui.pads[index], direction, dt, &mut changes);
    }

    look_move(ui, sticks, dt, &mut changes);
    changes
}

/// Advances camera mode by `dt` seconds and returns the touch changes it owes.
///
/// While a direction is held the look finger drags that way at a steady speed,
/// which is what a game's look-around control expects from a swiping finger
/// (a "stick deflect = camera speed" scheme). When it reaches the edge of its
/// box the finger is lifted and put back in the middle, so a long turn is just
/// a held stick. Letting go lifts the finger.
fn look_move(ui: &mut Ui, sticks: [f32; 4], dt: f32, changes: &mut Vec<MarkerTouch>) {
    if !ui.look {
        return;
    }
    let (x, y) = (sticks[2], sticks[3]);
    let pushed = x.abs().max(y.abs()) > LOOK_DEADZONE;
    if !pushed {
        if std::mem::replace(&mut ui.look_touching, false) {
            changes.push(look_touch(ui.look_at, MarkerAction::Up));
        }
        return;
    }
    if !ui.look_touching {
        ui.look_touching = true;
        ui.look_at = LOOK_CENTRE;
        changes.push(look_touch(ui.look_at, MarkerAction::Down));
        return;
    }
    let dt = dt.clamp(0.0, 0.1);
    let next = (
        ui.look_at.0 + x * LOOK_SPEED * dt,
        ui.look_at.1 + y * LOOK_SPEED * dt,
    );
    // The drag stays inside its box, so the camera's finger never wanders into
    // the game's own controls.
    let (lo_x, hi_x) = (LOOK_BOX.0 - LOOK_BOX.2, LOOK_BOX.0 + LOOK_BOX.2);
    let (lo_y, hi_y) = (
        LOOK_BOX.1 - LOOK_BOX.2 * 1.6,
        LOOK_BOX.1 + LOOK_BOX.2 * 1.6,
    );
    if next.0 < lo_x || next.0 > hi_x || next.1 < lo_y || next.1 > hi_y {
        // Out of room: finish this drag and start a fresh one from the middle,
        // so the camera keeps turning in the same direction.
        changes.push(look_touch(ui.look_at, MarkerAction::Up));
        ui.look_at = LOOK_CENTRE;
        changes.push(look_touch(ui.look_at, MarkerAction::Down));
        return;
    }
    // A tiny movement is not worth a Move event.
    if (next.0 - ui.look_at.0).abs() < 1e-4 && (next.1 - ui.look_at.1).abs() < 1e-4 {
        return;
    }
    ui.look_at = next;
    changes.push(look_touch(ui.look_at, MarkerAction::Move));
}

fn look_touch(position: (f32, f32), action: MarkerAction) -> MarkerTouch {
    MarkerTouch {
        marker: LOOK_FINGER,
        position,
        action,
    }
}

/// The finger id of the camera drag. It is above the pads' and the markers'
/// ranges, so it can never collide.
pub const LOOK_FINGER: u8 = 128;

/// One pad's worth of a stick sample: starts, moves and ends the touches of
/// the directions that changed.
/// One stick pad's worth of a stick sample: drives its analogue finger.
///
/// The circle is a joystick: its finger follows the stick, so it slides. That
/// is what a game's own analogue stick expects. (The cross is a D-pad and uses
/// [apply_pad_direction] instead, because a sliding finger there made the
/// player's character drift.)
fn apply_pad_push(
    pad: &mut Pad,
    axis: (f32, f32),
    push: f32,
    dt: f32,
    changes: &mut Vec<MarkerTouch>,
) {
    let was = (pad.axis, pad.push);
    let before = *pad;
    pad.axis = axis;
    pad.push = push;
    if push <= 0.0 {
        pad.rest += dt.clamp(0.0, 0.1);
    } else {
        pad.rest = 0.0;
    }
    // The finger stays down through a moment of rest, so reversing the stick
    // is a Move and not a release followed by a new touch. While it rests it
    // also stays where it is: sliding it back to the middle would be the
    // backwards drag that walking controls read as a direction change.
    let resting = push <= 0.0 && pad.touching && pad.rest < PAD_RELEASE_DELAY;
    if resting {
        pad.axis = before.axis;
        pad.push = before.push;
    }
    let pushed = push > 0.0 || resting;
    match (pushed, pad.touching) {
        (true, false) => {
            pad.touching = true;
            let mut start = *pad;
            start.push = 0.0;
            changes.push(pad_touch_at(&start, PadAction::Down));
            if push > MOVE_EPSILON {
                changes.push(pad_touch_at(pad, PadAction::Move));
            }
        }
        (true, true) => {
            if !resting {
                let moved = (axis.0 - was.0 .0).abs()
                    + (axis.1 - was.0 .1).abs()
                    + (push - was.1).abs();
                if moved > MOVE_EPSILON {
                    changes.push(pad_touch_at(pad, PadAction::Move));
                }
            }
        }
        (false, true) => {
            changes.push(pad_touch_at(&before, PadAction::Up));
            pad.touching = false;
            pad.axis = (0.0, 0.0);
            pad.push = 0.0;
            pad.rest = 0.0;
        }
        (false, false) => {}
    }
}

/// The circle's touch at its current analogue push.
fn pad_touch_at(pad: &Pad, action: PadAction) -> MarkerTouch {
    MarkerTouch {
        marker: PAD_FINGER_BASE + pad.kind.pad() as u8,
        // The circle's finger follows its stick; the cross's follows its
        // swipe. Reading the wrong one is why the cross sent every touch to
        // the middle of the pad and a weapon wheel saw nothing move.
        position: match pad.kind {
            PadKind::Circle => pad.finger_analogue(),
            PadKind::Cross => pad.swipe,
        },
        action: match action {
            PadAction::Down => MarkerAction::Down,
            PadAction::Move => MarkerAction::Move,
            PadAction::Up => MarkerAction::Up,
        },
    }
}

/// One pad's worth of a stick sample: drives its single D-pad touch.
///
/// A cross pad is a D-pad, not a stick: it picks the nearest of eight
/// directions and sends one touch that *stays put* while that direction is
/// held. It never slides around, because a sliding finger is what a game reads
/// as its own joystick being moved; on a phone shooter that made the player's
/// character drift while they were only turning the camera.
///
/// Changing direction is still one uninterrupted touch: the finger is not
/// lifted, it simply ends up on the new direction's spot, so walking round a
/// corner does not read as the stick being released.
/// How long the cross's swipe takes, in seconds.
///
/// A wheel reads the direction of travel, and many also read how fast the swipe
/// was, so the finger has to actually travel rather than appear at its
/// destination. This is short enough that a tap still feels instant and long
/// enough that the move events are spread over several samples, the way a real
/// thumb's are.
const CROSS_SWIPE_TIME: f32 = 0.09;

/// Advances the cross: a held direction puts a finger down in the middle of the
/// pad and slides it out along that direction.
///
/// A D-pad on a touch screen is a swipe, because that is what a weapon wheel
/// (and most other on-screen four-way controls) actually reads: the direction
/// the finger travels from the middle of the wheel. A touch that appears on the
/// rim never travels anywhere, so the wheel sees nothing -- which is exactly
/// why the D-pad appeared to do nothing at all.
///
/// The finger is dragged, never re-touched, so changing direction mid-swipe is
/// one continuous gesture; lifting it is delayed by [PAD_RELEASE_DELAY] so a
/// roll round the wheel does not read as a tap.
fn apply_pad_direction(
    pad: &mut Pad,
    direction: Option<usize>,
    dt: f32,
    changes: &mut Vec<MarkerTouch>,
) {
    let dt = dt.clamp(0.0, 0.1);
    match (direction, pad.touching) {
        (Some(direction), false) => {
            // A new swipe: the finger goes down in the middle and starts out.
            pad.held = Some(direction);
            pad.touching = true;
            pad.rest = 0.0;
            pad.swipe_elapsed = 0.0;
            pad.swipe_dir = direction;
            pad.swipe = pad.swipe_start();
            changes.push(pad_touch_at(pad, PadAction::Down));
            changes.push(pad_touch_at(pad, PadAction::Move));
        }
        (Some(direction), true) => {
            pad.rest = 0.0;
            if direction != pad.held.unwrap_or(direction) {
                // A new direction: start the travel over from the middle, on
                // the same finger. The game sees one thumb changing its mind.
                pad.held = Some(direction);
                pad.swipe_elapsed = 0.0;
                pad.swipe_dir = direction;
                pad.swipe = pad.swipe_start();
                changes.push(pad_touch_at(pad, PadAction::Move));
            } else {
                pad.swipe_elapsed += dt;
            }
            // Travel outwards while the swipe is still running, so the wheel
            // sees the finger move rather than jump.
            if pad.swipe_elapsed <= CROSS_SWIPE_TIME {
                let t = pad.swipe_elapsed / CROSS_SWIPE_TIME;
                pad.swipe_dir = direction;
                pad.swipe = pad.swipe_at(direction, t);
                changes.push(pad_touch_at(pad, PadAction::Move));
            }
        }
        (None, true) => {
            // The button is up. A tap is often shorter than the swipe, so the
            // travel is finished here before the grace period even starts:
            // lifting halfway through would leave the finger near the middle,
            // and a wheel would read that as no direction at all.
            pad.held = None;
            if pad.swipe_elapsed < CROSS_SWIPE_TIME {
                pad.swipe_elapsed = (pad.swipe_elapsed + dt).min(CROSS_SWIPE_TIME);
                let t = pad.swipe_elapsed / CROSS_SWIPE_TIME;
                pad.swipe = pad.swipe_at(pad.swipe_dir, t);
                changes.push(pad_touch_at(pad, PadAction::Move));
                // Do not start the release clock until the travel is done: the
                // finger must arrive before it can leave.
                return;
            }
            pad.rest += dt;
            if pad.rest >= PAD_RELEASE_DELAY {
                changes.push(pad_touch_at(pad, PadAction::Up));
                pad.touching = false;
                pad.rest = 0.0;
                pad.swipe_elapsed = 0.0;
            }
        }
        (None, false) => {
            pad.held = None;
            pad.rest = 0.0;
        }
    }
}

/// The Up for a pad that has to let go of its finger (it lost its stick, or it
/// is being removed). Picks the right spot for the pad's kind.
fn pad_release(pad: &mut Pad) -> MarkerTouch {
    let touch = pad_touch_at(pad, PadAction::Up);
    pad.held = None;
    pad.axis = (0.0, 0.0);
    pad.push = 0.0;
    pad.swipe_elapsed = 0.0;
    touch
}

fn lerp2(from: (f32, f32), to: (f32, f32), t: f32) -> (f32, f32) {
    (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
}

/// What a pad is doing to one of its four touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PadAction {
    Down,
    Move,
    Up,
}

/// The finger ids of the pads' touches start here, clear of marker ids.
pub const PAD_FINGER_BASE: u8 = 64;

/// `crate::window` keeps one tracker per window and calls
/// [AxisTracker::edges] on every poll.
pub fn axis_edges(tracker: &mut AxisTracker, inputs: AxisInputs) -> Vec<(Button, bool)> {
    tracker.edges(inputs)
}

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
    ui.pads = [
        Pad::new(PadKind::Circle, PAD_DEFAULT_CENTERS[0].0, PAD_DEFAULT_CENTERS[0].1),
        Pad::new(
            PadKind::Cross,
            PAD_DEFAULT_CENTERS[1].0,
            PAD_DEFAULT_CENTERS[1].1,
        ),
    ];
    ui.pad_press_moved = false;
    ui.dpad = 0;
    ui.look = false;
    ui.hidden = false;
    ui.look_touching = false;
    ui.look_at = LOOK_CENTRE;
    if let Some(app_id) = app_id {
        ui.app_id.push_str(app_id);
        if enabled {
            let (markers, pads) = load_entry_in(&bindings_path(), app_id);
            ui.markers = markers;
            if let Some((pads, look, hidden)) = pads {
                ui.pads = pads;
                ui.look = look;
                ui.hidden = hidden;
            }
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

/// `circle,x,y,radius;cross,...`, the pads half of an app's line.
fn format_pads(pads: &[Pad; PAD_COUNT], look: bool, hidden: bool) -> String {
    let mut text = pads
        .iter()
        .map(|pad| {
            format!(
                "{},{:.3},{:.3},{:.3},{},{:.2}",
                match pad.kind {
                    PadKind::Circle => "circle",
                    PadKind::Cross => "cross",
                },
                pad.cx,
                pad.cy,
                pad.radius,
                if pad.enabled { "on" } else { "off" },
                pad.reach
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    if look {
        text.push_str(";look");
    }
    if hidden {
        text.push_str(";hidden");
    }
    text
}

/// The inverse of [`format_pads`]. A missing or malformed pad keeps its
/// default, so files written before the pads existed still load.
fn parse_pads(text: &str) -> Option<([Pad; PAD_COUNT], bool, bool)> {
    let mut pads = [
        Pad::new(PadKind::Circle, PAD_DEFAULT_CENTERS[0].0, PAD_DEFAULT_CENTERS[0].1),
        Pad::new(
            PadKind::Cross,
            PAD_DEFAULT_CENTERS[1].0,
            PAD_DEFAULT_CENTERS[1].1,
        ),
    ];
    let mut found = false;
    let mut look = false;
    let mut hidden = false;
    for entry in text.split(';') {
        if entry.trim() == "look" {
            look = true;
            found = true;
            continue;
        }
        if entry.trim() == "hidden" {
            hidden = true;
            found = true;
            continue;
        }
        let mut fields = entry.split(',');
        let (Some(name), Some(x), Some(y), Some(radius)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let state = fields.next().map(str::trim);
        let reach = fields.next().and_then(|reach| reach.trim().parse::<f32>().ok());
        let kind = match name.trim() {
            "circle" => PadKind::Circle,
            // "triangle" is what the cross pad was called before it was
            // renamed, and files written then still name it that way.
            "cross" | "triangle" => PadKind::Cross,
            _ => continue,
        };
        let (Ok(x), Ok(y), Ok(radius)) = (
            x.trim().parse::<f32>(),
            y.trim().parse::<f32>(),
            radius.trim().parse::<f32>(),
        ) else {
            continue;
        };
        if !x.is_finite() || !y.is_finite() || !radius.is_finite() {
            continue;
        }
        let pad = &mut pads[kind.pad()];
        pad.cx = x.clamp(0.0, 1.0);
        pad.cy = y.clamp(0.0, 1.0);
        pad.radius = radius.clamp(0.02, 0.5);
        // Files written before pads could be switched off have no state, and
        // those were the builds where both pads were always on.
        pad.enabled = state != Some("off");
        // Files written before reach existed had the full travel.
        if let Some(reach) = reach.filter(|reach| reach.is_finite()) {
            pad.reach = reach.clamp(0.2, 1.0);
        }
        found = true;
    }
    found.then_some((pads, look, hidden))
}

/// Everything saved for an app: its markers (empty for a file written before
/// markers existed) and its pads (`None` for a file written before pads).
fn load_entry_in(
    path: &std::path::Path,
    app_id: &str,
) -> (Vec<Marker>, Option<([Pad; PAD_COUNT], bool, bool)>) {
    if app_id.is_empty() {
        return (Vec::new(), None);
    }
    let Ok(contents) = std::fs::read_to_string(path) else {
        return (Vec::new(), None);
    };
    for line in contents.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != app_id {
            continue;
        }
        // `markers | pads`: the split is on the last '|', so a hand-edited
        // entry cannot break the pads by adding one to the markers.
        return match value.rsplit_once('|') {
            Some((markers, pads)) => (parse_markers(markers), parse_pads(pads)),
            None => (parse_markers(value), None),
        };
    }
    (Vec::new(), None)
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
    // The pads always exist, so an app's line is written as soon as either
    // half has been changed; otherwise the line goes away entirely.
    let pads_changed = ui.pads.iter().any(|pad| {
        pad.enabled
            || pad.reach != 1.0
            || pad.cx != PAD_DEFAULT_CENTERS[pad.kind.pad()].0
            || pad.cy != PAD_DEFAULT_CENTERS[pad.kind.pad()].1
            || pad.radius != PAD_DEFAULT_RADIUS
    });
    if !ui.markers.is_empty() || pads_changed || ui.look || ui.hidden {
        contents.push_str(&ui.app_id);
        contents.push_str(": ");
        contents.push_str(&format_markers(&ui.markers));
        contents.push_str(" | ");
        contents.push_str(&format_pads(&ui.pads, ui.look, ui.hidden));
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

/// The menu label for a pad's reach, e.g. "REACH 100%".
fn reach_label(pad: &Pad) -> String {
    format!("REACH {}%", (pad.reach * 100.0).round() as i32)
}

/// One step of the reach adjuster: 100%, 85%, 70%, 55%, 40%, and back round.
const REACH_STEPS: [f32; 5] = [0.40, 0.55, 0.70, 0.85, 1.00];

/// Moves a pad's reach to the next step, downwards, wrapping at the bottom.
fn cycle_reach(pad: &mut Pad) {
    let current = REACH_STEPS
        .iter()
        .position(|step| (step - pad.reach).abs() < 0.01)
        .unwrap_or(REACH_STEPS.len() - 1);
    pad.reach = REACH_STEPS[(current + REACH_STEPS.len() - 1) % REACH_STEPS.len()];
}

/// Adds or removes one of the movement pads. Removing a pad lifts any finger
/// it was holding, so the game never sees a touch without its Up.
fn toggle_pad(ui: &mut Ui, index: usize) {
    let pad = &mut ui.pads[index];
    pad.enabled = !pad.enabled;
    if !pad.enabled && pad.kind == PadKind::Cross {
        // The cross is the D-pad's reader; with it off nothing would clear the
        // bits, and switching it back on would resurrect a direction.
        ui.dpad = 0;
    }
    if !pad.enabled {
        if std::mem::replace(&mut pad.touching, false) {
            queue_change(pad_release(pad));
        }
        pad.axis = (0.0, 0.0);
        pad.push = 0.0;
        pad.held = None;
    }
    save(ui);
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
    // The D-pad drives the cross pad. This is done here, on the edge, rather
    // than left for the next pad poll: a weapon-wheel tap can go down and up
    // inside one event poll, and a poll that only reads the held buttons would
    // never see it at all.
    if let Some((_, bit)) = DPAD_BUTTONS.iter().find(|(b, _)| *b == button) {
        let changes = dpad_edge(ui, *bit, pressed);
        if changes.is_some() {
            return changes;
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
    /// Each pad's centre and radius, in viewport pixels.
    pads: [(f32, f32, f32); PAD_COUNT],
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
    // The menu's rows, in the order they are drawn and hit-tested.
    const MENU_ROWS: [u16; 9] = [
        W_ADD,
        W_ADD_CIRCLE,
        W_REACH_CIRCLE,
        W_ADD_CROSS,
        W_REACH_CROSS,
        W_LOOK,
        W_HIDE,
        W_CLEAR,
        W_CLOSE,
    ];
    let menu = if ui.menu_open {
        let ids = MENU_ROWS;
        let width = (vw - 12.0 * s).clamp(80.0, (240.0 * s).max(80.0));
        let x = (vw - width) / 2.0;
        let y = button.y + button.h + 6.0 * s;
        let header_h = 22.0 * s;
        let hint_h = 37.0 * s;
        // Seven rows do not fit a short window, so the rows shrink rather
        // than spill off the bottom of the screen. The menu is a menu for a
        // gamepad overlay, not a document: every row has to stay reachable.
        let available = (vh - y - 4.0 * s).max(0.0);
        let row_h = ((available - header_h - hint_h - 6.0 * s) / ids.len() as f32)
            .clamp(14.0 * s, 28.0 * s);
        let height = header_h + row_h * ids.len() as f32 + hint_h + 6.0 * s;
        let mut rows = Vec::with_capacity(ids.len());
        for (index, id) in ids.iter().enumerate() {
            rows.push((
                *id,
                Rect {
                    x: x + 6.0 * s,
                    y: y + header_h + index as f32 * row_h + 1.0 * s,
                    w: width - 12.0 * s,
                    h: row_h - 2.0 * s,
                },
            ));
        }
        let hint = Rect {
            x: x + 8.0 * s,
            y: y + header_h + ids.len() as f32 * row_h + 2.0 * s,
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
    let marker_size = (MARKER_SIZE * s).max(MIN_MARKER_SIZE);
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
    let pads = ui.pads.map(|pad| {
        if !pad.enabled {
            return (0.0, 0.0, 0.0);
        }
        let radius = pad.radius_px(viewport);
        // Keep the whole pad on screen even if the saved position would put
        // part of it outside (or the window got smaller since it was saved).
        let inset_x = (radius / vw.max(1.0)).min(0.5);
        let inset_y = (radius / vh.max(1.0)).min(0.5);
        let cx = pad.cx.clamp(inset_x, 1.0 - inset_x);
        let cy = pad.cy.clamp(inset_y, 1.0 - inset_y);
        (cx * vw, cy * vh, radius)
    });
    Layout {
        scale: s,
        button,
        menu,
        markers,
        pads,
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
            OVERLAY_REVISION,
            menu.rect.x + 8.0 * s,
            menu.rect.y + 6.0 * s,
            11.0 * s,
            COL_TEXT_DIM,
        );
        for (id, row) in &menu.rows {
            let on_off = |pad: &Pad| {
                if pad.enabled {
                    "ON"
                } else {
                    "OFF"
                }
            };
            let (label, color) = match *id {
                W_ADD => ("ADD BUTTON".to_string(), COL_ACCENT),
                W_ADD_CIRCLE => (format!("CIRCLE: {}", on_off(&ui.pads[0])), COL_ROW),
                W_ADD_CROSS => (format!("CROSS: {}", on_off(&ui.pads[1])), COL_ROW),
                W_REACH_CIRCLE | W_REACH_CROSS => {
                    let index = if *id == W_REACH_CIRCLE { 0 } else { 1 };
                    (reach_label(&ui.pads[index]), COL_ROW)
                }
                W_LOOK => (
                    if ui.look {
                        "CAMERA: ON".to_string()
                    } else {
                        "CAMERA: OFF".to_string()
                    },
                    COL_ROW,
                ),
                W_HIDE => (
                    if ui.hidden {
                        "SHOW BUTTONS".to_string()
                    } else {
                        "HIDE BUTTONS".to_string()
                    },
                    COL_ROW,
                ),
                W_CLEAR => ("CLEAR ALL".to_string(), COL_ROW),
                _ => ("CLOSE".to_string(), COL_ROW),
            };
            scene.rect(row.x, row.y, row.w, row.h, color);
            scene.text(gles, &label, row.x + 6.0 * s, row.y + 5.0 * s, 11.0 * s, COL_TEXT);
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

    // The pads: a ring for the circle, a cross for the cross, with the knob
    // under the thumb and the arrows lit as the stick pushes. An enabled pad is
    // always drawn; [pad_alpha] only decides how strongly.
    for (index, (px, py, radius)) in layout.pads.iter().enumerate() {
        let Some(alpha) = pad_alpha(&ui, index) else {
            continue;
        };
        if *radius <= 0.0 {
            continue;
        }
        let pad = &ui.pads[index];
        draw_pad(gles, &mut scene, pad, *px, *py, *radius, alpha);
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
        let (fill, border) = if ui.hidden {
            (fade(fill, HIDDEN_ALPHA), fade(border, HIDDEN_ALPHA))
        } else {
            (fill, border)
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
        // `s` shrinks when a long label would not fit the marker, so "L1" and
        // "<LS" both stay inside it at any screen size.
        let marker_scale = (rect.w / MARKER_SIZE).clamp(0.4, 3.0);
        let label_size = (11.0 * marker_scale)
            .min(rect.w / (label.chars().count() as f32 * 0.62).max(1.0));
        let label_size = label_size.max(1.0);
        let label_width = (label.chars().count() as f32) * label_size * 0.6;
        let label_color = if ui.hidden {
            fade(COL_TEXT, HIDDEN_ALPHA)
        } else {
            COL_TEXT
        };
        scene.text(
            gles,
            &label,
            rect.x + (rect.w - label_width) / 2.0,
            rect.y + rect.h * 0.5 - label_size / 2.0,
            label_size,
            label_color,
        );
    }
    Some(scene)
}

/// Draws one pad: its outline, the four direction labels and the knob.
///
/// The scene only has axis-aligned rectangles and text, so the outline is a
/// dotted one: a ring of small squares for the circle, and the two arms of a
/// cross for the cross pad. That reads the same as the faint outlines other
/// on-screen-pad overlays draw, without needing a mesh.
unsafe fn draw_pad(
    gles: &mut dyn GLES,
    scene: &mut crate::trainer_ui::OverlayScene,
    pad: &Pad,
    px: f32,
    py: f32,
    radius: f32,
    alpha: f32,
) {
    let pad_col = fade(COL_PAD, alpha);
    let knob_col = fade(COL_PAD_KNOB, alpha);
    let knob_border = fade(COL_PAD_KNOB_BORDER, alpha);
    let dot = (radius * 0.055).max(1.5);
    for (x, y) in pad_outline(pad.kind, px, py, radius) {
        scene.rect(x - dot / 2.0, y - dot / 2.0, dot, dot, pad_col);
    }

    // The directions, on the rim, lit while the stick pushes them. The circle
    // is analogue, so it leans towards any angle and a diagonal lights two
    // arrows. The cross is a D-pad, so exactly one of its eight directions is
    // live and only that one lights up.
    let label_size = (radius * 0.26).clamp(6.0, 30.0);
    for (direction, text) in ["^", "v", "<", ">"].iter().enumerate() {
        let (dx, dy) = PAD_DIRECTIONS[direction];
        let (ax, ay) = (px + dx * radius * PAD_RIM, py + dy * radius * PAD_RIM);
        let lean = match pad.kind {
            PadKind::Circle => (dx * pad.axis.0 + dy * pad.axis.1).max(0.0) * pad.push,
            PadKind::Cross => {
                // PAD_DIRECTIONS is up/down/left/right; PAD_DIRECTIONS_8 is
                // clockwise from up, so the four cardinal ones are 0, 4, 6, 2.
                let wanted = match direction {
                    0 => 0,
                    1 => 4,
                    2 => 6,
                    _ => 2,
                };
                if pad.held == Some(wanted) {
                    1.0
                } else {
                    0.0
                }
            }
        };
        let color = if pad.touching && lean > 0.05 {
            fade(COL_PAD_ARROW, alpha)
        } else {
            fade(dim(COL_PAD_ARROW_IDLE, 1.0 - 0.35 * lean), alpha)
        };
        // Centred on the rim point, by eye: the atlas has no bearings.
        let width = label_size * 0.6;
        scene.text(
            gles,
            text,
            ax - width / 2.0,
            ay - label_size / 2.0,
            label_size,
            color,
        );
    }

    // The knob, under the thumb's stick position. A square with a lighter
    // square inside reads as a knob at these sizes.
    let (kx, ky) = knob_position(pad, px, py, radius);
    let knob = (radius * 0.34).max(7.0);
    let border = (knob * 0.12).max(1.0);
    scene.rect(
        kx - knob / 2.0,
        ky - knob / 2.0,
        knob,
        knob,
        knob_border,
    );
    scene.rect(
        kx - knob / 2.0 + border,
        ky - knob / 2.0 + border,
        knob - 2.0 * border,
        knob - 2.0 * border,
        knob_col,
    );
    let handle = pad_handle_rect(pad, px, py, radius);
    scene.rect(handle.x, handle.y, handle.w, handle.h, knob_border);
    // Two dots in the corner read as a "grab here and pull" grip.
    let dot = ((handle.w * 0.18).max(1.5)).min(handle.w / 3.0);
    let step = (handle.w - 2.0 * dot) / 3.0;
    for i in 0..2 {
        let at = dot + step * i as f32;
        scene.rect(handle.x + at, handle.y + at, dot, dot, knob_col);
    }
}

/// How strongly to draw the pad at `index`, or `None` when it is switched off.
///
/// A pad stays visible and grabbable whenever it is enabled. When its stick has
/// been taken -- by a marker, or by camera mode -- it fades to [TAKEN_ALPHA]
/// instead of disappearing: a pad the menu says is ON must be on screen, and a
/// vanished pad is indistinguishable from a broken one.
fn pad_alpha(ui: &Ui, index: usize) -> Option<f32> {
    let pad = &ui.pads[index];
    if !pad.enabled {
        return None;
    }
    // The camera no longer has anything to do with the cross: they read
    // different inputs, so an enabled pad is drawn at full strength whatever
    // the camera is doing.
    let alpha = if ui.hidden { HIDDEN_ALPHA } else { 1.0 };
    Some(alpha)
}

/// The dots that make up a pad's outline, in viewport pixels.
///
/// A circle is a ring; a cross is its two arms, which stop short of the middle
/// so the knob there stays readable.
fn pad_outline(kind: PadKind, px: f32, py: f32, radius: f32) -> Vec<(f32, f32)> {
    let mut dots = Vec::new();
    match kind {
        PadKind::Circle => {
            const DOTS: usize = 44;
            for index in 0..DOTS {
                let angle = index as f32 * std::f32::consts::TAU / DOTS as f32;
                let (sin, cos) = angle.sin_cos();
                dots.push((px + cos * radius, py + sin * radius));
            }
        }
        PadKind::Cross => {
            const PER_ARM: usize = 9;
            const INNER: f32 = 0.30;
            for (dx, dy) in PAD_DIRECTIONS {
                for step in 0..PER_ARM {
                    let t = INNER + (1.0 - INNER) * step as f32 / (PER_ARM - 1) as f32;
                    dots.push((px + dx * radius * t, py + dy * radius * t));
                }
            }
        }
    }
    dots
}

/// The knob's centre in viewport pixels.
///
/// The circle's knob is the stick's push, limited to the pad's travel. The
/// cross is a D-pad, so its knob sits on the held direction's arrow or in the
/// middle: it never drifts through the space between two directions, because
/// the touch it stands for never does either.
fn knob_position(pad: &Pad, px: f32, py: f32, radius: f32) -> (f32, f32) {
    if pad.kind == PadKind::Cross {
        // The knob sits where the swipe's finger is, so the pad shows the
        // gesture it is sending: out from the middle along the held direction.
        if pad.held.is_none() && !pad.touching {
            return (px, py);
        }
        return (
            px + (pad.swipe.0 - pad.cx) / pad.radius * radius,
            py + (pad.swipe.1 - pad.cy) / pad.radius * radius,
        );
    }
    let (x, y) = normalize_push(pad.axis.0, pad.axis.1);
    let travel = pad.push.clamp(0.0, 1.0) * KNOB_TRAVEL;
    (px + x * travel * radius, py + y * travel * radius)
}

/// Scales a push so it never leaves the unit circle, keeping its direction.
fn normalize_push(x: f32, y: f32) -> (f32, f32) {
    let length = x.hypot(y);
    if length > 1.0 {
        (x / length, y / length)
    } else {
        (x, y)
    }
}

fn dim(color: (f32, f32, f32, f32), factor: f32) -> (f32, f32, f32, f32) {
    (color.0 * factor, color.1 * factor, color.2 * factor, color.3)
}

/// Multiplies a colour's alpha, for "hide buttons".
fn fade(color: (f32, f32, f32, f32), alpha: f32) -> (f32, f32, f32, f32) {
    (color.0, color.1, color.2, color.3 * alpha)
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

/// Moves or resizes the pad a press started on.
///
/// Dragging the pad's body moves it; dragging the handle on its rim changes
/// its size. Both are clamped so a pad cannot be lost off-screen, shrunk out
/// of reach, or grown over the other pad.
fn pad_motion(ui: &mut Ui, abs: (f32, f32), viewport: (u32, u32, u32, u32)) -> bool {
    let id = ui.pending;
    let is_handle = id >= W_PAD_BASE + PAD_COUNT as u16;
    let index = if is_handle {
        (id - W_PAD_BASE - PAD_COUNT as u16) as usize
    } else {
        (id - W_PAD_BASE) as usize
    };
    if index >= PAD_COUNT {
        ui.pending = W_NONE;
        return false;
    }
    // A press has to travel a little before it moves anything, so a thumb
    // resting on a pad does not shove it around.
    if !ui.pad_press_moved {
        let moved = (abs.0 - ui.pad_press.0).hypot(abs.1 - ui.pad_press.1);
        if moved < RESIZE_THRESHOLD * scale_for(viewport) {
            return true;
        }
        ui.pad_press_moved = true;
    }
    let (vw, vh) = (viewport.2.max(1) as f32, viewport.3.max(1) as f32);
    // Viewport pixels and window pixels differ only by the origin here.
    let p = (abs.0 - viewport.0 as f32, abs.1 - viewport.1 as f32);
    if is_handle {
        // The handle sits on the rim, so the distance from the centre is the
        // radius the player is asking for.
        let pad = &ui.pads[index];
        let px = (pad.cx * vw, pad.cy * vh);
        let wanted = (p.0 - px.0).hypot(p.1 - px.1);
        let shortest = vw.min(vh);
        let limit = pad_radius_limit(ui, index, vw, vh);
        ui.pads[index].radius = (wanted / shortest).clamp(PAD_MIN_RADIUS / shortest, limit);
    } else {
        ui.pads[index].cx = (p.0 / vw).clamp(0.0, 1.0);
        ui.pads[index].cy = (p.1 / vh).clamp(0.0, 1.0);
    }
    clamp_pad_positions(ui, vw, vh);
    true
}

/// The largest radius the pad at `index` can take, as a fraction of the
/// shorter side.
///
/// It must fit in the viewport around its centre, and it must not reach the
/// middle of the other pad, so the two never cover each other's arrows.
fn pad_radius_limit(ui: &Ui, index: usize, vw: f32, vh: f32) -> f32 {
    let shortest = vw.min(vh);
    let pad = &ui.pads[index];
    let limit_x = pad.cx.min(1.0 - pad.cx) * vw;
    let limit_y = pad.cy.min(1.0 - pad.cy) * vh;
    let other = &ui.pads[1 - index];
    let distance = ((pad.cx - other.cx) * vw).hypot((pad.cy - other.cy) * vh);
    let limit = limit_x
        .min(limit_y)
        .min((distance - PAD_MIN_GAP).max(0.0))
        .min(0.5 * shortest);
    (limit / shortest).max(PAD_MIN_RADIUS / shortest)
}

/// Keeps both pads fully on screen after one of them moved or grew.
fn clamp_pad_positions(ui: &mut Ui, vw: f32, vh: f32) {
    for index in 0..PAD_COUNT {
        let radius = ui.pads[index].radius_px((0, 0, vw as u32, vh as u32));
        let pad = &mut ui.pads[index];
        let inset_x = (radius / vw.max(1.0)).min(0.5);
        let inset_y = (radius / vh.max(1.0)).min(0.5);
        pad.cx = pad.cx.clamp(inset_x, 1.0 - inset_x);
        pad.cy = pad.cy.clamp(inset_y, 1.0 - inset_y);
    }
}

/// The square handle on a pad's rim that changes its size. It sits just
/// outside the bottom-right of the rim, clear of the direction labels.
fn pad_handle_rect(_pad: &Pad, px: f32, py: f32, radius: f32) -> Rect {
    let size = (radius * 0.30).max(16.0);
    // On the rim at 45 degrees, half in and half out.
    let off = radius * std::f32::consts::FRAC_1_SQRT_2;
    Rect {
        x: px + off - size / 2.0,
        y: py + off - size / 2.0,
        w: size,
        h: size,
    }
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
    // Pads. The size handle is checked first: it sits on the rim, where a
    // pad's body would otherwise swallow the press.
    for (index, (px, py, radius)) in layout.pads.iter().enumerate() {
        let pad = &ui.pads[index];
        let handle = pad_handle_rect(pad, *px, *py, *radius);
        if handle.contains(p.0, p.1) {
            ui.pending = W_PAD_BASE + PAD_COUNT as u16 + index as u16;
            ui.press_start = p;
            ui.pad_press = p;
            ui.pad_press_moved = false;
            return true;
        }
        if (p.0 - px).hypot(p.1 - py) <= *radius {
            // Grabbing a pad moves it; grabbing the knob also feels like
            // grabbing the pad, which is what a thumb does anyway.
            ui.pending = W_PAD_BASE + index as u16;
            ui.press_start = p;
            ui.pad_press = p;
            ui.pad_press_moved = false;
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
    if ui.pending >= W_PAD_BASE {
        return pad_motion(ui, abs, viewport);
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
    if pending >= W_PAD_BASE {
        if std::mem::replace(&mut ui.pad_press_moved, false) {
            save(ui);
        }
        return true;
    }
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
        W_ADD_CIRCLE => {
            if hit {
                toggle_pad(ui, 0);
            }
        }
        W_ADD_CROSS => {
            if hit {
                toggle_pad(ui, 1);
            }
        }
        W_REACH_CIRCLE => {
            if hit && ui.pads[0].enabled {
                cycle_reach(&mut ui.pads[0]);
                save(ui);
            }
        }
        W_REACH_CROSS => {
            if hit && ui.pads[1].enabled {
                cycle_reach(&mut ui.pads[1]);
                save(ui);
            }
        }
        W_HIDE => {
            if hit {
                // The overlay's own button stays visible: without it there
                // would be no way to bring anything back.
                ui.hidden = !ui.hidden;
                ui.menu_open = false;
                save(ui);
            }
        }
        W_CLEAR => {
            if hit {
                clear_markers(ui);
            }
        }
        W_LOOK => {
            if hit {
                ui.look = !ui.look;
                // Turning the camera off must not leave a finger down.
                if !ui.look && std::mem::replace(&mut ui.look_touching, false) {
                    queue_change(look_touch(ui.look_at, MarkerAction::Up));
                }
                ui.menu_open = false;
                save(ui);
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
        let (loaded, _) = load_entry_in(&path, "com.test.game");
        assert_eq!(loaded.len(), 1);
        assert!((loaded[0].y - 0.30).abs() < 0.001);
        assert_eq!(loaded[0].button, Some(Button::Y));
        // The old app's line has no pads section, so it keeps the defaults.
        let (_, pads) = load_entry_in(&path, "com.other.game");
        assert!(pads.is_none());
        // Clearing every marker, with the pads untouched, removes the app's
        // line but nobody else's.
        ui.markers.clear();
        save_to(&ui, &path);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("com.test.game"));
        assert!(saved.contains("com.other.game"));
        assert!(load_entry_in(&path, "com.test.game").0.is_empty());
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
        // Tapping a row is looked up by its id, so adding rows cannot break
        // this test.
        let layout = compute_layout(&ui, VIEWPORT);
        let menu = layout.menu.expect("the menu is open");
        let row = |id: u16| {
            let (_, rect) = menu
                .rows
                .iter()
                .find(|(row_id, _)| *row_id == id)
                .expect("the menu has this row");
            (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
        };
        // "ADD BUTTON" adds one marker.
        let at = row(W_ADD);
        tap(&mut ui, at, VIEWPORT);
        assert_eq!(ui.markers.len(), 1);
        // "CLEAR ALL" removes it again.
        let at = row(W_CLEAR);
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

    fn sticks(left_x: f32, left_y: f32, right_x: f32, right_y: f32) -> AxisInputs {
        AxisInputs {
            sticks: [left_x, left_y, right_x, right_y],
            ..Default::default()
        }
    }

    #[test]
    fn stick_directions_become_button_edges() {
        let mut tracker = AxisTracker::default();
        // The first sample only records the baseline.
        assert!(tracker.edges(sticks(0.0, 0.0, 0.0, 0.0)).is_empty());
        // Push the left stick left and up: two presses, one per direction.
        let edges = tracker.edges(sticks(-1.0, -1.0, 0.0, 0.0));
        assert_eq!(
            edges,
            vec![
                (Button::LeftStickLeft, true),
                (Button::LeftStickUp, true)
            ]
        );
        // Holding it must not retrigger.
        assert!(tracker.edges(sticks(-1.0, -1.0, 0.0, 0.0)).is_empty());
        // Swinging to the right releases one direction and presses the other.
        let edges = tracker.edges(sticks(0.8, -1.0, 0.0, 0.0));
        assert_eq!(
            edges,
            vec![
                (Button::LeftStickLeft, false),
                (Button::LeftStickRight, true)
            ]
        );
        // Centring releases everything that was held.
        let edges = tracker.edges(sticks(0.0, 0.0, 0.0, 0.0));
        assert_eq!(
            edges,
            vec![
                (Button::LeftStickRight, false),
                (Button::LeftStickUp, false)
            ]
        );
    }

    #[test]
    fn each_stick_has_its_own_four_directions() {
        let mut tracker = AxisTracker::default();
        tracker.edges(AxisInputs::default());
        let edges = tracker.edges(sticks(0.0, 0.0, 1.0, 1.0));
        assert_eq!(
            edges,
            vec![
                (Button::RightStickRight, true),
                (Button::RightStickDown, true)
            ]
        );
        let mut tracker = AxisTracker::default();
        tracker.edges(AxisInputs::default());
        let edges = tracker.edges(sticks(0.0, 0.0, -1.0, -1.0));
        assert_eq!(
            edges,
            vec![
                (Button::RightStickLeft, true),
                (Button::RightStickUp, true)
            ]
        );
    }

    #[test]
    fn triggers_are_edges_too() {
        let mut tracker = AxisTracker::default();
        tracker.edges(AxisInputs::default());
        let mut inputs = AxisInputs::default();
        inputs.left_trigger = true;
        assert_eq!(tracker.edges(inputs), vec![(Button::LeftTrigger, true)]);
        inputs.left_trigger = false;
        inputs.right_trigger = true;
        assert_eq!(
            tracker.edges(inputs),
            vec![(Button::LeftTrigger, false), (Button::RightTrigger, true)]
        );
        assert_eq!(
            tracker.edges(AxisInputs::default()),
            vec![(Button::RightTrigger, false)]
        );
    }

    #[test]
    fn turning_the_overlay_on_with_a_stick_pushed_does_not_press_anything() {
        // The tracker keeps its baseline while the overlay is off, so the
        // first sample after enabling it is a no-op.
        let mut tracker = AxisTracker::default();
        tracker.edges(sticks(-1.0, 0.0, 0.0, 0.0));
        assert!(tracker.edges(sticks(-1.0, 0.0, 0.0, 0.0)).is_empty());
    }

    /// Every stick direction and trigger is a real button, so a marker can be
    /// bound to it like to A or X.
    #[test]
    fn stick_and_trigger_buttons_can_be_bound_to_markers() {
        for button in [
            Button::LeftStickLeft,
            Button::LeftStickRight,
            Button::LeftStickUp,
            Button::LeftStickDown,
            Button::RightStickLeft,
            Button::RightStickRight,
            Button::RightStickUp,
            Button::RightStickDown,
            Button::LeftTrigger,
            Button::RightTrigger,
            Button::LeftShoulder,
            Button::RightShoulder,
        ] {
            let mut ui = ui_with_marker(0.5, 0.5);
            tap(&mut ui, (240.0, 160.0), VIEWPORT);
            let changes = apply_gamepad(&mut ui, button, true).expect("the press is ours");
            assert!(changes.is_empty(), "{button:?} only bound, did not press");
            assert_eq!(ui.markers[0].button, Some(button));
            let changes = apply_gamepad(&mut ui, button, true).expect("now it presses");
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].action, MarkerAction::Down);
            let changes = apply_gamepad(&mut ui, button, false).expect("and releases");
            assert_eq!(changes[0].action, MarkerAction::Up);
        }
    }

    // ---- movement pads ----

    /// An overlay with both movement pads added, which is what the pad tests
    /// need; `Ui::new()` starts with neither, like a fresh app.
    fn pads_ui() -> Ui {
        let mut ui = Ui::new();
        ui.enabled = true;
        ui.pads[0].enabled = true;
        ui.pads[1].enabled = true;
        ui
    }

    /// Both symptoms the player reported after build `PADS R4` came from the
    /// same mistake: an enabled pad was hidden when camera mode was on, and the
    /// build tag never moved. A pad the menu calls `ON` must always draw.
    #[test]
    fn an_enabled_pad_is_always_drawn_at_full_strength() {
        // A pad the menu calls ON has to be on screen. The camera used to hide
        // it outright, which is the bug the player reported; now the camera is
        // not consulted at all.
        let mut ui = pads_ui();
        for cross_on in [false, true] {
            ui.pads[1].enabled = cross_on;
            for look in [false, true] {
                ui.look = look;
                let alpha = pad_alpha(&ui, 1);
                assert_eq!(
                    alpha,
                    if cross_on { Some(1.0) } else { None },
                    "cross {} with camera {}",
                    if cross_on { "ON" } else { "OFF" },
                    if look { "ON" } else { "OFF" }
                );
            }
        }
    }

    /// The circle keeps its analogue look while the camera is on; only the
    /// cross, whose stick the camera has, is faded.
    #[test]
    fn the_camera_does_not_dim_either_pad() {
        let mut ui = pads_ui();
        ui.look = true;
        assert_eq!(pad_alpha(&ui, 0), Some(1.0));
        assert_eq!(pad_alpha(&ui, 1), Some(1.0), "the cross is not faded");
        // "Hide buttons" is the only thing that dims them.
        ui.hidden = true;
        assert_eq!(pad_alpha(&ui, 0), Some(HIDDEN_ALPHA));
        assert_eq!(pad_alpha(&ui, 1), Some(HIDDEN_ALPHA));
    }

    /// `PADS R4` shipped with the round-4 code but the old tag, so the player
    /// could not tell builds apart. The tag has to move with the behaviour.
    #[test]
    fn the_revision_tag_names_this_build() {
        // The tag has to be a pad revision and must not be one of the two that
        // shipped with the wrong behaviour: R4 was round-4 code under the
        // round-3 tag, and R5 hid the cross whenever camera mode was on.
        let revision = OVERLAY_REVISION
            .strip_prefix("PADS R")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|number| number.parse::<u32>().ok())
            .expect("the tag names a pad revision");
        assert!(revision >= 6, "got {:?}", OVERLAY_REVISION);
    }

    /// The cross has to stay grabbable: a player who wants it out of the way
    /// must be able to move it, camera on or off.
    #[test]
    fn the_cross_can_still_be_dragged_with_the_camera_on() {
        let mut ui = pads_ui();
        ui.look = true;
        let (px, py, radius) = compute_layout(&ui, VIEWPORT).pads[1];
        assert!(radius > 0.0, "an enabled pad has a box");
        // The hit test does not consult the camera, so a press on the pad lands
        // on it, not on the game behind.
        let inside = (px, py);
        let mut hit = ui;
        assert!(touch_down_inner_abs(&mut hit, inside, VIEWPORT));
        assert_eq!(hit.pending, W_PAD_BASE + 1);
    }

    /// The point of the whole change: the camera and the cross are on the same
    /// controller but different inputs, so both work at once. The player turns
    /// the camera with the right stick while choosing a weapon with the D-pad.
    #[test]
    fn the_camera_and_the_cross_work_at_the_same_time() {
        let mut ui = pads_ui();
        ui.look = true;
        // Turn the camera with the right stick.
        let turning = push_sticks_dt(&mut ui, [0.0, 0.0, 1.0, 0.0], 0.016);
        assert!(
            turning.iter().any(|change| change.marker == LOOK_FINGER),
            "the camera drags"
        );
        // A second camera sample keeps its finger down (the first one only
        // took it up, and one poll is not a release).
        push_sticks_dt(&mut ui, [0.0, 0.0, 1.0, 0.0], 0.016);
        assert!(ui.look_touching, "the camera has a finger down");
        // ...while the D-pad picks a weapon.
        let changes = press_dpad(&mut ui, &[Button::DPadUp]);
        let cross_changes: Vec<_> = changes
            .iter()
            .filter(|change| change.marker == PAD_FINGER_BASE + 1)
            .collect();
        assert!(!cross_changes.is_empty(), "{changes:?}");
        assert_eq!(cross_changes[0].action, MarkerAction::Down);
        // Both are live: the camera's finger and the cross's.
        assert!(pad_of(&ui, PadKind::Cross).touching);
        assert_eq!(pad_of(&ui, PadKind::Cross).held, Some(0));
        // Walking still works on the left stick, so all three at once.
        let walking = push_sticks_dt(&mut ui, [0.0, -1.0, 1.0, 0.0], 0.016);
        assert!(
            walking.iter().any(|change| change.marker == PAD_FINGER_BASE),
            "the left stick still walks: {walking:?}"
        );
        // The cross kept its direction through all of that.
        assert_eq!(pad_of(&ui, PadKind::Cross).held, Some(0));
        assert!(pad_of(&ui, PadKind::Cross).touching);
        // Once the swipe has finished, the right stick cannot move the cross's
        // finger: that was the original bug, camera dragging a weapon choice.
        for _ in 0..8 {
            pads_move_inner(&mut ui, [0.0; 4], 0.02);
        }
        let settled = ui.pads[1].swipe;
        let turning = push_sticks_dt(&mut ui, [0.0, 0.0, -1.0, 0.0], 0.016);
        assert!(
            turning.iter().any(|change| change.marker == LOOK_FINGER),
            "the camera still drags: {turning:?}"
        );
        assert_eq!(ui.pads[1].swipe, settled, "the camera left the cross alone");
        assert_eq!(pad_of(&ui, PadKind::Cross).held, Some(0));
    }

    /// Camera mode used to take the right stick away from the cross, which is
    /// why the cross vanished. It must not affect the cross at all now.
    #[test]
    fn the_camera_never_stops_the_cross() {
        for look in [false, true] {
            let mut ui = pads_ui();
            ui.look = look;
            let changes = press_dpad(&mut ui, &[Button::DPadLeft]);
            assert!(
                changes.iter().any(|change| change.action == MarkerAction::Down),
                "camera {}: the cross must still press",
                if look { "ON" } else { "OFF" }
            );
            assert_eq!(pad_of(&ui, PadKind::Cross).held, Some(6));
        }
    }

    /// A D-pad button can still be bound to a marker; that takes the cross
    /// out of the way rather than fighting for it.
    #[test]
    fn binding_a_dpad_button_takes_the_cross_out_of_the_way() {
        let mut ui = pads_ui();
        let mut marker = Marker::new(0, 0.5, 0.5);
        marker.button = Some(Button::DPadUp);
        ui.markers.push(marker);
        let changes = press_dpad(&mut ui, &[Button::DPadUp]);
        // The marker presses its own spot; the cross must not press anything.
        assert!(
            !changes.iter().any(|change| change.marker == PAD_FINGER_BASE + 1),
            "the marker owns that button: {changes:?}"
        );
        assert!(changes.iter().any(|change| change.marker == 0), "the marker fires");
        assert!(!pad_of(&ui, PadKind::Cross).touching);
        // The untouched directions still work through the pad.
        let changes = press_dpad(&mut ui, &[Button::DPadDown]);
        assert!(
            changes.iter().any(|change| change.marker == PAD_FINGER_BASE + 1),
            "the rest of the D-pad still works: {changes:?}"
        );
    }

    /// Nothing may leave a D-pad bit stuck down: turning the pad off, or
    /// releasing a button, has to clear the state the cross reads.
    #[test]
    fn a_dpad_button_never_sticks_down() {
        let mut ui = pads_ui();
        press_dpad(&mut ui, &[Button::DPadUp]);
        assert_eq!(ui.dpad, 1);
        release_dpad(&mut ui, &[Button::DPadUp]);
        assert_eq!(ui.dpad, 0, "the bit is cleared");
        assert_eq!(pad_of(&ui, PadKind::Cross).held, None);
        // Switching the pad off and on again must not resurrect the direction.
        let mut ui = pads_ui();
        press_dpad(&mut ui, &[Button::DPadRight]);
        toggle_pad(&mut ui, 1);
        assert_eq!(ui.dpad, 0);
        toggle_pad(&mut ui, 1);
        assert_eq!(pads_move_inner(&mut ui, [0.0; 4], 0.0).len(), 0);
    }

    /// A weapon-wheel tap can go down and up inside one event poll. The press
    /// lands on the edge, so it is not missed, and the release still comes
    /// after the pad's grace period rather than in the same frame -- the game
    /// gets one clean tap.
    #[test]
    fn a_tap_inside_one_poll_still_reaches_the_game() {
        let mut ui = pads_ui();
        // Down and up with no poll in between, the way a quick tap arrives.
        let down = apply_gamepad(&mut ui, Button::DPadUp, true).unwrap();
        let up = apply_gamepad(&mut ui, Button::DPadUp, false).unwrap();
        assert!(
            down.iter().any(|change| change.action == MarkerAction::Down),
            "the press is not missed: {down:?}"
        );
        assert!(
            up.iter().all(|change| change.action != MarkerAction::Down),
            "the release does not press again: {up:?}"
        );
        // The finger is still down -- the grace period holds it -- and the
        // polls that follow lift it.
        assert!(pad_of(&ui, PadKind::Cross).touching);
        let mut lifted = Vec::new();
        for _ in 0..12 {
            lifted.extend(pads_move_inner(&mut ui, [0.0; 4], PAD_RELEASE_DELAY * 0.5));
        }
        assert!(
            lifted.iter().any(|change| change.action == MarkerAction::Up),
            "the tap is released: {lifted:?}"
        );
        assert!(!pad_of(&ui, PadKind::Cross).touching);
    }

    /// Two D-pad buttons at once is a diagonal, and letting go of one must
    /// leave the other still held -- a finger rolling round a wheel.
    #[test]
    fn rolling_round_the_dpad_is_one_finger() {
        let mut ui = pads_ui();
        apply_gamepad(&mut ui, Button::DPadUp, true);
        let changes = apply_gamepad(&mut ui, Button::DPadRight, true).unwrap();
        assert_eq!(pad_of(&ui, PadKind::Cross).held, Some(1), "up-right");
        assert!(
            changes
                .iter()
                .all(|change| change.action == MarkerAction::Move),
            "a direction change is a Move, never a fresh Down: {changes:?}"
        );
        // Let go of up: still pressing, now on right alone.
        let changes = apply_gamepad(&mut ui, Button::DPadUp, false).unwrap();
        assert_eq!(pad_of(&ui, PadKind::Cross).held, Some(2), "right");
        assert!(changes
            .iter()
            .all(|change| change.action == MarkerAction::Move));
        assert!(pad_of(&ui, PadKind::Cross).touching);
    }

    /// The core of the D-pad-as-swipe change: pressing a direction slides a
    /// finger from the middle of the pad outwards, and it covers real distance.
    /// A touch that merely appeared somewhere would give a wheel no direction
    /// to read, which is why the old D-pad did nothing at all.
    #[test]
    fn a_dpad_press_is_a_swipe_from_the_middle() {
        let mut ui = pads_ui();
        let centre = (ui.pads[1].cx, ui.pads[1].cy);
        let changes = press_dpad_dt(&mut ui, &[Button::DPadRight], 0.0);
        let down = changes
            .iter()
            .find(|change| change.action == MarkerAction::Down)
            .expect("the swipe starts");
        // It goes down in the middle of the pad, not on the rim.
        let radius = ui.pads[1].radius;
        let from_centre = (down.position.0 - centre.0).hypot(down.position.1 - centre.1);
        assert!(
            from_centre < radius * 0.2,
            "the finger starts near the middle: {from_centre} vs radius {radius}"
        );
        // Then it travels outwards, one Move at a time.
        let mut travelled = from_centre;
        let mut moves = 0;
        for _ in 0..12 {
            let changes = pads_move_inner(&mut ui, [0.0; 4], 0.02);
            for change in changes {
                if change.action == MarkerAction::Move {
                    moves += 1;
                    travelled = (change.position.0 - centre.0).hypot(change.position.1 - centre.1);
                }
            }
        }
        assert!(moves >= 3, "the swipe is spread over samples, not one jump: {moves}");
        // `radius` is stored as a fraction of the shorter screen side, so the
        // travel to compare against is the same fraction of the viewport.
        assert!(
            travelled > radius * 0.5,
            "the finger actually crossed the pad: {travelled} vs half radius {radius}"
        );
        // It travelled to the right, which is the direction that was pressed.
        let last = ui.pads[1].swipe;
        assert!(last.0 > centre.0, "the finger went right: {:?}", last);
        assert!((last.1 - centre.1).abs() < radius * 0.1, "and not up or down");
    }

    /// Up, down, left, right each slide their own way.
    #[test]
    fn each_dpad_button_swipes_its_own_way() {
        for (button, dx, dy) in [
            (Button::DPadUp, 0.0, -1.0),
            (Button::DPadDown, 0.0, 1.0),
            (Button::DPadLeft, -1.0, 0.0),
            (Button::DPadRight, 1.0, 0.0),
        ] {
            let mut ui = pads_ui();
            press_dpad_dt(&mut ui, &[button], 0.0);
            for _ in 0..10 {
                pads_move_inner(&mut ui, [0.0; 4], 0.02);
            }
            let centre = (ui.pads[1].cx, ui.pads[1].cy);
            let tip = ui.pads[1].swipe;
            let moved = (tip.0 - centre.0, tip.1 - centre.1);
            let length = moved.0.hypot(moved.1);
            assert!(length > ui.pads[1].radius * 0.4, "{:?} did not travel", button);
            // The angle matches the button.
            assert!(
                moved.0 * dx + moved.1 * dy > length * 0.9,
                "{:?} swiped {:?}, wanted ({}, {})",
                button,
                moved,
                dx,
                dy
            );
        }
    }

    /// Lifting the button lets go after the grace period, wherever the swipe
    /// got to, so a tap plays out as a tap.
    #[test]
    fn a_swipe_is_lifted_where_it_stopped() {
        let mut ui = pads_ui();
        press_dpad_dt(&mut ui, &[Button::DPadUp], 0.0);
        for _ in 0..8 {
            pads_move_inner(&mut ui, [0.0; 4], 0.02);
        }
        let tip = ui.pads[1].swipe;
        // Releasing must not move the finger: it only starts the grace period.
        release_dpad_dt(&mut ui, &[Button::DPadUp], 0.02);
        let mut up = None;
        for _ in 0..12 {
            for change in pads_move_inner(&mut ui, [0.0; 4], PAD_RELEASE_DELAY * 0.6) {
                if change.action == MarkerAction::Up {
                    up = Some(change.position);
                }
            }
        }
        let up = up.expect("the finger lifts");
        assert!(
            (up.0 - tip.0).abs() < 1e-3 && (up.1 - tip.1).abs() < 1e-3,
            "the lift is where the swipe stopped: {:?} vs {:?}",
            up,
            tip
        );
    }

    /// A weapon wheel is a wheel: it needs the finger to *start* near the
    /// middle and travel out. A touch that lands straight on the rim is a
    /// complete swipe with no travel, which most wheels read as nothing at
    /// all -- the reason the old D-pad appeared dead.
    #[test]
    fn the_swipe_starts_near_the_middle_not_on_the_rim() {
        let mut ui = pads_ui();
        let radius = ui.pads[1].radius;
        let centre = (ui.pads[1].cx, ui.pads[1].cy);
        // Every direction starts close to the middle...
        for button in [
            Button::DPadUp,
            Button::DPadDown,
            Button::DPadLeft,
            Button::DPadRight,
        ] {
            let mut ui2 = pads_ui();
            press_dpad_dt(&mut ui2, &[button], 0.0);
            let start = ui2.pads[1].swipe;
            let from_centre = (start.0 - centre.0).hypot(start.1 - centre.1);
            assert!(
                from_centre < radius * 0.15,
                "{:?} started {:?}, which is {} from the middle of a {} pad",
                button,
                start,
                from_centre,
                radius
            );
        }
        // ...and the press really is a travel, not a jump: the Down is near the
        // middle and the next Move is further out.
        let changes = press_dpad_dt(&mut ui, &[Button::DPadRight], 0.0);
        let down = changes
            .iter()
            .find(|change| change.action == MarkerAction::Down)
            .unwrap()
            .position;
        for _ in 0..3 {
            pads_move_inner(&mut ui, [0.0; 4], 0.02);
        }
        let now = ui.pads[1].swipe;
        assert!(
            now.0 - down.0 > radius * 0.2,
            "the finger travelled outwards: {:?} -> {:?}",
            down,
            now
        );
    }

    /// The swipe is driven by time, so it must finish even if the direction
    /// never changes again, and stop growing afterwards.
    #[test]
    fn a_held_direction_finishes_its_swipe_and_stops() {
        let mut ui = pads_ui();
        press_dpad_dt(&mut ui, &[Button::DPadUp], 0.0);
        let mut last = ui.pads[1].swipe;
        for _ in 0..20 {
            pads_move_inner(&mut ui, [0.0; 4], 0.02);
            let now = ui.pads[1].swipe;
            assert!(now.1 <= last.1 + 1e-6, "the finger never goes backwards");
            last = now;
        }
        let settled = ui.pads[1].swipe;
        for _ in 0..5 {
            pads_move_inner(&mut ui, [0.0; 4], 0.02);
        }
        assert_eq!(ui.pads[1].swipe, settled, "the swipe does not keep growing");
        // It reached the rim's neighbourhood rather than stopping short.
        let travel = (settled.1 - ui.pads[1].cy).abs();
        assert!(
            travel > ui.pads[1].radius * 0.6,
            "the swipe got most of the way out: {} of {}",
            travel,
            ui.pads[1].radius
        );
    }

    /// The player taps a weapon-wheel direction quickly -- quicker than the
    /// swipe takes. The finger must still travel all the way out before it
    /// lifts, or the game sees a touch near the middle of the wheel and picks
    /// nothing. This is the case that would still look "dead" if the release
    /// simply cut the swipe short.
    #[test]
    fn a_quick_tap_still_swipes_all_the_way_out() {
        let mut ui = pads_ui();
        let centre = (ui.pads[1].cx, ui.pads[1].cy);
        let radius = ui.pads[1].radius;

        // Down, then up almost at once: no time to travel.
        apply_gamepad(&mut ui, Button::DPadDown, true);
        let mut events: Vec<MarkerTouch> = Vec::new();
        events.extend(pads_move_inner(&mut ui, [0.0; 4], 0.016));
        apply_gamepad(&mut ui, Button::DPadDown, false);
        events.extend(pads_move_inner(&mut ui, [0.0; 4], 0.016));

        // The finger must still be travelling, inside the pad.
        let mid = ui.pads[1].swipe;
        let mid_travel = (mid.1 - centre.1).abs();
        assert!(
            (mid_travel - (CROSS_SWIPE * radius)).abs() > radius * 0.05,
            "the tap was cut short at {} of {}",
            mid_travel,
            CROSS_SWIPE * radius
        );

        // Let time pass with no button held: the travel finishes, then lifts.
        let mut lifted_after = None;
        let mut furthest = mid_travel;
        for step in 0..40 {
            for change in pads_move_inner(&mut ui, [0.0; 4], 0.016) {
                events.push(change);
                if change.action == MarkerAction::Up {
                    lifted_after = Some(step);
                    furthest = (change.position.1 - centre.1).abs();
                }
            }
        }
        let lifted_after = lifted_after.expect("the finger does eventually lift");
        assert!(
            lifted_after > 0,
            "it travelled before lifting, not on the same poll"
        );
        // Where it lifted is the fully-swiped spot, not the middle.
        assert!(
            furthest > radius * 0.6,
            "the tap reached the direction: {} of {}",
            furthest,
            radius
        );
        // And the up happens down-screen, which is the direction pressed.
        assert!(furthest > (mid_travel).max(radius * 0.6));
        assert!(!ui.pads[1].touching);
    }

    fn pad_of(ui: &Ui, kind: PadKind) -> Pad {
        ui.pads[kind.pad()]
    }

    /// Presses D-pad buttons and runs one poll with the given `dt`, so the
    /// swipe has a real slice of time to travel through.
    fn press_dpad_dt(ui: &mut Ui, buttons: &[Button], dt: f32) -> Vec<MarkerTouch> {
        let mut changes = Vec::new();
        for &button in buttons {
            if let Some(more) = apply_gamepad(ui, button, true) {
                changes.extend(more);
            }
        }
        changes.extend(pads_move_inner(ui, [0.0; 4], dt));
        changes
    }

    /// Releases D-pad buttons and runs one poll with the given `dt`.
    fn release_dpad_dt(ui: &mut Ui, buttons: &[Button], dt: f32) -> Vec<MarkerTouch> {
        let mut changes = Vec::new();
        for &button in buttons {
            if let Some(more) = apply_gamepad(ui, button, false) {
                changes.extend(more);
            }
        }
        changes.extend(pads_move_inner(ui, [0.0; 4], dt));
        changes
    }

    /// Presses D-pad buttons on the cross pad and returns what it sent.
    ///
    /// The cross reads the D-pad, so a test drives it the way a controller
    /// does: through [Ui::dpad], then one poll.
    fn press_dpad(ui: &mut Ui, buttons: &[Button]) -> Vec<MarkerTouch> {
        let mut changes = Vec::new();
        for &button in buttons {
            if let Some(more) = apply_gamepad(ui, button, true) {
                changes.extend(more);
            }
        }
        changes.extend(pads_move_inner(ui, [0.0; 4], 0.0));
        changes
    }

    /// Releases D-pad buttons and runs the poll that notices.
    fn release_dpad(ui: &mut Ui, buttons: &[Button]) -> Vec<MarkerTouch> {
        let mut changes = Vec::new();
        for &button in buttons {
            if let Some(more) = apply_gamepad(ui, button, false) {
                changes.extend(more);
            }
        }
        changes.extend(pads_move_inner(ui, [0.0; 4], 0.0));
        changes
    }

    /// Runs a stick sample through the pads and keeps the changes.
    fn push_sticks(ui: &mut Ui, sticks: [f32; 4]) -> Vec<MarkerTouch> {
        push_sticks_dt(ui, sticks, 0.0)
    }

    fn push_sticks_dt(ui: &mut Ui, sticks: [f32; 4], dt: f32) -> Vec<MarkerTouch> {
        pads_move_inner(ui, sticks, dt)
    }

    /// Centres the stick and runs out the release grace period, collecting
    /// every change on the way, so the finger really does lift.
    fn release_sticks(ui: &mut Ui) -> Vec<MarkerTouch> {
        let mut changes = Vec::new();
        // Several short samples, like real polls, until the grace period is
        // comfortably past.
        for _ in 0..6 {
            changes.extend(push_sticks_dt(ui, [0.0; 4], PAD_RELEASE_DELAY * 0.5));
        }
        changes
    }

    #[test]
    fn the_pads_start_bottom_left_and_bottom_right() {
        let ui = Ui::new();
        let circle = pad_of(&ui, PadKind::Circle);
        let cross = pad_of(&ui, PadKind::Cross);
        assert!(circle.cx < 0.5 && circle.cy > 0.5);
        assert!(cross.cx > 0.5 && cross.cy > 0.5);
        assert_eq!(circle.kind, PadKind::Circle);
        assert_eq!(cross.kind, PadKind::Cross);
    }

    #[test]
    fn the_circle_is_a_ring_and_the_cross_is_two_arms() {
        let (px, py, radius) = (100.0, 100.0, 50.0);
        let ring = pad_outline(PadKind::Circle, px, py, radius);
        // A ring keeps every dot at the radius, all the way round.
        assert!(ring.len() > 20);
        for (x, y) in &ring {
            let distance = (x - px).hypot(y - py);
            assert!((distance - radius).abs() < 0.5, "ring dot off the radius");
        }
        // A cross is four arms: dots on the axes, none off them, and a hole
        // in the middle for the knob.
        let cross = pad_outline(PadKind::Cross, px, py, radius);
        assert_eq!(cross.len(), 4 * 9);
        for (x, y) in &cross {
            let (dx, dy) = (x - px, y - py);
            assert!(
                dx.abs() < 0.5 || dy.abs() < 0.5,
                "a cross dot is on an axis"
            );
            let distance = dx.abs().max(dy.abs());
            assert!(distance >= radius * 0.29, "the middle is left clear");
            assert!(distance <= radius + 0.5);
        }
        // The two shapes really are different, so the drawing cannot silently
        // fall back to a circle.
        assert_ne!(ring.len(), cross.len());
    }

    #[test]
    fn each_pad_only_answers_its_own_input() {
        // The right stick belongs to nobody: not to the circle, and not to the
        // cross either, which reads the D-pad buttons instead. Moving it is the
        // camera's business and must not touch either pad.
        let mut ui = pads_ui();
        let changes = push_sticks(&mut ui, [0.0, 0.0, 1.0, 0.0]);
        assert!(changes.is_empty(), "the right stick drives no pad: {:?}", changes);
        assert_eq!(pad_of(&ui, PadKind::Cross).held, None);
        assert!(!pad_of(&ui, PadKind::Cross).touching);
        let circle = pad_of(&ui, PadKind::Circle);
        assert_eq!(circle.axis, (0.0, 0.0));
        assert!(!circle.touching);

        // The D-pad drives the cross, and only the cross.
        let changes = press_dpad(&mut ui, &[Button::DPadRight]);
        // A swipe: one Down, then Moves as the finger travels out.
        assert_eq!(changes[0].action, MarkerAction::Down);
        assert!(changes
            .iter()
            .all(|change| change.marker == PAD_FINGER_BASE + 1));
        assert!(changes.len() > 1, "the finger travels: {changes:?}");
        let cross = pad_of(&ui, PadKind::Cross);
        assert_eq!(cross.held, Some(2), "right is direction 2");
        assert!(cross.touching);
        assert_eq!(pad_of(&ui, PadKind::Circle).axis, (0.0, 0.0));

        // The left stick drives the circle, and only the circle. The cross may
        // still be finishing its swipe, so look at the circle's own finger.
        let circle_changes: Vec<_> = push_sticks(&mut ui, [0.0, -1.0, 0.0, 0.0])
            .into_iter()
            .filter(|change| change.marker == PAD_FINGER_BASE)
            .collect();
        assert!(!circle_changes.is_empty(), "the left stick walks");
    }

    /// The bug that made walking feel broken: each direction used to be its
    /// own finger, so a game that tracks one finger ignored the second one and
    /// the player had to lift the stick before the change registered. A pad is
    /// one finger, so every direction change is an ordinary Move.
    #[test]
    fn a_pad_is_one_finger_that_can_turn_anywhere() {
        let mut ui = pads_ui();
        // Walk forwards...
        let changes = push_sticks(&mut ui, [0.0, -1.0, 0.0, 0.0]);
        assert_eq!(changes[0].action, MarkerAction::Down);
        let finger = changes[0].marker;
        assert!(changes.iter().all(|change| change.marker == finger));
        // ...then turn down-left without lifting: a single Move, same finger.
        let changes = push_sticks(&mut ui, [-1.0, 1.0, 0.0, 0.0]);
        assert_eq!(changes.len(), 1, "turning is one move, not a new finger");
        assert_eq!(changes[0].action, MarkerAction::Move);
        assert_eq!(changes[0].marker, finger);
        let circle = pad_of(&ui, PadKind::Circle);
        // The finger went from above the middle to below and to the left.
        assert!(changes[0].position.1 > circle.cy);
        assert!(changes[0].position.0 < circle.cx);
        // Reversing straight backwards is also just a move.
        let changes = push_sticks(&mut ui, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Move);
        assert_eq!(changes[0].marker, finger);
        // Only coming back to rest lifts it, once.
        let changes = release_sticks(&mut ui);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Up);
        assert_eq!(changes[0].marker, finger);
    }

    /// Walking forward and then reversing must be one uninterrupted touch: no
    /// Up in between, or the game sees the stick released and the character
    /// stops until the player lifts and pushes again.
    #[test]
    fn reversing_while_walking_never_lifts_the_finger() {
        let mut ui = pads_ui();
        let forward = push_sticks(&mut ui, [0.0, -1.0, 0.0, 0.0]);
        assert_eq!(forward[0].action, MarkerAction::Down);
        // Sweep from forward to backward through the middle, as a thumb would.
        let mut saw_up = false;
        let mut actions = Vec::new();
        for y in [-1.0, -0.6, -0.2, 0.0, 0.2, 0.6, 1.0] {
            for change in push_sticks(&mut ui, [0.0, y, 0.0, 0.0]) {
                if change.action == MarkerAction::Up {
                    saw_up = true;
                }
                actions.push(change.action);
            }
        }
        assert!(
            !saw_up,
            "the finger stays down through the whole sweep: {actions:?}"
        );
        assert_eq!(
            *actions.last().unwrap(),
            MarkerAction::Move,
            "and the last thing the game sees is a move"
        );
        // The finger really did end up behind the middle.
        let circle = pad_of(&ui, PadKind::Circle);
        let end = push_sticks(&mut ui, [0.0, 1.0, 0.0, 0.0]);
        assert!(end.is_empty(), "holding it still is silent");
        assert!(circle.push > 0.0);
        assert!(circle.finger_analogue().1 > circle.cy || circle.axis.1 > 0.0);
    }

    /// A diagonal must actually be a diagonal: the finger sits on the stick's
    /// own angle, so walking diagonally goes diagonally.
    #[test]
    fn a_diagonal_finger_follows_the_sticks_angle() {
        let mut ui = pads_ui();
        push_sticks(&mut ui, [0.0, -1.0, 0.0, 0.0]);
        let changes = push_sticks(&mut ui, [-1.0, 1.0, 0.0, 0.0]);
        let circle = pad_of(&ui, PadKind::Circle);
        let (dx, dy) = (
            changes[0].position.0 - circle.cx,
            changes[0].position.1 - circle.cy,
        );
        // Down-left at 45 degrees: equal parts, both non-zero.
        assert!(dx < 0.0 && dy > 0.0);
        assert!((dx.abs() - dy.abs()).abs() < 1e-3, "{dx} vs {dy}");
    }

    #[test]
    fn a_brief_pause_does_not_lift_the_finger_but_a_long_one_does() {
        let mut ui = pads_ui();
        push_sticks_dt(&mut ui, [0.0, -1.0, 0.0, 0.0], 0.016);
        // A pause shorter than the grace period: still down.
        let changes = push_sticks_dt(&mut ui, [0.0; 4], PAD_RELEASE_DELAY * 0.4);
        assert!(changes.is_empty(), "a short pause is not a release");
        assert!(pad_of(&ui, PadKind::Circle).touching);
        // Resuming the same deflection is silent: the finger never moved.
        assert!(push_sticks_dt(&mut ui, [0.0, -1.0, 0.0, 0.0], 0.016).is_empty());
        // Resuming at a different deflection is a Move, not a new Down.
        let changes = push_sticks_dt(&mut ui, [1.0, -0.4, 0.0, 0.0], 0.016);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Move);
        // Resting for longer does lift it.
        let changes = release_sticks(&mut ui);
        assert_eq!(changes.last().unwrap().action, MarkerAction::Up);
        assert!(!pad_of(&ui, PadKind::Circle).touching);
    }

    /// A cross is a D-pad: its touch stays on one spot while a direction is
    /// held. A sliding finger is what a game reads as its own joystick moving,
    /// which made the character drift on a phone shooter while the player was
    /// only turning.
    #[test]
    fn the_right_stick_cannot_move_the_cross() {
        let mut ui = pads_ui();
        press_dpad(&mut ui, &[Button::DPadRight]);
        for _ in 0..8 {
            pads_move_inner(&mut ui, [0.0; 4], 0.02);
        }
        // Settled at the end of its swipe.
        let settled = ui.pads[1].swipe;
        // Wiggle the right stick while the button stays down: the finger must
        // not move. This is the bug the player hit -- turning the camera used
        // to drag a finger that was choosing a weapon.
        for (x, y) in [(0.9, 0.05), (1.0, -0.1), (0.75, 0.1), (0.0, -1.0)] {
            let changes = push_sticks(&mut ui, [0.0, 0.0, x, y]);
            assert!(
                !changes
                    .iter()
                    .any(|change| change.marker == PAD_FINGER_BASE + 1),
                "the right stick must not touch the cross: {changes:?}"
            );
        }
        assert_eq!(ui.pads[1].swipe, settled, "the finger has not moved");
        assert!(pad_of(&ui, PadKind::Cross).touching);
    }

    /// Where the cross's arrow for `direction` sits on a standard viewport.
    fn circle_rim(ui: &Ui, pad_index: usize, direction: usize) -> (f32, f32) {
        let pad = &ui.pads[pad_index];
        let (x, y) = PAD_DIRECTIONS_8[direction];
        (
            pad.cx + x * pad.reach * pad.radius * PAD_RIM,
            pad.cy + y * pad.reach * pad.radius * PAD_RIM,
        )
    }

    #[test]
    fn the_cross_picks_eight_directions() {
        // Every direction a weapon wheel can want, from the buttons alone: one
        // button for a cardinal, two together for a diagonal. No stick, no
        // threshold, no analogue travel to miss.
        use Button::{DPadDown, DPadLeft, DPadRight, DPadUp};
        let cases: [(&[Button], usize); 8] = [
            (&[DPadUp], 0),
            (&[DPadUp, DPadRight], 1),
            (&[DPadRight], 2),
            (&[DPadRight, DPadDown], 3),
            (&[DPadDown], 4),
            (&[DPadDown, DPadLeft], 5),
            (&[DPadLeft], 6),
            (&[DPadLeft, DPadUp], 7),
        ];
        for (buttons, index) in cases {
            let mut ui = pads_ui();
            press_dpad(&mut ui, buttons);
            assert_eq!(
                pad_of(&ui, PadKind::Cross).held,
                Some(index),
                "{:?} should be direction {}",
                buttons,
                index
            );
        }

        // Nothing held is nothing at all.
        let mut ui = pads_ui();
        assert_eq!(pads_move_inner(&mut ui, [0.0; 4], 0.0).len(), 0);
        assert_eq!(pad_of(&ui, PadKind::Cross).held, None);

        // Opposite buttons cancel rather than picking a diagonal.
        for pair in [
            [DPadUp, DPadDown],
            [DPadLeft, DPadRight],
        ] {
            let mut ui = pads_ui();
            press_dpad(&mut ui, &pair);
            assert_eq!(
                pad_of(&ui, PadKind::Cross).held,
                None,
                "{:?} is not a direction",
                pair
            );
        }
    }

    /// The camera must not be able to touch the game's own joystick: it drags a
    /// small box on the right-hand side of the screen, never the middle.
    #[test]
    fn the_camera_drag_stays_in_its_own_box() {
        let mut ui = pads_ui();
        ui.look = true;
        // Push every way and keep pushing: the finger never leaves the box.
        for (x, y) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            for _ in 0..200 {
                for change in push_sticks_dt(&mut ui, [0.0, 0.0, x, y], 0.05) {
                    assert_eq!(change.marker, LOOK_FINGER);
                    assert!(
                        (change.position.0 - LOOK_BOX.0).abs() <= LOOK_BOX.2 + 1e-4,
                        "camera left its box horizontally: {:?}",
                        change.position
                    );
                    assert!(
                        (change.position.1 - LOOK_BOX.1).abs() <= LOOK_BOX.2 * 1.6 + 1e-4,
                        "camera left its box vertically: {:?}",
                        change.position
                    );
                }
            }
            // Stop, so the next direction starts fresh.
            release_sticks(&mut ui);
        }
    }

    /// With camera mode on, the right stick belongs to the camera alone: the
    /// cross sends nothing, so the character cannot be nudged by turning.
    #[test]
    fn the_right_stick_is_inert_for_the_cross_in_camera_mode() {
        let mut ui = pads_ui();
        ui.look = true;
        for (x, y) in [(1.0, 0.0), (0.0, -1.0), (-1.0, 1.0)] {
            let changes = push_sticks_dt(&mut ui, [0.0, 0.0, x, y], 0.016);
            // Everything the right stick sent came from the camera.
            assert!(changes.iter().all(|change| change.marker == LOOK_FINGER));
        }
        let cross = pad_of(&ui, PadKind::Cross);
        assert!(
            !cross.touching,
            "the right stick never presses the cross"
        );
        assert_eq!(cross.held, None);
        // But the D-pad does press it, camera on or not.
        let changes = press_dpad(&mut ui, &[Button::DPadUp]);
        assert!(changes.iter().any(|change| change.marker == PAD_FINGER_BASE + 1));
        // The left stick still walks while the camera turns.
        let changes = push_sticks_dt(&mut ui, [0.0, -1.0, 0.0, 0.0], 0.016);
        assert!(changes.iter().any(|change| change.marker == PAD_FINGER_BASE));
    }

    #[test]
    fn the_pads_do_not_creep_on_a_wobbly_thumb() {
        let mut ui = pads_ui();
        // Inside the activation threshold: nothing down.
        let deadzone_amount = PAD_ACTIVATION * 0.5;
        assert!(push_sticks(&mut ui, [deadzone_amount, 0.0, 0.0, 0.0]).is_empty());
        let circle = pad_of(&ui, PadKind::Circle);
        assert_eq!(circle.axis, (0.0, 0.0));
        assert!(!circle.touching);
    }

    #[test]
    fn a_pad_touch_starts_in_the_middle_and_travels_outwards() {
        let mut ui = pads_ui();
        let circle = pad_of(&ui, PadKind::Circle);
        let rim = circle.point(3);
        // A full push: the finger goes down at the centre and ends on the rim.
        let changes = push_sticks(&mut ui, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(changes[0].action, MarkerAction::Down);
        assert!((changes[0].position.0 - circle.cx).abs() < 1e-6);
        assert!((changes[0].position.1 - circle.cy).abs() < 1e-6);
        // A full push in another direction is still one clean swipe.
        let mut ui = pads_ui();
        let changes = push_sticks(&mut ui, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(changes[0].action, MarkerAction::Down);
        assert!((changes[0].position.0 - circle.cx).abs() < 1e-6);
        assert!((changes[0].position.1 - circle.cy).abs() < 1e-6);
        let moved = changes.last().expect("the finger travelled");
        let full_travel = (circle.point(1).1 - circle.cy).abs();
        assert!((moved.position.1 - circle.cy - full_travel).abs() < 1e-4);
        // A light push: a short drag, so the finger barely leaves the middle.
        let mut ui = pads_ui();
        let light = PAD_ACTIVATION + (1.0 - PAD_ACTIVATION) * 0.1;
        let changes = push_sticks(&mut ui, [light, 0.0, 0.0, 0.0]);
        let travelled = changes
            .iter()
            .map(|change| (change.position.0 - circle.cx).abs())
            .fold(0.0f32, f32::max);
        let full = (rim.0 - circle.cx).abs();
        assert!(travelled < full * 0.2, "a light push is a short drag");
        // Releasing lifts the finger where it got to, at either strength, so
        // the game sees one clean swipe and not a drag back to the middle.
        let releases = release_sticks(&mut ui);
        assert_eq!(releases[0].action, MarkerAction::Up);
        let last = changes.last().unwrap();
        assert!((releases[0].position.0 - last.position.0).abs() < 1e-6);
        // And the same for a full push, which ends on the rim.
        let mut ui = pads_ui();
        let changes = push_sticks(&mut ui, [1.0, 0.0, 0.0, 0.0]);
        assert!((changes.last().unwrap().position.0 - rim.0).abs() < 1e-5);
        let releases = release_sticks(&mut ui);
        assert_eq!(releases[0].action, MarkerAction::Up);
        assert!((releases[0].position.0 - rim.0).abs() < 1e-5);
    }

    /// The bug that made the circle feel broken: the finger has to travel
    /// *further* the *harder* the stick is pushed. The game measures speed
    /// from the distance between its own stick's centre and the finger, so an
    /// inverted gesture walks at full speed on a tiny push and stands still at
    /// full deflection.
    #[test]
    fn a_harder_push_always_means_a_longer_drag() {
        let mut distances = Vec::new();
        for fraction in [0.02, 0.1, 0.3, 0.5, 0.75, 1.0] {
            let mut ui = pads_ui();
            let value = PAD_ACTIVATION + (1.0 - PAD_ACTIVATION) * fraction;
            let changes = push_sticks(&mut ui, [value, 0.0, 0.0, 0.0]);
            let circle = pad_of(&ui, PadKind::Circle);
            let last = changes.last().expect("the press sends something");
            distances.push(((last.position.0 - circle.cx).hypot(last.position.1 - circle.cy)));
        }
        for pair in distances.windows(2) {
            assert!(
                pair[1] > pair[0],
                "a harder push drags further: {distances:?}"
            );
        }
    }

    #[test]
    fn reach_shortens_the_drag_but_keeps_it_monotonic() {
        let mut ui = pads_ui();
        ui.pads[0].reach = 0.5;
        let circle = pad_of(&ui, PadKind::Circle);
        let changes = push_sticks(&mut ui, [1.0, 0.0, 0.0, 0.0]);
        let last = changes.last().unwrap();
        let travelled = (last.position.0 - circle.cx).abs();
        let full = (circle.point(3).0 - circle.cx).abs();
        assert!((travelled - full * 0.5).abs() < 1e-5, "half the reach");
        // Still monotonic, so a light push is still a shorter drag.
        let mut ui = pads_ui();
        ui.pads[0].reach = 0.5;
        let light = PAD_ACTIVATION + (1.0 - PAD_ACTIVATION) * 0.2;
        let light_changes = push_sticks(&mut ui, [light, 0.0, 0.0, 0.0]);
        let light_end = light_changes.last().unwrap();
        let light_travelled = (light_end.position.0 - circle.cx).abs();
        assert!(light_travelled < travelled);
    }

    #[test]
    fn cycling_reach_goes_round_and_stays_in_range() {
        let mut pad = Pad::new(PadKind::Circle, 0.2, 0.7);
        assert_eq!(pad.reach, 1.0);
        for _ in 0..REACH_STEPS.len() {
            cycle_reach(&mut pad);
            assert!(pad.reach >= 0.4 && pad.reach <= 1.0);
        }
        // A full cycle comes back to where it started.
        assert!((pad.reach - 1.0).abs() < 1e-6);
        assert_eq!(reach_label(&pad), "REACH 100%");
    }

    #[test]
    fn a_saved_reach_survives_and_missing_reach_means_full() {
        let parsed = parse_pads("circle,0.200,0.700,0.150,on,0.55").unwrap().0;
        assert!((parsed[0].reach - 0.55).abs() < 0.01);
        // Older entries have no reach field.
        let parsed = parse_pads("circle,0.200,0.700,0.150,on").unwrap().0;
        assert_eq!(parsed[0].reach, 1.0);
    }

    #[test]
    fn a_partial_push_sends_a_move_as_the_stick_goes_further() {
        let mut ui = pads_ui();
        let light = PAD_ACTIVATION + (1.0 - PAD_ACTIVATION) * 0.2;
        push_sticks(&mut ui, [light, 0.0, 0.0, 0.0]);
        let first = pad_of(&ui, PadKind::Circle).push;
        let changes = push_sticks(&mut ui, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Move);
        assert!(pad_of(&ui, PadKind::Circle).push > first);
        // The same value again is not a move.
        assert!(push_sticks(&mut ui, [1.0, 0.0, 0.0, 0.0]).is_empty());
    }

    #[test]
    fn dragging_the_handle_resizes_the_pad() {
        let mut ui = pads_ui();
        let layout = compute_layout(&ui, VIEWPORT);
        let (px, py, radius) = layout.pads[0];
        let handle = pad_handle_rect(&ui.pads[0], px, py, radius);
        let grab = (handle.x + handle.w / 2.0, handle.y + handle.h / 2.0);
        assert!(touch_down_inner_abs(&mut ui, grab, VIEWPORT));
        assert_eq!(ui.pending, W_PAD_BASE + PAD_COUNT as u16);
        // Drag the handle further out: the pad grows.
        let before = ui.pads[0].radius;
        touch_motion_inner_abs(&mut ui, (grab.0 + 40.0, grab.1 + 40.0), VIEWPORT);
        assert!(ui.pads[0].radius > before);
        assert!(touch_up_inner_abs(&mut ui, (grab.0 + 40.0, grab.1 + 40.0), VIEWPORT));
        // And dragging it in shrinks the pad again.
        let layout = compute_layout(&ui, VIEWPORT);
        let (px, py, radius) = layout.pads[0];
        let handle = pad_handle_rect(&ui.pads[0], px, py, radius);
        let grab = (handle.x + handle.w / 2.0, handle.y + handle.h / 2.0);
        assert!(touch_down_inner_abs(&mut ui, grab, VIEWPORT));
        let before = ui.pads[0].radius;
        touch_motion_inner_abs(&mut ui, (px, py + 4.0), VIEWPORT);
        assert!(ui.pads[0].radius < before);
        assert!(!ui.pads[0].radius.is_nan());
        touch_up_inner_abs(&mut ui, (px, py + 4.0), VIEWPORT);
    }

    #[test]
    fn a_pad_cannot_be_resized_over_the_other_or_off_screen() {
        let mut ui = pads_ui();
        // Make the circle huge from its own handle.
        let layout = compute_layout(&ui, VIEWPORT);
        let (px, py, radius) = layout.pads[0];
        let handle = pad_handle_rect(&ui.pads[0], px, py, radius);
        let grab = (handle.x + handle.w / 2.0, handle.y + handle.h / 2.0);
        touch_down_inner_abs(&mut ui, grab, VIEWPORT);
        touch_motion_inner_abs(&mut ui, (480.0, 320.0), VIEWPORT);
        touch_up_inner_abs(&mut ui, (480.0, 320.0), VIEWPORT);
        let circle = pad_of(&ui, PadKind::Circle);
        assert!(circle.radius < PAD_DEFAULT_RADIUS * 3.0, "radius is clamped");
        // Whatever happened, both pads are on screen.
        let layout = compute_layout(&ui, VIEWPORT);
        for (px, py, radius) in layout.pads {
            assert!(px - radius >= -1.0 && px + radius <= 481.0);
            assert!(py - radius >= -1.0 && py + radius <= 321.0);
        }
    }

    #[test]
    fn dragging_a_pad_moves_it() {
        let mut ui = pads_ui();
        let layout = compute_layout(&ui, VIEWPORT);
        let (px, py, _) = layout.pads[1];
        assert!(touch_down_inner_abs(&mut ui, (px, py), VIEWPORT));
        assert_eq!(ui.pending, W_PAD_BASE + 1);
        touch_motion_inner_abs(&mut ui, (px - 60.0, py - 40.0), VIEWPORT);
        let moved = pad_of(&ui, PadKind::Cross);
        assert!(moved.cx * 480.0 < px);
        assert!(moved.cy * 320.0 < py);
        // A press that does not move is just a tap: the pad stays put.
        let before = pad_of(&ui, PadKind::Cross);
        assert!(touch_up_inner_abs(&mut ui, (px - 60.0, py - 40.0), VIEWPORT));
        assert_eq!(pad_of(&ui, PadKind::Cross).cx, before.cx);
    }

    #[test]
    fn a_bound_direction_leaves_its_own_pad_alone() {
        // A marker on the left stick takes the circle, and the cross is
        // untouched because it is not on that stick any more.
        let mut ui = pads_ui();
        let mut marker = Marker::new(0, 0.5, 0.5);
        marker.button = Some(Button::LeftStickUp);
        ui.markers.push(marker);
        let changes = push_sticks(&mut ui, [0.0, -1.0, 0.0, 0.0]);
        assert!(
            changes.is_empty(),
            "the circle's stick belongs to the marker: {:?}",
            changes
        );
        assert!(!pad_of(&ui, PadKind::Circle).touching);
        assert_eq!(pad_of(&ui, PadKind::Cross).held, None);

        // A marker on a D-pad button takes the cross instead, and the circle
        // keeps working.
        let mut ui = pads_ui();
        let mut marker = Marker::new(0, 0.5, 0.5);
        marker.button = Some(Button::DPadRight);
        ui.markers.push(marker);
        let changes = press_dpad(&mut ui, &[Button::DPadRight]);
        assert!(
            !changes.iter().any(|change| change.marker == PAD_FINGER_BASE + 1),
            "the D-pad belongs to the marker: {:?}",
            changes
        );
        assert!(!pad_of(&ui, PadKind::Cross).touching);
        let changes = push_sticks(&mut ui, [0.0, -1.0, 0.0, 0.0]);
        assert!(!changes.is_empty(), "the circle still walks");
    }

    #[test]
    fn a_saved_triangle_entry_loads_as_the_cross_pad() {
        // Files written before the pad was renamed still have their cross
        // pad's position and size.
        let parsed = parse_pads("triangle,0.700,0.600,0.200,on");
        let (pads, _, _) = parsed.expect("the entry parses");
        assert!(pads[1].enabled);
        assert!((pads[1].cx - 0.700).abs() < 0.001);
        assert!((pads[1].radius - 0.200).abs() < 0.001);
    }

    #[test]
    fn pads_are_saved_and_loaded_with_the_app() {
        let dir = std::env::temp_dir().join(format!("hyperhle-pads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = bindings_path_in(&dir);
        let mut ui = pads_ui();
        ui.app_id = "com.test.pads".to_string();
        ui.pads[0].radius = 0.22;
        ui.pads[1].cx = 0.70;
        ui.markers.push(Marker::new(0, 0.5, 0.4));
        save_to(&ui, &path);
        let (markers, pads) = load_entry_in(&path, "com.test.pads");
        let (pads, look, hidden) = pads.expect("the file has the pads section");
        assert!(!look);
        assert!(!hidden);
        assert_eq!(markers.len(), 1);
        assert!((pads[0].radius - 0.22).abs() < 0.001);
        assert!((pads[1].cx - 0.70).abs() < 0.001);
        // A file from before the pads still loads its markers.
        std::fs::write(&path, "com.old.app: 0.500,0.500,A\n").unwrap();
        let (markers, pads) = load_entry_in(&path, "com.old.app");
        assert_eq!(markers.len(), 1);
        assert!(pads.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn camera_mode_drags_a_finger_across_the_screen() {
        let mut ui = pads_ui();
        ui.look = true;
        // Pushing the right stick right starts a drag in the middle.
        let changes = push_sticks_dt(&mut ui, [0.0, 0.0, 1.0, 0.0], 0.0);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Down);
        assert_eq!(changes[0].marker, LOOK_FINGER);
        let start = changes[0].position;
        assert!((start.0 - LOOK_CENTRE.0).abs() < 1e-6);
        // Holding it drags the finger right, and further each tick.
        let changes = push_sticks_dt(&mut ui, [0.0, 0.0, 1.0, 0.0], 0.05);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Move);
        assert!(changes[0].position.0 > start.0);
        // Letting go lifts it.
        let changes = push_sticks_dt(&mut ui, [0.0, 0.0, 0.0, 0.0], 0.05);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Up);
    }

    #[test]
    fn a_long_turn_re_grabs_from_the_middle_instead_of_hitting_the_edge() {
        let mut ui = pads_ui();
        ui.look = true;
        push_sticks_dt(&mut ui, [0.0, 0.0, 1.0, 0.0], 0.0);
        let mut ups = 0;
        let mut downs = 0;
        for _ in 0..200 {
            for change in push_sticks_dt(&mut ui, [0.0, 0.0, 1.0, 0.0], 0.05) {
                assert!(change.position.0 <= LOOK_BOX.0 + LOOK_BOX.2 + 1e-4);
                match change.action {
                    MarkerAction::Up => ups += 1,
                    MarkerAction::Down => downs += 1,
                    MarkerAction::Move => {}
                }
            }
        }
        // Several full drags' worth of turning happened, each re-grabbed.
        assert!(ups >= 2 && downs >= 2, "ups {ups}, downs {downs}");
    }

    #[test]
    fn the_right_stick_never_reaches_the_cross() {
        // With the camera on, the right stick belongs to the camera and the
        // cross is not listening to it in the first place, which is the fix.
        let mut ui = pads_ui();
        ui.look = true;
        let changes = push_sticks_dt(&mut ui, [0.0, 0.0, 0.0, 1.0], 0.0);
        assert!(changes.iter().all(|change| change.marker == LOOK_FINGER));
        assert!(!pad_of(&ui, PadKind::Cross).touching);
        // The circle is unaffected: walking while looking around works.
        assert!(!pad_of(&ui, PadKind::Circle).touching);

        // Even with the camera off, the right stick is nobody's. It used to
        // drive the cross, and that is what made the camera fight it.
        let mut ui = pads_ui();
        let changes = push_sticks_dt(&mut ui, [0.0, 0.0, 0.0, 1.0], 0.0);
        assert!(changes.is_empty(), "the right stick drives no pad: {changes:?}");
        assert!(!pad_of(&ui, PadKind::Cross).touching);
    }

    #[test]
    fn turning_the_camera_off_lifts_the_finger() {
        let mut ui = pads_ui();
        ui.look = true;
        push_sticks_dt(&mut ui, [0.0, 0.0, 1.0, 0.0], 0.0);
        assert!(ui.look_touching);
        // Switch it off the way the menu row does.
        let mut row_ui = pads_ui();
        row_ui.look = true;
        row_ui.look_touching = true;
        row_ui.menu_open = true;
        let layout = compute_layout(&row_ui, VIEWPORT);
        let menu = layout.menu.unwrap();
        let (_, rect) = menu.rows.iter().find(|(id, _)| *id == W_LOOK).unwrap();
        let at = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        assert!(touch_down_inner_abs(&mut row_ui, at, VIEWPORT));
        assert!(touch_up_inner_abs(&mut row_ui, at, VIEWPORT));
        assert!(!row_ui.look);
        assert!(!row_ui.look_touching);
        let up = take_pending_change().expect("the camera finger was lifted");
        assert_eq!(up.action, MarkerAction::Up);
        assert_eq!(up.marker, LOOK_FINGER);
    }

    #[test]
    fn camera_mode_is_saved_with_the_app() {
        let dir = std::env::temp_dir().join(format!("hyperhle-look-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = bindings_path_in(&dir);
        let mut ui = pads_ui();
        ui.app_id = "com.test.look".to_string();
        ui.look = true;
        save_to(&ui, &path);
        let (_, pads) = load_entry_in(&path, "com.test.look");
        let (_, look, _) = pads.expect("the file has an entry");
        assert!(look);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Every control the menu is supposed to offer has to be in the layout,
    /// or it is unreachable: the row array and the touch handlers drifted
    /// apart once already.
    #[test]
    fn the_menu_offers_every_control() {
        let mut ui = Ui::new();
        ui.enabled = true;
        ui.menu_open = true;
        let layout = compute_layout(&ui, VIEWPORT);
        let menu = layout.menu.expect("the menu is open");
        let ids: Vec<u16> = menu.rows.iter().map(|(id, _)| *id).collect();
        for expected in [
            W_ADD,
            W_ADD_CIRCLE,
            W_REACH_CIRCLE,
            W_ADD_CROSS,
            W_REACH_CROSS,
            W_LOOK,
            W_HIDE,
            W_CLEAR,
            W_CLOSE,
        ] {
            assert!(ids.contains(&expected), "the menu is missing {expected}");
        }
        // And each one is reachable from a tap in its middle.
        for (id, rect) in &menu.rows {
            let at = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
            ui.pending = W_NONE;
            assert!(touch_down_inner_abs(&mut ui, at, VIEWPORT), "row {id} is tappable");
            assert_eq!(ui.pending, *id, "the tap lands on row {id}");
            touch_up_inner_abs(&mut ui, at, VIEWPORT);
            ui.menu_open = true;
        }
    }

    #[test]
    fn pads_are_added_from_the_menu_like_markers() {
        let mut ui = Ui::new();
        ui.enabled = true;
        // A fresh app has no pads on screen at all.
        assert!(!ui.pads[0].enabled && !ui.pads[1].enabled);
        let layout = compute_layout(&ui, VIEWPORT);
        assert_eq!(layout.pads[0].2, 0.0);
        assert_eq!(layout.pads[1].2, 0.0);
        // A tap on where a pad would be goes to the game, not to a pad.
        assert!(!touch_down_inner_abs(&mut ui, (96.0, 224.0), VIEWPORT));
        // The menu adds them.
        ui.menu_open = true;
        let layout = compute_layout(&ui, VIEWPORT);
        let menu = layout.menu.unwrap();
        let row = |id: u16| {
            let (_, rect) = menu.rows.iter().find(|(row_id, _)| *row_id == id).unwrap();
            (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
        };
        let at = row(W_ADD_CIRCLE);
        tap(&mut ui, at, VIEWPORT);
        assert!(ui.pads[0].enabled && !ui.pads[1].enabled);
        assert!(compute_layout(&ui, VIEWPORT).pads[0].2 > 0.0);
        // And takes them away again.
        ui.menu_open = true;
        let layout = compute_layout(&ui, VIEWPORT);
        let menu = layout.menu.unwrap();
        let at = {
            let (_, rect) = menu.rows.iter().find(|(row_id, _)| *row_id == W_ADD_CIRCLE).unwrap();
            (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
        };
        tap(&mut ui, at, VIEWPORT);
        assert!(!ui.pads[0].enabled);
    }

    #[test]
    fn removing_a_pad_lifts_the_fingers_it_was_holding() {
        let mut ui = pads_ui();
        push_sticks(&mut ui, [-1.0, 1.0, 0.0, 0.0]);
        assert!(pad_of(&ui, PadKind::Circle).touching);
        toggle_pad(&mut ui, 0);
        assert!(!ui.pads[0].enabled);
        let mut ups = 0;
        while let Some(change) = take_pending_change() {
            if change.action == MarkerAction::Up {
                ups += 1;
            }
        }
        assert_eq!(ups, 1, "the pad's one finger is released");
    }

    #[test]
    fn hidden_buttons_still_work() {
        let mut ui = pads_ui();
        ui.hidden = true;
        // A hidden pad's stick still sends its swipe...
        let changes = push_sticks(&mut ui, [-1.0, 0.0, 0.0, 0.0]);
        assert!(changes.iter().any(|change| change.action == MarkerAction::Down));
        // ...and its marker can still be tapped and dragged.
        let mut marker = Marker::new(0, 0.5, 0.5);
        marker.button = Some(Button::X);
        ui.markers.push(marker);
        assert!(touch_down_inner_abs(&mut ui, (240.0, 160.0), VIEWPORT));
        assert!(touch_up_inner_abs(&mut ui, (240.0, 160.0), VIEWPORT));
        assert_eq!(ui.bind_target, Some(0));
    }

    #[test]
    fn the_hide_row_toggles_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("hyperhle-hide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = bindings_path_in(&dir);
        let mut ui = pads_ui();
        ui.app_id = "com.test.hide".to_string();
        ui.menu_open = true;
        let layout = compute_layout(&ui, VIEWPORT);
        let menu = layout.menu.unwrap();
        let (_, rect) = menu.rows.iter().find(|(id, _)| *id == W_HIDE).unwrap();
        let at = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        tap(&mut ui, at, VIEWPORT);
        assert!(ui.hidden);
        assert!(!ui.menu_open);
        save_to(&ui, &path);
        let (_, pads) = load_entry_in(&path, "com.test.hide");
        let (_, _, hidden) = pads.expect("the app has state");
        assert!(hidden);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_fresh_app_has_no_pads_and_says_so_in_the_file() {
        let dir = std::env::temp_dir().join(format!("hyperhle-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = bindings_path_in(&dir);
        let mut ui = Ui::new();
        ui.app_id = "com.test.fresh".to_string();
        save_to(&ui, &path);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("com.test.fresh"), "nothing changed, no line");
        ui.pads[0].enabled = true;
        save_to(&ui, &path);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("circle,") && saved.contains(",on"));
        std::fs::remove_dir_all(&dir).unwrap();
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

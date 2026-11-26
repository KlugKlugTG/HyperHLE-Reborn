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
const PAD_DEFAULT_CENTERS: [(f32, f32); PAD_COUNT] = [(0.18, 0.68), (0.82, 0.68)];
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

// Camera ("look") mode.
/// How fast a fully pushed right stick drags the look finger, in fractions of
/// the viewport per second.
const LOOK_SPEED: f32 = 0.55;
/// The look finger never leaves this inset box, so a drag can always continue
/// in every direction: when it reaches an edge it is lifted and put back in
/// the middle, the way a player re-grabs a screen.
const LOOK_MARGIN: f32 = 0.12;
/// Where the look finger starts.
const LOOK_CENTRE: (f32, f32) = (0.5, 0.5);
/// A look drag only counts while the stick is past this.
const LOOK_DEADZONE: f32 = 0.15;
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

/// One of the two movement pads: a circle or a triangle whose four
/// directions are driven by a stick, and which sends a touch to the game.
///
/// A pad is not a marker: markers inject a finger at a fixed spot, while a
/// pad is a virtual analogue stick. The stick's position selects one of the
/// four directions, and the pad's own knob is drawn under the thumb. The touch
/// it sends starts at `point(Direction)` — the direction's arrow, on the rim —
/// and is dragged towards `centre()` in steps, which is the gesture that
/// most mobile games use for walking.
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
    /// How far each of the four directions is pushed, after [PAD_ACTIVATION].
    dir: [f32; 4],
    /// Whether a finger is down on each direction's touch right now.
    down: [bool; 4],
}

impl Pad {
    const fn new(kind: PadKind, cx: f32, cy: f32) -> Self {
        Self {
            kind,
            cx,
            cy,
            radius: PAD_DEFAULT_RADIUS,
            dir: [0.0; 4],
            down: [false; 4],
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
}

/// The two pads. A pad is driven by one stick only, so the left pad (circle)
/// cannot be moved by the right stick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PadKind {
    /// Left stick, drawn as a ring.
    Circle,
    /// Right stick, drawn as a triangle.
    Triangle,
}

impl PadKind {
    fn pad(self) -> usize {
        match self {
            PadKind::Circle => 0,
            PadKind::Triangle => 1,
        }
    }

    fn stick(self) -> usize {
        match self {
            PadKind::Circle => 0,
            PadKind::Triangle => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            PadKind::Circle => "CIRCLE",
            PadKind::Triangle => "TRIANGLE",
        }
    }
}

/// Up, down, left, right: the order of [Pad::dir], [Pad::down] and
/// [Pad::point], and of the button each direction maps to.
const PAD_DIRECTIONS: [(f32, f32); 4] = [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)];
/// Direction buttons, in the same order: they are what the stick edges feed.
const PAD_DIRECTION_BUTTONS: [[Button; 4]; 2] = [
    [
        Button::LeftStickUp,
        Button::LeftStickDown,
        Button::LeftStickLeft,
        Button::LeftStickRight,
    ],
    [
        Button::RightStickUp,
        Button::RightStickDown,
        Button::RightStickLeft,
        Button::RightStickRight,
    ],
];
/// How far out the direction arrows sit, as a fraction of the pad radius.
const PAD_RIM: f32 = 0.78;

struct Ui {
    enabled: bool,
    menu_open: bool,
    markers: Vec<Marker>,
    /// The circle (left stick) and triangle (right stick) movement pads.
    pads: [Pad; PAD_COUNT],
    /// Where the press that is resizing a pad, or dragging it, started.
    pad_press: (f32, f32),
    pad_press_moved: bool,
    /// Camera mode: the right stick drags a finger across the middle of the
    /// screen instead of driving the triangle pad. This is the look-around
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
                    PadKind::Triangle,
                    PAD_DEFAULT_CENTERS[1].0,
                    PAD_DEFAULT_CENTERS[1].1,
                ),
            ],
            pad_press: (0.0, 0.0),
            pad_press_moved: false,
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
fn stick_directions(sticks: [f32; 4], stick: usize) -> [f32; 4] {
    let x = sticks[stick * 2];
    let y = sticks[stick * 2 + 1];
    // Below the threshold the direction is idle; above it the push rises
    // from 0 to 1, so a wobbly thumb can never make the character creep.
    let amount = |value: f32, positive: bool| {
        let amount = if positive { value } else { -value };
        ((amount - PAD_ACTIVATION) / (1.0 - PAD_ACTIVATION)).clamp(0.0, 1.0)
    };
    [
        amount(y, false),
        amount(y, true),
        amount(x, false),
        amount(x, true),
    ]
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
/// [PAD_ACTIVATION] starts a touch at that direction's arrow on the rim; the
/// finger then slides towards the centre as the stick goes further, which is
/// the swipe most mobile games use for walking. Directions are independent,
/// so a diagonal presses two of them at once.
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
    for kind in [PadKind::Circle, PadKind::Triangle] {
        let index = kind.pad();
        // The right stick drives the camera when look mode is on, so the
        // triangle below it would be fighting the camera for the same stick.
        if index == 1 && ui.look {
            // The triangle still shows where the stick is, but sends nothing.
            ui.pads[index].dir = stick_directions(sticks, kind.stick());
            continue;
        }
        // A stick either walks or presses markers, not both: if the player
        // bound one of this stick's directions to a marker, the stick is
        // theirs and the pad leaves it alone.
        let taken = ui.markers.iter().any(|marker| {
            marker
                .button
                .is_some_and(|button| PAD_DIRECTION_BUTTONS[index].contains(&button))
        });
        if taken {
            // Keep the knob and arrows showing where the stick is, but send
            // nothing: the stick belongs to the markers.
            ui.pads[index].dir = stick_directions(sticks, kind.stick());
            continue;
        }
        let dir = stick_directions(sticks, kind.stick());
        apply_pad_directions(&mut ui.pads[index], dir, &mut changes);
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
    let (lo, hi) = (LOOK_MARGIN, 1.0 - LOOK_MARGIN);
    if next.0 < lo || next.0 > hi || next.1 < lo || next.1 > hi {
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
fn apply_pad_directions(pad: &mut Pad, dir: [f32; 4], changes: &mut Vec<MarkerTouch>) {
    for direction in 0..4 {
        let was = pad.dir[direction];
        let now = dir[direction];
        let pushed = now > 0.0;
        match (pushed, pad.down[direction]) {
            (true, false) => {
                pad.dir[direction] = now;
                pad.down[direction] = true;
                changes.push(pad_touch(pad, direction, PadAction::Down));
            }
            (true, true) => {
                pad.dir[direction] = now;
                // Only tell the game when the finger actually moved: a stick
                // held still would otherwise queue a Move on every poll.
                if (now - was).abs() > MOVE_EPSILON {
                    changes.push(pad_touch(pad, direction, PadAction::Move));
                }
            }
            (false, true) => {
                pad.dir[direction] = 0.0;
                pad.down[direction] = false;
                changes.push(pad_touch(pad, direction, PadAction::Up));
            }
            (false, false) => {
                pad.dir[direction] = 0.0;
            }
        }
    }
}

/// The touch for one direction of a pad, at its current push.
///
/// The finger starts on the arrow on the rim and walks to the centre of the
/// pad, so the gesture is a short swipe in the direction of movement. It keeps
/// its finger id (`FingerId::PadArm`) across the whole push.
fn pad_touch(pad: &Pad, direction: usize, action: PadAction) -> MarkerTouch {
    let centre = (pad.cx, pad.cy);
    let along = pad.dir[direction].clamp(0.0, 1.0);
    let position = match action {
        // A releasing finger ends up at the centre, like a completed swipe.
        PadAction::Up => centre,
        PadAction::Down | PadAction::Move => lerp2(pad.point(direction), centre, along),
    };
    MarkerTouch {
        marker: PAD_FINGER_BASE + (pad.kind.pad() * 4 + direction) as u8,
        position,
        action: match action {
            PadAction::Down => MarkerAction::Down,
            PadAction::Move => MarkerAction::Move,
            PadAction::Up => MarkerAction::Up,
        },
    }
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
            PadKind::Triangle,
            PAD_DEFAULT_CENTERS[1].0,
            PAD_DEFAULT_CENTERS[1].1,
        ),
    ];
    ui.pad_press_moved = false;
    ui.look = false;
    ui.look_touching = false;
    ui.look_at = LOOK_CENTRE;
    if let Some(app_id) = app_id {
        ui.app_id.push_str(app_id);
        if enabled {
            let (markers, pads) = load_entry_in(&bindings_path(), app_id);
            ui.markers = markers;
            if let Some((pads, look)) = pads {
                ui.pads = pads;
                ui.look = look;
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

/// `circle,x,y,radius;triangle,...`, the pads half of an app's line.
fn format_pads(pads: &[Pad; PAD_COUNT], look: bool) -> String {
    let mut text = pads
        .iter()
        .map(|pad| {
            format!(
                "{},{:.3},{:.3},{:.3}",
                match pad.kind {
                    PadKind::Circle => "circle",
                    PadKind::Triangle => "triangle",
                },
                pad.cx,
                pad.cy,
                pad.radius
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    if look {
        text.push_str(";look");
    }
    text
}

/// The inverse of [`format_pads`]. A missing or malformed pad keeps its
/// default, so files written before the pads existed still load.
fn parse_pads(text: &str) -> Option<([Pad; PAD_COUNT], bool)> {
    let mut pads = [
        Pad::new(PadKind::Circle, PAD_DEFAULT_CENTERS[0].0, PAD_DEFAULT_CENTERS[0].1),
        Pad::new(
            PadKind::Triangle,
            PAD_DEFAULT_CENTERS[1].0,
            PAD_DEFAULT_CENTERS[1].1,
        ),
    ];
    let mut found = false;
    let mut look = false;
    for entry in text.split(';') {
        if entry.trim() == "look" {
            look = true;
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
        let kind = match name.trim() {
            "circle" => PadKind::Circle,
            "triangle" => PadKind::Triangle,
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
        found = true;
    }
    found.then_some((pads, look))
}

/// Everything saved for an app: its markers (empty for a file written before
/// markers existed) and its pads (`None` for a file written before pads).
fn load_entry_in(
    path: &std::path::Path,
    app_id: &str,
) -> (Vec<Marker>, Option<([Pad; PAD_COUNT], bool)>) {
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
        pad.cx != PAD_DEFAULT_CENTERS[pad.kind.pad()].0
            || pad.cy != PAD_DEFAULT_CENTERS[pad.kind.pad()].1
            || pad.radius != PAD_DEFAULT_RADIUS
    });
    if !ui.markers.is_empty() || pads_changed || ui.look {
        contents.push_str(&ui.app_id);
        contents.push_str(": ");
        contents.push_str(&format_markers(&ui.markers));
        contents.push_str(" | ");
        contents.push_str(&format_pads(&ui.pads, ui.look));
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
    let menu = if ui.menu_open {
        let width = (vw - 12.0 * s).clamp(80.0, (240.0 * s).max(80.0));
        let x = (vw - width) / 2.0;
        let y = button.y + button.h + 6.0 * s;
        let header_h = 22.0 * s;
        let row_h = 28.0 * s;
        let hint_h = 37.0 * s;
        let height = header_h + row_h * 4.0 + hint_h + 6.0 * s;
        let ids = [W_ADD, W_LOOK, W_CLEAR, W_CLOSE];
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
            y: y + header_h + 4.0 * row_h + 2.0 * s,
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
            "BIND BUTTONS",
            menu.rect.x + 8.0 * s,
            menu.rect.y + 6.0 * s,
            11.0 * s,
            COL_TEXT_DIM,
        );
        for (id, row) in &menu.rows {
            let (label, color) = match *id {
                W_ADD => ("ADD BUTTON", COL_ACCENT),
                W_LOOK => (if ui.look { "CAMERA: ON" } else { "CAMERA: OFF" }, COL_ROW),
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

    // The pads: a ring for the circle, a triangle for the triangle, with the
    // knob under the thumb and the four arrows lit as the stick pushes.
    for (index, (px, py, radius)) in layout.pads.iter().enumerate() {
        let pad = &ui.pads[index];
        draw_pad(gles, &mut scene, pad, *px, *py, *radius);
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
        // `s` shrinks when a long label would not fit the marker, so "L1" and
        // "<LS" both stay inside it at any screen size.
        let marker_scale = (rect.w / MARKER_SIZE).clamp(0.4, 3.0);
        let label_size = (11.0 * marker_scale)
            .min(rect.w / (label.chars().count() as f32 * 0.62).max(1.0));
        let label_size = label_size.max(1.0);
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

/// Draws one pad: its outline, the four direction labels and the knob.
///
/// The scene only has axis-aligned rectangles and text, so the outline is a
/// dotted one: a ring of small squares for the circle, and squares along the
/// three sides for the triangle. That reads the same as the faint outlines
/// other on-screen-pad overlays draw, without needing a mesh.
unsafe fn draw_pad(
    gles: &mut dyn GLES,
    scene: &mut crate::trainer_ui::OverlayScene,
    pad: &Pad,
    px: f32,
    py: f32,
    radius: f32,
) {
    let dot = (radius * 0.055).max(1.5);
    let mut dot_at = |x: f32, y: f32| {
        scene.rect(x - dot / 2.0, y - dot / 2.0, dot, dot, COL_PAD);
    };

    match pad.kind {
        PadKind::Circle => {
            const DOTS: usize = 44;
            for index in 0..DOTS {
                let angle = index as f32 * std::f32::consts::TAU / DOTS as f32;
                let (sin, cos) = angle.sin_cos();
                dot_at(px + cos * radius, py + sin * radius);
            }
        }
        PadKind::Triangle => {
            // Every side is split into the same number of dots, so the
            // spacing stays even around the whole outline.
            const PER_SIDE: usize = 15;
            for side in 0..3 {
                let a0 = -std::f32::consts::FRAC_PI_2 + side as f32 * std::f32::consts::TAU / 3.0;
                let a1 = a0 + std::f32::consts::TAU / 3.0;
                let p0 = (px + a0.cos() * radius, py + a0.sin() * radius);
                let p1 = (px + a1.cos() * radius, py + a1.sin() * radius);
                for step in 0..PER_SIDE {
                    let t = step as f32 / PER_SIDE as f32;
                    dot_at(p0.0 + (p1.0 - p0.0) * t, p0.1 + (p1.1 - p0.1) * t);
                }
            }
        }
    }

    // The four directions, on the rim, lit while the stick pushes them.
    let label_size = (radius * 0.26).clamp(6.0, 30.0);
    for (direction, text) in ["^", "v", "<", ">"].iter().enumerate() {
        let (dx, dy) = PAD_DIRECTIONS[direction];
        let (ax, ay) = (px + dx * radius * PAD_RIM, py + dy * radius * PAD_RIM);
        let pushed = pad.dir[direction].clamp(0.0, 1.0);
        let color = if pad.down[direction] {
            COL_PAD_ARROW
        } else {
            dim(COL_PAD_ARROW_IDLE, 1.0 - 0.35 * pushed)
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
        COL_PAD_KNOB_BORDER,
    );
    scene.rect(
        kx - knob / 2.0 + border,
        ky - knob / 2.0 + border,
        knob - 2.0 * border,
        knob - 2.0 * border,
        COL_PAD_KNOB,
    );
    let handle = pad_handle_rect(pad, px, py, radius);
    scene.rect(handle.x, handle.y, handle.w, handle.h, COL_PAD_KNOB_BORDER);
    // Two dots in the corner read as a "grab here and pull" grip.
    let dot = ((handle.w * 0.18).max(1.5)).min(handle.w / 3.0);
    let step = (handle.w - 2.0 * dot) / 3.0;
    for i in 0..2 {
        let at = dot + step * i as f32;
        scene.rect(handle.x + at, handle.y + at, dot, dot, COL_PAD_KNOB);
    }
}

/// The knob's centre in viewport pixels: the stick's push, limited to the
/// pad's travel.
fn knob_position(pad: &Pad, px: f32, py: f32, radius: f32) -> (f32, f32) {
    let right = pad.dir[3] - pad.dir[2];
    let down = pad.dir[1] - pad.dir[0];
    let (x, y) = normalize_push(right, down);
    (
        px + x * radius * KNOB_TRAVEL,
        py + y * radius * KNOB_TRAVEL,
    )
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

    fn pads_ui() -> Ui {
        let mut ui = Ui::new();
        ui.enabled = true;
        ui
    }

    fn pad_of(ui: &Ui, kind: PadKind) -> Pad {
        ui.pads[kind.pad()]
    }

    /// Runs a stick sample through the pads and keeps the changes.
    fn push_sticks(ui: &mut Ui, sticks: [f32; 4]) -> Vec<MarkerTouch> {
        push_sticks_dt(ui, sticks, 0.0)
    }

    fn push_sticks_dt(ui: &mut Ui, sticks: [f32; 4], dt: f32) -> Vec<MarkerTouch> {
        pads_move_inner(ui, sticks, dt)
    }

    #[test]
    fn the_pads_start_bottom_left_and_bottom_right() {
        let ui = Ui::new();
        let circle = pad_of(&ui, PadKind::Circle);
        let triangle = pad_of(&ui, PadKind::Triangle);
        assert!(circle.cx < 0.5 && circle.cy > 0.5);
        assert!(triangle.cx > 0.5 && triangle.cy > 0.5);
        assert_eq!(circle.kind, PadKind::Circle);
        assert_eq!(triangle.kind, PadKind::Triangle);
    }

    #[test]
    fn each_pad_only_answers_its_own_stick() {
        let mut ui = pads_ui();
        // Push only the right stick.
        let changes = push_sticks(&mut ui, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Down);
        // The triangle's right arrow, not the circle's.
        let pad = pad_of(&ui, PadKind::Triangle);
        assert!(pad.dir[3] > 0.0);
        assert_eq!(pad_of(&ui, PadKind::Circle).dir, [0.0; 4]);
        assert_eq!(changes[0].marker, PAD_FINGER_BASE + 4 + 3);
    }

    #[test]
    fn a_diagonal_presses_two_directions_at_once() {
        let mut ui = pads_ui();
        // Down-left on the left stick: the shape a player walks on.
        let changes = push_sticks(&mut ui, [-1.0, 1.0, 0.0, 0.0]);
        let downs: Vec<_> = changes
            .iter()
            .map(|change| change.marker - PAD_FINGER_BASE)
            .collect();
        assert_eq!(downs, vec![1, 2]);
        let circle = pad_of(&ui, PadKind::Circle);
        assert!(circle.down[1] && circle.down[2]);
        assert!(!circle.down[0] && !circle.down[3]);
        // Holding it still sends nothing new...
        assert!(push_sticks(&mut ui, [-1.0, 1.0, 0.0, 0.0]).is_empty());
        // ...and centring releases exactly the two that were down.
        let ups = push_sticks(&mut ui, [0.0, 0.0, 0.0, 0.0]);
        let ups: Vec<_> = ups
            .iter()
            .map(|change| (change.marker - PAD_FINGER_BASE, change.action))
            .collect();
        assert_eq!(
            ups,
            vec![(1, MarkerAction::Up), (2, MarkerAction::Up)]
        );
    }

    #[test]
    fn the_pads_do_not_creep_on_a_wobbly_thumb() {
        let mut ui = pads_ui();
        // Inside the activation threshold: nothing down.
        let deadzone_amount = PAD_ACTIVATION * 0.5;
        assert!(push_sticks(&mut ui, [deadzone_amount, 0.0, 0.0, 0.0]).is_empty());
        assert_eq!(pad_of(&ui, PadKind::Circle).dir, [0.0; 4]);
    }

    #[test]
    fn a_touch_starts_on_the_rim_and_walks_to_the_centre() {
        let mut ui = pads_ui();
        let circle = pad_of(&ui, PadKind::Circle);
        let rim = circle.point(3);
        let changes = push_sticks(&mut ui, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(changes[0].action, MarkerAction::Down);
        // Fully pushed: the finger is at the centre, i.e. a full swipe.
        assert!((changes[0].position.0 - circle.cx).abs() < 1e-6);
        assert!((changes[0].position.1 - circle.cy).abs() < 1e-6);
        // A partial push walks part of the way from the rim to the centre,
        // and further push walks further.
        let mut ui = pads_ui();
        let half = PAD_ACTIVATION + (1.0 - PAD_ACTIVATION) * 0.5;
        let changes = push_sticks(&mut ui, [half, 0.0, 0.0, 0.0]);
        let expected = (
            (rim.0 + circle.cx) / 2.0,
            (rim.1 + circle.cy) / 2.0,
        );
        assert!((changes[0].position.0 - expected.0).abs() < 1e-5);
        assert!((changes[0].position.1 - expected.1).abs() < 1e-5);
        // Just past the threshold it is still near the rim, so the swipe is
        // short for a light push.
        let mut ui = pads_ui();
        let light = PAD_ACTIVATION + (1.0 - PAD_ACTIVATION) * 0.1;
        let changes = push_sticks(&mut ui, [light, 0.0, 0.0, 0.0]);
        let from_rim = (changes[0].position.0 - rim.0).abs();
        let to_centre = (circle.cx - rim.0).abs();
        assert!(from_rim < to_centre * 0.2);
    }

    #[test]
    fn a_partial_push_sends_a_move_as_the_stick_goes_further() {
        let mut ui = pads_ui();
        push_sticks(&mut ui, [PAD_ACTIVATION * 1.2, 0.0, 0.0, 0.0]);
        let first = pad_of(&ui, PadKind::Circle).dir[3];
        let changes = push_sticks(&mut ui, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].action, MarkerAction::Move);
        assert!(pad_of(&ui, PadKind::Circle).dir[3] > first);
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
        let moved = pad_of(&ui, PadKind::Triangle);
        assert!(moved.cx * 480.0 < px);
        assert!(moved.cy * 320.0 < py);
        // A press that does not move is just a tap: the pad stays put.
        let before = pad_of(&ui, PadKind::Triangle);
        assert!(touch_up_inner_abs(&mut ui, (px - 60.0, py - 40.0), VIEWPORT));
        assert_eq!(pad_of(&ui, PadKind::Triangle).cx, before.cx);
    }

    #[test]
    fn a_stick_with_a_bound_direction_leaves_its_pad_alone() {
        let mut ui = pads_ui();
        // Bind LeftStickUp to a marker, as the button-binding flow would.
        let mut marker = Marker::new(0, 0.5, 0.5);
        marker.button = Some(Button::LeftStickUp);
        ui.markers.push(marker);
        let changes = push_sticks(&mut ui, [0.0, -1.0, 1.0, 0.0]);
        // Only the triangle answered.
        assert!(changes.iter().all(|change| change.marker >= PAD_FINGER_BASE + 4));
        // The circle still shows the stick, but has no finger down.
        let circle = pad_of(&ui, PadKind::Circle);
        assert!(circle.dir[0] > 0.0);
        assert!(circle.down.iter().all(|down| !down));
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
        let (pads, look) = pads.expect("the file has the pads section");
        assert!(!look);
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
                assert!(change.position.0 <= 1.0 - LOOK_MARGIN + 1e-4);
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
    fn camera_mode_takes_the_right_stick_from_the_triangle() {
        let mut ui = pads_ui();
        ui.look = true;
        let changes = push_sticks_dt(&mut ui, [0.0, 0.0, 0.0, 1.0], 0.0);
        assert!(changes.iter().all(|change| change.marker == LOOK_FINGER));
        let triangle = pad_of(&ui, PadKind::Triangle);
        assert!(triangle.down.iter().all(|down| !down));
        // The circle is unaffected: walking while looking around works.
        assert!(!pad_of(&ui, PadKind::Circle).down[0]);
        let mut ui = pads_ui();
        ui.look = true;
        let changes = push_sticks_dt(&mut ui, [0.0, -1.0, 0.0, 0.0], 0.0);
        assert!(changes.iter().all(|change| change.marker < PAD_FINGER_BASE + 4));
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
        let (_, look) = pads.expect("the file has an entry");
        assert!(look);
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

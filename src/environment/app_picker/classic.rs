/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! The classic app-picker look from older builds: a black home screen with
//! the brand watermark, white rounded shortcut buttons and a compact
//! "quick options" page with the most-used emulation switches.
//!
//! It is a separate layout rather than a restyle of the current one, so new
//! picker features keep working: the icon grid underneath is the live one,
//! and the shortcut buttons call the same delegate actions as the dock.

use super::{new_label, new_view, rect};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string;
use crate::frameworks::uikit::ui_font::UITextAlignmentCenter;
use crate::frameworks::uikit::ui_view::ui_control::ui_button::{
    UIButtonTypeCustom, UIButtonTypeRoundedRect,
};
use crate::frameworks::uikit::ui_view::ui_control::{
    UIControlEventTouchUpInside, UIControlEventValueChanged, UIControlStateNormal,
};
use crate::objc::{id, msg, msg_class};
use crate::Environment;

/// Vertical space reserved at the bottom of the classic home screen for the
/// shortcut buttons.
const SHORTCUT_AREA: CGFloat = 100.0;

/// Views that belong to the classic home screen and go away when the regular
/// layout comes back.
pub(super) struct ClassicHome {
    background: id,
    watermark: Vec<id>,
    buttons: Vec<id>,
    version: id,
}

impl ClassicHome {
    pub(super) fn remove(self, env: &mut Environment) {
        for view in std::iter::once(self.background)
            .chain(self.watermark)
            .chain(self.buttons)
            .chain(std::iter::once(self.version))
        {
            () = msg![env; view removeFromSuperview];
        }
    }
}

/// Values shown by the classic quick-options page. Snapshotted when the page
/// is built; the picker rebuilds it after a change.
#[derive(Clone, PartialEq)]
pub(super) struct QuickOptionsState {
    pub(super) scale_hack: Option<std::num::NonZeroU32>,
    pub(super) orientation: Option<crate::window::DeviceOrientation>,
    pub(super) network: bool,
    pub(super) analog_sticks: bool,
    pub(super) memory_limit_mb: Option<u32>,
    pub(super) show_fps: bool,
    pub(super) quick_menu: bool,
}

/// Builds the classic home screen: watermark, shortcut rows and version
/// caption. The background and watermark are inserted below the existing
/// subviews so the live icon grid stays on top of them.
pub(super) fn install_home(
    env: &mut Environment,
    delegate: id,
    parent: id,
    screen: CGSize,
    branding: &str,
) -> ClassicHome {
    let black: id = msg_class![env; UIColor blackColor];
    let background = new_view(env, rect(0.0, 0.0, screen.width, screen.height), black);
    () = msg![env; parent insertSubview:background atIndex:0];

    let brand_color: id = if branding == "UNOFFICIAL" {
        msg_class![env; UIColor redColor]
    } else {
        msg_class![env; UIColor grayColor]
    };
    let watermark_text = if branding.is_empty() {
        "HyperHLE"
    } else {
        branding
    };
    let mut watermark = Vec::new();
    for i in 1..=7 {
        let label = new_label(
            env,
            rect(
                0.0,
                (screen.height / 8.0) * (i as CGFloat) - 25.0,
                screen.width,
                50.0,
            ),
            watermark_text,
            48.0,
            false,
            brand_color,
        );
        () = msg![env; label setTextAlignment:(if i % 2 == 0 {
            crate::frameworks::uikit::ui_font::UITextAlignmentLeft
        } else {
            crate::frameworks::uikit::ui_font::UITextAlignmentRight
        })];
        // Index 0 is the background, so the watermark ends up under the grid.
        () = msg![env; parent insertSubview:label atIndex:(watermark.len() as u32 + 1)];
        watermark.push(label);
    }

    let divider = screen.height - SHORTCUT_AREA;
    let row_center = |fraction: CGFloat| divider + (screen.height - divider) / fraction;
    let mut buttons = Vec::new();
    for (row_y, titles) in [
        (
            row_center(4.0),
            vec![
                ("Add .ipa", "addIpa"),
                ("Quick options", "classicQuickOptionsShow"),
            ],
        ),
        (row_center(1.6), vec![("File manager", "addIpa")]),
    ] {
        buttons.extend(make_button_row(
            env,
            delegate,
            parent,
            screen,
            row_y,
            &titles,
        ));
    }

    let version_text = if branding.is_empty() {
        format!("HyperHLE {}", crate::VERSION)
    } else {
        format!("HyperHLE {} {}", crate::VERSION, branding)
    };
    let version: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    let version_frame = rect(
        screen.width - 190.0,
        screen.height - 22.0,
        185.0,
        18.0,
    );
    () = msg![env; version setFrame:version_frame];
    let text = ns_string::from_rust_string(env, version_text);
    () = msg![env; version setTitle:text forState:UIControlStateNormal];
    let clear: id = msg_class![env; UIColor clearColor];
    () = msg![env; version setBackgroundColor:clear];
    let gray: id = msg_class![env; UIColor lightGrayColor];
    () = msg![env; version setTitleColor:gray forState:UIControlStateNormal];
    let label: id = msg![env; version titleLabel];
    let font: id = msg_class![env; UIFont systemFontOfSize:(10.0 as CGFloat)];
    () = msg![env; label setFont:font];
    // UIControlContentHorizontalAlignmentRight.
    () = msg![env; version setContentHorizontalAlignment:2];
    let selector = env.objc.register_host_selector("classicExit".to_string(), &mut env.mem);
    () = msg![env; version addTarget:delegate
                        action:selector
              forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; parent addSubview:version];

    ClassicHome {
        background,
        watermark,
        buttons,
        version,
    }
}

/// One row of white rounded shortcut buttons, like the classic layout.
fn make_button_row(
    env: &mut Environment,
    delegate: id,
    parent: id,
    screen: CGSize,
    center_y: CGFloat,
    buttons: &[(&str, &str)],
) -> Vec<id> {
    let margin: CGFloat = 10.0;
    let height: CGFloat = 30.0;
    let width = (screen.width - margin) / (buttons.len() as CGFloat) - margin;
    let mut frame = CGRect {
        origin: CGPoint {
            x: margin,
            y: center_y - height / 2.0,
        },
        size: CGSize {
            width,
            height,
        },
    };
    let mut views = Vec::new();
    for (title, action) in buttons {
        let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeRoundedRect];
        let text = ns_string::from_rust_string(env, title.to_string());
        () = msg![env; button setTitle:text forState:UIControlStateNormal];
        () = msg![env; button setFrame:frame];
        () = msg![env; button layoutSubviews];
        let selector = env.objc.register_host_selector(action.to_string(), &mut env.mem);
        () = msg![env; button addTarget:delegate
                                action:selector
                      forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; parent addSubview:button];
        frame.origin.x += width + margin;
        views.push(button);
    }
    views
}

/// Builds the classic quick-options page: a label and a control row per
/// setting, stacked from the top like the old panel. The caller hides or
/// drops it when the close button is tapped; the picker rebuilds it after a
/// change so the highlighted choices stay in sync.
pub(super) fn make_quick_options(
    env: &mut Environment,
    delegate: id,
    parent: id,
    app_frame: CGRect,
    state: &QuickOptionsState,
) -> id {
    let width = app_frame.size.width;
    // The picker's main view starts below the status bar; extend the page over
    // it so the white background covers the whole screen.
    let top = -app_frame.origin.y;
    let height = app_frame.size.height + app_frame.origin.y;
    let white: id = msg_class![env; UIColor whiteColor];
    let main_view = new_view(env, rect(0.0, top, width, height), white);

    let close: id = msg_class![env; UIButton buttonWithType:UIButtonTypeRoundedRect];
    let close_size: CGFloat = 34.0;
    () = msg![env; close setFrame:(rect(width - close_size - 8.0, 8.0, close_size, close_size))];
    let text = ns_string::from_rust_string(env, "×".to_string());
    () = msg![env; close setTitle:text forState:UIControlStateNormal];
    let label: id = msg![env; close titleLabel];
    let font: id = msg_class![env; UIFont systemFontOfSize:(22.0 as CGFloat)];
    () = msg![env; label setFont:font];
    let selector = env.objc.register_host_selector(
        "classicQuickOptionsHide".to_string(),
        &mut env.mem,
    );
    () = msg![env; close addTarget:delegate
                           action:selector
                 forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; main_view addSubview:close];

    enum Row<'a> {
        LabelAndChoices(&'a str, &'a [(&'a str, &'a str, bool)]),
        LabelAndSwitch(&'a str, &'a str, bool),
    }
    let scale_hack_choices: &[(&str, &str, bool)] = &[
        ("Default", "scaleHackDefault", state.scale_hack.is_none()),
        ("Off", "scaleHack1", state.scale_hack == std::num::NonZeroU32::new(1)),
        ("2×", "scaleHack2", state.scale_hack == std::num::NonZeroU32::new(2)),
        ("3×", "scaleHack3", state.scale_hack == std::num::NonZeroU32::new(3)),
        ("4×", "scaleHack4", state.scale_hack == std::num::NonZeroU32::new(4)),
    ];
    let orientation_choices: &[(&str, &str, bool)] = &[
        ("Default", "orientationDefault", state.orientation.is_none()),
        (
            "←",
            "orientationLandscapeLeft",
            state.orientation == Some(crate::window::DeviceOrientation::LandscapeLeft),
        ),
        (
            "→",
            "orientationLandscapeRight",
            state.orientation == Some(crate::window::DeviceOrientation::LandscapeRight),
        ),
        (
            "↓",
            "orientationPortraitUpsideDown",
            state.orientation == Some(crate::window::DeviceOrientation::PortraitUpsideDown),
        ),
    ];
    let memory_choices: &[(&str, &str, bool)] = &[
        ("Default", "classicMemoryDefault", state.memory_limit_mb.is_none()),
        ("32 MB", "classicMemory32", state.memory_limit_mb == Some(32)),
        ("64 MB", "classicMemory64", state.memory_limit_mb == Some(64)),
    ];
    let rows = [
        Row::LabelAndChoices("Scale hack", scale_hack_choices),
        Row::LabelAndChoices("Orientation", orientation_choices),
        Row::LabelAndSwitch("Network access", "network:", state.network),
        Row::LabelAndSwitch(
            "Use analog sticks for tilt controls",
            "analogStickTiltControls:",
            state.analog_sticks,
        ),
        Row::LabelAndChoices("RAM limiter", memory_choices),
        Row::LabelAndSwitch("Performance overlay", "showFPS:", state.show_fps),
        Row::LabelAndSwitch("Quick menu", "quickMenu:", state.quick_menu),
    ];

    let content_top: CGFloat = 50.0;
    let block = (height - content_top - 8.0) / (rows.len() as CGFloat);
    let gray: id = msg_class![env; UIColor darkGrayColor];
    let mut y = content_top;
    for row in rows.iter() {
        let (title, control_y) = match row {
            Row::LabelAndChoices(title, _) | Row::LabelAndSwitch(title, _, _) => {
                (*title, y + block * 0.52)
            }
        };
        let label = new_label(
            env,
            rect(8.0, y + block * 0.08, width - 16.0, block * 0.36),
            title,
            14.0,
            false,
            gray,
        );
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
        () = msg![env; main_view addSubview:label];
        match row {
            Row::LabelAndChoices(_, choices) => {
                make_choices_row(env, delegate, main_view, width, control_y, block, choices);
            }
            Row::LabelAndSwitch(_, action, on) => {
                let switch: id = msg_class![env; UISwitch alloc];
                let switch: id = msg![env; switch initWithFrame:(rect(
                    width / 2.0 - 47.0,
                    control_y - 13.0,
                    94.0,
                    27.0
                ))];
                let is_on: bool = *on;
                () = msg![env; switch setOn:is_on];
                let selector = env.objc.register_host_selector(action.to_string(), &mut env.mem);
                () = msg![env; switch addTarget:delegate
                                        action:selector
                              forControlEvents:UIControlEventValueChanged];
                () = msg![env; main_view addSubview:switch];
            }
        }
        y += block;
    }

    () = msg![env; parent addSubview:main_view];
    main_view
}

fn make_choices_row(
    env: &mut Environment,
    delegate: id,
    parent: id,
    width: CGFloat,
    center_y: CGFloat,
    block: CGFloat,
    choices: &[(&str, &str, bool)],
) {
    let margin: CGFloat = 10.0;
    let gap: CGFloat = 6.0;
    let height: CGFloat = (block * 0.34).clamp(24.0, 30.0);
    let button_width =
        (width - margin * 2.0 - gap * (choices.len() as CGFloat - 1.0)) / (choices.len() as CGFloat);
    let mut x = margin;
    let black: id = msg_class![env; UIColor blackColor];
    let magenta: id = msg_class![env; UIColor magentaColor];
    let gray: id = msg_class![env; UIColor grayColor];
    for (title, action, selected) in choices {
        let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        () = msg![env; button setFrame:(CGRect {
            origin: CGPoint {
                x,
                y: center_y - height / 2.0,
            },
            size: CGSize {
                width: button_width,
                height,
            },
        })];
        let text = ns_string::from_rust_string(env, title.to_string());
        () = msg![env; button setTitle:text forState:UIControlStateNormal];
        () = msg![env; button setTitleColor:black forState:UIControlStateNormal];
        () = msg![env; button setBackgroundColor:(if *selected { magenta } else { gray })];
        let label: id = msg![env; button titleLabel];
        let font: id = msg_class![env; UIFont systemFontOfSize:(12.0 as CGFloat)];
        () = msg![env; label setFont:font];
        let layer: id = msg![env; button layer];
        () = msg![env; layer setCornerRadius:(6.0 as CGFloat)];
        if *action == "classicMemoryDefault"
            || *action == "classicMemory32"
            || *action == "classicMemory64"
        {
            let size: Option<u32> = match *action {
                "classicMemory32" => Some(32),
                "classicMemory64" => Some(64),
                _ => None,
            };
            let tag = super::memory_limit_tag(size);
            () = msg![env; button setTag:tag];
            let selector = env.objc.register_host_selector("memoryLimit:".to_string(), &mut env.mem);
            () = msg![env; button addTarget:delegate
                                    action:selector
                          forControlEvents:UIControlEventTouchUpInside];
        } else {
            let selector = env.objc.register_host_selector(action.to_string(), &mut env.mem);
            () = msg![env; button addTarget:delegate
                                    action:selector
                          forControlEvents:UIControlEventTouchUpInside];
        }
        () = msg![env; parent addSubview:button];
        x += button_width + gap;
    }
}

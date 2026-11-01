/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Game Center screen for the app picker.
//!
//! Games that report achievements and leaderboards to GameKit get them
//! recorded in the local Game Center store ([crate::game_center]). This
//! screen is the "Game Center app": it lists every app that reported
//! something, its achievements with progress bars, and its best leaderboard
//! scores. It also offers Reset, because the real service has no UI here and
//! wiping progress is the only way to test achievement flows again.
//!
//! The screen runs its own nested run loop, like the IPA store screen, and
//! returns to the home screen when Done is tapped.

use super::archive_store::{make_button, set_button_title, set_label};
use super::{new_label, new_view, rect, ui_color, AppPickerDelegateHostObject};
use crate::frameworks::core_graphics::{CGFloat, CGRect, CGSize};
use crate::frameworks::foundation::ns_run_loop::run_run_loop_single_iteration;
use crate::frameworks::uikit::ui_font::UITextAlignmentCenter;
use crate::game_center;
use crate::objc::{id, msg, msg_class};
use crate::Environment;

const NAV_HEIGHT: CGFloat = 52.0;
const STATUS_HEIGHT: CGFloat = 44.0;
/// Height of the heading that introduces one app's records.
const APP_HEADING_HEIGHT: CGFloat = 42.0;
/// Height of one achievement row, including its progress bar.
const ACHIEVEMENT_ROW_HEIGHT: CGFloat = 46.0;
/// Height of one leaderboard score row.
const SCORE_ROW_HEIGHT: CGFloat = 38.0;
const ROW_GAP: CGFloat = 6.0;
const SIDE_MARGIN: CGFloat = 10.0;

/// The views of the Game Center screen that survive a re-render.
struct GameCenterViews {
    root: id,
    status: id,
    reset_button: id,
    scroll: id,
    /// Content views, removed and rebuilt on every render.
    rows: Vec<id>,
    width: CGFloat,
}

/// Open the Game Center screen as a modal picker screen. Everything the
/// screen shows comes from the local store, so it returns nothing.
pub(super) fn run_game_center(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    parent: id,
    app_frame: CGRect,
) {
    let mut views = build_views(env, delegate, parent, app_frame);
    let mut dirty = true;
    // Reset is destructive and there is no undo, so the first tap only arms
    // the button and the second tap within the same visit erases progress.
    let mut reset_armed = false;

    loop {
        run_run_loop_single_iteration(env, run_loop);

        let (close, reset) = take_actions(env, delegate);
        if close {
            break;
        }
        if reset {
            if reset_armed {
                game_center::reset_all();
                log!("Game Center: local progress erased from the app picker");
                reset_armed = false;
                set_button_title(env, views.reset_button, "Reset");
                dirty = true;
            } else {
                reset_armed = true;
                set_button_title(env, views.reset_button, "Erase?");
            }
        }
        if dirty {
            render(env, &mut views);
            dirty = false;
        }
    }

    () = msg![env; (views.root) removeFromSuperview];
    let host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
    host.game_center_close = false;
    host.game_center_reset = false;
}

/// Consume the taps queued by the delegate methods.
fn take_actions(env: &mut Environment, delegate: id) -> (bool, bool) {
    let host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
    (
        std::mem::take(&mut host.game_center_close),
        std::mem::take(&mut host.game_center_reset),
    )
}

fn build_views(
    env: &mut Environment,
    delegate: id,
    parent: id,
    app_frame: CGRect,
) -> GameCenterViews {
    let width = app_frame.size.width;
    let status_offset = app_frame.origin.y;
    let full_height = app_frame.size.height + status_offset;
    let background = ui_color(env, 0.93, 0.94, 0.97, 1.0);
    let root = new_view(
        env,
        rect(0.0, -status_offset, width, full_height),
        background,
    );
    () = msg![env; parent addSubview:root];

    // Game Center's own colour, so the screen is recognisable next to the
    // blue Settings and dark IPA Store screens.
    let nav_color = ui_color(env, 0.09, 0.36, 0.27, 1.0);
    let nav_height = NAV_HEIGHT + status_offset;
    let nav = new_view(env, rect(0.0, 0.0, width, nav_height), nav_color);
    () = msg![env; root addSubview:nav];

    let white: id = msg_class![env; UIColor whiteColor];
    let clear: id = msg_class![env; UIColor clearColor];
    let title = new_label(
        env,
        rect(78.0, status_offset, width - 156.0, NAV_HEIGHT),
        "Game Center",
        17.0,
        true,
        white,
    );
    () = msg![env; title setTextAlignment:UITextAlignmentCenter];
    () = msg![env; nav addSubview:title];

    let reset_button = make_button(
        env,
        delegate,
        nav,
        rect(5.0, status_offset + 10.0, 70.0, 32.0),
        "Reset",
        "gameCenterReset",
        0,
        clear,
        white,
    );
    let done_button = make_button(
        env,
        delegate,
        nav,
        rect(width - 75.0, status_offset + 10.0, 70.0, 32.0),
        "Done",
        "gameCenterClose",
        0,
        clear,
        white,
    );

    let status_color = ui_color(env, 0.31, 0.37, 0.41, 1.0);
    let status = new_label(
        env,
        rect(SIDE_MARGIN + 2.0, nav_height + 4.0, width - 2.0 * SIDE_MARGIN, 36.0),
        "",
        11.0,
        false,
        status_color,
    );
    () = msg![env; status setNumberOfLines:2];
    () = msg![env; root addSubview:status];

    let scroll_y = nav_height + STATUS_HEIGHT;
    let scroll: id = msg_class![env; UIScrollView alloc];
    let scroll: id = msg![env; scroll initWithFrame:(rect(
        0.0,
        scroll_y,
        width,
        full_height - scroll_y
    ))];
    () = msg![env; scroll setBackgroundColor:clear];
    () = msg![env; scroll setClipsToBounds:true];
    () = msg![env; scroll setShowsHorizontalScrollIndicator:false];
    () = msg![env; root addSubview:scroll];

    // UIScrollView clips its content, but keep the opaque chrome above it
    // anyway, like the IPA store screen does.
    () = msg![env; root bringSubviewToFront:nav];
    () = msg![env; root bringSubviewToFront:status];

    GameCenterViews {
        root,
        status,
        reset_button,
        scroll,
        rows: Vec::new(),
        width,
    }
}

fn render(env: &mut Environment, views: &mut GameCenterViews) {
    for row in views.rows.drain(..) {
        () = msg![env; row removeFromSuperview];
    }

    let apps = game_center::summary();
    let total_achievements: usize = apps
        .iter()
        .map(|app| app.achievements.len())
        .sum();
    let completed_achievements: usize = apps
        .iter()
        .map(|app| app.completed_achievements())
        .sum();

    let status = if apps.is_empty() {
        format!(
            "Signed in as {}. No achievements yet: play a game that reports \
             Game Center achievements.",
            game_center::PLAYER_ALIAS
        )
    } else {
        format!(
            "Signed in as {} · {} app(s) · {}/{} achievements",
            game_center::PLAYER_ALIAS,
            apps.len(),
            completed_achievements,
            total_achievements
        )
    };
    set_label(env, views.status, &status);

    let dark: id = msg_class![env; UIColor blackColor];
    let white: id = msg_class![env; UIColor whiteColor];
    let gray = ui_color(env, 0.42, 0.47, 0.52, 1.0);
    let blue = ui_color(env, 0.09, 0.45, 0.85, 1.0);
    let bar_background = ui_color(env, 0.86, 0.88, 0.91, 1.0);
    let row_width = views.width - 2.0 * SIDE_MARGIN;

    let mut y: CGFloat = 4.0;
    if apps.is_empty() {
        let hint = new_label(
            env,
            rect(SIDE_MARGIN + 4.0, y + 10.0, row_width - 8.0, 96.0),
            "Achievements reported by games appear here, with their progress \
             and best leaderboard scores. Enable \"Local Game Center sign-in\" \
             in Settings for games that require a signed-in player.",
            12.0,
            false,
            gray,
        );
        () = msg![env; hint setNumberOfLines:6];
        () = msg![env; (views.scroll) addSubview:hint];
        views.rows.push(hint);
        y += 110.0;
    }

    for app in &apps {
        let heading = new_label(
            env,
            rect(SIDE_MARGIN + 2.0, y + 2.0, row_width - 104.0, 20.0),
            &app.display_name,
            15.0,
            true,
            dark,
        );
        () = msg![env; (views.scroll) addSubview:heading];
        views.rows.push(heading);
        let bundle = new_label(
            env,
            rect(SIDE_MARGIN + 2.0, y + 21.0, row_width - 104.0, 14.0),
            &app.bundle_id,
            10.0,
            false,
            gray,
        );
        () = msg![env; (views.scroll) addSubview:bundle];
        views.rows.push(bundle);

        let summary = format!(
            "{}/{} · {:.0}%",
            app.completed_achievements(),
            app.achievements.len(),
            app.average_percent()
        );
        let count = new_label(
            env,
            rect(views.width - SIDE_MARGIN - 98.0, y + 2.0, 96.0, 20.0),
            &summary,
            12.0,
            true,
            gray,
        );
        () = msg![env; count setTextAlignment:UITextAlignmentCenter];
        () = msg![env; (views.scroll) addSubview:count];
        views.rows.push(count);
        y += APP_HEADING_HEIGHT;

        for achievement in app.achievements.values() {
            let row = new_view(
                env,
                rect(SIDE_MARGIN, y, row_width, ACHIEVEMENT_ROW_HEIGHT - 2.0),
                white,
            );
            () = msg![env; (views.scroll) addSubview:row];
            views.rows.push(row);
            let row_layer: id = msg![env; row layer];
            () = msg![env; row_layer setCornerRadius:(9.0 as CGFloat)];

            let name = new_label(
                env,
                rect(10.0, 6.0, row_width - 76.0, 18.0),
                &achievement.identifier,
                12.0,
                false,
                dark,
            );
            () = msg![env; row addSubview:name];
            let percent = if achievement.completed {
                "Completed".to_string()
            } else {
                format!("{:.0}%", achievement.percent)
            };
            let value = new_label(
                env,
                rect(row_width - 76.0, 6.0, 66.0, 18.0),
                &percent,
                12.0,
                true,
                if achievement.completed { blue } else { gray },
            );
            () = msg![env; value setTextAlignment:UITextAlignmentCenter];
            () = msg![env; row addSubview:value];

            let bar_width = row_width - 20.0;
            let bar = new_view(env, rect(10.0, 30.0, bar_width, 5.0), bar_background);
            () = msg![env; row addSubview:bar];
            let fill_width = bar_width * (achievement.percent / 100.0).clamp(0.0, 1.0) as CGFloat;
            if fill_width > 0.0 {
                let fill = new_view(env, rect(10.0, 30.0, fill_width, 5.0), blue);
                () = msg![env; row addSubview:fill];
            }
            y += ACHIEVEMENT_ROW_HEIGHT;
        }

        if app.achievements.is_empty() {
            let empty = new_label(
                env,
                rect(SIDE_MARGIN + 2.0, y + 4.0, row_width - 4.0, 18.0),
                "No achievements reported yet.",
                12.0,
                false,
                gray,
            );
            () = msg![env; (views.scroll) addSubview:empty];
            views.rows.push(empty);
            y += 26.0;
        }

        for score in app.scores.values() {
            let row = new_view(
                env,
                rect(SIDE_MARGIN, y, row_width, SCORE_ROW_HEIGHT - ROW_GAP),
                white,
            );
            () = msg![env; (views.scroll) addSubview:row];
            views.rows.push(row);
            let row_layer: id = msg![env; row layer];
            () = msg![env; row_layer setCornerRadius:(9.0 as CGFloat)];
            let name = new_label(
                env,
                rect(10.0, 5.0, row_width - 106.0, 21.0),
                &score.leaderboard,
                12.0,
                false,
                dark,
            );
            () = msg![env; row addSubview:name];
            let formatted_text = if score.formatted.is_empty() {
                score.value.to_string()
            } else {
                score.formatted.clone()
            };
            let formatted = new_label(
                env,
                rect(row_width - 96.0, 5.0, 86.0, 21.0),
                &formatted_text,
                13.0,
                true,
                blue,
            );
            () = msg![env; formatted setTextAlignment:UITextAlignmentCenter];
            () = msg![env; row addSubview:formatted];
            y += SCORE_ROW_HEIGHT;
        }

        y += 8.0;
    }

    let frame: CGRect = msg![env; (views.scroll) frame];
    () = msg![env; (views.scroll) setContentSize:(CGSize {
        width: frame.size.width,
        height: y.max(frame.size.height),
    })];
}

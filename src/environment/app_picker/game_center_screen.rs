/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Game Center screen for the app picker.
//!
//! Games report achievements and leaderboard scores to GameKit, which stores
//! them in the local Game Center store ([crate::game_center]). This screen is
//! the "Game Center app": a list of games, a detail view per game, and two
//! flattened views of every achievement and every best score. Apple's own
//! titles and descriptions come from the App Store and are unavailable
//! offline, so achievements are named with
//! [crate::game_center::humanize_identifier] and the raw identifier is shown
//! underneath, so entries can still be matched against a game's own data.
//!
//! The screen runs its own nested run loop, like the IPA Store screen, and
//! returns to the home screen when Done is tapped.

use super::archive_store::{make_button, set_button_title, set_label};
use super::{new_label, new_view, rect, ui_color, AppPickerDelegateHostObject};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_run_loop::run_run_loop_single_iteration;
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_font::UITextAlignmentCenter;
use crate::frameworks::uikit::ui_view::ui_control::UIControlStateNormal;
use crate::game_center::{self, Achievement, AppRecord, Score};
use crate::objc::{id, msg, msg_class};
use crate::Environment;

const NAV_HEIGHT: CGFloat = 52.0;
const STATUS_HEIGHT: CGFloat = 34.0;
const TAB_BAR_HEIGHT: CGFloat = 46.0;
const SIDE_MARGIN: CGFloat = 10.0;
const ROW_GAP: CGFloat = 7.0;
/// A card in the game list: name, bundle id, progress bar.
const GAME_ROW_HEIGHT: CGFloat = 66.0;
/// One achievement card: title, identifier, percentage, progress bar.
const ACHIEVEMENT_ROW_HEIGHT: CGFloat = 56.0;
/// One leaderboard card: title, subtitle, best value.
const SCORE_ROW_HEIGHT: CGFloat = 48.0;
/// Heading above a group of rows.
const SECTION_HEADER_HEIGHT: CGFloat = 26.0;
/// Summary card at the top of a game's detail view.
const DETAIL_HEADER_HEIGHT: CGFloat = 84.0;

/// Which flattened list the user is looking at.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tab {
    Games,
    Achievements,
    Scores,
}

impl Tab {
    fn from_tag(tag: NSInteger) -> Self {
        match tag {
            1 => Self::Achievements,
            2 => Self::Scores,
            _ => Self::Games,
        }
    }
}

/// Where the screen currently is.
enum Screen {
    /// One of the bottom tabs.
    List(Tab),
    /// The achievements and scores of one game, identified by bundle id so a
    /// reload of the store cannot point at the wrong game.
    Detail(String),
}

/// What a tap on the scroll view landed on.
enum TapTarget {
    Game(String),
    Tab(Tab),
}

/// The views of the Game Center screen that survive a re-render.
struct GameCenterViews {
    root: id,
    nav_title: id,
    back_button: id,
    status: id,
    reset_button: id,
    scroll: id,
    tab_buttons: Vec<id>,
    /// Content views, removed and rebuilt on every render.
    rows: Vec<id>,
    /// Tappable bands inside the scroll view: (top, bottom, target).
    hotspots: Vec<(CGFloat, CGFloat, TapTarget)>,
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
    // The tab the user came from, so Back from a game returns there instead of
    // always to the game list.
    let mut list_tab = Tab::Games;
    let mut screen = Screen::List(list_tab);
    // Reset is destructive and there is no undo, so the first tap only arms
    // the button and the second tap within the same visit erases progress.
    let mut reset_armed = false;
    let mut dirty = true;

    loop {
        run_run_loop_single_iteration(env, run_loop);

        let actions = take_actions(env, delegate);
        if actions.close {
            break;
        }
        if let Some(tag) = actions.tab {
            list_tab = Tab::from_tag(tag);
            screen = Screen::List(list_tab);
            dirty = true;
        }
        if actions.back {
            // The left navigation button doubles as Back (inside a game) and
            // as the way out of the screen (on a tab), because there is no
            // other exit: this screen runs its own run loop.
            if matches!(screen, Screen::Detail(_)) {
                screen = Screen::List(list_tab);
                dirty = true;
            } else {
                break;
            }
        }
        if actions.reset {
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
        if let Some(point) = actions.tap {
            match tap_target(&views, point) {
                Some(TapTarget::Game(bundle_id)) => {
                    screen = Screen::Detail(bundle_id);
                    dirty = true;
                }
                Some(TapTarget::Tab(tab)) => {
                    list_tab = tab;
                    screen = Screen::List(tab);
                    dirty = true;
                }
                None => (),
            }
        }
        if dirty {
            render(env, &mut views, &screen);
            update_tabs(env, &views, &screen);
            dirty = false;
        }
    }

    () = msg![env; (views.root) removeFromSuperview];
    let host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
    host.game_center_close = false;
    host.game_center_reset = false;
    host.game_center_back = false;
    host.game_center_tab = None;
    host.game_center_tap = None;
}

struct Actions {
    close: bool,
    reset: bool,
    back: bool,
    tab: Option<NSInteger>,
    tap: Option<CGPoint>,
}

fn take_actions(env: &mut Environment, delegate: id) -> Actions {
    let host = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
    Actions {
        close: std::mem::take(&mut host.game_center_close),
        reset: std::mem::take(&mut host.game_center_reset),
        back: std::mem::take(&mut host.game_center_back),
        tab: host.game_center_tab.take(),
        tap: host.game_center_tap.take(),
    }
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
    let nav_title = new_label(
        env,
        rect(66.0, status_offset, width - 132.0, NAV_HEIGHT),
        "Game Center",
        17.0,
        true,
        white,
    );
    () = msg![env; nav_title setTextAlignment:UITextAlignmentCenter];
    () = msg![env; nav addSubview:nav_title];

    let back_button = make_button(
        env,
        delegate,
        nav,
        rect(3.0, status_offset + 10.0, 62.0, 32.0),
        "< Back",
        "gameCenterBack",
        0,
        clear,
        white,
    );
    () = msg![env; back_button setHidden:false];
    let reset_button = make_button(
        env,
        delegate,
        nav,
        rect(width - 74.0, status_offset + 10.0, 70.0, 32.0),
        "Reset",
        "gameCenterReset",
        0,
        clear,
        white,
    );

    let status_color = ui_color(env, 0.31, 0.37, 0.41, 1.0);
    let status = new_label(
        env,
        rect(SIDE_MARGIN + 2.0, nav_height + 2.0, width - 2.0 * SIDE_MARGIN, 30.0),
        "",
        11.0,
        false,
        status_color,
    );
    () = msg![env; status setNumberOfLines:2];
    () = msg![env; root addSubview:status];

    let tab_bar_height = TAB_BAR_HEIGHT;
    let tab_bar = new_view(
        env,
        rect(0.0, full_height - tab_bar_height, width, tab_bar_height),
        white,
    );
    () = msg![env; root addSubview:tab_bar];
    let separator_color = ui_color(env, 0.80, 0.83, 0.88, 1.0);
    let separator = new_view(env, rect(0.0, 0.0, width, 1.0), separator_color);
    () = msg![env; tab_bar addSubview:separator];
    let tab_width = width / 3.0;
    let mut tab_buttons = Vec::new();
    for (index, title) in ["Games", "Achievements", "Scores"].iter().enumerate() {
        let button = make_button(
            env,
            delegate,
            tab_bar,
            rect(
                index as CGFloat * tab_width,
                1.0,
                tab_width,
                tab_bar_height - 1.0,
            ),
            title,
            "gameCenterTab:",
            index as NSInteger,
            clear,
            white,
        );
        tab_buttons.push(button);
    }

    let scroll_y = nav_height + STATUS_HEIGHT;
    let scroll_height = full_height - scroll_y - tab_bar_height;
    let scroll: id = msg_class![env; UIScrollView alloc];
    let scroll: id = msg![env; scroll initWithFrame:(rect(
        0.0,
        scroll_y,
        width,
        scroll_height
    ))];
    () = msg![env; scroll setBackgroundColor:clear];
    () = msg![env; scroll setClipsToBounds:true];
    () = msg![env; scroll setShowsHorizontalScrollIndicator:false];
    () = msg![env; root addSubview:scroll];

    // Row cards are plain views, so taps are resolved from their position
    // inside the scroll view, as the IPA Store screen does.
    let tap_selector = env.objc.lookup_selector("gameCenterTap:").unwrap();
    let tap: id = msg_class![env; UITapGestureRecognizer alloc];
    let tap: id = msg![env; tap initWithTarget:delegate action:tap_selector];
    () = msg![env; tap setCancelsTouchesInView:false];
    () = msg![env; scroll addGestureRecognizer:tap];

    // UIScrollView clips its content, but keep the opaque chrome above it
    // anyway, like the IPA store screen does.
    for view in [nav, status, tab_bar] {
        () = msg![env; root bringSubviewToFront:view];
    }

    GameCenterViews {
        root,
        nav_title,
        back_button,
        status,
        reset_button,
        scroll,
        tab_buttons,
        rows: Vec::new(),
        hotspots: Vec::new(),
        width,
    }
}

/// Resolve a tap inside the scroll view to a row or a tab.
fn tap_target(views: &GameCenterViews, point: CGPoint) -> Option<TapTarget> {
    if point.x < 0.0 || point.x > views.width || point.y < 0.0 {
        return None;
    }
    views
        .hotspots
        .iter()
        .find(|(top, bottom, _)| point.y >= *top && point.y <= *bottom)
        .map(|(_, _, target)| match target {
            TapTarget::Game(bundle_id) => TapTarget::Game(bundle_id.clone()),
            TapTarget::Tab(tab) => TapTarget::Tab(*tab),
        })
}

/// Highlight the active tab, and hide the tab bar's selection while a game's
/// detail view is open.
fn update_tabs(env: &mut Environment, views: &GameCenterViews, screen: &Screen) {
    let active = match screen {
        Screen::List(tab) => Some(*tab),
        Screen::Detail(_) => None,
    };
    let selected_color = ui_color(env, 0.09, 0.45, 0.85, 1.0);
    let idle_color = ui_color(env, 0.45, 0.49, 0.55, 1.0);
    for (index, button) in views.tab_buttons.iter().enumerate() {
        let tab = Tab::from_tag(index as NSInteger);
        let color = if active == Some(tab) {
            selected_color
        } else {
            idle_color
        };
        () = msg![env; (*button) setTitleColor:color forState:UIControlStateNormal];
        let label: id = msg![env; (*button) titleLabel];
        () = msg![env; label setTextColor:color];
    }
}

fn render(env: &mut Environment, views: &mut GameCenterViews, screen: &Screen) {
    for row in views.rows.drain(..) {
        () = msg![env; row removeFromSuperview];
    }
    views.hotspots.clear();

    let apps = game_center::summary();
    let signed_in = env.options.game_center_authenticated;
    let total_achievements: usize = apps.iter().map(|app| app.achievements.len()).sum();
    let completed_achievements: usize = apps
        .iter()
        .map(|app| app.completed_achievements())
        .sum();

    let (nav_title, status) = match screen {
        Screen::Detail(bundle_id) => {
            let app = apps.iter().find(|app| &app.bundle_id == bundle_id);
            let title = app
                .map(|app| app.display_name.clone())
                .unwrap_or_else(|| "Game".to_string());
            let status = match app {
                Some(app) => format!(
                    "{}/{} achievements · {:.0}% average · {} leaderboard(s)",
                    app.completed_achievements(),
                    app.achievements.len(),
                    app.average_percent(),
                    app.scores.len()
                ),
                None => "This game has no Game Center data.".to_string(),
            };
            (title, status)
        }
        Screen::List(_) => (
            "Game Center".to_string(),
            if !signed_in {
                "Signed out · turn on \"Local Game Center sign-in\" in Settings".to_string()
            } else if apps.is_empty() {
                format!("Signed in as {}", game_center::PLAYER_ALIAS)
            } else {
                format!(
                    "{} · {} game(s) · {}/{} achievements",
                    game_center::PLAYER_ALIAS,
                    apps.len(),
                    completed_achievements,
                    total_achievements
                )
            },
        ),
    };
    set_label(env, views.nav_title, &nav_title);
    set_label(env, views.status, &status);
    let back_title = if matches!(screen, Screen::Detail(_)) {
        "< Back"
    } else {
        "Done"
    };
    set_button_title(env, views.back_button, back_title);

    let mut cursor = 4.0;
    match screen {
        Screen::List(Tab::Games) => render_games(env, views, &apps, &mut cursor),
        Screen::List(Tab::Achievements) => render_all_achievements(env, views, &apps, &mut cursor),
        Screen::List(Tab::Scores) => render_all_scores(env, views, &apps, &mut cursor),
        Screen::Detail(bundle_id) => {
            let app = apps.iter().find(|app| &app.bundle_id == bundle_id);
            match app {
                Some(app) => render_detail(env, views, app, &mut cursor),
                None => add_message(
                    env,
                    views,
                    &mut cursor,
                    "Nothing has been recorded for this game yet.",
                ),
            }
        }
    }
    if matches!(screen, Screen::List(_)) && apps.is_empty() {
        add_message(
            env,
            views,
            &mut cursor,
            "Achievements and best scores reported by games appear here. \
             Enable \"Local Game Center sign-in\" in Settings > Emulation for \
             games that require a signed-in player.",
        );
    }

    let frame: CGRect = msg![env; (views.scroll) frame];
    () = msg![env; (views.scroll) setContentSize:(CGSize {
        width: frame.size.width,
        height: (cursor + 8.0).max(frame.size.height),
    })];
}

/// A full-width explanatory label (empty states, hints).
fn add_message(
    env: &mut Environment,
    views: &mut GameCenterViews,
    cursor: &mut CGFloat,
    text: &str,
) {
    let gray = ui_color(env, 0.40, 0.45, 0.51, 1.0);
    let label = new_label(
        env,
        rect(SIDE_MARGIN + 6.0, *cursor + 8.0, views.width - 2.0 * SIDE_MARGIN - 12.0, 92.0),
        text,
        12.0,
        false,
        gray,
    );
    () = msg![env; label setNumberOfLines:6];
    () = msg![env; (views.scroll) addSubview:label];
    views.rows.push(label);
    *cursor += 108.0;
}

fn add_card(env: &mut Environment, views: &mut GameCenterViews, frame: CGRect) -> id {
    let white: id = msg_class![env; UIColor whiteColor];
    let card = new_view(env, frame, white);
    let layer: id = msg![env; card layer];
    () = msg![env; layer setCornerRadius:(10.0 as CGFloat)];
    () = msg![env; (views.scroll) addSubview:card];
    views.rows.push(card);
    card
}

/// A rounded progress bar: grey track plus a coloured fill.
fn add_progress_bar(
    env: &mut Environment,
    parent: id,
    frame: CGRect,
    fraction: f64,
    color: id,
) {
    let track_color = ui_color(env, 0.86, 0.88, 0.91, 1.0);
    let track = new_view(env, frame, track_color);
    let track_layer: id = msg![env; track layer];
    () = msg![env; track_layer setCornerRadius:(frame.size.height / 2.0)];
    () = msg![env; parent addSubview:track];
    let fraction = fraction.clamp(0.0, 1.0) as CGFloat;
    if fraction <= 0.0 {
        return;
    }
    let mut fill_frame = frame;
    fill_frame.size.width = frame.size.width * fraction;
    let fill = new_view(env, fill_frame, color);
    let fill_layer: id = msg![env; fill layer];
    () = msg![env; fill_layer setCornerRadius:(frame.size.height / 2.0)];
    () = msg![env; parent addSubview:fill];
}

fn add_label_to(
    env: &mut Environment,
    parent: id,
    frame: CGRect,
    text: &str,
    size: CGFloat,
    bold: bool,
    color: id,
    centered: bool,
) -> id {
    let label = new_label(env, frame, text, size, bold, color);
    if centered {
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
    }
    () = msg![env; parent addSubview:label];
    label
}

fn add_section_header(
    env: &mut Environment,
    views: &mut GameCenterViews,
    cursor: &mut CGFloat,
    text: &str,
) {
    let gray = ui_color(env, 0.42, 0.47, 0.53, 1.0);
    let label = new_label(
        env,
        rect(SIDE_MARGIN + 4.0, *cursor + 6.0, views.width - 2.0 * SIDE_MARGIN - 8.0, 16.0),
        text,
        11.0,
        false,
        gray,
    );
    () = msg![env; (views.scroll) addSubview:label];
    views.rows.push(label);
    *cursor += SECTION_HEADER_HEIGHT;
}

/// The "Games" tab: one card per game that reported anything.
fn render_games(
    env: &mut Environment,
    views: &mut GameCenterViews,
    apps: &[AppRecord],
    cursor: &mut CGFloat,
) {
    let dark: id = msg_class![env; UIColor blackColor];
    let gray = ui_color(env, 0.44, 0.49, 0.55, 1.0);
    let blue = ui_color(env, 0.09, 0.45, 0.85, 1.0);
    let row_width = views.width - 2.0 * SIDE_MARGIN;
    for app in apps {
        let card = add_card(
            env,
            views,
            rect(SIDE_MARGIN, *cursor, row_width, GAME_ROW_HEIGHT - ROW_GAP),
        );
        add_label_to(
            env,
            card,
            rect(12.0, 7.0, row_width - 24.0, 19.0),
            &app.display_name,
            14.0,
            true,
            dark,
            false,
        );
        add_label_to(
            env,
            card,
            rect(12.0, 26.0, row_width - 24.0, 13.0),
            &app.bundle_id,
            9.5,
            false,
            gray,
            false,
        );
        let summary = format!(
            "{}/{} · {:.0}%",
            app.completed_achievements(),
            app.achievements.len(),
            app.average_percent()
        );
        add_label_to(
            env,
            card,
            rect(row_width - 128.0, 22.0, 116.0, 16.0),
            &summary,
            11.0,
            true,
            gray,
            true,
        );
        add_progress_bar(
            env,
            card,
            rect(12.0, 42.0, row_width - 24.0, 6.0),
            app.average_percent() / 100.0,
            blue,
        );
        // Tap target covers the whole row.
        views.hotspots.push((
            *cursor,
            *cursor + GAME_ROW_HEIGHT - ROW_GAP,
            TapTarget::Game(app.bundle_id.clone()),
        ));
        *cursor += GAME_ROW_HEIGHT;
    }
}

/// One achievement card.
fn add_achievement_card(
    env: &mut Environment,
    views: &mut GameCenterViews,
    cursor: CGFloat,
    achievement: &Achievement,
    subtitle: &str,
) {
    let row_width = views.width - 2.0 * SIDE_MARGIN;
    let dark: id = msg_class![env; UIColor blackColor];
    let gray = ui_color(env, 0.47, 0.52, 0.58, 1.0);
    let blue = ui_color(env, 0.09, 0.45, 0.85, 1.0);
    let green = ui_color(env, 0.13, 0.55, 0.32, 1.0);
    let card = add_card(
        env,
        views,
        rect(SIDE_MARGIN, cursor, row_width, ACHIEVEMENT_ROW_HEIGHT - ROW_GAP),
    );
    add_label_to(
        env,
        card,
        rect(12.0, 6.0, row_width - 110.0, 18.0),
        &achievement.title(),
        13.0,
        true,
        dark,
        false,
    );
    add_label_to(
        env,
        card,
        rect(12.0, 24.0, row_width - 110.0, 13.0),
        subtitle,
        9.5,
        false,
        gray,
        false,
    );
    let (value_text, value_color) = if achievement.completed {
        ("Completed".to_string(), green)
    } else {
        (format!("{:.0}%", achievement.percent), blue)
    };
    add_label_to(
        env,
        card,
        rect(row_width - 104.0, 16.0, 92.0, 18.0),
        &value_text,
        12.0,
        true,
        value_color,
        true,
    );
    add_progress_bar(
        env,
        card,
        rect(12.0, 42.0, row_width - 24.0, 5.0),
        achievement.percent / 100.0,
        if achievement.completed { green } else { blue },
    );
}

/// One leaderboard card.
fn add_score_card(
    env: &mut Environment,
    views: &mut GameCenterViews,
    cursor: CGFloat,
    score: &Score,
    subtitle: &str,
) {
    let row_width = views.width - 2.0 * SIDE_MARGIN;
    let dark: id = msg_class![env; UIColor blackColor];
    let gray = ui_color(env, 0.47, 0.52, 0.58, 1.0);
    let blue = ui_color(env, 0.09, 0.45, 0.85, 1.0);
    let card = add_card(
        env,
        views,
        rect(SIDE_MARGIN, cursor, row_width, SCORE_ROW_HEIGHT - ROW_GAP),
    );
    add_label_to(
        env,
        card,
        rect(12.0, 6.0, row_width - 120.0, 18.0),
        &score.title(),
        13.0,
        true,
        dark,
        false,
    );
    add_label_to(
        env,
        card,
        rect(12.0, 24.0, row_width - 120.0, 12.0),
        subtitle,
        9.5,
        false,
        gray,
        false,
    );
    let value = if score.formatted.is_empty() {
        score.value.to_string()
    } else {
        score.formatted.clone()
    };
    add_label_to(
        env,
        card,
        rect(row_width - 110.0, 13.0, 98.0, 20.0),
        &value,
        15.0,
        true,
        blue,
        true,
    );
}

/// Achievements of one game, in a reading order that puts what is still to do
/// first, then the finished ones, each group alphabetically.
fn ordered_achievements(app: &AppRecord) -> Vec<&Achievement> {
    let mut achievements: Vec<&Achievement> = app.achievements.values().collect();
    achievements.sort_by(|a, b| {
        a.completed
            .cmp(&b.completed)
            .then_with(|| a.title().to_lowercase().cmp(&b.title().to_lowercase()))
            .then_with(|| a.identifier.cmp(&b.identifier))
    });
    achievements
}

fn ordered_scores(app: &AppRecord) -> Vec<&Score> {
    let mut scores: Vec<&Score> = app.scores.values().collect();
    scores.sort_by(|a, b| {
        a.title()
            .to_lowercase()
            .cmp(&b.title().to_lowercase())
            .then_with(|| a.leaderboard.cmp(&b.leaderboard))
    });
    scores
}

/// The "Achievements" tab: every achievement of every game.
fn render_all_achievements(
    env: &mut Environment,
    views: &mut GameCenterViews,
    apps: &[AppRecord],
    cursor: &mut CGFloat,
) {
    let mut any = false;
    for app in apps {
        let achievements = ordered_achievements(app);
        if achievements.is_empty() {
            continue;
        }
        any = true;
        let header = format!(
            "{} — {}/{}",
            app.display_name,
            app.completed_achievements(),
            app.achievements.len()
        );
        add_section_header(env, views, cursor, &header);
        for achievement in achievements {
            add_achievement_card(
                env,
                views,
                *cursor,
                achievement,
                &format!("{} · {}", app.display_name, achievement.identifier),
            );
            *cursor += ACHIEVEMENT_ROW_HEIGHT;
        }
    }
    if !any && !apps.is_empty() {
        add_message(env, views, cursor, "No achievements have been reported yet.");
    }
}

/// The "Scores" tab: the best score of every leaderboard of every game.
fn render_all_scores(
    env: &mut Environment,
    views: &mut GameCenterViews,
    apps: &[AppRecord],
    cursor: &mut CGFloat,
) {
    let mut any = false;
    for app in apps {
        let scores = ordered_scores(app);
        if scores.is_empty() {
            continue;
        }
        any = true;
        add_section_header(env, views, cursor, &app.display_name);
        for score in scores {
            add_score_card(
                env,
                views,
                *cursor,
                score,
                &format!("Best score · {}", score.leaderboard),
            );
            *cursor += SCORE_ROW_HEIGHT;
        }
    }
    if !any && !apps.is_empty() {
        add_message(env, views, cursor, "No leaderboard scores have been reported yet.");
    }
}

/// One game's own page: summary, achievements, leaderboards.
fn render_detail(
    env: &mut Environment,
    views: &mut GameCenterViews,
    app: &AppRecord,
    cursor: &mut CGFloat,
) {
    let row_width = views.width - 2.0 * SIDE_MARGIN;
    let dark: id = msg_class![env; UIColor blackColor];
    let gray = ui_color(env, 0.44, 0.49, 0.55, 1.0);
    let blue = ui_color(env, 0.09, 0.45, 0.85, 1.0);

    let header = add_card(
        env,
        views,
        rect(SIDE_MARGIN, *cursor, row_width, DETAIL_HEADER_HEIGHT - ROW_GAP),
    );
    add_label_to(
        env,
        header,
        rect(12.0, 8.0, row_width - 24.0, 20.0),
        &app.display_name,
        16.0,
        true,
        dark,
        false,
    );
    add_label_to(
        env,
        header,
        rect(12.0, 28.0, row_width - 24.0, 13.0),
        &app.bundle_id,
        9.5,
        false,
        gray,
        false,
    );
    let summary = format!(
        "{}/{} achievements · {:.0}% average",
        app.completed_achievements(),
        app.achievements.len(),
        app.average_percent()
    );
    add_label_to(
        env,
        header,
        rect(12.0, 44.0, row_width - 24.0, 15.0),
        &summary,
        11.5,
        true,
        gray,
        false,
    );
    add_progress_bar(
        env,
        header,
        rect(12.0, 62.0, row_width - 24.0, 6.0),
        app.average_percent() / 100.0,
        blue,
    );
    *cursor += DETAIL_HEADER_HEIGHT;

    if app.achievements.is_empty() {
        add_section_header(env, views, cursor, "ACHIEVEMENTS");
        add_message(env, views, cursor, "No achievements reported by this game yet.");
    } else {
        let header_text = format!(
            "ACHIEVEMENTS · {}/{} COMPLETE",
            app.completed_achievements(),
            app.achievements.len()
        );
        add_section_header(env, views, cursor, &header_text);
        for achievement in ordered_achievements(app) {
            add_achievement_card(env, views, *cursor, achievement, &achievement.identifier);
            *cursor += ACHIEVEMENT_ROW_HEIGHT;
        }
    }

    if !app.scores.is_empty() {
        add_section_header(env, views, cursor, "LEADERBOARDS");
        for score in ordered_scores(app) {
            add_score_card(env, views, *cursor, score, &score.leaderboard);
            *cursor += SCORE_ROW_HEIGHT;
        }
    }
}

/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GKAchievement`
//!
//! Achievement reporting, backed by the local Game Center store
//! ([crate::game_center]) instead of Apple's servers. Games that report
//! progress see it remembered across launches, and games that read the
//! achievement list back get the locally recorded one.
//!
//! Reference: <https://developer.apple.com/documentation/gamekit/gkachievement>

use super::gk_local_player::{
    invoke_error_block, invoke_two_object_block, is_signed_in, make_not_authenticated_error,
};
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSUInteger;
use crate::game_center;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

#[derive(Default)]
struct GKAchievementHostObject {
    /// The achievement identifier string (retained NSString*).
    identifier: id,
    /// Percentage complete (0.0 to 100.0).
    percent_complete: f64,
    /// Whether a banner should be shown on completion.
    shows_completion_banner: bool,
    /// `NSString*` player identifier this achievement was reported for.
    player_id: id,
}
impl HostObject for GKAchievementHostObject {}

/// Build a `GKAchievement` host object for an achievement from the local
/// store and add it to `array`. The object is autoreleased once added, which
/// leaves the array as its only owner.
fn add_stored_achievement(
    env: &mut Environment,
    array: id,
    achievement: &game_center::Achievement,
) {
    let identifier = ns_string::from_rust_string(env, achievement.identifier.clone());
    autorelease(env, identifier);
    let object: id = msg_class![env; GKAchievement alloc];
    let object: id = msg![env; object initWithIdentifier:identifier];
    () = msg![env; object setPercentComplete:(achievement.percent)];
    () = msg![env; object setShowsCompletionBanner:false];
    () = msg![env; array addObject:object];
    autorelease(env, object);
}

/// Record one reported achievement in the local store and log the change.
fn record_report(env: &mut Environment, identifier: id, percent: f64, banner: bool) {
    if identifier == nil {
        log!("GKAchievement: report with a nil identifier ignored");
        return;
    }
    let name = ns_string::to_rust_string(env, identifier).into_owned();
    let update = game_center::report_achievement(env, &name, percent);
    if update.newly_completed {
        log!(
            "Game Center (local): achievement completed: {} ({:.0}%){}",
            name,
            update.achievement.percent,
            if banner {
                " [completion banner requested]"
            } else {
                ""
            }
        );
    } else if update.improved {
        log!(
            "Game Center (local): achievement progress: {} is now {:.0}%",
            name,
            update.achievement.percent
        );
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GKAchievement: NSObject

// MARK: - Class methods (must come before instance methods in objc_classes! macro)

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GKAchievementHostObject {
        identifier: nil,
        percent_complete: 0.0,
        shows_completion_banner: false,
        player_id: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// + (void)loadAchievementsWithCompletionHandler:(void (^)(NSArray *, NSError *))handler
//
// Apple: "Retrieves all of the achievements that the local player has
// reported." We hand back what games reported to this emulator before, which
// lets in-game achievement lists work offline.
+ (())loadAchievementsWithCompletionHandler:(id)handler {
    if !is_signed_in(env) {
        let error = make_not_authenticated_error(env);
        invoke_two_object_block(env, handler, nil, error);
        return;
    }
    let achievements = game_center::achievements_for_current_app(env);
    log!(
        "GKAchievement loadAchievementsWithCompletionHandler: \
         returning {} locally stored achievement(s)",
        achievements.len()
    );
    let array: id = msg_class![env; NSMutableArray array];
    for achievement in &achievements {
        add_stored_achievement(env, array, achievement);
    }
    invoke_two_object_block(env, handler, array, nil);
}

// + (void)resetAchievementsWithCompletionHandler:(void (^)(NSError *))handler
//
// Apple: "Resets all achievement progress for the local player." Local
// progress only, as there is no server to reset.
+ (())resetAchievementsWithCompletionHandler:(id)handler {
    if !is_signed_in(env) {
        let error = make_not_authenticated_error(env);
        invoke_error_block(env, handler, error);
        return;
    }
    log!("GKAchievement resetAchievementsWithCompletionHandler: clearing local progress");
    game_center::reset_current_app(env);
    invoke_error_block(env, handler, nil);
}

// + (void)reportAchievements:(NSArray *)achievements
//       withCompletionHandler:(void (^)(NSError *))handler
+ (())reportAchievements:(id)achievements withCompletionHandler:(id)handler {
    if !is_signed_in(env) {
        let error = make_not_authenticated_error(env);
        invoke_error_block(env, handler, error);
        return;
    }
    let count: NSUInteger = if achievements == nil {
        0
    } else {
        msg![env; achievements count]
    };
    for index in 0..count {
        let object: id = msg![env; achievements objectAtIndex:index];
        let (identifier, percent, banner) = {
            let host = env.objc.borrow::<GKAchievementHostObject>(object);
            (
                host.identifier,
                host.percent_complete,
                host.shows_completion_banner,
            )
        };
        record_report(env, identifier, percent, banner);
    }
    invoke_error_block(env, handler, nil);
}

// MARK: - Instance methods

- (id)initWithIdentifier:(id)identifier {
    retain(env, identifier);
    env.objc.borrow_mut::<GKAchievementHostObject>(this).identifier = identifier;
    this
}

- (id)initWithIdentifier:(id)identifier player:(id)player {
    retain(env, identifier);
    retain(env, player);
    {
        let host = env.objc.borrow_mut::<GKAchievementHostObject>(this);
        host.identifier = identifier;
        host.player_id = player;
    }
    this
}

- (id)identifier {
    env.objc.borrow::<GKAchievementHostObject>(this).identifier
}

- (())setIdentifier:(id)identifier {
    let old = env.objc.borrow::<GKAchievementHostObject>(this).identifier;
    if old != identifier {
        retain(env, identifier);
        release(env, old);
    }
    env.objc.borrow_mut::<GKAchievementHostObject>(this).identifier = identifier;
}

- (id)playerID {
    let player_id = env.objc.borrow::<GKAchievementHostObject>(this).player_id;
    if player_id != nil {
        return player_id;
    }
    let player_id = ns_string::from_rust_string(env, game_center::PLAYER_ID.to_string());
    autorelease(env, player_id)
}

- (f64)percentComplete {
    env.objc.borrow::<GKAchievementHostObject>(this).percent_complete
}

- (())setPercentComplete:(f64)value {
    env.objc.borrow_mut::<GKAchievementHostObject>(this).percent_complete = value;
}

- (bool)showsCompletionBanner {
    env.objc.borrow::<GKAchievementHostObject>(this).shows_completion_banner
}

- (())setShowsCompletionBanner:(bool)value {
    env.objc.borrow_mut::<GKAchievementHostObject>(this).shows_completion_banner = value;
}

- (bool)isCompleted {
    env.objc.borrow::<GKAchievementHostObject>(this).percent_complete >= 100.0
}

// Apple: "The date the achievement was last reported." Filled in from the
// local store so games that sort or display achievements by date work.
- (id)lastReportedDate {
    let identifier = env.objc.borrow::<GKAchievementHostObject>(this).identifier;
    if identifier == nil {
        return nil;
    }
    let identifier = ns_string::to_rust_string(env, identifier).into_owned();
    let Some(achievement) = game_center::achievements_for_current_app(env)
        .into_iter()
        .find(|achievement| achievement.identifier == identifier)
    else {
        return nil;
    };
    if achievement.last_reported == 0 {
        return nil;
    }
    let date: id =
        msg_class![env; NSDate dateWithTimeIntervalSince1970:(achievement.last_reported as f64)];
    autorelease(env, date)
}

// - (void)reportAchievementWithCompletionHandler:(void (^)(NSError *))completionHandler
- (())reportAchievementWithCompletionHandler:(id)handler {
    if !is_signed_in(env) {
        let error = make_not_authenticated_error(env);
        invoke_error_block(env, handler, error);
        return;
    }
    let (identifier, percent, banner) = {
        let host = env.objc.borrow::<GKAchievementHostObject>(this);
        (
            host.identifier,
            host.percent_complete,
            host.shows_completion_banner,
        )
    };
    record_report(env, identifier, percent, banner);
    invoke_error_block(env, handler, nil);
}

- (())dealloc {
    let (identifier, player_id) = {
        let host = env.objc.borrow::<GKAchievementHostObject>(this);
        (host.identifier, host.player_id)
    };
    release(env, identifier);
    release(env, player_id);
    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)description {
    let ident = env.objc.borrow::<GKAchievementHostObject>(this).identifier;
    let pct = env.objc.borrow::<GKAchievementHostObject>(this).percent_complete;
    let ident_s = if ident != nil {
        ns_string::to_rust_string(env, ident).into_owned()
    } else {
        "<nil>".to_string()
    };
    let desc = format!("<GKAchievement: id={} percentComplete={}>", ident_s, pct);
    let ns = ns_string::from_rust_string(env, desc);
    autorelease(env, ns)
}

@end

};

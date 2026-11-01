/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GKLeaderboard`.
//!
//! Leaderboards are served from the local Game Center store
//! ([crate::game_center]): the best score each game reported is the only
//! entry, ranked first. That is enough for games that show "your best score"
//! or need a score object to compare against; there are no other players.
//!
//! Reference: <https://developer.apple.com/documentation/gamekit/gkleaderboard>

use super::gk_local_player::invoke_two_object_block;
use super::gk_score::make_local_score;
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSRange;
use crate::game_center;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};

struct GKLeaderboardHostObject {
    /// `NSString*` leaderboard identifier.
    identifier: id,
    /// `NSString*` localised title shown to the player.
    title: id,
    /// `NSString*` deprecated iOS 7 alias of `identifier`.
    category: id,
    /// `NSRange` of scores the caller asked for. Game Center defaults to the
    /// top 25, so this is what a freshly created leaderboard reports.
    range: NSRange,
    /// `GKLeaderboardPlayerScope`
    player_scope: i32,
    /// `GKLeaderboardTimeScope`
    time_scope: i32,
}
impl Default for GKLeaderboardHostObject {
    fn default() -> Self {
        Self {
            identifier: nil,
            title: nil,
            category: nil,
            range: NSRange {
                location: 1,
                length: 25,
            },
            player_scope: 0,
            time_scope: 0,
        }
    }
}
impl HostObject for GKLeaderboardHostObject {}

/// Build a `GKLeaderboard` for a stored leaderboard identifier, with the
/// identifier set on both the modern and the deprecated property.
fn make_leaderboard(env: &mut crate::Environment, identifier: &str) -> id {
    let identifier_string = ns_string::from_rust_string(env, identifier.to_string());
    let leaderboard: id = msg_class![env; GKLeaderboard alloc];
    let leaderboard: id = msg![env; leaderboard init];
    () = msg![env; leaderboard setIdentifier:identifier_string];
    autorelease(env, identifier_string);
    leaderboard
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GKLeaderboard: NSObject

// MARK: - Class methods (must come before instance methods in objc_classes! macro)

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<GKLeaderboardHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// + (void)loadLeaderboardsWithCompletionHandler:(void (^)(NSArray *, NSError *))handler
//
// Apple: "Loads the list of leaderboards for the current game." We can only
// offer the leaderboards this game has already reported scores to.
+ (())loadLeaderboardsWithCompletionHandler:(id)handler {
    let scores = game_center::scores_for_current_app(env);
    let array: id = msg_class![env; NSMutableArray array];
    for score in &scores {
        let leaderboard = make_leaderboard(env, &score.leaderboard);
        () = msg![env; array addObject:leaderboard];
        autorelease(env, leaderboard);
    }
    log!(
        "GKLeaderboard loadLeaderboardsWithCompletionHandler: \
         returning {} locally stored leaderboard(s)",
        scores.len()
    );
    invoke_two_object_block(env, handler, array, nil);
}

// + (void)loadCategoriesWithCompletionHandler:(void (^)(NSArray *, NSArray *, NSError *))handler
//
// Deprecated before iOS 7 but still the API early games use. The payload is
// the list of leaderboard identifiers, so it is the same local list.
+ (())loadCategoriesWithCompletionHandler:(id)handler {
    let scores = game_center::scores_for_current_app(env);
    let array: id = msg_class![env; NSMutableArray array];
    for score in &scores {
        let identifier = ns_string::from_rust_string(env, score.leaderboard.clone());
        () = msg![env; array addObject:identifier];
        release(env, identifier);
    }
    invoke_two_object_block(env, handler, array, nil);
}

// MARK: - Init / dealloc

- (id)init {
    this
}

- (id)initWithPlayerIDs:(id)_player_ids {
    this
}

- (())dealloc {
    let (identifier, title, category) = {
        let host = env.objc.borrow::<GKLeaderboardHostObject>(this);
        (host.identifier, host.title, host.category)
    };
    release(env, identifier);
    if category != identifier {
        release(env, category);
    }
    release(env, title);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Identity

- (id)identifier {
    env.objc.borrow::<GKLeaderboardHostObject>(this).identifier
}

- (())setIdentifier:(id)identifier {
    let old = env.objc.borrow::<GKLeaderboardHostObject>(this).identifier;
    if old != identifier {
        retain(env, identifier);
        release(env, old);
    }
    {
        let host = env.objc.borrow_mut::<GKLeaderboardHostObject>(this);
        host.identifier = identifier;
        // `category` is the same string on an iOS 7+ leaderboard, and games
        // written for iOS 4/5 only ever read `category`.
        host.category = identifier;
    }
}

- (id)category {
    env.objc.borrow::<GKLeaderboardHostObject>(this).category
}

- (())setCategory:(id)category {
    () = msg![env; this setIdentifier:category];
}

- (id)title {
    let title = env.objc.borrow::<GKLeaderboardHostObject>(this).title;
    if title != nil {
        return title;
    }
    // No App Store metadata is available offline, so the identifier is the
    // most useful title we can show.
    env.objc.borrow::<GKLeaderboardHostObject>(this).identifier
}

- (())setTitle:(id)title {
    let old = env.objc.borrow::<GKLeaderboardHostObject>(this).title;
    if old != title {
        retain(env, title);
        release(env, old);
    }
    env.objc.borrow_mut::<GKLeaderboardHostObject>(this).title = title;
}

- (NSRange)range {
    env.objc.borrow::<GKLeaderboardHostObject>(this).range
}

- (())setRange:(NSRange)range {
    env.objc.borrow_mut::<GKLeaderboardHostObject>(this).range = range;
}

- (i32)playerScope {
    env.objc.borrow::<GKLeaderboardHostObject>(this).player_scope
}

- (())setPlayerScope:(i32)scope {
    env.objc.borrow_mut::<GKLeaderboardHostObject>(this).player_scope = scope;
}

- (i32)timeScope {
    env.objc.borrow::<GKLeaderboardHostObject>(this).time_scope
}

- (())setTimeScope:(i32)scope {
    env.objc.borrow_mut::<GKLeaderboardHostObject>(this).time_scope = scope;
}

// MARK: - Scores

// Apple: "The score for the local player." nil when the player never
// reported a score to this leaderboard.
- (id)localPlayerScore {
    let identifier = env.objc.borrow::<GKLeaderboardHostObject>(this).identifier;
    let scores = game_center::scores_for_current_app(env);
    let score = if identifier == nil {
        scores.first()
    } else {
        let identifier = ns_string::to_rust_string(env, identifier).into_owned();
        scores
            .iter()
            .find(|score| score.leaderboard == identifier)
    };
    let Some(score) = score else {
        return nil;
    };
    let object = make_local_score(env, score, 1);
    autorelease(env, object)
}

// - (void)loadScoresWithCompletionHandler:(void (^)(NSArray *, NSError *))handler
//
// Apple: "The scores are sorted from the highest to the lowest." With no
// other players, the local player's best score is the whole leaderboard.
- (())loadScoresWithCompletionHandler:(id)handler {
    let identifier = env.objc.borrow::<GKLeaderboardHostObject>(this).identifier;
    let identifier = if identifier == nil {
        None
    } else {
        Some(ns_string::to_rust_string(env, identifier).into_owned())
    };
    let array: id = msg_class![env; NSMutableArray array];
    for (index, score) in game_center::scores_for_current_app(env).iter().enumerate() {
        if let Some(identifier) = &identifier {
            if &score.leaderboard != identifier {
                continue;
            }
        }
        let object = make_local_score(env, score, (index + 1) as i64);
        () = msg![env; array addObject:object];
        autorelease(env, object);
    }
    invoke_two_object_block(env, handler, array, nil);
}

@end

};

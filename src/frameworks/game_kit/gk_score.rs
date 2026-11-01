/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `GKScore`.

use super::gk_local_player::{invoke_error_block, is_signed_in, make_not_authenticated_error};
use crate::frameworks::foundation::ns_string;
use crate::game_center;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use std::borrow::Cow;

type GKLeaderboardTimeScope = i32;
type GKLeaderboardPlayerScope = i32;

#[derive(Default)]
struct GKScoreHostObject {
    category: id,
    leaderboard_identifier: id,
    player_id: id,
    formatted_value: id,
    value: i64,
    rank: i64,
    context: u64,
    date: id,
    should_set_default_leaderboard: bool,
}
impl HostObject for GKScoreHostObject {}

/// Build a `GKScore` describing an entry from the local Game Center store.
///
/// The returned object is retained: the caller takes ownership and must
/// autorelease or release it. Used by `GKLeaderboard` to hand back scores it
/// never received from the network.
pub(super) fn make_local_score(
    env: &mut crate::Environment,
    score: &game_center::Score,
    rank: i64,
) -> id {
    let leaderboard = ns_string::from_rust_string(env, score.leaderboard.clone());
    let object: id = msg_class![env; GKScore alloc];
    let object: id = msg![env; object initWithCategory:leaderboard];
    release(env, leaderboard);

    let formatted = ns_string::from_rust_string(env, score.formatted.clone());
    let date = msg_class![env; NSDate dateWithTimeIntervalSince1970:(score.last_reported as f64)];
    let date = retain(env, date);
    let player_id = ns_string::from_rust_string(env, game_center::PLAYER_ID.to_string());

    let old_formatted = env.objc.borrow::<GKScoreHostObject>(object).formatted_value;
    release(env, old_formatted);
    {
        let host = env.objc.borrow_mut::<GKScoreHostObject>(object);
        host.value = score.value;
        host.formatted_value = formatted;
        host.rank = rank;
        host.date = date;
        host.player_id = player_id;
    }
    object
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GKScore: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GKScoreHostObject {
        category: nil,
        leaderboard_identifier: nil,
        player_id: nil,
        formatted_value: nil,
        value: 0,
        rank: 0,
        context: 0,
        date: nil,
        should_set_default_leaderboard: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithCategory:(id)category {
    // One retain for both properties: `dealloc` only releases the second one
    // when it differs from the first.
    retain(env, category);
    {
        let host = env.objc.borrow_mut::<GKScoreHostObject>(this);
        host.category = category;
        host.leaderboard_identifier = category;
    }

    let fmt = ns_string::from_rust_string(env, "0".to_string());
    env.objc.borrow_mut::<GKScoreHostObject>(this).formatted_value = fmt;
    this
}

- (id)initWithLeaderboardIdentifier:(id)identifier {
    msg![env; this initWithCategory:identifier]
}

- (id)initWithLeaderboardIdentifier:(id)identifier player:(id)player {
    let this: id = msg![env; this initWithLeaderboardIdentifier:identifier];
    retain(env, player);
    env.objc.borrow_mut::<GKScoreHostObject>(this).player_id = player;
    this
}

- (())dealloc {
    let host = env.objc.borrow::<GKScoreHostObject>(this);

    let (category, leaderboard_identifier, player_id, formatted_value, date) = (
        host.category,
        host.leaderboard_identifier,
        host.player_id,
        host.formatted_value,
        host.date,
    );

    release(env, category);
    if leaderboard_identifier != category {
        release(env, leaderboard_identifier);
    }
    release(env, player_id);
    release(env, formatted_value);
    release(env, date);

    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)category {
    env.objc.borrow::<GKScoreHostObject>(this).category
}

- (())setCategory:(id)category {
    let old = env.objc.borrow::<GKScoreHostObject>(this).category;
    release(env, old);

    retain(env, category);
    env.objc.borrow_mut::<GKScoreHostObject>(this).category = category;
}

- (id)leaderboardIdentifier {
    env.objc.borrow::<GKScoreHostObject>(this).leaderboard_identifier
}

- (())setLeaderboardIdentifier:(id)identifier {
    let old = env.objc.borrow::<GKScoreHostObject>(this).leaderboard_identifier;

    release(env, old);
    retain(env, identifier);
    env.objc.borrow_mut::<GKScoreHostObject>(this).leaderboard_identifier = identifier;
}

- (i64)value {
    env.objc.borrow::<GKScoreHostObject>(this).value
}

- (())setValue:(i64)value {
    env.objc.borrow_mut::<GKScoreHostObject>(this).value = value;

    let old_fmt = env.objc.borrow::<GKScoreHostObject>(this).formatted_value;
    release(env, old_fmt);
    let fmt = ns_string::from_rust_string(env, value.to_string());
    env.objc.borrow_mut::<GKScoreHostObject>(this).formatted_value = fmt;
}

- (i64)rank {
    env.objc.borrow::<GKScoreHostObject>(this).rank
}

- (u64)context {
    env.objc.borrow::<GKScoreHostObject>(this).context
}

- (())setContext:(u64)context {
    env.objc.borrow_mut::<GKScoreHostObject>(this).context = context;
}

- (id)formattedValue {
    env.objc.borrow::<GKScoreHostObject>(this).formatted_value
}

- (id)playerID {
    let player_id = env.objc.borrow::<GKScoreHostObject>(this).player_id;
    if player_id != nil {
        return player_id;
    }
    // A score a game built itself has no player attached, but it is always
    // the local player's score, so report the local player.
    let player_id = ns_string::from_rust_string(env, game_center::PLAYER_ID.to_string());
    autorelease(env, player_id)
}

- (id)player {
    env.objc.borrow::<GKScoreHostObject>(this).player_id
}

- (id)date {
    env.objc.borrow::<GKScoreHostObject>(this).date
}

- (bool)shouldSetDefaultLeaderboard {
    env.objc.borrow::<GKScoreHostObject>(this).should_set_default_leaderboard
}

- (())setShouldSetDefaultLeaderboard:(bool)value {
    env.objc.borrow_mut::<GKScoreHostObject>(this).should_set_default_leaderboard = value;
}

// Apple reference (iOS 4.1, deprecated iOS 7):
// <https://developer.apple.com/documentation/gamekit/gkscore/1621741-reportscorewithcompletionhandler>
// "Sends a score to Game Center. [...] When the score is sent, the block is
//  called." Scores go to the local Game Center store ([crate::game_center]);
// the best value per leaderboard is kept, like the server would.
- (())reportScoreWithCompletionHandler:(id)completion_handler {
    if !is_signed_in(env) {
        let error = make_not_authenticated_error(env);
        invoke_error_block(env, completion_handler, error);
        return;
    }
    let (value, formatted, leaderboard) = {
        let host = env.objc.borrow::<GKScoreHostObject>(this);
        (
            host.value,
            host.formatted_value,
            if host.leaderboard_identifier != nil {
                host.leaderboard_identifier
            } else {
                host.category
            },
        )
    };

    let formatted = if formatted != nil {
        ns_string::to_rust_string(env, formatted).into_owned()
    } else {
        value.to_string()
    };

    if leaderboard == nil {
        log!(
            "GKScore reportScoreWithCompletionHandler: no leaderboard identifier set (value={})",
            value
        );
        invoke_error_block(env, completion_handler, nil);
        return;
    }

    let leaderboard = ns_string::to_rust_string(env, leaderboard).into_owned();
    let update = game_center::report_score(env, &leaderboard, value, &formatted);
    log!(
        "Game Center (local): score reported: {} = {}{}",
        leaderboard,
        formatted,
        if update.new_best { " (new best)" } else { "" }
    );
    invoke_error_block(env, completion_handler, nil);
}

- (id)description {
    let value = env.objc.borrow::<GKScoreHostObject>(this).value;
    let cat   = env.objc.borrow::<GKScoreHostObject>(this).category;

    let cat_str: Cow<str> = if cat != nil {
        ns_string::to_rust_string(env, cat)
    } else {
        Cow::Borrowed("<nil>")
    };

    let desc = format!("<GKScore: category={} value={}>", cat_str, value);
    let ns = ns_string::from_rust_string(env, desc);
    autorelease(env, ns)
}

@end

};

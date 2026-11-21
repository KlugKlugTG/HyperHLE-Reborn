/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GKLeaderboard`.
//!
//! Game Center is not available on the emulated device, so a leaderboard is
//! a value holder: the properties read back what the app set, and every
//! load fails with `GKErrorNotAuthenticated`. Completion handlers are called
//! synchronously, before the method returns (real GameKit calls them later).

use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str, to_rust_string};
use crate::frameworks::foundation::{NSInteger, NSRange, NSTimeInterval, NSUInteger};
use crate::mem::ConstPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

/// `GKLeaderboardPlayerScope`
pub type GKLeaderboardPlayerScope = NSInteger;
pub const GKLeaderboardPlayerScopeGlobal: GKLeaderboardPlayerScope = 0;
pub const GKLeaderboardPlayerScopeFriendsOnly: GKLeaderboardPlayerScope = 1;

/// `GKLeaderboardTimeScope`
pub type GKLeaderboardTimeScope = NSInteger;
pub const GKLeaderboardTimeScopeToday: GKLeaderboardTimeScope = 0;
pub const GKLeaderboardTimeScopeWeek: GKLeaderboardTimeScope = 1;
pub const GKLeaderboardTimeScopeAllTime: GKLeaderboardTimeScope = 2;

/// `GKErrorNotAuthenticated` in `GKErrorDomain`.
const GK_ERROR_NOT_AUTHENTICATED: NSInteger = 6;

struct GKLeaderboardHostObject {
    player_scope: GKLeaderboardPlayerScope,
    time_scope: GKLeaderboardTimeScope,
    /// `NSString *`, copied. Also what `category` reads and writes.
    identifier: id,
    /// `NSString *`, retained. Never filled in: there is no server.
    title: id,
    range: NSRange,
    /// `NSArray *` of `GKScore *`, retained. Never filled in.
    scores: id,
    /// `GKScore *`, retained. Never filled in.
    local_player_score: id,
    /// `NSString *`, retained. Never filled in.
    group_identifier: id,
    /// `NSArray *` passed to `initWithPlayerIDs:` / `initWithPlayers:`, retained.
    #[allow(dead_code)] // Only a real server query would use it.
    players: id,
}
impl HostObject for GKLeaderboardHostObject {}

/// Retain-copy `val` into the field picked by `field`, releasing the old
/// value. No borrow is held across `copy` or `release`.
fn set_copy<F>(env: &mut Environment, this: id, field: F, val: id)
where
    F: FnOnce(&mut GKLeaderboardHostObject) -> &mut id,
{
    // `copy` returns +1, which the host object keeps. nil stays nil.
    let copied: id = if val == nil { nil } else { msg![env; val copy] };
    let old = std::mem::replace(
        field(env.objc.borrow_mut::<GKLeaderboardHostObject>(this)),
        copied,
    );
    release(env, old);
}

/// Store a retained, copied array of players.
fn set_players(env: &mut Environment, this: id, players: id) {
    set_copy(env, this, |h| &mut h.players, players);
}

// MARK: - Blocks

/// The invoke function of an Objective-C block (ARM32 layout:
/// isa, flags, reserved, invoke, ...). `None` for nil or a NULL invoke.
fn block_invoke(env: &mut Environment, block: id) -> Option<GuestFunction> {
    if block == nil {
        return None;
    }
    let invoke_addr: u32 = env.mem.read(ConstPtr::<u32>::from_bits(block.to_bits() + 12));
    if invoke_addr == 0 {
        log!("GKLeaderboard: block {:?} has a NULL invoke pointer, ignoring", block);
        return None;
    }
    Some(GuestFunction::from_addr_with_thumb_bit(invoke_addr))
}

/// Calls a block taking two object arguments, e.g. `void (^)(NSArray *, NSError *)`.
fn call_block_2(env: &mut Environment, block: id, a: id, b: id) {
    if let Some(invoke) = block_invoke(env, block) {
        let _: () = invoke.call_from_host(env, (block, a, b));
    }
}

/// Calls a block taking three object arguments.
fn call_block_3(env: &mut Environment, block: id, a: id, b: id, c: id) {
    if let Some(invoke) = block_invoke(env, block) {
        let _: () = invoke.call_from_host(env, (block, a, b, c));
    }
}

/// An autoreleased `GKErrorNotAuthenticated` error: the answer to every
/// request on a device with no Game Center account.
fn not_authenticated_error(env: &mut Environment) -> id {
    let domain = get_static_str(env, "GKErrorDomain");
    let code: NSInteger = GK_ERROR_NOT_AUTHENTICATED;
    msg_class![env; NSError errorWithDomain:domain code:code userInfo:nil]
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GKLeaderboard: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::new(GKLeaderboardHostObject {
        player_scope: GKLeaderboardPlayerScopeGlobal,
        time_scope: GKLeaderboardTimeScopeAllTime,
        identifier: nil,
        title: nil,
        range: NSRange { location: 1, length: 25 },
        scores: nil,
        local_player_score: nil,
        group_identifier: nil,
        players: nil,
    });
    env.objc.alloc_object(this, host, &mut env.mem)
}

+ (())loadLeaderboardsWithCompletionHandler:(id)handler {
    log!("GKLeaderboard loadLeaderboards: Game Center not available, failing");
    let error = not_authenticated_error(env);
    call_block_2(env, handler, nil, error);
}

// iOS 10+
+ (())loadLeaderboardsWithIDs:(id)_ids completionHandler:(id)handler {
    let error = not_authenticated_error(env);
    call_block_2(env, handler, nil, error);
}

// iOS 4-6: (categories, titles, error)
+ (())loadCategoriesWithCompletionHandler:(id)handler {
    let error = not_authenticated_error(env);
    call_block_3(env, handler, nil, nil, error);
}
    
// MARK: - Initialisers

- (id)init {
    // Used to return nil, which left apps with no leaderboard object at all.
    this
}

// iOS 6+: scores for these players only.
- (id)initWithPlayerIDs:(id)player_ids {
    set_players(env, this, player_ids);
    // Real GameKit treats a player list as a friends-only query.
    env.objc.borrow_mut::<GKLeaderboardHostObject>(this).player_scope =
        GKLeaderboardPlayerScopeFriendsOnly;
    this
}

// iOS 8+: same, with `GKPlayer` objects.
- (id)initWithPlayers:(id)players {
    set_players(env, this, players);
    env.objc.borrow_mut::<GKLeaderboardHostObject>(this).player_scope =
        GKLeaderboardPlayerScopeFriendsOnly;
    this
}

- (())dealloc {
    let objects = {
        let h = env.objc.borrow::<GKLeaderboardHostObject>(this);
        [
            h.identifier,
            h.title,
            h.scores,
            h.local_player_score,
            h.group_identifier,
            h.players,
        ]
    };
    for obj in objects {
        release(env, obj);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Query configuration

- (NSInteger)playerScope {
    env.objc.borrow::<GKLeaderboardHostObject>(this).player_scope
}
- (())setPlayerScope:(NSInteger)scope {
    env.objc.borrow_mut::<GKLeaderboardHostObject>(this).player_scope = scope;
}

- (NSInteger)timeScope {
    env.objc.borrow::<GKLeaderboardHostObject>(this).time_scope
}
- (())setTimeScope:(NSInteger)scope {
    env.objc.borrow_mut::<GKLeaderboardHostObject>(this).time_scope = scope;
}

// `identifier` (iOS 7+) and `category` (iOS 4-6) are the same property.
- (id)identifier {
    env.objc.borrow::<GKLeaderboardHostObject>(this).identifier
}
- (())setIdentifier:(id)identifier {
    set_copy(env, this, |h| &mut h.identifier, identifier);
}
- (id)category {
    env.objc.borrow::<GKLeaderboardHostObject>(this).identifier
}
- (())setCategory:(id)category {
    set_copy(env, this, |h| &mut h.identifier, category);
}

- (NSRange)range {
    env.objc.borrow::<GKLeaderboardHostObject>(this).range
}
- (())setRange:(NSRange)range {
    env.objc.borrow_mut::<GKLeaderboardHostObject>(this).range = range;
}

// MARK: - Results (always empty: there is no Game Center)

- (id)title {
    env.objc.borrow::<GKLeaderboardHostObject>(this).title
}

- (id)scores {
    env.objc.borrow::<GKLeaderboardHostObject>(this).scores
}

- (id)localPlayerScore {
    env.objc.borrow::<GKLeaderboardHostObject>(this).local_player_score
}

- (NSUInteger)maxRange {
    0
}

// Loads complete before the method returns, so this is never true.
- (bool)isLoading {
    false
}

- (id)groupIdentifier {
    env.objc.borrow::<GKLeaderboardHostObject>(this).group_identifier
}

// iOS 7+ recurring leaderboards.
- (id)startDate {
    nil
}
- (id)nextStartDate {
    nil
}
- (NSTimeInterval)duration {
    0.0
}

// MARK: - Loading instance data

- (())loadScoresWithCompletionHandler:(id)handler {
    log!("GKLeaderboard {:?} loadScores: Game Center not available, failing", this);
    let error = not_authenticated_error(env);
    call_block_2(env, handler, nil, error);
}

- (())loadImageWithCompletionHandler:(id)handler {
    let error = not_authenticated_error(env);
    call_block_2(env, handler, nil, error);
}

- (())loadPreviousOccurrenceWithCompletionHandler:(id)handler {
    let error = not_authenticated_error(env);
    call_block_2(env, handler, nil, error);
}

- (id)description {
    let (identifier, player_scope, time_scope) = {
        let h = env.objc.borrow::<GKLeaderboardHostObject>(this);
        (h.identifier, h.player_scope, h.time_scope)
    };
    let identifier = if identifier == nil {
        "(null)".to_string()
    } else {
        to_rust_string(env, identifier).into_owned()
    };
    let range = env.objc.borrow::<GKLeaderboardHostObject>(this).range;
    // Copy out of the (possibly packed) NSRange before formatting.
    let (location, length) = (range.location, range.length);
    let s = format!(
        "<GKLeaderboard: {:?}; identifier = {}; playerScope = {}; timeScope = {}; range = {{{}, {}}}>",
        this, identifier, player_scope, time_scope, location, length
    );
    let ns = from_rust_string(env, s);
    autorelease(env, ns)
}

@end

};

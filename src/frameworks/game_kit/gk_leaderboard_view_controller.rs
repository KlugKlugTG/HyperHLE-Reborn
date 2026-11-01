/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `GKLeaderboardViewController` and `GKAchievementViewController`.

use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_application::UIInterfaceOrientation;
use crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, nil, objc_classes, release, retain, ClassExports,
    NSZonePtr,
};
use crate::Environment;

type GKLeaderboardTimeScope = NSInteger;
type GKLeaderboardPlayerScope = NSInteger;

// GKLeaderboardTimeScope values
const GKLeaderboardTimeScopeToday: GKLeaderboardTimeScope = 0;
const GKLeaderboardTimeScopeWeek: GKLeaderboardTimeScope = 1;
const GKLeaderboardTimeScopeAllTime: GKLeaderboardTimeScope = 2;

// GKLeaderboardPlayerScope values
const GKLeaderboardPlayerScopeGlobal: GKLeaderboardPlayerScope = 0;
const GKLeaderboardPlayerScopeFriendsOnly: GKLeaderboardPlayerScope = 1;

// MARK: - GKLeaderboardViewController

#[derive(Default)]
struct GKLeaderboardViewControllerHostObject {
    superclass: UIViewControllerHostObject,
    /// GKLeaderboardViewControllerDelegate — weak reference
    leaderboard_delegate: id,
    /// NSString* — category filter
    category: id,
    time_scope: GKLeaderboardTimeScope,
    /// Set once the delegate has been told the user is finished, so the
    /// dismiss callback is delivered exactly once per presentation.
    did_finish_delivered: bool,
}
impl_HostObject_with_superclass!(GKLeaderboardViewControllerHostObject);

// MARK: - GKFriendRequestComposeViewController

#[derive(Default)]
struct GKFriendRequestComposeViewControllerHostObject {
    superclass: UIViewControllerHostObject,
    /// GKFriendRequestComposeViewControllerDelegate — weak reference
    compose_view_delegate: id,
    /// NSString*
    message: id,
    /// NSInteger
    max_recipients: NSInteger,
    /// See `GKLeaderboardViewControllerHostObject::did_finish_delivered`.
    did_finish_delivered: bool,
}
impl_HostObject_with_superclass!(GKFriendRequestComposeViewControllerHostObject);

// MARK: - GKAchievementViewController

#[derive(Default)]
struct GKAchievementViewControllerHostObject {
    superclass: UIViewControllerHostObject,
    /// GKAchievementViewControllerDelegate — weak reference
    achievement_delegate: id,
    /// See `GKLeaderboardViewControllerHostObject::did_finish_delivered`.
    did_finish_delivered: bool,
}
impl_HostObject_with_superclass!(GKAchievementViewControllerHostObject);

/// Host selector used to notify each controller's delegate on the next
/// run-loop turn. All three controllers take no arguments and share it.
const DID_FINISH_SELECTOR: &str = "_touchHLE_deliverDidFinish";

/// Ask `controller` to notify its delegate on the next run-loop turn, unless
/// that has already been requested.
fn schedule_did_finish(env: &mut Environment, controller: id, selector: &str) {
    let sel = env
        .objc
        .register_host_selector(selector.to_string(), &mut env.mem);
    () = msg![env; controller performSelector:sel withObject:nil afterDelay:0.0_f64];
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// GKLeaderboardViewController
// =========================================================================

@implementation GKLeaderboardViewController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GKLeaderboardViewControllerHostObject {
        time_scope: GKLeaderboardTimeScopeAllTime,
        ..Default::default()
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (())dealloc {
    let host = env.objc.borrow::<GKLeaderboardViewControllerHostObject>(this);
    let (delegate, category) = (host.leaderboard_delegate, host.category);
    release(env, category);
    // delegate is weak — no release
    let _ = delegate;
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Delegate

- (id)leaderboardDelegate {
    env.objc.borrow::<GKLeaderboardViewControllerHostObject>(this).leaderboard_delegate
}

- (())setLeaderboardDelegate:(id)delegate {
    // Weak reference — no retain.
    env.objc.borrow_mut::<GKLeaderboardViewControllerHostObject>(this)
        .leaderboard_delegate = delegate;
}

// MARK: Category

- (id)category {
    env.objc.borrow::<GKLeaderboardViewControllerHostObject>(this).category
}

- (())setCategory:(id)category {
    let old = env.objc.borrow::<GKLeaderboardViewControllerHostObject>(this).category;
    release(env, old);
    retain(env, category);
    env.objc.borrow_mut::<GKLeaderboardViewControllerHostObject>(this).category = category;
}

// MARK: Time scope

- (GKLeaderboardTimeScope)timeScope {
    env.objc.borrow::<GKLeaderboardViewControllerHostObject>(this).time_scope
}

- (())setTimeScope:(GKLeaderboardTimeScope)scope {
    env.objc.borrow_mut::<GKLeaderboardViewControllerHostObject>(this).time_scope = scope;
}

// MARK: Presentation stubs

- (())viewDidLoad {
    log!("GKLeaderboardViewController viewDidLoad: stubbed (UI not shown)");
}

- (())viewWillAppear:(bool)_animated {
    log!("GKLeaderboardViewController viewWillAppear: stubbed");
    // There is no leaderboard UI to interact with, so the delegate is told
    // straight away that the user is done, exactly like tapping Done on a
    // device. That notification is deferred by one run-loop turn rather than
    // sent from here: on a device the delegate runs after the presentation
    // has completed, and games use it to restore the status bar, resume
    // their director and dismiss this controller. Doing that from inside
    // `-presentModalViewController:animated:` left the game in a state it
    // never sees on a device (this is the call that used to flip the
    // screen when a game opened Game Center).
    schedule_did_finish(env, this, DID_FINISH_SELECTOR);
}

- (())viewDidAppear:(bool)_animated {
    // Nothing to do: `schedule_did_finish` handles the dismissal.
}

- (())_touchHLE_deliverDidFinish {
    let already_delivered = env
        .objc
        .borrow_mut::<GKLeaderboardViewControllerHostObject>(this)
        .did_finish_delivered;
    if already_delivered {
        return;
    }
    env.objc
        .borrow_mut::<GKLeaderboardViewControllerHostObject>(this)
        .did_finish_delivered = true;
    let delegate = env
        .objc
        .borrow::<GKLeaderboardViewControllerHostObject>(this)
        .leaderboard_delegate;
    if delegate == nil {
        return;
    }
    let sel = env.objc.register_host_selector(
        "leaderboardViewControllerDidFinish:".to_string(),
        &mut env.mem,
    );
    let responds: bool = msg![env; delegate respondsToSelector:sel];
    if responds {
        log!("GKLeaderboardViewController: telling the delegate the user is done");
        () = msg![env; delegate leaderboardViewControllerDidFinish:this];
    }
}

// GameKit's real view controllers present their own chrome and handle
// rotation themselves. This stand-in shows nothing and exists only to run the
// delegate callback, so it must not take part in the interface rotation that
// `UIWindow` applies when a view controller is attached to it.
- (bool)shouldAutorotateToInterfaceOrientation:(UIInterfaceOrientation)_orientation {
    false
}

- (bool)shouldAutorotate {
    false
}

- (())presentModalViewController:(id)_vc animated:(bool)_animated {
    log!("GKLeaderboardViewController presentModalViewController: stubbed");
}

- (())dismissModalViewControllerAnimated:(bool)_animated {
    log!("GKLeaderboardViewController dismissModalViewControllerAnimated: stubbed");
}

@end

// =========================================================================
// GKFriendRequestComposeViewController
// =========================================================================

@implementation GKFriendRequestComposeViewController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<GKFriendRequestComposeViewControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (())dealloc {
    let host = env.objc.borrow::<GKFriendRequestComposeViewControllerHostObject>(this);
    let message = host.message;
    release(env, message);
    // delegate is weak — no release
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Delegate

- (id)composeViewDelegate {
    env.objc.borrow::<GKFriendRequestComposeViewControllerHostObject>(this).compose_view_delegate
}

- (())setComposeViewDelegate:(id)delegate {
    // Weak reference — no retain.
    env.objc.borrow_mut::<GKFriendRequestComposeViewControllerHostObject>(this)
        .compose_view_delegate = delegate;
}

// MARK: Properties

- (id)message {
    env.objc.borrow::<GKFriendRequestComposeViewControllerHostObject>(this).message
}

- (())setMessage:(id)message {
    let old = env.objc.borrow::<GKFriendRequestComposeViewControllerHostObject>(this).message;
    release(env, old);
    retain(env, message);
    env.objc.borrow_mut::<GKFriendRequestComposeViewControllerHostObject>(this).message = message;
}

- (NSInteger)maxRecipients {
    env.objc.borrow::<GKFriendRequestComposeViewControllerHostObject>(this).max_recipients
}

- (())setMaxRecipients:(NSInteger)max_recipients {
    env.objc.borrow_mut::<GKFriendRequestComposeViewControllerHostObject>(this).max_recipients = max_recipients;
}

// MARK: Add Recipients

- (())addRecipientsWithPlayerIDs:(id)_playerIDs {
    // В эмуляторе реальная отправка не происходит, поэтому мы просто принимаем
    // данные
}

- (())addRecipientsWithEmailAddresses:(id)_emailAddresses {
}

- (())addRecipientWithPlayerID:(id)_playerID {
}

// MARK: Presentation & Lifecycle

- (())viewDidLoad {
    log!("GKFriendRequestComposeViewController viewDidLoad: stubbed (UI not shown)");
}

- (())viewWillAppear:(bool)_animated {
    log!("GKFriendRequestComposeViewController viewWillAppear: stubbed");
    // Close the compose sheet immediately so the game does not hang, but do
    // it on the next run-loop turn, like a device does (see the leaderboard
    // controller above).
    schedule_did_finish(env, this, DID_FINISH_SELECTOR);
}

- (())viewDidAppear:(bool)_animated {
    // Nothing to do: `schedule_did_finish` handles the dismissal.
}

- (())_touchHLE_deliverDidFinish {
    let already_delivered = env
        .objc
        .borrow_mut::<GKFriendRequestComposeViewControllerHostObject>(this)
        .did_finish_delivered;
    if already_delivered {
        return;
    }
    env.objc
        .borrow_mut::<GKFriendRequestComposeViewControllerHostObject>(this)
        .did_finish_delivered = true;
    let delegate = env
        .objc
        .borrow::<GKFriendRequestComposeViewControllerHostObject>(this)
        .compose_view_delegate;
    if delegate == nil {
        return;
    }
    let sel = env.objc.register_host_selector(
        "friendRequestComposeViewControllerDidFinish:".to_string(),
        &mut env.mem,
    );
    let responds: bool = msg![env; delegate respondsToSelector:sel];
    if responds {
        () = msg![env; delegate friendRequestComposeViewControllerDidFinish:this];
    }
}

- (bool)shouldAutorotateToInterfaceOrientation:(UIInterfaceOrientation)_orientation {
    false
}

- (bool)shouldAutorotate {
    false
}

- (())presentModalViewController:(id)_vc animated:(bool)_animated {
    log!("GKFriendRequestComposeViewController presentModalViewController: stubbed");
}

- (())dismissModalViewControllerAnimated:(bool)_animated {
    log!("GKFriendRequestComposeViewController dismissModalViewControllerAnimated: stubbed");
}

@end

// =========================================================================
// GKAchievementViewController
// =========================================================================

@implementation GKAchievementViewController: UIViewController

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<GKAchievementViewControllerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (())dealloc {
    // delegate is weak — no release
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Delegate

- (id)achievementDelegate {
    env.objc.borrow::<GKAchievementViewControllerHostObject>(this).achievement_delegate
}

- (())setAchievementDelegate:(id)delegate {
    // Weak reference — no retain.
    env.objc.borrow_mut::<GKAchievementViewControllerHostObject>(this)
        .achievement_delegate = delegate;
}

// MARK: Presentation stubs

- (())viewDidLoad {
    log!("GKAchievementViewController viewDidLoad: stubbed (UI not shown)");
}

- (())viewWillAppear:(bool)_animated {
    log!("GKAchievementViewController viewWillAppear: stubbed");
    // Dismissed on the next run-loop turn, like a device (see the
    // leaderboard controller above).
    schedule_did_finish(env, this, DID_FINISH_SELECTOR);
}

- (())viewDidAppear:(bool)_animated {
    // Nothing to do: `schedule_did_finish` handles the dismissal.
}

- (())_touchHLE_deliverDidFinish {
    let already_delivered = env
        .objc
        .borrow_mut::<GKAchievementViewControllerHostObject>(this)
        .did_finish_delivered;
    if already_delivered {
        return;
    }
    env.objc
        .borrow_mut::<GKAchievementViewControllerHostObject>(this)
        .did_finish_delivered = true;
    let delegate = env
        .objc
        .borrow::<GKAchievementViewControllerHostObject>(this)
        .achievement_delegate;
    if delegate == nil {
        return;
    }
    let sel = env.objc.register_host_selector(
        "achievementViewControllerDidFinish:".to_string(),
        &mut env.mem,
    );
    let responds: bool = msg![env; delegate respondsToSelector:sel];
    if responds {
        () = msg![env; delegate achievementViewControllerDidFinish:this];
    }
}

- (bool)shouldAutorotateToInterfaceOrientation:(UIInterfaceOrientation)_orientation {
    false
}

- (bool)shouldAutorotate {
    false
}

- (())presentModalViewController:(id)_vc animated:(bool)_animated {
    log!("GKAchievementViewController presentModalViewController: stubbed");
}

- (())dismissModalViewControllerAnimated:(bool)_animated {
    log!("GKAchievementViewController dismissModalViewControllerAnimated: stubbed");
}

@end

};

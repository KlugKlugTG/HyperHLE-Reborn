/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `GKLeaderboardViewController`, `GKFriendRequestComposeViewController` and
//! `GKAchievementViewController`.
//!
//! None of these show any UI. Each reports "finished" to its delegate as soon
//! as it would appear, so the app doesn't wait for user interaction.

use super::gk_leaderboard::{GKLeaderboardTimeScope, GKLeaderboardTimeScopeAllTime};
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_view_controller::UIViewControllerHostObject;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_super, nil, objc_classes, release, ClassExports,
    NSZonePtr,
};
use crate::Environment;

fn responds_to(env: &mut Environment, obj: id, selector: &str) -> bool {
    match env.objc.lookup_selector(selector) {
        Some(sel) => {
            let responds: bool = msg![env; obj respondsToSelector:sel];
            responds
        }
        None => false,
    }
}

// MARK: - Host objects

#[derive(Default)]
struct GKLeaderboardViewControllerHostObject {
    superclass: UIViewControllerHostObject,
    /// GKLeaderboardViewControllerDelegate — weak reference
    leaderboard_delegate: id,
    /// NSString*, copied. Also what `leaderboardIdentifier` reads and writes.
    category: id,
    time_scope: GKLeaderboardTimeScope,
}
impl_HostObject_with_superclass!(GKLeaderboardViewControllerHostObject);

#[derive(Default)]
struct GKFriendRequestComposeViewControllerHostObject {
    superclass: UIViewControllerHostObject,
    /// GKFriendRequestComposeViewControllerDelegate — weak reference
    compose_view_delegate: id,
    /// NSString*, copied.
    message: id,
    max_recipients: NSInteger,
}
impl_HostObject_with_superclass!(GKFriendRequestComposeViewControllerHostObject);

#[derive(Default)]
struct GKAchievementViewControllerHostObject {
    superclass: UIViewControllerHostObject,
    /// GKAchievementViewControllerDelegate — weak reference
    achievement_delegate: id,
}
impl_HostObject_with_superclass!(GKAchievementViewControllerHostObject);

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
    // The delegate is weak, so only the category is released.
    let category = env.objc.borrow::<GKLeaderboardViewControllerHostObject>(this).category;
    release(env, category);
    msg_super![env; this dealloc]
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
    // `copy` returns +1, which the host object keeps. nil stays nil. The new
    // value exists before the old one is released.
    let copied: id = if category == nil { nil } else { msg![env; category copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<GKLeaderboardViewControllerHostObject>(this).category,
        copied,
    );
    release(env, old);
}

// iOS 7 name for the same property.
- (id)leaderboardIdentifier {
    env.objc.borrow::<GKLeaderboardViewControllerHostObject>(this).category
}

- (())setLeaderboardIdentifier:(id)identifier {
    let copied: id = if identifier == nil { nil } else { msg![env; identifier copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<GKLeaderboardViewControllerHostObject>(this).category,
        copied,
    );
    release(env, old);
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
    // Immediately dismiss so the app doesn't hang waiting for user interaction.
    let delegate = env.objc
        .borrow::<GKLeaderboardViewControllerHostObject>(this)
        .leaderboard_delegate;

    if delegate != nil && responds_to(env, delegate, "leaderboardViewControllerDidFinish:") {
        () = msg![env; delegate leaderboardViewControllerDidFinish:this];
    }
}

- (())viewDidAppear:(bool)_animated {
    // Already dismissed in viewWillAppear — nothing to do.
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
    // The delegate is weak, so only the message is released.
    let message = env.objc
        .borrow::<GKFriendRequestComposeViewControllerHostObject>(this)
        .message;
    release(env, message);
    msg_super![env; this dealloc]
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
    let copied: id = if message == nil { nil } else { msg![env; message copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<GKFriendRequestComposeViewControllerHostObject>(this).message,
        copied,
    );
    release(env, old);
}

- (NSInteger)maxRecipients {
    env.objc.borrow::<GKFriendRequestComposeViewControllerHostObject>(this).max_recipients
}

- (())setMaxRecipients:(NSInteger)max_recipients {
    env.objc.borrow_mut::<GKFriendRequestComposeViewControllerHostObject>(this).max_recipients = max_recipients;
}

// MARK: Add Recipients

- (())addRecipientsWithPlayerIDs:(id)_playerIDs {
    // Nothing is ever sent in the emulator, so the data is just accepted.
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

    // "Close" right after "opening" so the game doesn't hang.
    let delegate = env.objc
        .borrow::<GKFriendRequestComposeViewControllerHostObject>(this)
        .compose_view_delegate;

    if delegate != nil
        && responds_to(env, delegate, "friendRequestComposeViewControllerDidFinish:")
    {
        () = msg![env; delegate friendRequestComposeViewControllerDidFinish:this];
    }
}

- (())viewDidAppear:(bool)_animated {
    // Already "closed" in viewWillAppear — nothing to do.
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
    // The delegate is weak, so there is nothing of our own to release.
    msg_super![env; this dealloc]
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
    // Immediately dismiss so the app doesn't hang.
    let delegate = env.objc
        .borrow::<GKAchievementViewControllerHostObject>(this)
        .achievement_delegate;

    if delegate != nil && responds_to(env, delegate, "achievementViewControllerDidFinish:") {
        () = msg![env; delegate achievementViewControllerDidFinish:this];
    }
}

- (())viewDidAppear:(bool)_animated {
    // Already dismissed in viewWillAppear — nothing to do.
}

- (())presentModalViewController:(id)_vc animated:(bool)_animated {
    log!("GKAchievementViewController presentModalViewController: stubbed");
}

- (())dismissModalViewControllerAnimated:(bool)_animated {
    log!("GKAchievementViewController dismissModalViewControllerAnimated: stubbed");
}

@end

};

/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GKAchievementDescription`.
//!
//! Games use this class to show the achievement list: it is the only GameKit
//! API that provides achievement *titles*, descriptions and point values.
//! Apple serves that metadata from the App Store, which is unreachable here,
//! so the descriptions are built from the achievements recorded in the local
//! Game Center store ([crate::game_center]) and named with
//! [crate::game_center::humanize_identifier], which turns identifiers like
//! `com.studio.game.achievements.first_win` into "First Win".
//!
//! Reference:
//! <https://developer.apple.com/documentation/gamekit/gkachievementdescription>

use super::gk_local_player::{invoke_two_object_block, is_signed_in, make_not_authenticated_error};
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSInteger;
use crate::game_center;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

/// What a description says when the emulator has no real text. It has to be
/// honest about being a placeholder, because games display it verbatim.
const PLACEHOLDER_DESCRIPTION: &str =
    "This achievement's text is stored by the App Store, which is not available offline.";

struct GKAchievementDescriptionHostObject {
    /// `NSString*` achievement identifier (retained).
    identifier: id,
    /// `NSString*` human-readable title (retained).
    title: id,
    /// `NSString*` text shown while the achievement is unfinished (retained).
    unachieved_description: id,
    /// `NSString*` text shown once the achievement is finished (retained).
    achieved_description: id,
    /// `NSString*` group identifier, or nil (retained).
    group_identifier: id,
    hidden: bool,
    /// `GKAchievementDescription` reports 0 here: the SDK documents that the
    /// value comes from App Store configuration that iOS 4/5 apps have none of.
    maximum_points: NSInteger,
}
impl Default for GKAchievementDescriptionHostObject {
    fn default() -> Self {
        Self {
            identifier: nil,
            title: nil,
            unachieved_description: nil,
            achieved_description: nil,
            group_identifier: nil,
            hidden: false,
            maximum_points: 0,
        }
    }
}
impl HostObject for GKAchievementDescriptionHostObject {}

/// Build one description object for a stored achievement. The returned object
/// is retained; the caller owns it.
fn make_description(env: &mut Environment, achievement: &game_center::Achievement) -> id {
    let identifier = ns_string::from_rust_string(env, achievement.identifier.clone());
    let title = ns_string::from_rust_string(env, achievement.title());
    let description = ns_string::from_rust_string(env, PLACEHOLDER_DESCRIPTION.to_string());

    let object: id = msg_class![env; GKAchievementDescription alloc];
    let object: id = msg![env; object init];

    // Take the host object's own references first: `retain` needs `env`, so
    // it cannot be called while a field of the host object is borrowed.
    let kept_identifier = retain(env, identifier);
    let kept_title = retain(env, title);
    // A finished achievement shows `achievedDescription`, an unfinished one
    // `unachievedDescription`; both get the same placeholder for now, so they
    // share a single retain (see `dealloc`).
    let kept_description = retain(env, description);
    {
        let host = env.objc.borrow_mut::<GKAchievementDescriptionHostObject>(object);
        host.identifier = kept_identifier;
        host.title = kept_title;
        host.unachieved_description = kept_description;
        host.achieved_description = kept_description;
    }
    release(env, identifier);
    release(env, title);
    release(env, description);
    object
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GKAchievementDescription: NSObject

// MARK: - Class methods (must come before instance methods in objc_classes! macro)

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<GKAchievementDescriptionHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// + (void)loadAchievementDescriptionsWithCompletionHandler:
//       (void (^)(NSArray *descriptions, NSError *error))handler
//
// Apple: "Retrieves the descriptions of the achievements supported by your
// game." Only achievements this emulator has seen reported can be described,
// because that is all the local store knows.
+ (())loadAchievementDescriptionsWithCompletionHandler:(id)handler {
    if !is_signed_in(env) {
        let error = make_not_authenticated_error(env);
        invoke_two_object_block(env, handler, nil, error);
        return;
    }
    let achievements = game_center::achievements_for_current_app(env);
    let array: id = msg_class![env; NSMutableArray array];
    for achievement in &achievements {
        let description = make_description(env, achievement);
        () = msg![env; array addObject:description];
        release(env, description);
    }
    log!(
        "GKAchievementDescription loadAchievementDescriptionsWithCompletionHandler: \
         returning {} description(s)",
        achievements.len()
    );
    invoke_two_object_block(env, handler, array, nil);
}

// Apple: "The placeholder image used to display an achievement that has not
// been earned." There are no achievement images offline.
+ (id)incompleteAchievementImage {
    nil
}

// Apple: "The placeholder image used to display an achievement that has been
// earned." There are no achievement images offline.
+ (id)placeholderCompletedAchievementImage {
    nil
}

// MARK: - Init / dealloc

- (id)init {
    this
}

- (())dealloc {
    let (identifier, title, unachieved, achieved, group) = {
        let host = env.objc.borrow::<GKAchievementDescriptionHostObject>(this);
        (
            host.identifier,
            host.title,
            host.unachieved_description,
            host.achieved_description,
            host.group_identifier,
        )
    };
    release(env, identifier);
    release(env, title);
    release(env, unachieved);
    if achieved != unachieved {
        release(env, achieved);
    }
    release(env, group);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Metadata

- (id)identifier {
    env.objc.borrow::<GKAchievementDescriptionHostObject>(this).identifier
}

- (id)title {
    env.objc.borrow::<GKAchievementDescriptionHostObject>(this).title
}

- (id)unachievedDescription {
    env.objc.borrow::<GKAchievementDescriptionHostObject>(this).unachieved_description
}

- (id)achievedDescription {
    env.objc.borrow::<GKAchievementDescriptionHostObject>(this).achieved_description
}

- (id)groupIdentifier {
    env.objc.borrow::<GKAchievementDescriptionHostObject>(this).group_identifier
}

- (bool)isHidden {
    env.objc.borrow::<GKAchievementDescriptionHostObject>(this).hidden
}

- (NSInteger)maximumPoints {
    env.objc.borrow::<GKAchievementDescriptionHostObject>(this).maximum_points
}

// Apple: "The image for the achievement." nil until the game supplies one,
// which it never can offline.
- (id)image {
    nil
}

- (id)description {
    let identifier = env.objc.borrow::<GKAchievementDescriptionHostObject>(this).identifier;
    let title = env.objc.borrow::<GKAchievementDescriptionHostObject>(this).title;
    let identifier = if identifier == nil {
        "<nil>".to_string()
    } else {
        ns_string::to_rust_string(env, identifier).into_owned()
    };
    let title = if title == nil {
        "<nil>".to_string()
    } else {
        ns_string::to_rust_string(env, title).into_owned()
    };
    let text = format!("<GKAchievementDescription: {identifier} ({title})>");
    let ns = ns_string::from_rust_string(env, text);
    autorelease(env, ns)
}

@end

};

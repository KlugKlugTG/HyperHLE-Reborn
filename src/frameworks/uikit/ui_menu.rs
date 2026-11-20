/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIMenuController` and `UIMenuItem`.
//!
//! Editing menus (Copy/Paste, and custom items added by the app). Chrome and
//! other text-heavy apps construct menu items during launch; touchHLE does
//! not draw the menu, but the object model has to exist so that those calls
//! don't fail. Showing and hiding only updates state and posts the
//! `UIMenuController*MenuNotification`s.
//!
//! References:
//! - <https://developer.apple.com/documentation/uikit/uimenucontroller>
//! - <https://developer.apple.com/documentation/uikit/uimenuitem>

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
    SEL,
};
use crate::Environment;

/// `UIMenuControllerArrowDirection`
pub type UIMenuControllerArrowDirection = NSInteger;
pub const UIMenuControllerArrowDefault: UIMenuControllerArrowDirection = 0;
pub const UIMenuControllerArrowUp: UIMenuControllerArrowDirection = 1;
pub const UIMenuControllerArrowDown: UIMenuControllerArrowDirection = 2;
pub const UIMenuControllerArrowLeft: UIMenuControllerArrowDirection = 3;
pub const UIMenuControllerArrowRight: UIMenuControllerArrowDirection = 4;

pub const UIMenuControllerWillShowMenuNotification: &str =
    "UIMenuControllerWillShowMenuNotification";
pub const UIMenuControllerDidShowMenuNotification: &str =
    "UIMenuControllerDidShowMenuNotification";
pub const UIMenuControllerWillHideMenuNotification: &str =
    "UIMenuControllerWillHideMenuNotification";
pub const UIMenuControllerDidHideMenuNotification: &str =
    "UIMenuControllerDidHideMenuNotification";
pub const UIMenuControllerMenuFrameDidChangeNotification: &str =
    "UIMenuControllerMenuFrameDidChangeNotification";

pub const CONSTANTS: ConstantExports = &[
    (
        "_UIMenuControllerWillShowMenuNotification",
        HostConstant::NSString(UIMenuControllerWillShowMenuNotification),
    ),
    (
        "_UIMenuControllerDidShowMenuNotification",
        HostConstant::NSString(UIMenuControllerDidShowMenuNotification),
    ),
    (
        "_UIMenuControllerWillHideMenuNotification",
        HostConstant::NSString(UIMenuControllerWillHideMenuNotification),
    ),
    (
        "_UIMenuControllerDidHideMenuNotification",
        HostConstant::NSString(UIMenuControllerDidHideMenuNotification),
    ),
    (
        "_UIMenuControllerMenuFrameDidChangeNotification",
        HostConstant::NSString(UIMenuControllerMenuFrameDidChangeNotification),
    ),
];

#[derive(Default)]
struct UIMenuItemHostObject {
    /// `NSString *`, copied.
    title: id,
    action: SEL,
}
impl HostObject for UIMenuItemHostObject {}

#[derive(Default)]
struct UIMenuControllerHostObject {
    /// `NSArray *` of `UIMenuItem *`, copied.
    menu_items: id,
    /// The target rectangle, in `target_view`'s coordinate space.
    target_rect: CGRect,
    /// Weak (UIKit does not retain the view). Never dereferenced here.
    target_view: id,
    visible: bool,
    /// `UIMenuControllerArrowDirection`
    arrow_direction: UIMenuControllerArrowDirection,
}
impl HostObject for UIMenuControllerHostObject {}

#[derive(Default)]
pub struct State {
    /// The shared menu controller (lazily created).
    shared_menu_controller: id,
}

/// Post `name` with `controller` as the sender.
fn post_notification(env: &mut Environment, controller: id, name: &'static str) {
    let name = get_static_str(env, name);
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];
    let _: () = msg![env; center postNotificationName:name object:controller];
}

/// Change visibility, posting the Will/Did notifications only if it actually
/// changes (real UIKit does not post for a redundant show or hide).
fn set_visible(env: &mut Environment, controller: id, visible: bool) {
    let was_visible = env
        .objc
        .borrow::<UIMenuControllerHostObject>(controller)
        .visible;
    if was_visible == visible {
        return;
    }
    let (will, did) = if visible {
        (
            UIMenuControllerWillShowMenuNotification,
            UIMenuControllerDidShowMenuNotification,
        )
    } else {
        (
            UIMenuControllerWillHideMenuNotification,
            UIMenuControllerDidHideMenuNotification,
        )
    };
    post_notification(env, controller, will);
    env.objc
        .borrow_mut::<UIMenuControllerHostObject>(controller)
        .visible = visible;
    post_notification(env, controller, did);
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIMenuItem: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<UIMenuItemHostObject>::default(), &mut env.mem)
}

- (id)initWithTitle:(id)title action:(SEL)action {
    // `title` is a copy property. `copy` returns +1 (and nil for nil), which
    // is the reference the host object keeps.
    let title: id = msg![env; title copy];
    let old = {
        let host = env.objc.borrow_mut::<UIMenuItemHostObject>(this);
        let old = std::mem::replace(&mut host.title, title);
        host.action = action;
        old
    };
    release(env, old);
    this
}

- (id)title { env.objc.borrow::<UIMenuItemHostObject>(this).title }
- (())setTitle:(id)title {
    let title: id = msg![env; title copy];
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIMenuItemHostObject>(this).title,
        title,
    );
    release(env, old);
}

- (SEL)action { env.objc.borrow::<UIMenuItemHostObject>(this).action }
- (())setAction:(SEL)action {
    env.objc.borrow_mut::<UIMenuItemHostObject>(this).action = action;
}

- (())dealloc {
    let title = std::mem::take(&mut env.objc.borrow_mut::<UIMenuItemHostObject>(this).title);
    release(env, title);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation UIMenuController: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<UIMenuControllerHostObject>::default(), &mut env.mem)
}

+ (id)sharedMenuController {
    let existing = env.framework_state.uikit.ui_menu.shared_menu_controller;
    if existing != nil {
        return existing;
    }
    let new_instance: id = msg![env; this alloc];
    let new_instance: id = msg![env; new_instance init];
    env.framework_state.uikit.ui_menu.shared_menu_controller = new_instance;
    new_instance
}

// MARK: - Visibility

- (bool)isMenuVisible {
    env.objc.borrow::<UIMenuControllerHostObject>(this).visible
}
- (())setMenuVisible:(bool)visible {
    set_visible(env, this, visible);
}
- (())setMenuVisible:(bool)visible animated:(bool)_animated {
    set_visible(env, this, visible);
}

// iOS 13+
- (())showMenuFromView:(id)view rect:(CGRect)rect {
    let _: () = msg![env; this setTargetRect:rect inView:view];
    set_visible(env, this, true);
}
- (())hideMenuFromView:(id)_view {
    set_visible(env, this, false);
}
- (())hideMenu {
    set_visible(env, this, false);
}

// The real value is the menu's frame in window coordinates. Nothing is drawn
// here, so the target rect (view coordinates) is the closest approximation.
- (CGRect)menuFrame {
    let (visible, rect) = {
        let host = env.objc.borrow::<UIMenuControllerHostObject>(this);
        (host.visible, host.target_rect)
    };
    if visible { rect } else { CGRect::default() }
}

// MARK: - Items

- (id)menuItems {
    env.objc.borrow::<UIMenuControllerHostObject>(this).menu_items
}
- (())setMenuItems:(id)items {
    // `menuItems` is a copy property: keep our own snapshot, so the app
    // mutating the array it passed in later can't change the menu.
    let items: id = msg![env; items copy];
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<UIMenuControllerHostObject>(this).menu_items,
        items,
    );
    release(env, old);
}

// MARK: - Target

// Per Apple, the rectangle is in the view's coordinate space. It is only a
// positioning hint for the (unrendered) menu, so storing it is enough.
- (())setTargetRect:(CGRect)rect inView:(id)view {
    let host = env.objc.borrow_mut::<UIMenuControllerHostObject>(this);
    host.target_rect = rect;
    host.target_view = view;
}

- (NSInteger)arrowDirection {
    env.objc.borrow::<UIMenuControllerHostObject>(this).arrow_direction
}
- (())setArrowDirection:(NSInteger)direction {
    env.objc.borrow_mut::<UIMenuControllerHostObject>(this).arrow_direction = direction;
}

// Recomputes the menu's contents; there is nothing to redraw here.
- (())update {}

- (())dealloc {
    let items = std::mem::take(&mut env.objc.borrow_mut::<UIMenuControllerHostObject>(this).menu_items);
    release(env, items);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

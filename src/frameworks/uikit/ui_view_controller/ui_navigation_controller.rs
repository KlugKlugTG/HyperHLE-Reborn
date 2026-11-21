/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UINavigationController`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::{CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{ns_array, NSUInteger};
use crate::mem::{ConstVoidPtr, MutPtr};
use crate::objc::{
    autorelease, id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes,
    release, retain, ClassExports, NSZonePtr,
};
use crate::Environment;

#[derive(Default)]
struct UINavigationControllerHostObject {
    superclass: super::UIViewControllerHostObject,
    delegate: id,
    navigation_stack: Vec<id>,
    navigation_bar: id,
    laying_out: bool,
    toolbar: id,
    /// Whether the navigation bar is hidden.
    navigation_bar_hidden: bool,
    /// Whether the bottom toolbar is hidden (default true in UIKit).
    toolbar_hidden: bool,
    /// Whether an interactive pop gesture recogniser is enabled (iOS 7+).
    interactve_pop_gesture_enabled: bool,
    /// Whether we hide the navigation bar on keyboard show (default false).
    hides_bars_on_keyboard_appearance: bool,
    /// Whether we hide bars when swiping down (default false).
    hides_bars_on_swipe: bool,
    /// Whether we hide bars when the tap is in a readable content area (default
    //false).
    hides_bars_when_browsing: bool,
}
impl_HostObject_with_superclass!(UINavigationControllerHostObject);

/// Legacy UIKit owns the bar and fits the top controller below opaque bars.
/// https://developer.apple.com/library/archive/documentation/WindowsViews/Conceptual/ViewControllerPGforiOSLegacy/NavigationControllers/NavigationControllers.html
fn layout_navigation_controller(env: &mut Environment, this: id) {
    let host = env.objc.borrow::<UINavigationControllerHostObject>(this);
    let view = host.superclass.view;
    if view == nil || host.laying_out {
        return;
    }
    env.objc
        .borrow_mut::<UINavigationControllerHostObject>(this)
        .laying_out = true;
    let bounds: CGRect = msg![env; view bounds];
    let bar: id = msg![env; this navigationBar];
    let hidden = env
        .objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_bar_hidden;
    let fitting: CGSize = msg![env; bar sizeThatFits:(bounds.size)];
    let bar_height = fitting.height.min(bounds.size.height);
    let mut bar_frame = bounds;
    bar_frame.size.height = bar_height;
    () = msg![env; bar setFrame:bar_frame];
    () = msg![env; bar setHidden:hidden];

    let top: id = msg![env; this topViewController];
    if top != nil {
        let child: id = msg![env; top view];
        let translucent: bool = msg![env; bar isTranslucent];
        let full_screen: bool = msg![env; top wantsFullScreenLayout];
        let inset = if hidden || (translucent && full_screen) {
            0.0
        } else {
            bar_height
        };
        let mut frame = bounds;
        frame.origin.y += inset;
        frame.size.height = (frame.size.height - inset).max(0.0);
        () = msg![env; child setFrame:frame];
        () = msg![env; child layoutSubviews];
    }
    let parent: id = msg![env; bar superview];
    if parent != view {
        () = msg![env; view addSubview:bar];
    } else {
        () = msg![env; view bringSubviewToFront:bar];
    }
    () = msg![env; bar layoutSubviews];
    env.objc
        .borrow_mut::<UINavigationControllerHostObject>(this)
        .laying_out = false;
}

fn update_navigation_items(env: &mut Environment, this: id) {
    let stack = env
        .objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_stack
        .clone();
    let mut items = Vec::with_capacity(stack.len());
    for controller in stack {
        let item: id = msg![env; controller navigationItem];
        retain(env, item);
        items.push(item);
    }
    let items = ns_array::from_vec(env, items);
    let bar: id = msg![env; this navigationBar];
    () = msg![env; bar setItems:items];
    release(env, items);
    layout_navigation_controller(env, this);
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UINavigationController: UIViewController

// =========================================================================
// MARK: - Allocation / init
// =========================================================================

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UINavigationControllerHostObject {
        toolbar_hidden: true,
        interactve_pop_gesture_enabled: true,
        ..Default::default()
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithRootViewController:(id)root_vc {
    let this: id = msg![env; this init];
    let _: () = msg![env; this pushViewController:root_vc animated:false];
    this
}

- (())loadView {
    let screen: id = msg_class![env; UIScreen mainScreen];
    let frame: CGRect = msg![env; screen applicationFrame];
    let view: id = msg_class![env; TouchHLENavigationControllerView alloc];
    let view: id = msg![env; view initWithFrame:frame];
    () = msg![env; this setView:view];
    release(env, view);
    layout_navigation_controller(env, this);
}

- (())viewWillLayoutSubviews {
    () = msg_super![env; this viewWillLayoutSubviews];
    layout_navigation_controller(env, this);
}

- (id)initWithCoder:(id)coder {
    let this: id = msg_super![env; this initWithCoder:coder];

    // A UINavigationController archived in a nib stores its view-controller
    // stack under "UIViewControllers" (and, for older archives, its already
    // built navigation bar under "UINavigationBar"). UIViewController's
    // -initWithCoder: doesn't know about these keys, so without decoding them
    // here the navigation stack stays empty: -topViewController is nil, the
    // root view controller's view is never added, and the app shows an empty
    // (white) navigation controller view. Decode them so the initial view
    // controller actually appears.
    let bar_key = get_static_str(env, "UINavigationBar");
    let bar: id = msg![env; coder decodeObjectForKey:bar_key];
    if bar != nil {
        retain(env, bar);
        let old = std::mem::replace(
            &mut env.objc.borrow_mut::<UINavigationControllerHostObject>(this).navigation_bar,
            bar,
        );
        if old != nil { release(env, old); }
    }

    let vcs_key = get_static_str(env, "UIViewControllers");
    let view_controllers: id = msg![env; coder decodeObjectForKey:vcs_key];
    if view_controllers != nil {
        let count: NSUInteger = msg![env; view_controllers count];
        if count != 0 {
            let _: () = msg![env; this setViewControllers:view_controllers];
        }
    }

    this
}

- (id)initWithNavigationBarClass:(id)_nav_bar_class
                    toolbarClass:(id)_toolbar_class {
    // Ignore custom classes — use standard UINavigationBar/UIToolbar.
    msg![env; this init]
}

// =========================================================================
// MARK: - Delegate
// =========================================================================

- (id)delegate {
    env.objc.borrow::<UINavigationControllerHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    log_dbg!("UINavigationController setDelegate:{:?}", delegate);
    env.objc.borrow_mut::<UINavigationControllerHostObject>(this).delegate = delegate;
}

// =========================================================================
// MARK: - Push / pop
// =========================================================================

- (())pushViewController:(id)view_controller animated:(bool)animated {
    if view_controller == nil { return; }

    let previous: id = msg![env; this topViewController];

    {
        let stack = &mut env
            .objc
            .borrow_mut::<UINavigationControllerHostObject>(this)
            .navigation_stack;
        if stack.contains(&view_controller) {
            log!("Warning: UINavigationController pushViewController: vc already on stack — ignoring");
            return;
        }
        stack.push(view_controller);
    }
    retain(env, view_controller);
    let _: () = msg![env; view_controller setNavigationController:this];
    () = msg![env; view_controller setParentViewController:this];

    // Delegate: willShow. Use `register_host_selector` so we don't panic when
    // the app's delegate doesn't implement (and therefore never references)
    // this selector — some apps simply skip the optional UINavigationController
    // delegate callbacks.
    let delegate = env.objc.borrow::<UINavigationControllerHostObject>(this).delegate;
    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "navigationController:willShowViewController:animated:".to_string(),
            &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let _: () = msg![env; delegate navigationController:this
                                       willShowViewController:view_controller
                                                     animated:false];
        }
    }

    // Add view. The navigation controller is responsible for sizing the
    // pushed view controller's view to fill its content area; views loaded
    // from a nib often arrive with a zero (or stale) frame, which would
    // otherwise leave the screen blank.
    let self_view: id = msg![env; this view];
    let vc_view:   id = msg![env; view_controller view];
    if previous != nil {
        let old_view = env.objc.borrow::<super::UIViewControllerHostObject>(previous).view;
        let parent: id = msg![env; old_view superview];
        if parent == self_view {
            () = msg![env; previous viewWillDisappear:animated];
            () = msg![env; old_view removeFromSuperview];
            () = msg![env; previous viewDidDisappear:animated];
        }
    }
    update_navigation_items(env, this);
    let _: () = msg![env; view_controller viewWillAppear:false];
    let _: () = msg![env; self_view addSubview:vc_view];
    layout_navigation_controller(env, this);
    let _: () = msg![env; view_controller viewDidAppear:false];

    // Delegate: didShow.
    let delegate = env.objc.borrow::<UINavigationControllerHostObject>(this).delegate;
    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "navigationController:didShowViewController:animated:".to_string(),
            &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let _: () = msg![env; delegate navigationController:this
                                        didShowViewController:view_controller
                                                     animated:false];
        }
    }
}

- (id)popViewControllerAnimated:(bool)animated {
    let stack_len = env.objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_stack.len();
    if stack_len <= 1 {
        // Cannot pop the root view controller.
        return nil;
    }
    let popped = env.objc
        .borrow_mut::<UINavigationControllerHostObject>(this)
        .navigation_stack
        .pop()
        .unwrap_or(nil);

    if popped == nil { return nil; }

    // Remove view.
    let vc_view: id = msg![env; popped view];
    let _: () = msg![env; popped viewWillDisappear:animated];
    let _: () = msg![env; vc_view removeFromSuperview];
    let _: () = msg![env; popped viewDidDisappear:animated];

    let _: () = msg![env; popped setNavigationController:nil];
    () = msg![env; popped setParentViewController:nil];

    // Show the new top VC.
    let new_top = env.objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_stack
        .last()
        .copied()
        .unwrap_or(nil);
    if new_top != nil {
        let self_view: id = msg![env; this view];
        let child: id = msg![env; new_top view];
        let _: () = msg![env; new_top viewWillAppear:animated];
        () = msg![env; self_view addSubview:child];
        update_navigation_items(env, this);
        let _: () = msg![env; new_top viewDidAppear:animated];
        // Delegate: didShow new top.
        let delegate = env.objc.borrow::<UINavigationControllerHostObject>(this).delegate;
        if delegate != nil {
            let sel = env.objc.register_host_selector(
                "navigationController:didShowViewController:animated:".to_string(),
                &mut env.mem,
            );
            let responds: bool = msg![env; delegate respondsToSelector:sel];
            if responds {
                let _: () = msg![env; delegate navigationController:this
                                            didShowViewController:new_top
                                                         animated:animated];
            }
        }
    }

    autorelease(env, popped)
}

- (id)popToViewController:(id)view_controller animated:(bool)animated {
    let idx = {
        let stack = &env.objc
            .borrow::<UINavigationControllerHostObject>(this)
            .navigation_stack;
        stack.iter().position(|&vc| vc == view_controller)
    };
    let Some(idx) = idx else {
        log!("Warning: UINavigationController popToViewController: vc not in stack");
        return nil;
    };

    // Collect everything above idx.
    let to_pop: Vec<id> = env.objc
        .borrow_mut::<UINavigationControllerHostObject>(this)
        .navigation_stack
        .drain((idx + 1)..)
        .collect();

    let popped_arr_vcs: Vec<id> = to_pop.iter().map(|&vc| { retain(env, vc); vc }).collect();
    for &vc in &to_pop {
        let vc_view: id = msg![env; vc view];
        let _: () = msg![env; vc viewWillDisappear:animated];
        let _: () = msg![env; vc_view removeFromSuperview];
        let _: () = msg![env; vc viewDidDisappear:animated];
        let _: () = msg![env; vc setNavigationController:nil];
        () = msg![env; vc setParentViewController:nil];
        release(env, vc);
    }

    if view_controller != nil {
        let self_view: id = msg![env; this view];
        let child: id = msg![env; view_controller view];
        let _: () = msg![env; view_controller viewWillAppear:animated];
        () = msg![env; self_view addSubview:child];
        update_navigation_items(env, this);
        let _: () = msg![env; view_controller viewDidAppear:animated];
    }

    // Build return array of popped VCs.
    let arr = ns_array::from_vec(env, popped_arr_vcs);
    autorelease(env, arr)
}

- (id)popToRootViewControllerAnimated:(bool)animated {
    let root = {
        let stack = &env.objc
            .borrow::<UINavigationControllerHostObject>(this)
            .navigation_stack;
        stack.first().copied().unwrap_or(nil)
    };
    if root == nil { return nil; }
    msg![env; this popToViewController:root animated:animated]
}

// =========================================================================
// MARK: - Appearance forwarding
// =========================================================================
// UINavigationController forwards appearance callbacks to the currently
// visible (top) child view controller. Without this, frameworks like
// IASKAppSettingsKit — which do all of their setup in -viewWillAppear: —
// never get called when the navigation controller itself is presented.

- (())viewWillAppear:(bool)animated {
    () = msg_super![env; this viewWillAppear:animated];
    let top: id = msg![env; this topViewController];
    if top != nil {
        () = msg![env; top viewWillAppear:animated];
    }
}

- (())viewDidAppear:(bool)animated {
    () = msg_super![env; this viewDidAppear:animated];
    let top: id = msg![env; this topViewController];
    if top != nil {
        () = msg![env; top viewDidAppear:animated];
    }
}

- (())viewWillDisappear:(bool)animated {
    let top: id = msg![env; this topViewController];
    if top != nil {
        () = msg![env; top viewWillDisappear:animated];
    }
    () = msg_super![env; this viewWillDisappear:animated];
}

- (())viewDidDisappear:(bool)animated {
    let top: id = msg![env; this topViewController];
    if top != nil {
        () = msg![env; top viewDidDisappear:animated];
    }
    () = msg_super![env; this viewDidDisappear:animated];
}

// =========================================================================
// MARK: - Stack accessors
// =========================================================================

- (id)topViewController {
    env.objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_stack
        .last()
        .copied()
        .unwrap_or(nil)
}

- (id)visibleViewController {
    // Same as topViewController in our stub (no modal on top of nav).
    msg![env; this topViewController]
}

- (id)viewControllers {
    let vcs = env.objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_stack
        .clone();
    for &vc in &vcs { retain(env, vc); }
    let arr = ns_array::from_vec(env, vcs);
    autorelease(env, arr)
}

- (())setViewControllers:(id)controllers {
    let _: () = msg![env; this setViewControllers:controllers animated:false];
}

- (())setViewControllers:(id)controllers animated:(bool)animated {
    let _self_view = env.objc
        .borrow::<UINavigationControllerHostObject>(this)
        .superclass
        .view;

    let old_stack = std::mem::take(
        &mut env.objc
            .borrow_mut::<UINavigationControllerHostObject>(this)
            .navigation_stack,
    );
    for vc in old_stack {
        let vc_view: id = msg![env; vc view];
        let _: () = msg![env; vc_view removeFromSuperview];
        let _: () = msg![env; vc setNavigationController:nil];
        () = msg![env; vc setParentViewController:nil];
        release(env, vc);
    }

    let count: NSUInteger = msg![env; controllers count];
    if count == 0 {
        update_navigation_items(env, this);
        return;
    }

    let mut tmp: Vec<id> = Vec::new();
    for i in 0..(count - 1) {
        let vc: id = msg![env; controllers objectAtIndex:i];
        retain(env, vc);
        let _: () = msg![env; vc setNavigationController:this];
        () = msg![env; vc setParentViewController:this];
        tmp.push(vc);
    }
    env.objc
        .borrow_mut::<UINavigationControllerHostObject>(this)
        .navigation_stack = tmp;

    let last_vc: id = msg![env; controllers objectAtIndex:(count - 1)];
    let _: () = msg![env; this pushViewController:last_vc animated:animated];
}

// =========================================================================
// MARK: - Navigation bar
// =========================================================================

- (id)navigationBar {
    let bar = env.objc.borrow::<UINavigationControllerHostObject>(this).navigation_bar;
    if bar != nil { return bar; }
    let bar: id = msg_class![env; UINavigationBar alloc];
    let bar: id = msg![env; bar init];
    env.objc.borrow_mut::<UINavigationControllerHostObject>(this).navigation_bar = bar;
    () = msg![env; bar setDelegate:this];
    bar
}

- (bool)navigationBar:(id)_bar shouldPopItem:(id)item {
    let top: id = msg![env; this topViewController];
    let top_item: id = msg![env; top navigationItem];
    if item == top_item {
        // Popping the controller updates the entire bar stack. Prevent the
        // bar from also removing an item after the controller has done so.
        let _: id = msg![env; this popViewControllerAnimated:true];
        false
    } else {
        true
    }
}

- (bool)isNavigationBarHidden {
    env.objc.borrow::<UINavigationControllerHostObject>(this).navigation_bar_hidden
}

- (())setNavigationBarHidden:(bool)hidden {
    let _: () = msg![env; this setNavigationBarHidden:hidden animated:false];
}

- (())setNavigationBarHidden:(bool)hidden animated:(bool)_animated {
    log_dbg!("UINavigationController setNavigationBarHidden:{}", hidden);
    env.objc.borrow_mut::<UINavigationControllerHostObject>(this).navigation_bar_hidden = hidden;
    let bar = msg![env; this navigationBar];
    let _: () = msg![env; bar setHidden:hidden];
    layout_navigation_controller(env, this);
}

// =========================================================================
// MARK: - Toolbar
// =========================================================================

- (id)toolbar {
    let tb = env.objc.borrow::<UINavigationControllerHostObject>(this).toolbar;
    if tb != nil { return tb; }
    let tb: id = msg_class![env; UIToolbar alloc];
    let tb: id = msg![env; tb init];
    env.objc.borrow_mut::<UINavigationControllerHostObject>(this).toolbar = tb;
    tb
}

- (bool)isToolbarHidden {
    env.objc.borrow::<UINavigationControllerHostObject>(this).toolbar_hidden
}

- (())setToolbarHidden:(bool)hidden {
    let _: () = msg![env; this setToolbarHidden:hidden animated:false];
}

- (())setToolbarHidden:(bool)hidden animated:(bool)_animated {
    log_dbg!("UINavigationController setToolbarHidden:{}", hidden);
    env.objc.borrow_mut::<UINavigationControllerHostObject>(this).toolbar_hidden = hidden;
    let tb = msg![env; this toolbar];
    let _: () = msg![env; tb setHidden:hidden];
}

// =========================================================================
// MARK: - iOS 7+ bar-hiding properties
// =========================================================================

- (bool)hidesBarsOnKeyboardAppearance {
    env.objc.borrow::<UINavigationControllerHostObject>(this).hides_bars_on_keyboard_appearance
}
- (())setHidesBarsOnKeyboardAppearance:(bool)v {
    env.objc.borrow_mut::<UINavigationControllerHostObject>(this)
        .hides_bars_on_keyboard_appearance = v;
}

- (bool)hidesBarsOnSwipe {
    env.objc.borrow::<UINavigationControllerHostObject>(this).hides_bars_on_swipe
}
- (())setHidesBarsOnSwipe:(bool)v {
    env.objc.borrow_mut::<UINavigationControllerHostObject>(this).hides_bars_on_swipe = v;
}

- (bool)hidesBarsWhenBrowsing {
    env.objc.borrow::<UINavigationControllerHostObject>(this).hides_bars_when_browsing
}
- (())setHidesBarsWhenBrowsing:(bool)v {
    env.objc.borrow_mut::<UINavigationControllerHostObject>(this).hides_bars_when_browsing = v;
}

// =========================================================================
// MARK: - Interactive pop gesture (iOS 7+)
// =========================================================================

// Returns a stub gesture recogniser id (nil is safe here — apps check it
// but rarely call methods on it in 32-bit era apps).
- (id)interactivePopGestureRecognizer {
    nil
}

// =========================================================================
// MARK: - Dealloc
// =========================================================================

- (())dealloc {
    let (stack, bar, tb) = {
        let h = env.objc.borrow::<UINavigationControllerHostObject>(this);
        (h.navigation_stack.clone(), h.navigation_bar, h.toolbar)
    };
    for vc in stack {
        let _: () = msg![env; vc setNavigationController:nil];
        () = msg![env; vc setParentViewController:nil];
        release(env, vc);
    }
    release(env, bar);
    release(env, tb);
    () = msg_super![env; this dealloc];
}

// =========================================================================
// MARK: - Description
// =========================================================================

- (id)description {
    let depth = env.objc
        .borrow::<UINavigationControllerHostObject>(this)
        .navigation_stack.len();
    let s = format!("<UINavigationController: {:?}; stackDepth={}>", this, depth);
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

// Internal container whose layout remains tied to its owning controller
// when a modal is sized or rotated after its child has already loaded.
@implementation TouchHLENavigationControllerView: UIView

- (())setFrame:(CGRect)frame {
    () = msg_super![env; this setFrame:frame];
    () = msg![env; this layoutSubviews];
}

- (())setBounds:(CGRect)bounds {
    () = msg_super![env; this setBounds:bounds];
    () = msg![env; this layoutSubviews];
}

- (())layoutSubviews {
    let controller = crate::frameworks::uikit::ui_view::view_controller(env, this);
    if controller != nil {
        layout_navigation_controller(env, controller);
    }
}

@end

};

// =========================================================================
// MARK: - Constants
// =========================================================================

/// `UINavigationControllerHideShowBarDuration` — the animation duration UIKit
/// uses when `-setNavigationBarHidden:animated:` slides the navigation bar in
/// or out. Declared in `<UIKit/UINavigationController.h>` as
/// `UIKIT_EXTERN const CGFloat`; UIKit's value is 0.2 seconds.
/// <https://developer.apple.com/documentation/uikit/1624442-uinavigationcontrollerhideshowbar>
fn navigation_controller_hide_show_bar_duration(env: &mut Environment) -> ConstVoidPtr {
    let ptr: MutPtr<u32> = env.mem.alloc(4).cast();
    env.mem.write(ptr, 0.2f32.to_bits());
    ptr.cast().cast_const()
}

pub const CONSTANTS: ConstantExports = &[(
    "_UINavigationControllerHideShowBarDuration",
    HostConstant::Custom(navigation_controller_hide_show_bar_duration),
)];

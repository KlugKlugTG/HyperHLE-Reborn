/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UINavigationBar`.

use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_view::UIViewHostObject;
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_class, msg_super, nil, objc_classes, release,
    retain, ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

type UIBarStyle = NSInteger;
const UIBarStyleDefault: UIBarStyle = 0;
const UIBarStyleBlack: UIBarStyle = 1;
const UIBarStyleBlackOpaque: UIBarStyle = 2;
const UIBarStyleBlackTranslucent: UIBarStyle = 3;

#[derive(Default)]
struct UINavigationBarHostObject {
    /// Embedded `UIView` host state. `UINavigationBar` is a `UIView` subclass,
    /// so it must carry the superclass host object; without it every `UIView`
    /// method (frame, subviews, layout, ...) sent to a nav bar fails to borrow
    /// and falls back to a zeroed phantom.
    superclass: UIViewHostObject,
    /// UINavigationBarDelegate — weak reference
    delegate: id,
    bar_style: UIBarStyle,
    translucent: bool,
    tint_color: id,            // UIColor* — retained
    bar_tint_color: id,        // UIColor* — retained
    title_text_attributes: id, // NSDictionary* — retained
    /// NSMutableArray* of UINavigationItem* — retained
    items: id,
    /// UIImage* — retained
    shadow_image: id,
    /// UIImage* background — retained
    background_image: id,
    title_label: id,
    back_button: id,
    /// Weak references; the view hierarchy and navigation items own them.
    displayed_views: Vec<id>,
}
impl_HostObject_with_superclass!(UINavigationBarHostObject);

// MARK: - UINavigationItem

#[derive(Default)]
struct UINavigationItemHostObject {
    title: id,        // NSString* — retained
    title_view: id,   // UIView* — retained
    prompt: id,       // NSString* — retained
    back_button: id,  // UIBarButtonItem* — retained
    left_button: id,  // UIBarButtonItem* — retained
    right_button: id, // UIBarButtonItem* — retained
    left_items: id,   // NSArray* — retained
    right_items: id,  // NSArray* — retained
    hides_back_button: bool,
    left_items_supplemented: bool,
    /// The bar currently displaying this item, without retaining it.
    navigation_bar: id,
}
impl HostObject for UINavigationItemHostObject {}

fn item_did_change(env: &mut Environment, item: id) {
    let bar = env
        .objc
        .borrow::<UINavigationItemHostObject>(item)
        .navigation_bar;
    if bar != nil {
        () = msg![env; bar layoutSubviews];
    }
}

fn button_items(env: &mut Environment, item: id, left: bool) -> Vec<id> {
    let host = env.objc.borrow::<UINavigationItemHostObject>(item);
    let (array, single) = if left {
        (host.left_items, host.left_button)
    } else {
        (host.right_items, host.right_button)
    };
    if array != nil {
        let count: u32 = msg![env; array count];
        (0..count)
            .map(|index| msg![env; array objectAtIndex:index])
            .collect()
    } else if single != nil {
        vec![single]
    } else {
        Vec::new()
    }
}

fn layout_buttons(
    env: &mut Environment,
    bar: id,
    buttons: Vec<id>,
    left: bool,
    bounds: CGRect,
) -> f32 {
    let mut edge = if left {
        bounds.origin.x + 5.0
    } else {
        bounds.origin.x + bounds.size.width - 5.0
    };
    let height = (bounds.size.height - 8.0).max(0.0);
    for button in buttons {
        let fitting = CGSize {
            width: bounds.size.width,
            height,
        };
        let size: CGSize = msg![env; button sizeThatFits:fitting];
        let width = size.width.min((bounds.size.width - 10.0).max(0.0));
        let x = if left { edge } else { edge - width };
        let frame = CGRect {
            origin: CGPoint {
                x,
                y: bounds.origin.y + 4.0,
            },
            size: CGSize { width, height },
        };
        () = msg![env; button setFrame:frame];
        () = msg![env; button layoutSubviews];
        () = msg![env; bar addSubview:button];
        env.objc
            .borrow_mut::<UINavigationBarHostObject>(bar)
            .displayed_views
            .push(button);
        edge = if left { x + width + 5.0 } else { x - 5.0 };
    }
    edge
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UINavigationBar: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let items = msg_class![env; NSMutableArray new];
    let host_object = Box::new(UINavigationBarHostObject {
        superclass: Default::default(),
        delegate: nil,
        bar_style: UIBarStyleDefault,
        // Legacy UIKit (iOS <= 6) defaults to an opaque navigation bar.
        translucent: false,
        tint_color: nil,
        bar_tint_color: nil,
        title_text_attributes: nil,
        items,
        shadow_image: nil,
        background_image: nil,
        title_label: nil,
        back_button: nil,
        displayed_views: Vec::new(),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    // Run UIView's initializer so the backing layer and view state are set up.
    let this: id = msg_super![env; this init];
    let color: id = msg_class![env; UIColor darkGrayColor];
    () = msg![env; this setBackgroundColor:color];
    this
}

- (id)initWithFrame:(CGRect)frame {
    msg_super![env; this initWithFrame:frame]
}

- (id)initWithCoder:(id)coder {
    msg_super![env; this initWithCoder:coder]
}

- (())dealloc {
    let host = env.objc.borrow::<UINavigationBarHostObject>(this);
    let (tint_color, bar_tint_color, title_attrs, items, shadow, bg, title_label, back_button) = (
        host.tint_color,
        host.bar_tint_color,
        host.title_text_attributes,
        host.items,
        host.shadow_image,
        host.background_image,
        host.title_label,
        host.back_button,
    );
    let count: u32 = msg![env; items count];
    for index in 0..count {
        let item: id = msg![env; items objectAtIndex:index];
        let host = env.objc.borrow_mut::<UINavigationItemHostObject>(item);
        if host.navigation_bar == this { host.navigation_bar = nil; }
    }
    release(env, tint_color);
    release(env, bar_tint_color);
    release(env, title_attrs);
    release(env, items);
    release(env, shadow);
    release(env, bg);
    release(env, title_label);
    release(env, back_button);
    // delegate is weak — no release
    () = msg_super![env; this dealloc];
}

// MARK: - Delegate

- (id)delegate {
    env.objc.borrow::<UINavigationBarHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    // Weak reference — no retain.
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).delegate = delegate;
}

// MARK: - Style

- (UIBarStyle)barStyle {
    env.objc.borrow::<UINavigationBarHostObject>(this).bar_style
}

- (())setBarStyle:(UIBarStyle)style {
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).bar_style = style;
    let color: id = if style == UIBarStyleDefault {
        msg_class![env; UIColor darkGrayColor]
    } else {
        msg_class![env; UIColor blackColor]
    };
    () = msg![env; this setBackgroundColor:color];
}

- (bool)isTranslucent {
    env.objc.borrow::<UINavigationBarHostObject>(this).translucent
}

- (())setTranslucent:(bool)value {
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).translucent = value;
}

// MARK: - Colors

- (id)tintColor {
    env.objc.borrow::<UINavigationBarHostObject>(this).tint_color
}

- (())setTintColor:(id)color {
    let old = env.objc.borrow::<UINavigationBarHostObject>(this).tint_color;
    release(env, old);
    retain(env, color);
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).tint_color = color;
}

- (id)barTintColor {
    env.objc.borrow::<UINavigationBarHostObject>(this).bar_tint_color
}

- (())setBarTintColor:(id)color {
    let old = env.objc.borrow::<UINavigationBarHostObject>(this).bar_tint_color;
    release(env, old);
    retain(env, color);
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).bar_tint_color = color;
}

// MARK: - Title attributes

- (id)titleTextAttributes {
    env.objc.borrow::<UINavigationBarHostObject>(this).title_text_attributes
}

- (())setTitleTextAttributes:(id)attrs {
    let old = env.objc.borrow::<UINavigationBarHostObject>(this).title_text_attributes;
    release(env, old);
    retain(env, attrs);
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).title_text_attributes = attrs;
}

// MARK: - Background / shadow images

- (id)shadowImage {
    env.objc.borrow::<UINavigationBarHostObject>(this).shadow_image
}

- (())setShadowImage:(id)image {
    let old = env.objc.borrow::<UINavigationBarHostObject>(this).shadow_image;
    release(env, old);
    retain(env, image);
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).shadow_image = image;
}

- (id)backgroundImageForBarMetrics:(NSInteger)_metrics {
    env.objc.borrow::<UINavigationBarHostObject>(this).background_image
}

- (())setBackgroundImage:(id)image forBarMetrics:(NSInteger)_metrics {
    let old = env.objc.borrow::<UINavigationBarHostObject>(this).background_image;
    release(env, old);
    retain(env, image);
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).background_image = image;
}

- (())setBackgroundImage:(id)image
         forBarPosition:(NSInteger)_position
             barMetrics:(NSInteger)_metrics {
    () = msg![env; this setBackgroundImage:image forBarMetrics:_metrics];
}

// MARK: - Items stack

- (id)items {
    env.objc.borrow::<UINavigationBarHostObject>(this).items
}

- (())setItems:(id)items { // NSArray*
    let old_items = env.objc.borrow::<UINavigationBarHostObject>(this).items;
    let new_items: id = msg_class![env; NSMutableArray alloc];
    let new_items: id = msg![env; new_items initWithArray:items];
    let old_count: u32 = msg![env; old_items count];
    for index in 0..old_count {
        let item: id = msg![env; old_items objectAtIndex:index];
        let host = env.objc.borrow_mut::<UINavigationItemHostObject>(item);
        if host.navigation_bar == this { host.navigation_bar = nil; }
    }
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).items = new_items;
    let count: u32 = msg![env; new_items count];
    for index in 0..count {
        let item: id = msg![env; new_items objectAtIndex:index];
        env.objc.borrow_mut::<UINavigationItemHostObject>(item).navigation_bar = this;
    }
    // Remove the old displayed subviews while their items are still alive.
    () = msg![env; this layoutSubviews];
    release(env, old_items);
}

- (())setItems:(id)items animated:(bool)_animated {
    () = msg![env; this setItems:items];
}

- (id)topItem {
    let items = env.objc.borrow::<UINavigationBarHostObject>(this).items;
    let count: u32 = msg![env; items count];
    if count == 0 { return nil; }
    msg![env; items objectAtIndex:(count - 1)]
}

- (id)backItem {
    let items = env.objc.borrow::<UINavigationBarHostObject>(this).items;
    let count: u32 = msg![env; items count];
    if count < 2 { return nil; }
    msg![env; items objectAtIndex:(count - 2)]
}

- (())pushNavigationItem:(id)item animated:(bool)_animated {
    let delegate = env.objc.borrow::<UINavigationBarHostObject>(this).delegate;
    // Ask delegate if we should push.
    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "navigationBar:shouldPushItem:".to_string(), &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let should: bool = msg![env; delegate navigationBar:this shouldPushItem:item];
            if !should { return; }
        }
    }
    let items = env.objc.borrow::<UINavigationBarHostObject>(this).items;
    () = msg![env; items addObject:item];
    env.objc.borrow_mut::<UINavigationItemHostObject>(item).navigation_bar = this;
    () = msg![env; this layoutSubviews];

    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "navigationBar:didPushItem:".to_string(), &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            () = msg![env; delegate navigationBar:this didPushItem:item];
        }
    }
}

- (id)popNavigationItemAnimated:(bool)_animated {
    let items = env.objc.borrow::<UINavigationBarHostObject>(this).items;
    let count: u32 = msg![env; items count];
    if count == 0 { return nil; }

    let top: id = msg![env; items objectAtIndex:(count - 1)];

    let delegate = env.objc.borrow::<UINavigationBarHostObject>(this).delegate;
    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "navigationBar:shouldPopItem:".to_string(), &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            let should: bool = msg![env; delegate navigationBar:this shouldPopItem:top];
            if !should { return nil; }
        }
    }

    retain(env, top);
    env.objc.borrow_mut::<UINavigationItemHostObject>(top).navigation_bar = nil;
    () = msg![env; items removeLastObject];
    () = msg![env; this layoutSubviews];

    if delegate != nil {
        let sel = env.objc.register_host_selector(
            "navigationBar:didPopItem:".to_string(), &mut env.mem,
        );
        let responds: bool = msg![env; delegate respondsToSelector:sel];
        if responds {
            () = msg![env; delegate navigationBar:this didPopItem:top];
        }
    }
    crate::objc::autorelease(env, top)
}

// MARK: - Size

- (CGSize)sizeThatFits:(CGSize)size {
    // The legacy iPhone bar is compact in landscape; iPad uses 44 points.
    let ipad = env.options.device_family.is_some_and(|family| family.is_ipad());
    CGSize { width: size.width, height: if size.width > size.height && !ipad { 32.0 } else { 44.0 } }
}

- (())layoutSubviews {
    // A bar displays the top item's title/buttons and the preceding item's
    // back button, as specified by Apple's legacy navigation controller guide.
    // https://developer.apple.com/library/archive/documentation/WindowsViews/Conceptual/ViewControllerPGforiOSLegacy/NavigationControllers/NavigationControllers.html
    let old_views = std::mem::take(&mut env.objc.borrow_mut::<UINavigationBarHostObject>(this).displayed_views);
    for view in old_views {
        () = msg![env; view removeFromSuperview];
    }
    let top: id = msg![env; this topItem];
    if top == nil { return; }
    let bounds: CGRect = msg![env; this bounds];
    let mut left = button_items(env, top, true);
    let back: id = msg![env; this backItem];
    let hides_back: bool = msg![env; top hidesBackButton];
    if left.is_empty() && back != nil && !hides_back {
        let custom_back: id = msg![env; back backBarButtonItem];
        let mut title: id = if custom_back != nil { msg![env; custom_back title] } else { msg![env; back title] };
        if title == nil { title = get_static_str(env, "Back"); }
        let mut button = env.objc.borrow::<UINavigationBarHostObject>(this).back_button;
        if button == nil {
            let action = env.objc.register_host_selector("touchHLEBackButtonPressed".to_owned(), &mut env.mem);
            button = msg_class![env; UIBarButtonItem alloc];
            button = msg![env; button initWithTitle:title style:0i32 target:nil action:action];
            // UIControl keeps its targets weak. Avoid a bar/item retain cycle
            // in the emulator's existing UIBarButtonItem implementation.
            () = msg![env; button addTarget:this action:action forControlEvents:(1u32 << 6)];
            env.objc.borrow_mut::<UINavigationBarHostObject>(this).back_button = button;
        } else {
            () = msg![env; button setTitle:title];
        }
        left.push(button);
    }
    let right = button_items(env, top, false);
    let left_edge = layout_buttons(env, this, left, true, bounds);
    let right_edge = layout_buttons(env, this, right, false, bounds);
    let custom_title: id = msg![env; top titleView];
    let title_view = if custom_title != nil {
        custom_title
    } else {
        let mut label = env.objc.borrow::<UINavigationBarHostObject>(this).title_label;
        if label == nil {
            label = msg_class![env; UILabel new];
            let font: id = msg_class![env; UIFont boldSystemFontOfSize:17.0f32];
            let color: id = msg_class![env; UIColor whiteColor];
            let clear: id = msg_class![env; UIColor clearColor];
            () = msg![env; label setFont:font];
            () = msg![env; label setTextColor:color];
            () = msg![env; label setBackgroundColor:clear];
            () = msg![env; label setTextAlignment:1i32];
            env.objc.borrow_mut::<UINavigationBarHostObject>(this).title_label = label;
        }
        let title: id = msg![env; top title];
        () = msg![env; label setText:title];
        label
    };
    let margin = (left_edge - bounds.origin.x).max(bounds.origin.x + bounds.size.width - right_edge);
    let frame = CGRect {
        origin: CGPoint { x: bounds.origin.x + margin, y: bounds.origin.y },
        size: CGSize { width: (bounds.size.width - margin * 2.0).max(0.0), height: bounds.size.height },
    };
    () = msg![env; title_view setFrame:frame];
    () = msg![env; this addSubview:title_view];
    env.objc.borrow_mut::<UINavigationBarHostObject>(this).displayed_views.push(title_view);
}

- (())touchHLEBackButtonPressed {
    let _: id = msg![env; this popNavigationItemAnimated:true];
}

@end

@implementation UINavigationItem: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UINavigationItemHostObject {
        title: nil,
        title_view: nil,
        prompt: nil,
        back_button: nil,
        left_button: nil,
        right_button: nil,
        left_items: nil,
        right_items: nil,
        hides_back_button: false,
        left_items_supplemented: false,
        navigation_bar: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTitle:(id)title {
    retain(env, title);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).title = title;
    this
}

- (id)init {
    this
}

// --- ДОБАВЛЕНО: Поддержка загрузки из NIB-файла ---
- (id)initWithCoder:(id)_coder {
    msg_super![env; this init]
}
// --------------------------------------------------

- (())dealloc {
    let host = env.objc.borrow::<UINavigationItemHostObject>(this);
    let (title, title_view, prompt, back, left, right, left_items, right_items) = (
        host.title, host.title_view, host.prompt,
        host.back_button, host.left_button, host.right_button,
        host.left_items, host.right_items,
    );
    release(env, title);
    release(env, title_view);
    release(env, prompt);
    release(env, back);
    release(env, left);
    release(env, right);
    release(env, left_items);
    release(env, right_items);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)title {
    env.objc.borrow::<UINavigationItemHostObject>(this).title
}

- (())setTitle:(id)title {
    let old = env.objc.borrow::<UINavigationItemHostObject>(this).title;
    retain(env, title);
    release(env, old);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).title = title;
    item_did_change(env, this);
}

- (id)titleView {
    env.objc.borrow::<UINavigationItemHostObject>(this).title_view
}

- (())setTitleView:(id)view {
    let old = env.objc.borrow::<UINavigationItemHostObject>(this).title_view;
    release(env, old);
    retain(env, view);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).title_view = view;
    item_did_change(env, this);
}

- (id)prompt {
    env.objc.borrow::<UINavigationItemHostObject>(this).prompt
}

- (())setPrompt:(id)prompt {
    let old = env.objc.borrow::<UINavigationItemHostObject>(this).prompt;
    release(env, old);
    retain(env, prompt);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).prompt = prompt;
}

- (id)backBarButtonItem {
    env.objc.borrow::<UINavigationItemHostObject>(this).back_button
}

- (())setBackBarButtonItem:(id)item {
    let old = env.objc.borrow::<UINavigationItemHostObject>(this).back_button;
    release(env, old);
    retain(env, item);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).back_button = item;
    item_did_change(env, this);
}

- (id)leftBarButtonItem {
    env.objc.borrow::<UINavigationItemHostObject>(this).left_button
}

- (())setLeftBarButtonItem:(id)item {
    let old = env.objc.borrow::<UINavigationItemHostObject>(this).left_button;
    retain(env, item);
    release(env, old);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).left_button = item;
    item_did_change(env, this);
}

- (())setLeftBarButtonItem:(id)item animated:(bool)_animated {
    () = msg![env; this setLeftBarButtonItem:item];
}

- (id)rightBarButtonItem {
    env.objc.borrow::<UINavigationItemHostObject>(this).right_button
}

- (())setRightBarButtonItem:(id)item {
    let old = env.objc.borrow::<UINavigationItemHostObject>(this).right_button;
    retain(env, item);
    release(env, old);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).right_button = item;
    item_did_change(env, this);
}

- (())setRightBarButtonItem:(id)item animated:(bool)_animated {
    () = msg![env; this setRightBarButtonItem:item];
}

- (id)leftBarButtonItems {
    env.objc.borrow::<UINavigationItemHostObject>(this).left_items
}

- (())setLeftBarButtonItems:(id)items {
    let old = env.objc.borrow::<UINavigationItemHostObject>(this).left_items;
    release(env, old);
    retain(env, items);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).left_items = items;
    // Sync single-item accessor to first element.
    if items != nil {
        let count: u32 = msg![env; items count];
        let first: id = if count > 0 { msg![env; items objectAtIndex:0u32] } else { nil };
        () = msg![env; this setLeftBarButtonItem:first];
    }
}

- (id)rightBarButtonItems {
    env.objc.borrow::<UINavigationItemHostObject>(this).right_items
}

- (())setRightBarButtonItems:(id)items {
    let old = env.objc.borrow::<UINavigationItemHostObject>(this).right_items;
    release(env, old);
    retain(env, items);
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).right_items = items;
    if items != nil {
        let count: u32 = msg![env; items count];
        let first: id = if count > 0 { msg![env; items objectAtIndex:0u32] } else { nil };
        () = msg![env; this setRightBarButtonItem:first];
    }
}

- (bool)hidesBackButton {
    env.objc.borrow::<UINavigationItemHostObject>(this).hides_back_button
}

- (())setHidesBackButton:(bool)value {
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).hides_back_button = value;
    item_did_change(env, this);
}

- (())setHidesBackButton:(bool)value animated:(bool)_animated {
    () = msg![env; this setHidesBackButton:value];
}

- (bool)leftItemsSupplementBackButton {
    env.objc.borrow::<UINavigationItemHostObject>(this).left_items_supplemented
}

- (())setLeftItemsSupplementBackButton:(bool)value {
    env.objc.borrow_mut::<UINavigationItemHostObject>(this).left_items_supplemented = value;
}

@end

};

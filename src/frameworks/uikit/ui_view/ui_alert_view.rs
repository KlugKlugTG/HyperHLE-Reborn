/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIAlertView` — shows an SDL2 message box dialog.

use crate::frameworks::core_graphics::cg_affine_transform::{
    CGAffineTransform, CGAffineTransformIdentity,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{ns_string, NSInteger, NSUInteger};
use crate::objc::{
    id, msg, msg_class, msg_super, nil, objc_classes, release, retain, Class, ClassExports,
    HostObject, NSZonePtr,
};
use crate::window;

pub type UIAlertViewStyle = NSInteger;
pub const UIAlertViewStyleDefault: UIAlertViewStyle = 0;
pub const UIAlertViewStyleSecureTextInput: UIAlertViewStyle = 1;
pub const UIAlertViewStylePlainTextInput: UIAlertViewStyle = 2;
pub const UIAlertViewStyleLoginAndPasswordInput: UIAlertViewStyle = 3;

#[derive(Default)]
pub struct UIAlertViewHostObject {
    title: id,
    message: id,
    delegate: id,
    button_titles: id,
    cancel_button_index: NSInteger,
    visible: bool,
    alert_view_style: UIAlertViewStyle,
    tag: NSInteger,
    /// Views the app added with `-addSubview:`. Real `UIAlertView` is a
    /// `UIView`, so apps that need text input add their own `UITextField`
    /// (usually on top of a placeholder `@"\n\n"` message) — the classic
    /// pre-iOS 5 way of asking for a name.
    subviews: Vec<id>,
    /// The view among [Self::subviews] that input should be routed to.
    text_field: id,
    /// Panel of real views that hosts the text field while the alert is up.
    panel: id,
}
impl HostObject for UIAlertViewHostObject {}

#[derive(Default)]
pub struct State {
    /// The alert whose text field is currently the first responder, if any.
    /// The soft keyboard's return key dismisses that alert (see
    /// [handle_return]), because the alert's own buttons are not rendered.
    pub active_text_alert: Option<id>,
}

/// Hosts an alert's text field in a small panel of real views so the user can
/// actually type into it.
///
/// `UIAlertView` is not a view in this emulator (it used to be a blocking SDL2
/// message box), and a native message box cannot contain a text field the
/// guest can read back. Pre-iOS 5 apps ask for input the classic way: add a
/// `UITextField` to the alert on top of a `@"\n\n"` placeholder message.
/// Those alerts get a panel that the compositor draws, the app's own field
/// inside it, and the host keyboard. Returns false if there is nothing to edit.
///
/// Unlike the message box, this does not block the guest: real
/// `-[UIAlertView show]` returns immediately, and the guest has to keep running
/// for the typed characters to reach it.
fn show_text_alert(env: &mut crate::Environment, alert: id) -> bool {
    let (text_field, title, message) = {
        let host = env.objc.borrow::<UIAlertViewHostObject>(alert);
        (host.text_field, host.title, host.message)
    };
    if text_field == nil {
        return false;
    }

    // Prefer the key window, but fall back to the topmost visible window: many
    // games (Heavy Mach 2 included) build their window from a NIB without
    // ever sending it `-makeKeyAndVisible`.
    let alert_window = env
        .framework_state
        .uikit
        .ui_view
        .ui_window
        .key_window
        .or_else(|| {
            env.framework_state
                .uikit
                .ui_view
                .ui_window
                .windows
                .last()
                .copied()
        });

    if let Some(window) = alert_window {
        let window_bounds: CGRect = msg![env; window bounds];

        let panel_width = (window_bounds.size.width - 32.0).clamp(180.0, 280.0);
        let panel_height = 132.0;
        let panel_rect = CGRect {
            origin: CGPoint {
                x: ((window_bounds.size.width - panel_width) / 2.0).max(0.0),
                y: ((window_bounds.size.height - panel_height) / 2.0).max(0.0),
            },
            size: CGSize {
                width: panel_width,
                height: panel_height,
            },
        };

        let panel: id = msg_class![env; UIView alloc];
        let panel: id = msg![env; panel initWithFrame:panel_rect];
        let panel_color: id = msg_class![env; UIColor colorWithWhite:0.10 alpha:0.96];
        let _: () = msg![env; panel setBackgroundColor:panel_color];

        let inner_width = panel_width - 24.0;
        if title != nil && !ns_string::to_rust_string(env, title).trim().is_empty() {
            add_alert_label(
                env,
                panel,
                title,
                CGRect {
                    origin: CGPoint { x: 12.0, y: 10.0 },
                    size: CGSize { width: inner_width, height: 22.0 },
                },
                16.0,
                true,
            );
        }
        if message != nil && !ns_string::to_rust_string(env, message).trim().is_empty() {
            add_alert_label(
                env,
                panel,
                message,
                CGRect {
                    origin: CGPoint { x: 12.0, y: 36.0 },
                    size: CGSize { width: inner_width, height: 42.0 },
                },
                13.0,
                false,
            );
        }

        // Keep the app's own text field, but place it inside the panel: apps
        // position it relative to the real alert, whose geometry we do not
        // reproduce.
        let field_frame: CGRect = msg![env; text_field frame];
        let field_height = if field_frame.size.height > 8.0 {
            field_frame.size.height
        } else {
            28.0
        };
        let field_rect = CGRect {
            origin: CGPoint {
                x: 12.0,
                y: panel_height - field_height - 14.0,
            },
            size: CGSize {
                width: inner_width,
                height: field_height,
            },
        };
        let _: () = msg![env; text_field setFrame:field_rect];
        // `UIView::setFrame:` only forwards `-layoutSubviews` to a subview
        // whose size changed, so a text field whose label started at zero size
        // would keep an empty label frame (and therefore draw no text).
        let _: () = msg![env; text_field layoutSubviews];
        let _: () = msg![env; panel addSubview:text_field];

        // The window's coordinate space is the device's *portrait* space even
        // when the app runs in landscape: the app rotates its own drawing and
        // the presenter rotates the finished frame. A panel built by the host
        // has to pre-rotate itself the same way, or it shows up sideways.
        let panel_transform = panel_transform_for_orientation(
            env.window().current_rotation(),
        );
        let _: () = msg![env; panel setTransform:panel_transform];

        let _: () = msg![env; window addSubview:panel];

        retain(env, panel);
        env.objc.borrow_mut::<UIAlertViewHostObject>(alert).panel = panel;
        log!(
            "UIAlertView: hosting the text field {:?} in a panel {:?} so the host keyboard can be used",
            text_field,
            panel
        );
    } else {
        log!(
            "UIAlertView: no key window; making the text field {:?} the first responder without a panel",
            text_field
        );
    }

    env.framework_state.uikit.ui_alert_view.active_text_alert = Some(alert);
    let became: bool = msg![env; text_field becomeFirstResponder];
    if !became {
        log!("UIAlertView: the text field refused to become the first responder");
        env.framework_state.uikit.ui_alert_view.active_text_alert = None;
        return false;
    }
    true
}

/// Compensates for the presenter's rotation of the finished frame, so a panel
/// laid out in the window's portrait space appears upright on a rotated
/// display. The signs match what the app's own EAGL view effectively does:
/// landscape content is drawn a quarter turn away from portrait.
fn panel_transform_for_orientation(
    orientation: crate::window::DeviceOrientation,
) -> CGAffineTransform {
    use crate::window::DeviceOrientation;
    let quarter = std::f32::consts::FRAC_PI_2;
    match orientation {
        DeviceOrientation::Portrait => CGAffineTransformIdentity,
        DeviceOrientation::PortraitUpsideDown => CGAffineTransform::make_rotation(std::f32::consts::PI),
        DeviceOrientation::LandscapeLeft => CGAffineTransform::make_rotation(quarter),
        DeviceOrientation::LandscapeRight => CGAffineTransform::make_rotation(-quarter),
    }
}

/// Undoes [show_text_alert]: drops the panel, stops editing. Safe to call for
/// alerts that never had a text field.
fn teardown_text_alert(env: &mut crate::Environment, alert: id) {
    let (panel, text_field) = {
        let host = env.objc.borrow::<UIAlertViewHostObject>(alert);
        (host.panel, host.text_field)
    };
    if env.framework_state.uikit.ui_alert_view.active_text_alert == Some(alert) {
        env.framework_state.uikit.ui_alert_view.active_text_alert = None;
    }
    if panel == nil {
        return;
    }
    if text_field != nil {
        let _: () = msg![env; text_field resignFirstResponder];
        let _: () = msg![env; text_field removeFromSuperview];
    }
    let _: () = msg![env; panel removeFromSuperview];
    env.objc.borrow_mut::<UIAlertViewHostObject>(alert).panel = nil;
    release(env, panel);
}

/// Creates the text field that `alertViewStyle` promises, for apps written
/// against iOS 5+ that never call `-addSubview:` themselves.
fn create_style_text_field(env: &mut crate::Environment, alert: id) {
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: 260.0, height: 28.0 },
    };
    let field: id = msg_class![env; UITextField alloc];
    let field: id = msg![env; field initWithFrame:frame];
    retain(env, field);
    let host = env.objc.borrow_mut::<UIAlertViewHostObject>(alert);
    host.text_field = field;
    host.subviews.push(field);
    log!("UIAlertView: created a text field for alertViewStyle");
}

/// Adds a white label to an alert panel. `UILabel` draws nothing without a
/// font and a text color (see its `-drawRect:`), so both are always set.
fn add_alert_label(
    env: &mut crate::Environment,
    panel: id,
    text: id,
    frame: CGRect,
    font_size: CGFloat,
    bold: bool,
) {
    let label: id = msg_class![env; UILabel alloc];
    let label: id = msg![env; label initWithFrame:frame];
    let clear: id = msg_class![env; UIColor clearColor];
    let white: id = msg_class![env; UIColor whiteColor];
    let font: id = if bold {
        msg_class![env; UIFont boldSystemFontOfSize:font_size]
    } else {
        msg_class![env; UIFont systemFontOfSize:font_size]
    };
    let _: () = msg![env; label setBackgroundColor:clear];
    let _: () = msg![env; label setTextColor:white];
    let _: () = msg![env; label setFont:font];
    let _: () = msg![env; label setText:text];
    let _: () = msg![env; panel addSubview:label];
    release(env, label);
}

/// Whether a view the app added to an alert is something the user can type
/// into. `UITextField` and `UITextView` are UIKit's two text inputs, and
/// neither implements `-insertText:` itself (the emulator dispatches keyboard
/// text to them by class), so this has to be a class check.
fn view_accepts_text_input(env: &mut crate::Environment, view: id) -> bool {
    if view == nil {
        return false;
    }
    let class: Class = msg![env; view class];
    for name in ["UITextField", "UITextView"] {
        let known = env.objc.get_known_class(name, &mut env.mem);
        if known != nil && env.objc.class_is_subclass_of(class, known) {
            return true;
        }
    }
    false
}

/// Whether `responder` is the text field of the alert that is waiting for
/// input. Used to turn a newline arriving as typed text into a confirmation.
pub fn is_active_text_field(env: &mut crate::Environment, responder: id) -> bool {
    let Some(alert) = env.framework_state.uikit.ui_alert_view.active_text_alert else {
        return false;
    };
    env.objc.borrow::<UIAlertViewHostObject>(alert).text_field == responder
}

/// Treats the return key as "the user pressed the alert's default button" for
/// an alert that is waiting for text. Alerts are rendered by the host, so they
/// have no buttons to tap; without this the app would never learn that the
/// user finished typing. Returns true if the event was consumed.
pub fn handle_return(env: &mut crate::Environment, responder: id) -> bool {
    let Some(alert) = env.framework_state.uikit.ui_alert_view.active_text_alert else {
        return false;
    };
    if env.objc.borrow::<UIAlertViewHostObject>(alert).text_field != responder {
        return false;
    }
    let cancel_button_index = env
        .objc
        .borrow::<UIAlertViewHostObject>(alert)
        .cancel_button_index;
    let button_index = if cancel_button_index >= 0 {
        cancel_button_index
    } else {
        0
    };
    log!(
        "UIAlertView: return key confirmed the text-input alert; dismissing with button index {}",
        button_index
    );
    let _: () = msg![env; alert dismissWithClickedButtonIndex:button_index animated:false];
    true
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIAlertView: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIAlertViewHostObject {
        title:               nil,
        message:             nil,
        delegate:            nil,
        button_titles:       nil,
        cancel_button_index: -1,
        visible:             false,
        alert_view_style:    UIAlertViewStyleDefault,
        tag:                 0,
        subviews:            Vec::new(),
        text_field:          nil,
        panel:               nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTitle:(id)title
            message:(id)message
           delegate:(id)delegate
  cancelButtonTitle:(id)cancel_title
  otherButtonTitles:(id)other_titles {

    // ВАЖНО: Вызов базового инициализатора для корректной регистрации объекта
    let this: id = msg_super![env; this init];

    let buttons: id = msg_class![env; NSMutableArray new];
    retain(env, title);
    retain(env, message);
    {
        // ИСПРАВЛЕНИЕ: Добавлен `mut`, так как изменение полей `RefMut` требует
        // мутабельности переменной
        let host = env.objc.borrow_mut::<UIAlertViewHostObject>(this);
        host.title    = title;
        host.message  = message;
        host.delegate = delegate;
        host.button_titles = buttons;
    }
    if cancel_title != nil {
        let idx: NSUInteger = msg![env; buttons count];
        let _: () = msg![env; buttons addObject:cancel_title];
        env.objc.borrow_mut::<UIAlertViewHostObject>(this).cancel_button_index = idx as NSInteger;
    }
    if other_titles != nil {
        let _: () = msg![env; buttons addObject:other_titles];
    }
    let title_str = if title != nil { ns_string::to_rust_string(env, title).into_owned() } else { "(nil)".into() };
    let msg_str   = if message != nil { ns_string::to_rust_string(env, message).into_owned() } else { "(nil)".into() };
    log!("UIAlertView init title={:?} message={:?}", title_str, msg_str);
    this
}

- (())dealloc {
    // ИСПРАВЛЕНИЕ: Блокируем `host` в узком scope, чтобы снять заимствование до
    // `release`
    let (title, message, buttons, subviews, panel) = {
        let host = env.objc.borrow::<UIAlertViewHostObject>(this);
        (host.title, host.message, host.button_titles, host.subviews.clone(), host.panel)
    };
    release(env, title);
    release(env, message);
    release(env, buttons);
    for subview in subviews {
        release(env, subview);
    }
    if panel != nil {
        release(env, panel);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)title   { env.objc.borrow::<UIAlertViewHostObject>(this).title }
- (id)message { env.objc.borrow::<UIAlertViewHostObject>(this).message }
- (id)delegate { env.objc.borrow::<UIAlertViewHostObject>(this).delegate }
- (())setTitle:(id)title {
    let old = env.objc.borrow::<UIAlertViewHostObject>(this).title;
    release(env, old); retain(env, title);
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).title = title;
}
- (())setMessage:(id)message {
    let old = env.objc.borrow::<UIAlertViewHostObject>(this).message;
    release(env, old); retain(env, message);
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).message = message;
}
- (())setDelegate:(id)delegate {
    // Делегаты в UIKit не удерживаются!
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).delegate = delegate;
}
- (NSInteger)tag { env.objc.borrow::<UIAlertViewHostObject>(this).tag }
- (())setTag:(NSInteger)tag { env.objc.borrow_mut::<UIAlertViewHostObject>(this).tag = tag; }
- (UIAlertViewStyle)alertViewStyle { env.objc.borrow::<UIAlertViewHostObject>(this).alert_view_style }
- (())setAlertViewStyle:(UIAlertViewStyle)style { env.objc.borrow_mut::<UIAlertViewHostObject>(this).alert_view_style = style; }
- (bool)isVisible { env.objc.borrow::<UIAlertViewHostObject>(this).visible }

- (NSInteger)addButtonWithTitle:(id)title {
    let buttons = env.objc.borrow::<UIAlertViewHostObject>(this).button_titles;
    let idx: NSUInteger = msg![env; buttons count];
    let _: () = msg![env; buttons addObject:title];
    idx as NSInteger
}
- (NSUInteger)numberOfButtons {
    let buttons = env.objc.borrow::<UIAlertViewHostObject>(this).button_titles;
    msg![env; buttons count]
}
- (id)buttonTitleAtIndex:(NSInteger)index {
    let buttons = env.objc.borrow::<UIAlertViewHostObject>(this).button_titles;
    let count: NSUInteger = msg![env; buttons count];
    if index < 0 || index as NSUInteger >= count { return nil; }
    msg![env; buttons objectAtIndex:(index as NSUInteger)]
}
- (NSInteger)cancelButtonIndex { env.objc.borrow::<UIAlertViewHostObject>(this).cancel_button_index }
- (())setCancelButtonIndex:(NSInteger)index { env.objc.borrow_mut::<UIAlertViewHostObject>(this).cancel_button_index = index; }
- (NSInteger)firstOtherButtonIndex {
    // ИСПРАВЛЕНИЕ: Скоуп не даст возникнуть ошибке заимствования на `msg![env;
    // ...]`
    let (buttons, cancel) = {
        let host = env.objc.borrow::<UIAlertViewHostObject>(this);
        (host.button_titles, host.cancel_button_index)
    };
    let count: NSUInteger = msg![env; buttons count];
    for i in 0..count { if i as NSInteger != cancel { return i as NSInteger; } }
    -1
}
- (id)textFieldAtIndex:(NSInteger)index {
    // iOS 5+ apps ask the alert for the text field it created for them when
    // `alertViewStyle` is one of the input styles; older apps add their own
    // field with `-addSubview:` instead. Support both.
    if index != 0 {
        return nil;
    }
    if env.objc.borrow::<UIAlertViewHostObject>(this).text_field == nil {
        let style: UIAlertViewStyle = msg![env; this alertViewStyle];
        if matches!(
            style,
            UIAlertViewStylePlainTextInput
                | UIAlertViewStyleSecureTextInput
                | UIAlertViewStyleLoginAndPasswordInput
        ) {
            create_style_text_field(env, this);
        }
    }
    env.objc.borrow::<UIAlertViewHostObject>(this).text_field
}

- (())addSubview:(id)view {
    if view == nil {
        return;
    }
    // Real UIAlertView is a UIView, and pre-iOS 5 apps put a UITextField in it
    // to ask the user for input. Keep the view so [show] can host it, and
    // remember the first text input among them as the field to edit.
    let is_text_input = view_accepts_text_input(env, view);
    retain(env, view);
    let host = env.objc.borrow_mut::<UIAlertViewHostObject>(this);
    if is_text_input && host.text_field == nil {
        host.text_field = view;
    }
    host.subviews.push(view);
    log!("UIAlertView addSubview:{:?} (text_input={})", view, is_text_input);
}
- (())removeFromSuperview {
    log_dbg!("UIAlertView removeFromSuperview: ignored");
}
- (())setHidden:(bool)_hidden {
    log_dbg!("UIAlertView setHidden: ignored");
}
- (CGRect)frame {
    CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 0.0, height: 0.0 } }
}
- (())setFrame:(CGRect)_frame {
    log_dbg!("UIAlertView setFrame: ignored");
}

// UIAlertView inherits from UIView in real iOS.  We can't structurally
// derive from UIView here (SDL2 dialog), so we stub the selectors that
// guest apps actually invoke on alert views.
- (())setTransform:(CGAffineTransform)_transform {
    // UIAlertView in touchHLE is rendered via SDL2 system dialog,
    // so geometric transforms are not applicable.
    log_dbg!("UIAlertView setTransform: ignored (SDL2 dialog)");
}

- (id)viewWithTag:(NSInteger)tag {
    // Real UIAlertView would search subviews, but touchHLE doesn't manage
    // a subview hierarchy for alerts.  Return self if the tag matches,
    // otherwise nil (Apple semantics: receiver is searched first).
    let own_tag: NSInteger = msg![env; this tag];
    if own_tag == tag { return this; }
    nil
}

- (CGSize)sizeThatFits:(CGSize)size {
    // В iOS этот метод возвращает оптимальный размер на основе содержимого.
    // Так как touchHLE выводит SDL2-диалог, размер контролируется самой ОС,
    // поэтому мы пробрасываем текущий запрошенный размер дальше.
    size
}

- (())sizeToFit {
    // 1. Получаем текущий frame
    let frame: CGRect = msg![env; this frame];

    // 2. ВАЖНО: Выносим размер в отдельную переменную.
    // Макрос msg! не принимает "frame.size" как аргумент после двоеточия.
    let current_size = frame.size;

    // 3. Запрашиваем подходящий размер
    let new_size: CGSize = msg![env; this sizeThatFits:current_size];

    // 4. Формируем новый frame и применяем его
    let new_frame = CGRect { origin: frame.origin, size: new_size };
    let _: () = msg![env; this setFrame:new_frame];
}

- (())show {
    log!("UIAlertView show");
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).visible = true;

    // An alert that asks for text must not go through the blocking SDL2
    // message box: the guest would be frozen inside this call and could never
    // receive the typed characters, and the emulator has no way to render a
    // text field inside a native message box. Host the app's own text field in
    // a small panel of real views instead, so the host keyboard opens, the
    // text is delivered to the guest as usual, and the return key confirms.
    if env.objc.borrow::<UIAlertViewHostObject>(this).text_field != nil {
        if show_text_alert(env, this) {
            return;
        }
    }

    let (title, message, buttons, cancel_index) = {
        let h = env.objc.borrow::<UIAlertViewHostObject>(this);
        (h.title, h.message, h.button_titles, h.cancel_button_index)
    };

    // Raw (un-substituted) strings, used to decide whether the alert
    // actually has anything to show the user.
    let raw_title: String = if title != nil {
        ns_string::to_rust_string(env, title).into_owned()
    } else { String::new() };
    let raw_message: String = if message != nil {
        ns_string::to_rust_string(env, message).into_owned()
    } else { String::new() };

    // touchHLE renders UIAlertView as a *blocking* SDL2 system dialog,
    // whereas real iOS `-[UIAlertView show]` is asynchronous and returns
    // immediately. Some apps (notably Outfit7 titles like Talking Angela)
    // create a content-less alert — empty/`nil` title *and* message — as a
    // transient placeholder that they dismiss programmatically once some
    // background work finishes. Presenting a blocking modal for such an
    // alert freezes the app behind an empty dialog box that the user can
    // never meaningfully act on. Since there is nothing to display, skip
    // the native dialog and simulate an immediate dismissal so the guest's
    // run loop keeps going (matching iOS's non-blocking semantics). Any
    // delegate callbacks are still delivered via dismissWithClickedButtonIndex.
    if raw_title.trim().is_empty() && raw_message.trim().is_empty() {
        log!(
            "UIAlertView show: empty title and message; \
             skipping blocking SDL2 dialog and dismissing asynchronously"
        );
        let dismiss_index = if cancel_index >= 0 { cancel_index } else { 0 };
        let _: () = msg![env; this dismissWithClickedButtonIndex:dismiss_index animated:false];
        return;
    }

    let title_str: String = if raw_title.is_empty() { "Alert".into() } else { raw_title };
    let message_str: String = raw_message;

    let btn_count: NSUInteger = msg![env; buttons count];
    let mut btn_strings: Vec<String> = Vec::new();
    for i in 0..btn_count {
        let btn: id = msg![env; buttons objectAtIndex:i];
        btn_strings.push(if btn != nil {
            ns_string::to_rust_string(env, btn).into_owned()
        } else { format!("Button {}", i) });
    }
    if btn_strings.is_empty() { btn_strings.push("OK".into()); }

    let btn_refs: Vec<&str> = btn_strings.iter().map(|s| s.as_str()).collect();
    let clicked = window::show_alert_dialog(env, &title_str, &message_str, &btn_refs);

    let dismiss_index = if clicked >= 0 && (clicked as NSUInteger) < btn_count {
        clicked as NSInteger
    } else if cancel_index >= 0 {
        cancel_index
    } else { 0 };

    let _: () = msg![env; this dismissWithClickedButtonIndex:dismiss_index animated:false];
}

- (())dismissWithClickedButtonIndex:(NSInteger)button_index animated:(bool)_animated {
    teardown_text_alert(env, this);
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).visible = false;
    let delegate = env.objc.borrow::<UIAlertViewHostObject>(this).delegate;

    // Честно проверяем, не был ли делегат удален (isa != 0)
    if delegate != nil {
        let isa: u32 = env.mem.read(delegate.cast());
        if isa != 0 {
            if let Some(sel) = env.objc.lookup_selector("alertView:clickedButtonAtIndex:") {
                let responds: bool = msg![env; delegate respondsToSelector:sel];
                if responds { let _: () = msg![env; delegate alertView:this clickedButtonAtIndex:button_index]; }
            }
            if let Some(sel) = env.objc.lookup_selector("alertView:willDismissWithButtonIndex:") {
                let responds: bool = msg![env; delegate respondsToSelector:sel];
                if responds { let _: () = msg![env; delegate alertView:this willDismissWithButtonIndex:button_index]; }
            }
            if let Some(sel) = env.objc.lookup_selector("alertView:didDismissWithButtonIndex:") {
                let responds: bool = msg![env; delegate respondsToSelector:sel];
                if responds { let _: () = msg![env; delegate alertView:this didDismissWithButtonIndex:button_index]; }
            }
        }
    }
}

- (id)description {
    let (title, visible) = { let h = env.objc.borrow::<UIAlertViewHostObject>(this); (h.title, h.visible) };
    let title_str = if title != nil { ns_string::to_rust_string(env, title).into_owned() } else { "(nil)".into() };
    let s = format!("<UIAlertView: {:?}; title={:?}; visible={}>", this, title_str, visible);
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};

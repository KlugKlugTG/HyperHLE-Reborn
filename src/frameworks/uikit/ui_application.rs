/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIApplication` and `UIApplicationMain`.

use crate::abi::{CallFromHost, GuestFunction};

use super::ui_device::*;
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str};
use crate::frameworks::foundation::{ns_array, ns_string, NSInteger, NSUInteger};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr, SEL,
};
use crate::window::DeviceOrientation;
use crate::Environment;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    /// [UIApplication sharedApplication]
    shared_application: Option<id>,
    pub(super) status_bar_hidden: bool,
    /// Whether shake to edit is enabled
    pub(super) application_supports_shake_to_edit: bool,
    pub(super) ignoring_interaction_events_count: u32,
}

#[derive(Default)]
struct UIApplicationHostObject {
    delegate: id,
    delegate_is_retained: bool,
    status_bar_style: UIStatusBarStyle,
    /// The most recent value set via `-setApplicationIconBadgeNumber:`.
    /// Per Apple's UIApplication docs the property is read/write and
    /// defaults to 0; we honour both the getter and the setter even
    /// though touchHLE has no springboard to actually render the badge.
    application_icon_badge_number: NSInteger,
    remote_notifications_registered: bool,
    user_notification_settings: id,
    background_tasks: HashMap<NSUInteger, BackgroundTask>,
    next_background_task_id: NSUInteger,
}

struct BackgroundTask {
    name: id,
    expiration_handler: id,
    expiration_running: bool,
}

fn release_background_task(env: &mut Environment, task: BackgroundTask) {
    release(env, task.name);
    crate::libc::blocks::_Block_release(env, task.expiration_handler.cast_void().cast_const());
}

fn invoke_void_block(env: &mut Environment, block: id) {
    if block == nil {
        return;
    }
    let invoke_address: u32 = env.mem.read(block.cast::<u32>() + 3u32);
    if invoke_address != 0 {
        let invoke = GuestFunction::from_addr_with_thumb_bit(invoke_address);
        let _: () = invoke.call_from_host(env, (block,));
    }
}

fn expire_background_tasks(env: &mut Environment, application: id) {
    let identifiers: Vec<NSUInteger> = env
        .objc
        .borrow::<UIApplicationHostObject>(application)
        .background_tasks
        .keys()
        .copied()
        .collect();
    for identifier in identifiers {
        let handler = {
            let host = &mut *env.objc.borrow_mut::<UIApplicationHostObject>(application);
            let Some(task) = host.background_tasks.get_mut(&identifier) else {
                continue;
            };
            if task.expiration_running {
                continue;
            }
            task.expiration_running = true;
            task.expiration_handler
        };
        invoke_void_block(env, handler);
        let task = env
            .objc
            .borrow_mut::<UIApplicationHostObject>(application)
            .background_tasks
            .remove(&identifier);
        if let Some(task) = task {
            release_background_task(env, task);
        }
    }
}
impl HostObject for UIApplicationHostObject {}

#[derive(Default)]
struct UIUserNotificationSettingsHostObject {
    types: NSUInteger,
    categories: id,
}
impl HostObject for UIUserNotificationSettingsHostObject {}

pub type UIInterfaceOrientation = UIDeviceOrientation;
#[allow(unused)]
pub const UIInterfaceOrientationPortrait: UIInterfaceOrientation = UIDeviceOrientationPortrait;
#[allow(unused)]
pub const UIInterfaceOrientationPortraitUpsideDown: UIInterfaceOrientation =
    UIDeviceOrientationPortraitUpsideDown;
// These are intentionally swapped and documented as such (the UI on the device
// rotates in the opposite direction to how the device is rotated).
pub const UIInterfaceOrientationLandscapeLeft: UIInterfaceOrientation =
    UIDeviceOrientationLandscapeRight;
pub const UIInterfaceOrientationLandscapeRight: UIInterfaceOrientation =
    UIDeviceOrientationLandscapeLeft;
type UIRemoteNotificationType = NSUInteger;
type UIStatusBarAnimation = NSInteger;
type UIStatusBarStyle = NSInteger;
pub type UIApplicationState = NSInteger;
pub const UIApplicationStateActive: UIApplicationState = 0;
pub const UIApplicationStateInactive: UIApplicationState = 1;
pub const UIApplicationStateBackground: UIApplicationState = 2;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);
@implementation UIApplication: UIResponder

// This should only be called by UIApplicationMain
+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIApplicationHostObject {
        delegate: nil,
        delegate_is_retained: false,
        status_bar_style: 0,
        application_icon_badge_number: 0,
        remote_notifications_registered: false,
        user_notification_settings: nil,
        background_tasks: HashMap::new(),
        next_background_task_id: 1,
    });
    env.objc.alloc_static_object(this, host_object, &mut env.mem)
}

+ (id)sharedApplication {
    env.framework_state.uikit.ui_application.shared_application.unwrap_or(nil)
}

- (())setNetworkActivityIndicatorVisible:(bool)visible {
    // touchHLE doesn't render the iOS status bar, so we just stub this
    // and ignore the request to show/hide the spinner.
    log_dbg!("Stubbed setNetworkActivityIndicatorVisible: {}", visible);
}

- (bool)isNetworkActivityIndicatorVisible {
    // Always report that it's hidden.
    false
}

// This should only be called by UIApplicationMain
- (id)init {
    assert!(env.framework_state.uikit.ui_application.shared_application.is_none());
    env.framework_state.uikit.ui_application.shared_application = Some(this);
    this
}

// This is a singleton, it shouldn't be deallocated.
- (id)retain { this }
- (id)autorelease { this }
- (())release {}

- (id)delegate {
    env.objc.borrow::<UIApplicationHostObject>(this).delegate
}
- (())setDelegate:(id)delegate { // something implementing UIApplicationDelegate
    let host_object = env.objc.borrow_mut::<UIApplicationHostObject>(this);
    // This property is quasi-non-retaining: https://stackoverflow.com/a/14271150/736162
    let old_delegate = std::mem::replace(&mut host_object.delegate, delegate);
    if host_object.delegate_is_retained {
        host_object.delegate_is_retained = false;
        if delegate != old_delegate {
            release(env, old_delegate);
        }
    }
}

- (bool)isStatusBarHidden {
    env.framework_state.uikit.ui_application.status_bar_hidden
}
- (())setStatusBarHidden:(bool)hidden {
    env.framework_state.uikit.ui_application.status_bar_hidden = hidden;
}
- (())setStatusBarHidden:(bool)hidden
                animated:(bool)_animated {
    // TODO: animation
    msg![env; this setStatusBarHidden:hidden]
}
- (())setStatusBarHidden:(bool)hidden
           withAnimation:(UIStatusBarAnimation)_animation {
    // TODO: animation
    msg![env; this setStatusBarHidden:hidden]
}

- (())setStatusBarStyle:(UIStatusBarStyle)style {
    env.objc.borrow_mut::<UIApplicationHostObject>(this).status_bar_style = style;
}
- (UIStatusBarStyle)statusBarStyle {
    env.objc.borrow::<UIApplicationHostObject>(this).status_bar_style
}

- (())setStatusBarStyle:(UIStatusBarStyle)style
               animated:(bool)_animated {
    msg![env; this setStatusBarStyle:style]
}

- (UIInterfaceOrientation)statusBarOrientation {
    // ULTRAHLE_MINIONJUMP_STATUSBAR_BEGIN
    if matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) {
        return 4 as UIInterfaceOrientation;
    }
    // ULTRAHLE_MINIONJUMP_STATUSBAR_END

    match env.window().current_rotation() {
        DeviceOrientation::Portrait => UIDeviceOrientationPortrait,
        DeviceOrientation::PortraitUpsideDown => UIDeviceOrientationPortraitUpsideDown,
        DeviceOrientation::LandscapeLeft => UIDeviceOrientationLandscapeLeft,
        DeviceOrientation::LandscapeRight => UIDeviceOrientationLandscapeRight
    }
}

- (f64)statusBarOrientationAnimationDuration {
    0.3
}

- (())setStatusBarOrientation:(UIInterfaceOrientation)orientation {
    // touchHLE rotates the simulated device here, because many old games call
    // this to pick their orientation and then rotate their drawing to match.
    // Once a root view controller has been autorotated to landscape, though,
    // its view has landscape bounds and the app draws for that, so going back
    // to portrait would show its landscape frame cropped inside a portrait
    // screen. On a real device the status bar call doesn't rotate the device
    // either. cocos2d's UIViewController autorotation template does exactly
    // this, calling [director setDeviceOrientation:kCCDeviceOrientationPortrait]
    // after the view controller rotated (Gravity Guy).
    if env.framework_state.uikit.autorotation_transform_applied
        && matches!(orientation, UIDeviceOrientationPortrait | UIDeviceOrientationPortraitUpsideDown)
        && env.window.as_ref().is_some_and(|window| {
            matches!(
                window.current_rotation(),
                DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight
            )
        })
    {
        log_dbg!(
            "setStatusBarOrientation:{} ignored: root view controller is autorotated to landscape",
            orientation
        );
        return;
    }
    match orientation {
        UIDeviceOrientationUnknown => {
            // Per Apple docs UIDeviceOrientationUnknown (0) means the
            // orientation cannot be determined.  Ignore it.
        }
        UIDeviceOrientationPortrait => {
            env.on_parent_stack_in_coroutine(|window, _| window.rotate_device(DeviceOrientation::Portrait));
        }
        UIDeviceOrientationPortraitUpsideDown => {
            env.on_parent_stack_in_coroutine(|window, _| window.rotate_device(DeviceOrientation::PortraitUpsideDown));
        }
        UIDeviceOrientationLandscapeLeft => {
            env.on_parent_stack_in_coroutine(|window, _| window.rotate_device(DeviceOrientation::LandscapeLeft));
        }
        UIDeviceOrientationLandscapeRight => {
            env.on_parent_stack_in_coroutine(|window, _| window.rotate_device(DeviceOrientation::LandscapeRight));
        }
        _ => {
            log!("Warning: Orientation {} not handled yet (ignoring to prevent panic)", orientation);
        }
    }
}

- (())setStatusBarOrientation:(UIInterfaceOrientation)orientation
                     animated:(bool)_animated {
    // TODO: animation
    msg![env; this setStatusBarOrientation:orientation]
}

- (bool)isIdleTimerDisabled {
    !env.window().is_screen_saver_enabled()
}
- (())setIdleTimerDisabled:(bool)disabled {
    env.on_parent_stack_in_coroutine(|window, _| window.set_screen_saver_enabled(!disabled))
}

- (bool)canOpenURL:(id)url { // NSURL
    // Apple `-[UIApplication canOpenURL:]` documentation: returns `YES`
    // if the URL can be handled — i.e. the device has at least one
    // installed application that has registered for the URL's scheme
    // (`CFBundleURLTypes` → `CFBundleURLSchemes`). On iOS 9+ apps must
    // additionally list each scheme they intend to query in
    // `LSApplicationQueriesSchemes`, otherwise the call always returns
    // NO.
    //
    // touchHLE has no app launcher, but legitimately reports YES for
    // the URL schemes that the host environment (Android) routes to
    // its own Intent handlers — `http`/`https`/`tel`/`mailto`/`sms`
    // are universally handled, and we permit them here so apps that
    // gate sharing buttons on `canOpenURL:` (Talking Carl's social
    // links, the Bubble Witch share sheet, the Imobamoba "rate me"
    // popup) take the "yes, link the user out" branch instead of
    // greying out the button. Schemes in the app's own
    // `CFBundleURLTypes` are checked below. Other application-specific
    // schemes (e.g. `fb://`, `twitter://`) remain unavailable because
    // no host-side app responds to them.
    if url == nil {
        return false;
    }
    let ns_string: id = msg![env; url scheme];
    if ns_string == nil {
        return false;
    }
    let scheme = ns_string::to_rust_string(env, ns_string);
    let scheme_lower = scheme.to_lowercase();

    // Schemes the host environment (browsers / mail / phone / SMS /
    // file viewers) is guaranteed to handle on every reasonable
    // device. Source: Apple "URL Schemes" Technote (
    // https://developer.apple.com/library/archive/featuredarticles/iPhoneURLScheme_Reference/
    // ).
    const HOST_HANDLED_SCHEMES: &[&str] = &[
        "http",
        "https",
        "ftp",
        "tel",
        "telprompt",
        "facetime",
        "facetime-audio",
        "mailto",
        "sms",
        "imessage",
        "file",
        "data",
        "itms",
        "itms-apps",
        "itms-services",
        "itmss",
        "maps",
    ];
    if HOST_HANDLED_SCHEMES.contains(&scheme_lower.as_str()) {
        return true;
    }

    let main_bundle: id = msg_class![env; NSBundle mainBundle];
    if main_bundle != nil {
        let url_types_key = ns_string::get_static_str(env, "CFBundleURLTypes");
        let url_types: id = msg![env; main_bundle objectForInfoDictionaryKey:url_types_key];
        if url_types != nil {
            let schemes_key = ns_string::get_static_str(env, "CFBundleURLSchemes");
            let type_count: u32 = msg![env; url_types count];
            for i in 0..type_count {
                let url_type: id = msg![env; url_types objectAtIndex:i];
                if url_type == nil {
                    continue;
                }
                let schemes: id = msg![env; url_type objectForKey:schemes_key];
                if schemes == nil {
                    continue;
                }
                let scheme_count: u32 = msg![env; schemes count];
                for j in 0..scheme_count {
                    let registered_scheme: id = msg![env; schemes objectAtIndex:j];
                    if registered_scheme == nil {
                        continue;
                    }
                    let registered_scheme = ns_string::to_rust_string(env, registered_scheme);
                    if registered_scheme.to_lowercase() == scheme_lower {
                        log_dbg!(
                            "canOpenURL: {:?} is registered by the running app; returning YES",
                            scheme_lower
                        );
                        return true;
                    }
                }
            }
        }

        // Honour the Info.plist allow-list (`LSApplicationQueriesSchemes`).
        // Real iOS uses this only to *gate* the query, not to answer it,
        // but if the app lists a scheme there it almost always genuinely
        // expects the answer to be NO when the corresponding app is not
        // installed. We therefore log a debug note and return false so the
        // app's "app isn't installed" fallback runs.
        let key_str = ns_string::get_static_str(env, "LSApplicationQueriesSchemes");
        let allowed_arr: id = msg![env; main_bundle objectForInfoDictionaryKey:key_str];
        if allowed_arr != nil {
            let count: u32 = msg![env; allowed_arr count];
            for i in 0..count {
                let entry: id = msg![env; allowed_arr objectAtIndex:i];
                if entry == nil {
                    continue;
                }
                let entry_str = ns_string::to_rust_string(env, entry);
                if entry_str.to_lowercase() == scheme_lower {
                    log_dbg!(
                        "canOpenURL: {:?} is in LSApplicationQueriesSchemes; \
                         host can't launch it, returning NO",
                        scheme_lower
                    );
                    return false;
                }
            }
        }
    }

    log_dbg!(
        "canOpenURL: scheme {:?} not handled by host, returning NO",
        scheme_lower
    );
    false
}

- (bool)openURL:(id)url { // NSURL
    let ns_string = msg![env; url absoluteString];
    let url_string = ns_string::to_rust_string(env, ns_string);
    // Hand the URL to the host (on Android this opens the system browser
    // via MainActivity.openExternalUrl and the emulator keeps running —
    // exiting the process here killed the app before the intent could
    // dispatch, so links never opened).
    if let Err(e) = crate::window::open_url(env, &url_string) {
        echo!("App opened URL {:?} unsuccessfully ({}).", url_string, e);
    } else {
        echo!("App opened URL {:?}.", url_string);
    }
    true
}

- (())beginIgnoringInteractionEvents {
    env.framework_state.uikit.ui_application.ignoring_interaction_events_count += 1;
}

- (bool)isIgnoringInteractionEvents {
    env.framework_state.uikit.ui_application.ignoring_interaction_events_count > 0
}

- (())endIgnoringInteractionEvents {
    let count = &mut env.framework_state.uikit.ui_application.ignoring_interaction_events_count;
    if *count > 0 {
        *count -= 1;
    } else {
        // В реальной iOS здесь выбрасывается исключение
        // NSInternalInconsistencyException,
        // но для стабильности эмулятора мы просто залогируем предупреждение,
        // если игра ошиблась со счетчиком.
        log!("Warning: endIgnoringInteractionEvents called without matching beginIgnoringInteractionEvents");
    }
}

- (())sendEvent:(id)event { // UIEvent*
    log_dbg!("UIApplication sendEvent: forwarding to key window");
    let window: id = msg![env; this keyWindow];
    if window != nil {
        msg![env; window sendEvent:event]
    }
}

- (bool)sendAction:(SEL)action
                to:(id)target
              from:(id)sender
          forEvent:(id)event { // UIEvent*
    let has_event_argument = action.as_str(&env.mem).bytes().filter(|&c| c == b':').count() == 2;
    if target != nil {
        let responds: bool = msg![env; target respondsToSelector:action];
        if responds {
            if has_event_argument {
                () = msg![env; target performSelector:action withObject:sender withObject:event];
            } else {
                () = msg![env; target performSelector:action withObject:sender];
            }
            return true;
        }
        return false;
    }
    // Walk responder chain if target is nil.
    let mut responder: id = sender;
    while responder != nil {
        let responds: bool = msg![env; responder respondsToSelector:action];
        if responds {
            if has_event_argument {
                () = msg![env; responder performSelector:action withObject:sender withObject:event];
            } else {
                () = msg![env; responder performSelector:action withObject:sender];
            }
            return true;
        }
        responder = msg![env; responder nextResponder];
    }
    false
}

- (NSUInteger)beginBackgroundTaskWithExpirationHandler:(id)handler {
    msg![env; this beginBackgroundTaskWithName:nil expirationHandler:handler]
}

- (NSUInteger)beginBackgroundTaskWithName:(id)name expirationHandler:(id)handler {
    let name: id = if name == nil {
        nil
    } else {
        msg![env; name copy]
    };
    let handler: id = if handler == nil {
        nil
    } else {
        crate::mem::Ptr::from_bits(
            crate::libc::blocks::_Block_copy(env, handler.cast_void().cast_const()).to_bits(),
        )
    };
    let task_identifier = {
        let host = env.objc.borrow_mut::<UIApplicationHostObject>(this);
        let mut identifier = host.next_background_task_id;
        while identifier == 0
            || identifier == NSUInteger::MAX
            || host.background_tasks.contains_key(&identifier)
        {
            identifier = identifier.wrapping_add(1);
        }
        host.next_background_task_id = identifier.wrapping_add(1);
        host.background_tasks.insert(
            identifier,
            BackgroundTask {
                name,
                expiration_handler: handler,
                expiration_running: false,
            },
        );
        identifier
    };
    log_dbg!(
        "UIApplication began background task {:?} with identifier {}",
        name,
        task_identifier
    );
    task_identifier
}

- (())endBackgroundTask:(NSUInteger)task_identifier {
    if task_identifier == NSUInteger::MAX {
        return;
    }
    let task = {
        let host = env.objc.borrow_mut::<UIApplicationHostObject>(this);
        if let Some(task) = host.background_tasks.get(&task_identifier) {
            if task.expiration_running {
                return;
            }
        }
        host.background_tasks.remove(&task_identifier)
    };
    if let Some(task) = task {
        release(env, task.name);
        crate::libc::blocks::_Block_release(env, task.expiration_handler.cast_void().cast_const());
    } else {
        log_dbg!(
            "UIApplication ended unknown background task {}",
            task_identifier
        );
    }
}

- (NSUInteger)backgroundTimeRemaining {
    // Report effectively infinite time remaining.
    NSUInteger::MAX
}

- (UIApplicationState)applicationState {
    // Always report active.
    UIApplicationStateActive
}

- (bool)isProtectedDataAvailable {
    true
}

- (())setMinimumBackgroundFetchInterval:(f64)_interval {
    log!("UIApplication setMinimumBackgroundFetchInterval: stubbed");
}

- (())registerForRemoteNotifications {
    let delegate = msg![env; this delegate];
    env.objc
        .borrow_mut::<UIApplicationHostObject>(this)
        .remote_notifications_registered = true;

    if delegate != nil
        && env.objc.object_has_method_named(
            &env.mem,
            delegate,
            "application:didRegisterForRemoteNotificationsWithDeviceToken:",
        )
    {
        let token_bytes = [
            0x48, 0x79, 0x70, 0x65, 0x72, 0x48, 0x4c, 0x45,
            0x00, 0x00, 0x00, 0x01, 0x52, 0x45, 0x47, 0x49,
        ];
        let token_length: u32 = token_bytes.len().try_into().unwrap();
        let token_buffer = env.mem.alloc(token_length);
        env.mem
            .bytes_at_mut(token_buffer.cast(), token_length)
            .copy_from_slice(&token_bytes);
        let token_ptr = token_buffer.cast_const().cast_void();
        let token_data: id = msg_class![env; NSData dataWithBytes:token_ptr length:token_length];
        env.mem.free(token_buffer.cast());
        let _: () = msg![env;
            delegate application:this didRegisterForRemoteNotificationsWithDeviceToken:token_data
        ];
    }
}

- (())unregisterForRemoteNotifications {
    env.objc
        .borrow_mut::<UIApplicationHostObject>(this)
        .remote_notifications_registered = false;
}

- (bool)isRegisteredForRemoteNotifications {
    env.objc
        .borrow::<UIApplicationHostObject>(this)
        .remote_notifications_registered
}

- (())registerUserNotificationSettings:(id)settings {
    let old_settings = {
        let host = env.objc.borrow_mut::<UIApplicationHostObject>(this);
        let old = host.user_notification_settings;
        host.user_notification_settings = settings;
        old
    };
    retain(env, settings);
    release(env, old_settings);

    let delegate = msg![env; this delegate];
    if delegate != nil
        && env.objc.object_has_method_named(
            &env.mem,
            delegate,
            "application:didRegisterUserNotificationSettings:",
        )
    {
        let _: () = msg![env; delegate application:this didRegisterUserNotificationSettings:settings];
    }
}

- (id)currentUserNotificationSettings {
    let settings = env
        .objc
        .borrow::<UIApplicationHostObject>(this)
        .user_notification_settings;
    if settings == nil {
        nil
    } else {
        retain(env, settings);
        autorelease(env, settings)
    }
}

- (())cancelAllLocalNotifications {
    log_dbg!("UIApplication cancelAllLocalNotifications: no-op (notifications not delivered by host)");
}

- (())cancelLocalNotification:(id)_notification {
    log_dbg!("UIApplication cancelLocalNotification: no-op (notifications not delivered by host)");
}

- (())scheduleLocalNotification:(id)_notification {
    log_dbg!("UIApplication scheduleLocalNotification: no-op (notifications not delivered by host)");
}

- (id)scheduledLocalNotifications {
    msg_class![env; NSArray new]
}

- (())setScheduledLocalNotifications:(id)_notifications {
    log!("UIApplication setScheduledLocalNotifications: stubbed");
}

- (bool)supportsShakeToEdit {
    false
}

- (())setSupportsShakeToEdit:(bool)_value {
    // Stub.
}

- (bool)applicationSupportsShakeToEdit {
    env.framework_state.uikit.ui_application.application_supports_shake_to_edit
}

- (())setApplicationSupportsShakeToEdit:(bool)value {
    env.framework_state.uikit.ui_application.application_supports_shake_to_edit = value;
}

- (())clearKeychainIfNecessary {
    // Stub.
}

- (CGRect)statusBarFrame {
    // We don't render a status bar, so report what iOS reports for a hidden
    // one: CGRectZero. A 320-wide frame breaks cocos2d's CCMenu, which in
    // landscape subtracts the status bar's width (its height on screen) from
    // the window height when centring itself, pushing menus such as Forlorn's
    // Play button off the bottom of the screen.
    CGRect {
        origin: crate::frameworks::core_graphics::CGPoint { x: 0.0, y: 0.0 },
        size: crate::frameworks::core_graphics::CGSize { width: 0.0, height: 0.0 },
    }
}

- (())presentLocalNotificationNow:(id)_notification {
    log_dbg!("UIApplication presentLocalNotificationNow: no-op (notifications not delivered by host)");
}

- (id)keyWindow {
    let Some(key_window) = env
        .framework_state
        .uikit
        .ui_view
        .ui_window
        .key_window else {
        return nil;
    };
    assert!(env
        .framework_state
        .uikit
        .ui_view
        .ui_window
        .windows
        .contains(&key_window));
    key_window
}

- (id)windows {
    let windows: Vec<id> = (*env
        .framework_state
        .uikit
        .ui_view
        .ui_window
        .windows).to_vec();
    for window in &windows {
        retain(env, *window);
    }
    let windows = ns_array::from_vec(env, windows);
    autorelease(env, windows)
}

- (())registerForRemoteNotificationTypes:(UIRemoteNotificationType)types {
    // Push notifications cannot work in an emulator; apps register on every
    // launch, so keep the log quiet (debug level only).
    log_dbg!("registerForRemoteNotificationTypes:{} ignored", types);
}

// `- (UIRemoteNotificationType)enabledRemoteNotificationTypes` —
// per Apple's [UIApplication Reference](https://developer.apple.com/documentation/uikit/uiapplication/1623060-enabledremotenotificationtypes):
// the bitmask of remote notification types the user has explicitly
// enabled in Settings. touchHLE has no system Settings UI and no real
// push notification subsystem, so no notification types are enabled —
// the documented value for this state is `UIRemoteNotificationTypeNone`
// (0). Returning this lets apps using the iOS 3.0–7.x API path skip
// their push-registration branch without crashing.
- (UIRemoteNotificationType)enabledRemoteNotificationTypes {
    0 // UIRemoteNotificationTypeNone
}

// `applicationIconBadgeNumber` is the integer shown on the SpringBoard
// app icon badge. touchHLE has no SpringBoard, but games (e.g.
// notification-driven trial flows) often *read* the value back after
// setting it to gate logic, so we store it for round-trip fidelity per
// Apple's [UIApplication Reference](https://developer.apple.com/documentation/uikit/uiapplication/1622918-applicationiconbadgenumber).
- (NSInteger)applicationIconBadgeNumber {
    env.objc.borrow::<UIApplicationHostObject>(this).application_icon_badge_number
}
- (())setApplicationIconBadgeNumber:(NSInteger)bn {
    log_dbg!("setApplicationIconBadgeNumber:{}", bn);
    env.objc.borrow_mut::<UIApplicationHostObject>(this).application_icon_badge_number = bn;
}

- (id)nextResponder {
    let delegate = msg![env; this delegate];
    let app_delegate_class = msg![env; delegate class];
    let ui_responder_class = env.objc.get_known_class("UIResponder", &mut env.mem);
    if env.objc.class_is_subclass_of(app_delegate_class, ui_responder_class) {
        delegate
    } else {
        nil
    }
}

@end

// UIUserNotificationSettings — holds notification permission settings.
// https://developer.apple.com/documentation/uikit/uiusernotificationsettings
// On iOS 8+, apps call [UIApplication registerUserNotificationSettings:settings]
// with an instance of this class. Since touchHLE does not deliver notifications,
// we expose a minimal stub that satisfies alloc/init and settingsForTypes:categories:.
@implementation UIUserNotificationSettings: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::new(UIUserNotificationSettingsHostObject {
            types: 0,
            categories: nil,
        }),
        &mut env.mem,
    )
}

+ (id)settingsForTypes:(NSUInteger)types categories:(id)categories {
    let settings: id = msg![env; this alloc];
    let host = env.objc.borrow_mut::<UIUserNotificationSettingsHostObject>(settings);
    host.types = types;
    host.categories = categories;
    retain(env, categories);
    autorelease(env, settings)
}

- (id)init {
    this
}

- (())dealloc {
    let categories = env
        .objc
        .borrow::<UIUserNotificationSettingsHostObject>(this)
        .categories;
    release(env, categories);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (NSUInteger)types {
    env.objc
        .borrow::<UIUserNotificationSettingsHostObject>(this)
        .types
}

- (id)categories {
    let categories = env
        .objc
        .borrow::<UIUserNotificationSettingsHostObject>(this)
        .categories;
    if categories == nil {
        nil
    } else {
        retain(env, categories);
        autorelease(env, categories)
    }
}

@end

};

/// Best-effort discovery of the application delegate class.
///
/// `UIApplicationMain` normally learns the delegate class from its
/// `delegateClassName` argument (or from a connection in the main nib). When
/// neither is available, look through the loaded classes for one that declares
/// a `UIApplicationDelegate` launch method and use that. Returns `nil` if no
/// candidate is found.
fn find_app_delegate_class(env: &Environment) -> id {
    for sel_name in [
        "application:didFinishLaunchingWithOptions:",
        "applicationDidFinishLaunching:",
    ] {
        if let Some(class) = env.objc.class_declaring_instance_method(sel_name) {
            return class;
        }
    }
    nil
}

/// `UIApplicationMain`, the entry point of the application.
pub(super) fn UIApplicationMain(
    env: &mut Environment,
    _argc: i32,
    _argv: MutPtr<MutPtr<u8>>,
    principal_class_name: id, // NSString*
    delegate_class_name: id,  // NSString*
) {
    let ui_application = {
        let pool: id = msg_class![env; NSAutoreleasePool new];

        let principal_class = if principal_class_name != nil {
            let name = ns_string::to_rust_string(env, principal_class_name);
            env.objc.get_known_class(&name, &mut env.mem)
        } else {
            env.objc.get_known_class("UIApplication", &mut env.mem)
        };
        let ui_application: id = msg![env; principal_class new];

        let device_family = env.options.device_family;
        if let Some(main_nib_filename) = env
            .bundle
            .main_nib_filename(device_family)
            .map(str::to_owned)
        {
            let ns_main_nib_filename = from_rust_string(env, main_nib_filename);
            let type_: id = get_static_str(env, "nib");
            let bundle: id = msg_class![env; NSBundle mainBundle];
            let res: id = msg![env; bundle pathForResource:ns_main_nib_filename ofType:type_];
            if res != nil {
                let nib: id = msg_class![env; UINib nibWithNibName:ns_main_nib_filename bundle:nil];
                release(env, ns_main_nib_filename);
                let _: id = msg![env; nib instantiateWithOwner:ui_application
                                               options:nil];
            } else {
                log!("Warning: couldn't load main nib file.");
            }
        }

        if env.bundle.status_bar_hidden() {
            let _: () = msg![env; ui_application setStatusBarHidden:true];
        }

        let delegate: id = msg![env; ui_application delegate];
        if delegate != nil {
            // The delegate was wired up while loading the main nib.
            env.objc
                .borrow_mut::<UIApplicationHostObject>(ui_application)
                .delegate_is_retained = true;
            retain(env, delegate);
        } else if delegate_class_name != nil
            && msg![env; delegate_class_name isEqual:principal_class_name]
        {
            // The app uses its principal class as its own delegate.
            let _: () = msg![env; ui_application setDelegate:ui_application];
        } else {
            // The delegate is normally created from the class named by the
            // `delegate_class_name` argument. Some apps reach this point
            // without a usable name: the argument is nil (the delegate was
            // expected from the main nib, but no nib connection set it), or
            // the value the binary passed could not be resolved to a real
            // class (e.g. it was derived from a class reference that ended up
            // nil, which decodes to the empty string). Previously this left
            // the application with a nil delegate, so
            // `application:didFinishLaunchingWithOptions:` was never sent and
            // the app stayed frozen on its launch image. Fall back to
            // discovering the app's delegate class from the loaded classes.
            let mut delegate_class: id = nil;
            if delegate_class_name != nil {
                let name = ns_string::to_rust_string(env, delegate_class_name).into_owned();
                if !name.is_empty() {
                    delegate_class = env
                        .objc
                        .try_get_known_class(&name, &mut env.mem)
                        .unwrap_or(nil);
                }
            }
            if delegate_class == nil {
                delegate_class = find_app_delegate_class(env);
                if delegate_class != nil {
                    log!(
                        "UIApplicationMain: no usable delegate class name was \
                         provided; using discovered application delegate class."
                    );
                }
            }
            if delegate_class != nil {
                let delegate: id = msg![env; delegate_class new];
                let _: () = msg![env; ui_application setDelegate:delegate];
            } else {
                log!(
                    "Warning: UIApplicationMain could not determine an \
                     application delegate; the app may not finish launching."
                );
            }
        };

        // UIMainStoryboardFile (iOS 5+) replaces NSMainNibFile for modern
        // apps. When present, the documented launch flow is:
        //   1. Load the named storyboard.
        //   2. Create a UIWindow sized to the main screen's bounds.
        //   3. Instantiate the storyboard's initial view controller.
        //   4. Set that view controller as the window's rootViewController.
        //   5. Send -makeKeyAndVisible to the window.
        //   6. Assign the window to the app delegate's `window` property
        //      via KVC (Apple's UIApplicationMain does this so app
        //      delegates can return the window from `-window` without any
        //      manual wiring inside `application:didFinishLaunchingWithOptions:`).
        let storyboard_name = env
            .bundle
            .main_storyboard_filename(device_family)
            .map(str::to_owned);
        if let Some(storyboard_name) = storyboard_name {
            let storyboard_name_ns = from_rust_string(env, storyboard_name.clone());
            let storyboard_class = env.objc.get_known_class("UIStoryboard", &mut env.mem);
            let storyboard: id =
                msg![env; storyboard_class storyboardWithName:storyboard_name_ns bundle:nil];
            release(env, storyboard_name_ns);

            if storyboard != nil {
                let initial_vc: id = msg![env; storyboard instantiateInitialViewController];
                if initial_vc != nil {
                    let screen: id = msg_class![env; UIScreen mainScreen];
                    let bounds: CGRect = msg![env; screen bounds];
                    let window: id = msg_class![env; UIWindow alloc];
                    let window: id = msg![env; window initWithFrame:bounds];

                    let _: () = msg![env; window setRootViewController:initial_vc];
                    let _: () = msg![env; window makeKeyAndVisible];

                    let delegate: id = msg![env; ui_application delegate];
                    if delegate != nil {
                        let window_key: id = get_static_str(env, "window");
                        let _: () = msg![env; delegate setValue:window forKey:window_key];
                    }
                } else {
                    log!(
                        "Warning: storyboard {:?} has no initial view controller; \
                         skipping window setup.",
                        storyboard_name,
                    );
                }
            } else {
                log!(
                    "Warning: couldn't load main storyboard {:?}.",
                    storyboard_name
                );
            }
        }

        let _: () = msg![env; pool drain];
        ui_application
    };

    {
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let delegate: id = msg![env; ui_application delegate];
        if env.objc.object_has_method_named(
            &env.mem,
            delegate,
            "application:didFinishLaunchingWithOptions:",
        ) {
            let empty_dict: id = msg_class![env; NSDictionary dictionary];
            () = msg![env; delegate application:ui_application didFinishLaunchingWithOptions:empty_dict];
        } else if env.objc.object_has_method_named(
            &env.mem,
            delegate,
            "applicationDidFinishLaunching:",
        ) {
            () = msg![env; delegate applicationDidFinishLaunching:ui_application];
        }

        let center: id = msg_class![env; NSNotificationCenter defaultCenter];
        let notif_name = get_static_str(env, UIApplicationDidFinishLaunchingNotification);
        () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];

        let _: () = msg![env; pool drain];
    }

    let views = env.framework_state.uikit.ui_view.views.clone();
    for view in views {
        () = msg![env; view layoutSubviews];
    }

    {
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let delegate: id = msg![env; ui_application delegate];
        if env
            .objc
            .object_has_method_named(&env.mem, delegate, "applicationDidBecomeActive:")
        {
            () = msg![env; delegate applicationDidBecomeActive:ui_application];
        }
        let center: id = msg_class![env; NSNotificationCenter defaultCenter];
        let notif_name = get_static_str(env, UIApplicationDidBecomeActiveNotification);
        () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
        let _: () = msg![env; pool drain];
    }

    // Apple docs: after `beginGeneratingDeviceOrientationNotifications` the
    // device begins generating `UIDeviceOrientationDidChangeNotification`s.
    // On a real iPhone the accelerometer wakes the moment the user picks up
    // the device, so the very first orientation event practically always
    // arrives during launch. Some games (e.g. Dead Space) gate the start of
    // their C++ engine on this first notification — if it never fires they
    // sit forever in the run loop waiting for it. Post an initial
    // notification here so the app can transition out of its idle splash
    // state. See `UIDevice` documentation:
    //   https://developer.apple.com/documentation/uikit/uidevice/1620018-beginGeneratingdeviceorientationn
    {
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let current_device: id = msg_class![env; UIDevice currentDevice];
        let is_generating: bool =
            msg![env; current_device isGeneratingDeviceOrientationNotifications];
        if is_generating {
            log_dbg!(
                "Posting initial UIDeviceOrientationDidChangeNotification \
                 so apps observing device orientation can finish initializing."
            );
            let _: () = msg![env; current_device _postOrientationChangeNotification];
        }
        let _: () = msg![env; pool drain];
    }

    let run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    let _: () = msg![env; run_loop run];
}

/// Dispatches `applicationWillResignActive:` and posts
/// `UIApplicationWillResignActiveNotification`. Called when the OS tells
/// touchHLE the app is about to stop being active (Android `onPause()`,
/// iOS `applicationWillResignActive:`). Unlike [exit], this does NOT
/// terminate the process — the app may return to the foreground later.
///
/// Apple docs: <https://developer.apple.com/documentation/uikit/uiapplicationdelegate/applicationwillresignactive(_:)>
pub(super) fn handle_will_resign_active(env: &mut Environment) {
    let ui_application: id = msg_class![env; UIApplication sharedApplication];
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];

    let pool: id = msg_class![env; NSAutoreleasePool new];
    let delegate: id = msg![env; ui_application delegate];
    if env
        .objc
        .object_has_method_named(&env.mem, delegate, "applicationWillResignActive:")
    {
        () = msg![env; delegate applicationWillResignActive:ui_application];
    }
    let notif_name = get_static_str(env, UIApplicationWillResignActiveNotification);
    () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
    let _: () = msg![env; pool drain];
}

/// Dispatches `applicationDidEnterBackground:` and posts
/// `UIApplicationDidEnterBackgroundNotification`.
///
/// Apple docs: <https://developer.apple.com/documentation/uikit/uiapplicationdelegate/applicationdidenterbackground(_:)>
pub(super) fn handle_did_enter_background(env: &mut Environment) {
    let ui_application: id = msg_class![env; UIApplication sharedApplication];
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];

    let pool: id = msg_class![env; NSAutoreleasePool new];
    // Real iOS apps are often killed by the OS shortly after entering the
    // background, so persist user defaults eagerly: it's the last reliable
    // moment to flush any data.
    if !env.is_app_picker {
        let user_defaults: id = msg_class![env; NSUserDefaults standardUserDefaults];
        let _: bool = msg![env; user_defaults synchronize];
    }
    let delegate: id = msg![env; ui_application delegate];
    if env
        .objc
        .object_has_method_named(&env.mem, delegate, "applicationDidEnterBackground:")
    {
        () = msg![env; delegate applicationDidEnterBackground:ui_application];
    }
    let notif_name = get_static_str(env, UIApplicationDidEnterBackgroundNotification);
    () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
    expire_background_tasks(env, ui_application);
    let _: () = msg![env; pool drain];
}

/// Dispatches `applicationWillEnterForeground:` and posts
/// `UIApplicationWillEnterForegroundNotification`.
///
/// Apple docs: <https://developer.apple.com/documentation/uikit/uiapplicationdelegate/applicationwillenterforeground(_:)>
pub(super) fn handle_will_enter_foreground(env: &mut Environment) {
    let ui_application: id = msg_class![env; UIApplication sharedApplication];
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];

    let pool: id = msg_class![env; NSAutoreleasePool new];
    let delegate: id = msg![env; ui_application delegate];
    if env
        .objc
        .object_has_method_named(&env.mem, delegate, "applicationWillEnterForeground:")
    {
        () = msg![env; delegate applicationWillEnterForeground:ui_application];
    }
    let notif_name = get_static_str(env, UIApplicationWillEnterForegroundNotification);
    () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
    let _: () = msg![env; pool drain];
}

/// Dispatches `applicationDidBecomeActive:` and posts
/// `UIApplicationDidBecomeActiveNotification`. Used both at launch time and
/// when the app returns to the foreground after being inactive/backgrounded.
///
/// Apple docs: <https://developer.apple.com/documentation/uikit/uiapplicationdelegate/applicationdidbecomeactive(_:)>
pub(super) fn handle_did_become_active(env: &mut Environment) {
    let ui_application: id = msg_class![env; UIApplication sharedApplication];
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];

    let pool: id = msg_class![env; NSAutoreleasePool new];
    let delegate: id = msg![env; ui_application delegate];
    if env
        .objc
        .object_has_method_named(&env.mem, delegate, "applicationDidBecomeActive:")
    {
        () = msg![env; delegate applicationDidBecomeActive:ui_application];
    }
    let notif_name = get_static_str(env, UIApplicationDidBecomeActiveNotification);
    () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
    let _: () = msg![env; pool drain];
}

pub(super) fn exit(env: &mut Environment) {
    let ui_application: id = msg_class![env; UIApplication sharedApplication];
    let center: id = msg_class![env; NSNotificationCenter defaultCenter];

    {
        let pool: id = msg_class![env; NSAutoreleasePool new];
        if !env.is_app_picker {
            let user_defaults: id = msg_class![env; NSUserDefaults standardUserDefaults];
            let _: bool = msg![env; user_defaults synchronize];
        }
        let delegate: id = msg![env; ui_application delegate];
        if env
            .objc
            .object_has_method_named(&env.mem, delegate, "applicationWillResignActive:")
        {
            () = msg![env; delegate applicationWillResignActive:ui_application];
        }
        let notif_name = get_static_str(env, UIApplicationWillResignActiveNotification);
        () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
        let _: () = msg![env; pool drain];
    };
    {
        let pool: id = msg_class![env; NSAutoreleasePool new];
        let delegate: id = msg![env; ui_application delegate];
        if env
            .objc
            .object_has_method_named(&env.mem, delegate, "applicationWillTerminate:")
        {
            () = msg![env; delegate applicationWillTerminate:ui_application];
        }
        let notif_name = get_static_str(env, UIApplicationWillTerminateNotification);
        () = msg![env; center postNotificationName:notif_name object:ui_application userInfo:nil];
        let _: () = msg![env; pool drain];
    };

    std::process::exit(0);
}

const UIApplicationDidFinishLaunchingNotification: &str =
    "UIApplicationDidFinishLaunchingNotification";
const UIApplicationDidBecomeActiveNotification: &str = "UIApplicationDidBecomeActiveNotification";
const UIApplicationDidEnterBackgroundNotification: &str =
    "UIApplicationDidEnterBackgroundNotification";
const UIApplicationWillEnterForegroundNotification: &str =
    "UIApplicationWillEnterForegroundNotification";
const UIApplicationWillResignActiveNotification: &str = "UIApplicationWillResignActiveNotification";
const UIApplicationWillTerminateNotification: &str = "UIApplicationWillTerminateNotification";
const UIApplicationLaunchOptionsRemoteNotificationKey: &str =
    "UIApplicationLaunchOptionsRemoteNotificationKey";
const UIApplicationDidReceiveMemoryWarningNotification: &str =
    "UIApplicationDidReceiveMemoryWarningNotification";

// Apple `UIApplication.h` declares these as
// `UIKIT_EXTERN NSNotificationName const ...` (and `NSString * const` in
// older SDKs). The exact literal values are what Apple uses for
// notification posting and dictionary keys; matching them lets observer
// registration via `-[NSNotificationCenter addObserver:selector:name:object:]`
// resolve correctly.
//
// References:
// * Apple [UIApplication notification names](https://developer.apple.com/documentation/uikit/uiapplication)
// * Apple [Launch options keys](https://developer.apple.com/documentation/uikit/uiapplication/launchoptionskey)
// * Apple [Status-bar related notifications](https://developer.apple.com/documentation/uikit/uiapplicationdidchangestatusbarframenotification)
const UIApplicationProtectedDataDidBecomeAvailable: &str =
    "UIApplicationProtectedDataDidBecomeAvailable";
const UIApplicationProtectedDataWillBecomeUnavailable: &str =
    "UIApplicationProtectedDataWillBecomeUnavailable";
const UIApplicationSignificantTimeChangeNotification: &str =
    "UIApplicationSignificantTimeChangeNotification";
const UIApplicationDidChangeStatusBarFrameNotification: &str =
    "UIApplicationDidChangeStatusBarFrameNotification";
const UIApplicationWillChangeStatusBarFrameNotification: &str =
    "UIApplicationWillChangeStatusBarFrameNotification";
const UIApplicationDidChangeStatusBarOrientationNotification: &str =
    "UIApplicationDidChangeStatusBarOrientationNotification";
const UIApplicationWillChangeStatusBarOrientationNotification: &str =
    "UIApplicationWillChangeStatusBarOrientationNotification";
const UIApplicationStatusBarFrameUserInfoKey: &str = "UIApplicationStatusBarFrameUserInfoKey";
const UIApplicationStatusBarOrientationUserInfoKey: &str =
    "UIApplicationStatusBarOrientationUserInfoKey";
const UIApplicationBackgroundFetchIntervalMinimum: &str =
    "UIApplicationBackgroundFetchIntervalMinimum";
const UIApplicationBackgroundFetchIntervalNever: &str = "UIApplicationBackgroundFetchIntervalNever";
// Launch options keys — Apple `UIApplication.h` (`UIApplicationLaunchOptionsKey`).
const UIApplicationLaunchOptionsURLKey: &str = "UIApplicationLaunchOptionsURLKey";
const UIApplicationLaunchOptionsSourceApplicationKey: &str =
    "UIApplicationLaunchOptionsSourceApplicationKey";
const UIApplicationLaunchOptionsAnnotationKey: &str = "UIApplicationLaunchOptionsAnnotationKey";
const UIApplicationLaunchOptionsLocalNotificationKey: &str =
    "UIApplicationLaunchOptionsLocalNotificationKey";
const UIApplicationLaunchOptionsLocationKey: &str = "UIApplicationLaunchOptionsLocationKey";
const UIApplicationLaunchOptionsNewsstandDownloadsKey: &str =
    "UIApplicationLaunchOptionsNewsstandDownloadsKey";
const UIApplicationLaunchOptionsBluetoothCentralsKey: &str =
    "UIApplicationLaunchOptionsBluetoothCentralsKey";
const UIApplicationLaunchOptionsBluetoothPeripheralsKey: &str =
    "UIApplicationLaunchOptionsBluetoothPeripheralsKey";
const UIApplicationLaunchOptionsShortcutItemKey: &str = "UIApplicationLaunchOptionsShortcutItemKey";
// `UIApplicationOpenSettingsURLString` from `UIApplication.h`. iOS apps
// pass this to `-[UIApplication openURL:]` to open the system Settings
// app at their own page; the literal value is `"app-settings:"` per
// Apple's docs (<https://developer.apple.com/documentation/uikit/uiapplicationopensettingsurlstring>).
const UIApplicationOpenSettingsURLString: &str = "app-settings:";
// Newer (iOS 9) open-URL option keys — `UIApplicationOpenURLOptionsKey`.
const UIApplicationOpenURLOptionsSourceApplicationKey: &str =
    "UIApplicationOpenURLOptionsSourceApplicationKey";
const UIApplicationOpenURLOptionsAnnotationKey: &str = "UIApplicationOpenURLOptionsAnnotationKey";
const UIApplicationOpenURLOptionsOpenInPlaceKey: &str = "UIApplicationOpenURLOptionsOpenInPlaceKey";
const UIApplicationOpenURLOptionUniversalLinksOnly: &str =
    "UIApplicationOpenURLOptionUniversalLinksOnly";
pub const CONSTANTS: ConstantExports = &[
    (
        "_UIApplicationDidFinishLaunchingNotification",
        HostConstant::NSString(UIApplicationDidFinishLaunchingNotification),
    ),
    (
        "_UIApplicationDidBecomeActiveNotification",
        HostConstant::NSString(UIApplicationDidBecomeActiveNotification),
    ),
    (
        "_UIApplicationDidEnterBackgroundNotification",
        HostConstant::NSString(UIApplicationDidEnterBackgroundNotification),
    ),
    (
        "_UIApplicationWillEnterForegroundNotification",
        HostConstant::NSString(UIApplicationWillEnterForegroundNotification),
    ),
    (
        "_UIApplicationWillResignActiveNotification",
        HostConstant::NSString(UIApplicationWillResignActiveNotification),
    ),
    (
        "_UIApplicationWillTerminateNotification",
        HostConstant::NSString(UIApplicationWillTerminateNotification),
    ),
    (
        "_UIApplicationDidReceiveMemoryWarningNotification",
        HostConstant::NSString(UIApplicationDidReceiveMemoryWarningNotification),
    ),
    (
        "_UIApplicationLaunchOptionsRemoteNotificationKey",
        HostConstant::NSString(UIApplicationLaunchOptionsRemoteNotificationKey),
    ),
    (
        "_UIApplicationProtectedDataDidBecomeAvailable",
        HostConstant::NSString(UIApplicationProtectedDataDidBecomeAvailable),
    ),
    (
        "_UIApplicationProtectedDataWillBecomeUnavailable",
        HostConstant::NSString(UIApplicationProtectedDataWillBecomeUnavailable),
    ),
    (
        "_UIApplicationSignificantTimeChangeNotification",
        HostConstant::NSString(UIApplicationSignificantTimeChangeNotification),
    ),
    (
        "_UIApplicationDidChangeStatusBarFrameNotification",
        HostConstant::NSString(UIApplicationDidChangeStatusBarFrameNotification),
    ),
    (
        "_UIApplicationWillChangeStatusBarFrameNotification",
        HostConstant::NSString(UIApplicationWillChangeStatusBarFrameNotification),
    ),
    (
        "_UIApplicationDidChangeStatusBarOrientationNotification",
        HostConstant::NSString(UIApplicationDidChangeStatusBarOrientationNotification),
    ),
    (
        "_UIApplicationWillChangeStatusBarOrientationNotification",
        HostConstant::NSString(UIApplicationWillChangeStatusBarOrientationNotification),
    ),
    (
        "_UIApplicationStatusBarFrameUserInfoKey",
        HostConstant::NSString(UIApplicationStatusBarFrameUserInfoKey),
    ),
    (
        "_UIApplicationStatusBarOrientationUserInfoKey",
        HostConstant::NSString(UIApplicationStatusBarOrientationUserInfoKey),
    ),
    (
        "_UIApplicationBackgroundFetchIntervalMinimum",
        HostConstant::NSString(UIApplicationBackgroundFetchIntervalMinimum),
    ),
    (
        "_UIApplicationBackgroundFetchIntervalNever",
        HostConstant::NSString(UIApplicationBackgroundFetchIntervalNever),
    ),
    (
        "_UIApplicationLaunchOptionsURLKey",
        HostConstant::NSString(UIApplicationLaunchOptionsURLKey),
    ),
    (
        "_UIApplicationLaunchOptionsSourceApplicationKey",
        HostConstant::NSString(UIApplicationLaunchOptionsSourceApplicationKey),
    ),
    (
        "_UIApplicationLaunchOptionsAnnotationKey",
        HostConstant::NSString(UIApplicationLaunchOptionsAnnotationKey),
    ),
    (
        "_UIApplicationLaunchOptionsLocalNotificationKey",
        HostConstant::NSString(UIApplicationLaunchOptionsLocalNotificationKey),
    ),
    (
        "_UIApplicationLaunchOptionsLocationKey",
        HostConstant::NSString(UIApplicationLaunchOptionsLocationKey),
    ),
    (
        "_UIApplicationLaunchOptionsNewsstandDownloadsKey",
        HostConstant::NSString(UIApplicationLaunchOptionsNewsstandDownloadsKey),
    ),
    (
        "_UIApplicationLaunchOptionsBluetoothCentralsKey",
        HostConstant::NSString(UIApplicationLaunchOptionsBluetoothCentralsKey),
    ),
    (
        "_UIApplicationLaunchOptionsBluetoothPeripheralsKey",
        HostConstant::NSString(UIApplicationLaunchOptionsBluetoothPeripheralsKey),
    ),
    (
        "_UIApplicationLaunchOptionsShortcutItemKey",
        HostConstant::NSString(UIApplicationLaunchOptionsShortcutItemKey),
    ),
    (
        "_UIApplicationOpenSettingsURLString",
        HostConstant::NSString(UIApplicationOpenSettingsURLString),
    ),
    (
        "_UIApplicationOpenURLOptionsSourceApplicationKey",
        HostConstant::NSString(UIApplicationOpenURLOptionsSourceApplicationKey),
    ),
    (
        "_UIApplicationOpenURLOptionsAnnotationKey",
        HostConstant::NSString(UIApplicationOpenURLOptionsAnnotationKey),
    ),
    (
        "_UIApplicationOpenURLOptionsOpenInPlaceKey",
        HostConstant::NSString(UIApplicationOpenURLOptionsOpenInPlaceKey),
    ),
    (
        "_UIApplicationOpenURLOptionUniversalLinksOnly",
        HostConstant::NSString(UIApplicationOpenURLOptionUniversalLinksOnly),
    ),
];
pub const FUNCTIONS: FunctionExports = &[export_c_func!(UIApplicationMain(_, _, _, _))];

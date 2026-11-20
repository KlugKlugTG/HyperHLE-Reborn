/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UILocalNotification`.
//!
//! A plain value object. Nothing here schedules or delivers notifications;
//! that would be `UIApplication`'s `scheduleLocalNotification:` and friends.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::ns_string::{from_rust_string, to_rust_string};
use crate::frameworks::foundation::NSUInteger;
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, Class, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

#[derive(Default, Clone)]
pub(super) struct UILocalNotificationHostObject {
    fire_date: id,
    time_zone: id,
    repeat_interval: NSUInteger,
    repeat_calendar: id,
    alert_body: id,
    alert_action: id,
    alert_launch_image: id,
    sound_name: id,
    application_icon_badge_number: i32,
    user_info: id,
    has_action: bool,
    // iOS 8+
    alert_title: id,
    category: id,
    region: id,
    region_triggers_once: bool,
}
impl HostObject for UILocalNotificationHostObject {}

impl UILocalNotificationHostObject {
    /// Every retained object field, for dealloc and copy.
    fn objects(&self) -> [id; 9] {
        [
            self.fire_date,
            self.time_zone,
            self.repeat_calendar,
            self.alert_body,
            self.alert_action,
            self.alert_launch_image,
            self.sound_name,
            self.user_info,
            self.alert_title,
        ]
    }
    /// The iOS 8 object fields that aren't in [Self::objects] (kept separate
    /// only so that array stays a fixed size).
    fn extra_objects(&self) -> [id; 2] {
        [self.category, self.region]
    }
}

/// Retain `val`, store it in the field picked by `field`, release the old
/// value. Retaining first makes `setX:` with the current value safe. No
/// borrow is held across `retain`/`release`.
fn replace_field<F>(env: &mut Environment, this: id, field: F, val: id)
where
    F: FnOnce(&mut UILocalNotificationHostObject) -> &mut id,
{
    retain(env, val);
    let old = std::mem::replace(
        field(env.objc.borrow_mut::<UILocalNotificationHostObject>(this)),
        val,
    );
    release(env, old);
}

/// `-description` of `obj` as a Rust string, or "(null)" for nil.
fn describe(env: &mut Environment, obj: id) -> String {
    if obj == nil {
        return "(null)".to_string();
    }
    let desc: id = msg![env; obj description];
    if desc == nil {
        "(null)".to_string()
    } else {
        to_rust_string(env, desc).into_owned()
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UILocalNotification: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UILocalNotificationHostObject {
        has_action: true,
        region_triggers_once: true,
        ..Default::default()
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (())dealloc {
    let (objects, extra) = {
        let host = env.objc.borrow::<UILocalNotificationHostObject>(this);
        (host.objects(), host.extra_objects())
    };
    for obj in objects {
        release(env, obj);
    }
    for obj in extra {
        release(env, obj);
    }

    env.objc.dealloc_object(this, &mut env.mem);
}

// MARK: - NSCopying

- (id)copyWithZone:(NSZonePtr)_zone {
    // Copy the subclass too, if this is one.
    let class: Class = msg![env; this class];
    let new: id = msg![env; class alloc];
    let new: id = msg![env; new init];

    let snapshot = env.objc.borrow::<UILocalNotificationHostObject>(this).clone();
    // The copy owns its own reference to each object.
    for obj in snapshot.objects() {
        retain(env, obj);
    }
    for obj in snapshot.extra_objects() {
        retain(env, obj);
    }
    // The new object's object fields are all nil, so nothing is leaked here.
    *env.objc.borrow_mut::<UILocalNotificationHostObject>(new) = snapshot;
    new
}

// MARK: - Properties

- (id)fireDate { env.objc.borrow::<UILocalNotificationHostObject>(this).fire_date }
- (())setFireDate:(id)val {
    replace_field(env, this, |h| &mut h.fire_date, val);
}

- (id)timeZone { env.objc.borrow::<UILocalNotificationHostObject>(this).time_zone }
- (())setTimeZone:(id)val {
    replace_field(env, this, |h| &mut h.time_zone, val);
}

- (NSUInteger)repeatInterval { env.objc.borrow::<UILocalNotificationHostObject>(this).repeat_interval }
- (())setRepeatInterval:(NSUInteger)val {
    env.objc.borrow_mut::<UILocalNotificationHostObject>(this).repeat_interval = val;
}

- (id)repeatCalendar { env.objc.borrow::<UILocalNotificationHostObject>(this).repeat_calendar }
- (())setRepeatCalendar:(id)val {
    replace_field(env, this, |h| &mut h.repeat_calendar, val);
}

- (id)alertBody { env.objc.borrow::<UILocalNotificationHostObject>(this).alert_body }
- (())setAlertBody:(id)val {
    replace_field(env, this, |h| &mut h.alert_body, val);
}

- (id)alertAction { env.objc.borrow::<UILocalNotificationHostObject>(this).alert_action }
- (())setAlertAction:(id)val {
    replace_field(env, this, |h| &mut h.alert_action, val);
}

- (id)alertLaunchImage { env.objc.borrow::<UILocalNotificationHostObject>(this).alert_launch_image }
- (())setAlertLaunchImage:(id)val {
    replace_field(env, this, |h| &mut h.alert_launch_image, val);
}

- (bool)hasAction { env.objc.borrow::<UILocalNotificationHostObject>(this).has_action }
- (())setHasAction:(bool)val {
    env.objc.borrow_mut::<UILocalNotificationHostObject>(this).has_action = val;
}

- (id)soundName { env.objc.borrow::<UILocalNotificationHostObject>(this).sound_name }
- (())setSoundName:(id)val {
    replace_field(env, this, |h| &mut h.sound_name, val);
}

- (i32)applicationIconBadgeNumber { env.objc.borrow::<UILocalNotificationHostObject>(this).application_icon_badge_number }
- (())setApplicationIconBadgeNumber:(i32)val {
    env.objc.borrow_mut::<UILocalNotificationHostObject>(this).application_icon_badge_number = val;
}

- (id)userInfo { env.objc.borrow::<UILocalNotificationHostObject>(this).user_info }
- (())setUserInfo:(id)val {
    replace_field(env, this, |h| &mut h.user_info, val);
}

// iOS 8.2+
- (id)alertTitle { env.objc.borrow::<UILocalNotificationHostObject>(this).alert_title }
- (())setAlertTitle:(id)val {
    replace_field(env, this, |h| &mut h.alert_title, val);
}

// iOS 8+
- (id)category { env.objc.borrow::<UILocalNotificationHostObject>(this).category }
- (())setCategory:(id)val {
    replace_field(env, this, |h| &mut h.category, val);
}

// iOS 8+: a CLRegion. Stored only; location triggers never fire.
- (id)region { env.objc.borrow::<UILocalNotificationHostObject>(this).region }
- (())setRegion:(id)val {
    replace_field(env, this, |h| &mut h.region, val);
}

- (bool)regionTriggersOnce { env.objc.borrow::<UILocalNotificationHostObject>(this).region_triggers_once }
- (())setRegionTriggersOnce:(bool)val {
    env.objc.borrow_mut::<UILocalNotificationHostObject>(this).region_triggers_once = val;
}

// MARK: - Description

- (id)description {
    let (fire_date, time_zone, repeat_interval, user_info) = {
        let host = env.objc.borrow::<UILocalNotificationHostObject>(this);
        (host.fire_date, host.time_zone, host.repeat_interval, host.user_info)
    };
    let fire_date = describe(env, fire_date);
    let time_zone = describe(env, time_zone);
    let user_info = describe(env, user_info);
    let s = format!(
        "<UILocalNotification: {:?}>{{fire date = {}, time zone = {}, repeat interval = {}, user info = {}}}",
        this, fire_date, time_zone, repeat_interval, user_info
    );
    let ns = from_rust_string(env, s);
    autorelease(env, ns)
}

@end

};

// ЭКСПОРТ КОНСТАНТЫ (Решает ошибку линковщика)
pub const CONSTANTS: ConstantExports = &[(
    "_UILocalNotificationDefaultSoundName",
    HostConstant::NSString("UILocalNotificationDefaultSoundName"),
)];

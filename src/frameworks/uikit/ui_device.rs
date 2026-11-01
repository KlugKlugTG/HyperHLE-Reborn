/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#![allow(dead_code)]
//! `UIDevice`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::{ns_string, NSInteger};
use crate::objc::{id, msg, msg_class, nil, objc_classes, ClassExports, TrivialHostObject};
use crate::window::{get_battery_status, BatteryState, DeviceOrientation};

pub const UIDeviceOrientationDidChangeNotification: &str =
    "UIDeviceOrientationDidChangeNotification";
pub const UIDeviceBatteryLevelDidChangeNotification: &str =
    "UIDeviceBatteryLevelDidChangeNotification";

pub const UIDeviceBatteryStateDidChangeNotification: &str =
    "UIDeviceBatteryStateDidChangeNotification";
pub const UIDeviceProximityStateDidChangeNotification: &str =
    "UIDeviceProximityStateDidChangeNotification";

pub type UIDeviceOrientation = NSInteger;

pub const UIDeviceOrientationUnknown: UIDeviceOrientation = 0;
pub const UIDeviceOrientationPortrait: UIDeviceOrientation = 1;

pub const UIDeviceOrientationPortraitUpsideDown: UIDeviceOrientation = 2;
pub const UIDeviceOrientationLandscapeLeft: UIDeviceOrientation = 3;
pub const UIDeviceOrientationLandscapeRight: UIDeviceOrientation = 4;

pub const UIDeviceOrientationFaceUp: UIDeviceOrientation = 5;

pub const UIDeviceOrientationFaceDown: UIDeviceOrientation = 6;

pub type UIDeviceBatteryState = NSInteger;
pub const UIDeviceBatteryStateUnknown: UIDeviceBatteryState = 0;
pub const UIDeviceBatteryStateUnplugged: UIDeviceBatteryState = 1;
pub const UIDeviceBatteryStateCharging: UIDeviceBatteryState = 2;
pub const UIDeviceBatteryStateFull: UIDeviceBatteryState = 3;

pub type UIUserInterfaceIdiom = NSInteger;
pub const UIUserInterfaceIdiomPhone: UIUserInterfaceIdiom = 0;
pub const UIUserInterfaceIdiomPad: UIUserInterfaceIdiom = 1;

#[derive(Default)]
pub struct State {
    current_device: Option<id>,
    battery_monitoring_enabled: bool,
    proximity_monitoring_enabled: bool,
    generates_device_orientation_notifications: bool,
    /// Lazily-initialised NSUUID returned by `-identifierForVendor`.
    identifier_for_vendor: Option<id>,
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_UIDeviceOrientationDidChangeNotification",
        HostConstant::NSString(UIDeviceOrientationDidChangeNotification),
    ),
    (
        "_UIDeviceBatteryLevelDidChangeNotification",
        HostConstant::NSString(UIDeviceBatteryLevelDidChangeNotification),
    ),
    (
        "_UIDeviceBatteryStateDidChangeNotification",
        HostConstant::NSString(UIDeviceBatteryStateDidChangeNotification),
    ),
    (
        "_UIDeviceProximityStateDidChangeNotification",
        HostConstant::NSString(UIDeviceProximityStateDidChangeNotification),
    ),
];

/// Deterministic UUIDv5 (SHA-1, RFC 4122 §4.3) of `vendor` under the
/// pre-defined "URL" namespace. Used by `-identifierForVendor` so the same
/// vendor string always yields the same NSUUID.
fn uuid_v5_for_vendor(vendor: &str) -> [u8; 16] {
    // RFC 4122 §C — URL namespace UUID.
    const URL_NS: [u8; 16] = [
        0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30,
        0xc8,
    ];
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(URL_NS);
    h.update(b"ios-vendor:");
    h.update(vendor.as_bytes());
    let digest = h.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Set version 5 (bits 12-15 of time_hi_and_version).
    bytes[6] = (bytes[6] & 0x0F) | 0x50;
    // Set variant (RFC 4122) — top two bits of clock_seq_hi = 10.
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    bytes
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIDevice: NSObject

+ (id)currentDevice {
    if let Some(device) = env.framework_state.uikit.ui_device.current_device {
        device
    } else {
        let new = env.objc.alloc_static_object(
            this,
            Box::new(TrivialHostObject),
            &mut env.mem,
        );
        env.framework_state.uikit.ui_device.current_device = Some(new);
        new
    }
}

// MARK: - Orientation

- (())beginGeneratingDeviceOrientationNotifications {
    env.framework_state.uikit.ui_device.generates_device_orientation_notifications = true;
}

- (())endGeneratingDeviceOrientationNotifications {
    env.framework_state.uikit.ui_device.generates_device_orientation_notifications = false;
}

- (UIDeviceOrientation)orientation {
    // ULTRAHLE_MINIONJUMP_DEVICE_ORIENTATION_BEGIN
    if matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) {
        return UIDeviceOrientationLandscapeRight;
    }
    // ULTRAHLE_MINIONJUMP_DEVICE_ORIENTATION_END

    match env.window().current_rotation() {
        DeviceOrientation::Portrait      => UIDeviceOrientationPortrait,
        DeviceOrientation::PortraitUpsideDown => UIDeviceOrientationPortraitUpsideDown,
        DeviceOrientation::LandscapeLeft  => UIDeviceOrientationLandscapeLeft,
        DeviceOrientation::LandscapeRight => UIDeviceOrientationLandscapeRight,
    }
}

- (())setOrientation:(UIDeviceOrientation)orientation {
    env.window_mut().rotate_device(match orientation {
        UIDeviceOrientationPortrait      => DeviceOrientation::Portrait,
        UIDeviceOrientationPortraitUpsideDown => DeviceOrientation::PortraitUpsideDown,
        UIDeviceOrientationLandscapeLeft  => DeviceOrientation::LandscapeLeft,
        UIDeviceOrientationLandscapeRight => DeviceOrientation::LandscapeRight,
        _ => {
            log!("Warning: UIDevice setOrientation:{} not handled, ignoring", orientation);
            return;
        }
    });
}

- (bool)isGeneratingDeviceOrientationNotifications {
    env.framework_state.uikit.ui_device.generates_device_orientation_notifications
}

// MARK: - Identity

- (id)model {
    // ULTRAHLE_MINIONJUMP_MODEL_BEGIN
    if matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) || crate::env_flag_cached!("TOUCHHLE_FORCE_IPAD_DEVICE_IDENTITY") {
        return ns_string::get_static_str(env, "iPad");
    }
    // ULTRAHLE_MINIONJUMP_MODEL_END

    let family = env.window().device_family();
    let model = if family.is_ipad() {
        "iPad"
    } else if family.is_ipod_touch() {
        "iPod touch"
    } else {
        "iPhone"
    };
    ns_string::get_static_str(env, model)
}
- (id)localizedModel {
    msg![env; this model]
}
- (id)name {
    // ULTRAHLE_MINIONJUMP_NAME_BEGIN
    if matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) || crate::env_flag_cached!("TOUCHHLE_FORCE_IPAD_DEVICE_IDENTITY") {
        return ns_string::get_static_str(env, "iPad");
    }
    // ULTRAHLE_MINIONJUMP_NAME_END

    let family = env.window().device_family();
    let name = if family.is_ipad() {
        "iPad"
    } else if family.is_ipod_touch() {
        "iPod touch"
    } else {
        "iPhone"
    };
    ns_string::get_static_str(env, name)
}
- (id)systemName {
    ns_string::get_static_str(env, "iPhone OS")
}
- (id)systemVersion {
    let (major, minor, patch) = env
        .options
        .as_ref()
        .ios_version
        .unwrap_or(crate::options::LATEST_IOS_VERSION);
    ns_string::from_rust_string(env, format!("{major}.{minor}.{patch}"))
}
- (id)uniqueIdentifier {
    ns_string::get_static_str(env, "touchHLEdevice..........................")
}

// `-identifierForVendor` (iOS 6.0+) — `NSUUID *` that uniquely identifies the
// device to the app's vendor. Apple guarantees the value is stable for all
// apps from the same vendor while at least one is installed; touchHLE has
// no system-wide install database, so we derive a deterministic UUIDv5 from
// the vendor prefix of the app's bundle identifier. Same vendor → same
// UUID across launches; different vendors → different UUIDs.
// <https://developer.apple.com/documentation/uikit/uidevice/1620059-identifierforvendor>
- (id)identifierForVendor { // NSUUID*
    if let Some(existing) =
        env.framework_state.uikit.ui_device.identifier_for_vendor
    {
        return existing;
    }
    let bundle_id = env.bundle.bundle_identifier().to_owned();
    // Vendor scope = first two dot-separated components of the bundle id
    // (e.g. `com.example.MyApp` → `com.example`). Falls back to the full
    // bundle id if there are fewer than two components.
    let vendor: String = {
        let mut parts = bundle_id.split('.');
        match (parts.next(), parts.next()) {
            (Some(a), Some(b)) => format!("{}.{}", a, b),
            _ => bundle_id.clone(),
        }
    };
    let bytes = uuid_v5_for_vendor(&vendor);
    let uuid_str = format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        bytes[0],  bytes[1],  bytes[2],  bytes[3],
        bytes[4],  bytes[5],
        bytes[6],  bytes[7],
        bytes[8],  bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    );
    let ns_uuid_str = ns_string::from_rust_string(env, uuid_str);
    let uuid: id = msg_class![env; NSUUID alloc];
    let uuid: id = msg![env; uuid initWithUUIDString:ns_uuid_str];
    crate::objc::release(env, ns_uuid_str);
    if uuid == nil {
        return nil;
    }
    crate::objc::retain(env, uuid);
    env.framework_state.uikit.ui_device.identifier_for_vendor = Some(uuid);
    uuid
}

// MARK: - Idiom

- (UIUserInterfaceIdiom)userInterfaceIdiom {
    // ULTRAHLE_MINIONJUMP_IDIOM_BEGIN
    if matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) || crate::env_flag_cached!("TOUCHHLE_FORCE_IPAD_DEVICE_IDENTITY") {
        return UIUserInterfaceIdiomPad;
    }
    // ULTRAHLE_MINIONJUMP_IDIOM_END

    if env.window().device_family().is_ipad() {
        UIUserInterfaceIdiomPad
    } else {
        UIUserInterfaceIdiomPhone
    }
}

// MARK: - Capabilities

- (bool)isMultitaskingSupported {
    false
}

- (bool)isProximityMonitoringEnabled {
    env.framework_state.uikit.ui_device.proximity_monitoring_enabled
}

- (())setProximityMonitoringEnabled:(bool)enabled {
    env.framework_state.uikit.ui_device.proximity_monitoring_enabled = enabled;
}

- (bool)proximityState {
    // Proximity sensor never triggered.
    false
}

// MARK: - Battery

- (bool)isBatteryMonitoringEnabled {
    env.framework_state.uikit.ui_device.battery_monitoring_enabled
}

- (())setBatteryMonitoringEnabled:(bool)enabled {
    env.framework_state.uikit.ui_device.battery_monitoring_enabled = enabled;
}

- (f32)batteryLevel {
    let pct = get_battery_status().0;
    if pct < 0 {
        log_dbg!("batteryLevel: could not determine percentage, returning 1.0");
        return 1.0;
    }
    pct as f32 / 100.0
}

- (UIDeviceBatteryState)batteryState {
    match get_battery_status().1 {
        BatteryState::Unknown   => UIDeviceBatteryStateUnknown,
        BatteryState::OnBattery => UIDeviceBatteryStateUnplugged,
        BatteryState::NoBattery |
        BatteryState::Charging  => UIDeviceBatteryStateCharging,
        BatteryState::Full      => UIDeviceBatteryStateFull,
    }
}

// MARK: - Hardware info

- (id)platform {
    // ULTRAHLE_MINIONJUMP_PLATFORM_BEGIN: force an iPad identity for apps that
    // gate their iPad landscape render path on the reported hardware model.
    if matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) || crate::env_flag_cached!("TOUCHHLE_FORCE_IPAD_DEVICE_IDENTITY") {
        return ns_string::get_static_str(env, "iPad2,1");
    }
    // ULTRAHLE_MINIONJUMP_PLATFORM_END

    // Matches the sysctl hw.machine value for the emulated device family.
    // This must agree with sysctl/uname: apps with device whitelists (e.g.
    // BioShock) check the model through UIDevice categories like this one
    // and refuse to start the engine if they see an unsupported device.
    let machine_name = env.window().device_family().machine_name();
    ns_string::get_static_str(env, machine_name)
}

- (id)hwModel {
    // ULTRAHLE_MINIONJUMP_HWMODEL_BEGIN
    if matches!(
        env.bundle.bundle_identifier(),
        "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
    ) || crate::env_flag_cached!("TOUCHHLE_FORCE_IPAD_DEVICE_IDENTITY") {
        return ns_string::get_static_str(env, "iPad2,1");
    }
    // ULTRAHLE_MINIONJUMP_HWMODEL_END

    let machine_name = env.window().device_family().machine_name();
    ns_string::get_static_str(env, machine_name)
}

// MARK: - Notifications (post helpers used by subcomponents)

- (())_postOrientationChangeNotification {
    let name = ns_string::get_static_str(
        env,
        UIDeviceOrientationDidChangeNotification,
    );
    let nc: id = msg_class![env; NSNotificationCenter defaultCenter];
    msg![env; nc postNotificationName:name object:this]
}

@end

};

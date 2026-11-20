/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIScreenMode`.

use crate::frameworks::core_graphics::{CGFloat, CGSize};
use crate::frameworks::foundation::ns_string::from_rust_string;
use crate::frameworks::foundation::NSUInteger;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

#[derive(Default)]
pub struct UIScreenModeHostObject {
    pub size: CGSize,
    pub pixel_aspect_ratio: CGFloat,
}
impl HostObject for UIScreenModeHostObject {}

/// Copies a mode's fields into plain locals, so nothing downstream takes a
/// reference into the (packed) `CGSize` or holds an `env.objc` borrow.
/// Returns `(width, height, pixel_aspect_ratio)`.
fn read_mode(env: &Environment, mode: id) -> (CGFloat, CGFloat, CGFloat) {
    let host = env.objc.borrow::<UIScreenModeHostObject>(mode);
    let size = host.size;
    let ratio = host.pixel_aspect_ratio;
    let (width, height) = (size.width, size.height);
    (width, height, ratio)
}

/// FNV-1a over the three values. `x + 0.0` maps -0.0 to 0.0, so values that
/// compare equal with `==` also hash equal.
fn hash_mode(width: CGFloat, height: CGFloat, ratio: CGFloat) -> u32 {
    let mut h: u32 = 2166136261;
    for value in [width, height, ratio] {
        for byte in (value + 0.0).to_bits().to_le_bytes() {
            h ^= byte as u32;
            h = h.wrapping_mul(16777619);
        }
    }
    h
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIScreenMode: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIScreenModeHostObject {
        size: CGSize { width: 320.0, height: 480.0 },
        pixel_aspect_ratio: 1.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Properties

- (CGSize)size {
    env.objc.borrow::<UIScreenModeHostObject>(this).size
}

- (CGFloat)pixelAspectRatio {
    env.objc.borrow::<UIScreenModeHostObject>(this).pixel_aspect_ratio
}

// MARK: - NSObject

- (bool)isEqual:(id)other {
    if this == other { return true; }
    if other == nil { return false; }
    // There is no try_borrow, so check the class before borrowing.
    let mode_class = env.objc.get_known_class("UIScreenMode", &mut env.mem);
    let is_mode: bool = msg![env; other isKindOfClass:mode_class];
    if !is_mode { return false; }
    read_mode(env, this) == read_mode(env, other)
}

- (NSUInteger)hash {
    let (width, height, ratio) = read_mode(env, this);
    hash_mode(width, height, ratio)
}

- (id)description {
    let (width, height, pixel_aspect_ratio) = read_mode(env, this);
    let s = format!(
        "<UIScreenMode: size=({}, {}), pixelAspectRatio={}>",
        width, height, pixel_aspect_ratio
    );
    let ns = from_rust_string(env, s);
    autorelease(env, ns)
}

@end

};

/// Allocate an autoreleased `UIScreenMode` with a specific size and pixel
/// aspect ratio. For use by `UIScreen` when building its `availableModes`
/// array.
pub fn from_size(env: &mut Environment, size: CGSize, pixel_aspect_ratio: CGFloat) -> id {
    let mode: id = msg_class![env; UIScreenMode alloc];
    {
        let host = env.objc.borrow_mut::<UIScreenModeHostObject>(mode);
        host.size = size;
        host.pixel_aspect_ratio = pixel_aspect_ratio;
    }
    autorelease(env, mode)
}

#[cfg(test)]
mod tests {
    use super::hash_mode;

    #[test]
    fn equal_values_hash_equal() {
        assert_eq!(hash_mode(320.0, 480.0, 1.0), hash_mode(320.0, 480.0, 1.0));
    }

    #[test]
    fn different_values_hash_differently() {
        assert_ne!(hash_mode(320.0, 480.0, 1.0), hash_mode(640.0, 960.0, 1.0));
        assert_ne!(hash_mode(320.0, 480.0, 1.0), hash_mode(480.0, 320.0, 1.0));
    }

    #[test]
    fn negative_zero_hashes_like_zero() {
        // -0.0 == 0.0, so isEqual: says equal and the hashes must match.
        assert_eq!(hash_mode(-0.0, 480.0, 1.0), hash_mode(0.0, 480.0, 1.0));
    }
}

/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaEntity`.
//!
//! Abstract superclass of `MPMediaItem` and `MPMediaItemCollection`. There is
//! no media library on the emulated device, so every property lookup reports
//! "no value". The class deliberately has no host object of its own, so that
//! subclasses are free to use theirs.

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::ns_string::to_rust_string;
use crate::objc::{id, msg, nil, objc_classes, ClassExports};

/// `MPMediaEntityPersistentID` (`uint64_t`)
pub type MPMediaEntityPersistentID = u64;

pub const MPMediaEntityPropertyPersistentID: &str = "persistentID";

pub const CONSTANTS: ConstantExports = &[(
    "_MPMediaEntityPropertyPersistentID",
    HostConstant::NSString(MPMediaEntityPropertyPersistentID),
)];

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaEntity: NSObject

// MARK: - Class methods

// The only property known at this level is the persistent ID. Subclasses
// override this to add their own filterable properties.
+ (bool)canFilterByProperty:(id)property {
    if property == nil {
        return false;
    }
    to_rust_string(env, property) == MPMediaEntityPropertyPersistentID
}

// NSSecureCoding. Nothing is ever archived here.
+ (bool)supportsSecureCoding {
    true
}

// MARK: - Property access

- (id)valueForProperty:(id)_property {
    // No media data, so no value for any property.
    nil
}

// With no values to report, the block is never called.
- (())enumerateValuesForProperties:(id)_properties usingBlock:(id)_block {
}

// Subscripting: `entity[property]`.
- (id)objectForKeyedSubscript:(id)key {
    msg![env; this valueForProperty:key]
}

- (u64)persistentID {
    0
}

@end

};

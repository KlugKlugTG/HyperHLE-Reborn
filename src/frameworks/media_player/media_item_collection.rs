/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMediaItemCollection`.
//!
//! An ordered list of `MPMediaItem`s. There is no media library on the
//! emulated device, so collections normally come from the app itself
//! (`collectionWithItems:`) and are usually empty.

use crate::frameworks::foundation::ns_string::from_rust_string;
use crate::frameworks::foundation::NSUInteger;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

/// `MPMediaType` (bit flags)
pub type MPMediaType = NSUInteger;
pub const MPMediaTypeMusic: MPMediaType = 1 << 0;
pub const MPMediaTypePodcast: MPMediaType = 1 << 1;
pub const MPMediaTypeAudioBook: MPMediaType = 1 << 2;
pub const MPMediaTypeAudioITunesU: MPMediaType = 1 << 3;
pub const MPMediaTypeAnyAudio: MPMediaType = 0x00ff;
pub const MPMediaTypeMovie: MPMediaType = 1 << 8;
pub const MPMediaTypeTVShow: MPMediaType = 1 << 9;
pub const MPMediaTypeVideoPodcast: MPMediaType = 1 << 10;
pub const MPMediaTypeMusicVideo: MPMediaType = 1 << 11;
pub const MPMediaTypeVideoITunesU: MPMediaType = 1 << 12;
pub const MPMediaTypeHomeVideo: MPMediaType = 1 << 13;
pub const MPMediaTypeAnyVideo: MPMediaType = 0xff00;
pub const MPMediaTypeAny: MPMediaType = !0;

#[derive(Default)]
struct MPMediaItemCollectionHostObject {
    /// `NSArray *` of `MPMediaItem *`, a retained copy. nil is treated as empty.
    items: id,
}
impl HostObject for MPMediaItemCollectionHostObject {}

fn responds_to(env: &mut Environment, obj: id, selector: &str) -> bool {
    match env.objc.lookup_selector(selector) {
        Some(sel) => {
            let responds: bool = msg![env; obj respondsToSelector:sel];
            responds
        }
        None => false,
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMediaItemCollection: MPMediaEntity

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::<MPMediaItemCollectionHostObject>::default();
    env.objc.alloc_object(this, host, &mut env.mem)
}

// MARK: - Construction

+ (id)collectionWithItems:(id)items {
    let obj: id = msg![env; this alloc];
    let obj: id = msg![env; obj initWithItems:items];
    autorelease(env, obj)
}

- (id)initWithItems:(id)items {
    // `copy` returns +1, which the host object keeps. nil stays nil.
    let items_copy: id = if items == nil { nil } else { msg![env; items copy] };
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<MPMediaItemCollectionHostObject>(this).items,
        items_copy,
    );
    release(env, old);
    this
}

- (())dealloc {
    let items = std::mem::take(
        &mut env.objc.borrow_mut::<MPMediaItemCollectionHostObject>(this).items,
    );
    release(env, items);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Contents

- (id)items {
    let items = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    if items == nil {
        msg_class![env; NSArray array]
    } else {
        items
    }
}

- (NSUInteger)count {
    let items = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    if items == nil {
        0
    } else {
        msg![env; items count]
    }
}

// The first item, or nil for an empty collection.
- (id)representativeItem {
    let items = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    if items == nil {
        return nil;
    }
    let count: NSUInteger = msg![env; items count];
    if count == 0 {
        return nil;
    }
    msg![env; items objectAtIndex:(0 as NSUInteger)]
}

// The OR of every item's mediaType; 0 for an empty collection.
- (NSUInteger)mediaTypes {
    let items = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    if items == nil {
        return 0;
    }
    let count: NSUInteger = msg![env; items count];
    let mut types: NSUInteger = 0;
    for i in 0..count {
        let item: id = msg![env; items objectAtIndex:i];
        // Items that aren't media items (or don't implement mediaType) are skipped.
        if item != nil && responds_to(env, item, "mediaType") {
            let item_type: NSUInteger = msg![env; item mediaType];
            types |= item_type;
        }
    }
    types
}

// MARK: - Description

- (id)description {
    let items = env.objc.borrow::<MPMediaItemCollectionHostObject>(this).items;
    let count: NSUInteger = if items == nil { 0 } else { msg![env; items count] };
    let s = format!("<MPMediaItemCollection: {:?}; count = {}>", this, count);
    let ns = from_rust_string(env, s);
    autorelease(env, ns)
}

@end

};

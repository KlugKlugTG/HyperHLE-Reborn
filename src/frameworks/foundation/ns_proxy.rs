/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSProxy`.
//!
//! Abstract class for objects that stand in for other objects. Guest code
//! always subclasses it (e.g. a lazy-loading or remote-object wrapper).
//! Declared as an `NSObject` subclass because the class macro cannot declare
//! a root class. Instances of guest subclasses are allocated by `NSObject`,
//! so ivars of the subclass work as usual.

use crate::objc::{id, msg_super, nil, objc_classes, ClassExports};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSProxy: NSObject

// NSProxy is the one class where this answers YES.
- (bool)isProxy {
    true
}

// Abstract. Real NSProxy raises if a subclass does not override these.
// touchHLE has no NSInvocation or method signatures, so log and carry on.
- (id)methodSignatureForSelector:(id)_sel {
    log!(
        "Warning: -[NSProxy methodSignatureForSelector:] called on {:?}, which does not override it; returning nil",
        this
    );
    nil
}

- (())forwardInvocation:(id)_invocation {
    log!(
        "Warning: -[NSProxy forwardInvocation:] called on {:?}, which does not override it; ignoring",
        this
    );
}

// Real NSProxy answers from its own class here and does not forward, which
// is what the NSObject implementation does.
- (bool)respondsToSelector:(id)sel {
    msg_super![env; this respondsToSelector:sel]
}

- (id)description {
    msg_super![env; this description]
}

- (id)debugDescription {
    msg_super![env; this description]
}

// Deprecated GC hook, still sent by some old code.
- (())finalize {}

@end

};

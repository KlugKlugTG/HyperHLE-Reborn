/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Foundation value transformers, including the shared registry. Guest
//! subclasses and categories supply their transformations through ordinary
//! Objective-C dispatch.
//! https://developer.apple.com/documentation/foundation/valuetransformer

use super::ns_string::{get_static_str, to_rust_string};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, Class, ClassExports,
};
use crate::Environment;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    transformers: HashMap<String, id>,
    initialized: bool,
}

fn ensure_builtins(env: &mut Environment) {
    if env
        .framework_state
        .foundation
        .ns_value_transformer
        .initialized
    {
        return;
    }
    env.framework_state
        .foundation
        .ns_value_transformer
        .initialized = true;
    for (name, class_name) in [
        ("NSNegateBoolean", "NSNegateBooleanTransformer"),
        ("NSIsNil", "NSIsNilTransformer"),
        ("NSIsNotNil", "NSIsNotNilTransformer"),
        (
            "NSKeyedUnarchiveFromData",
            "NSKeyedUnarchiveFromDataTransformer",
        ),
    ] {
        let class = env.objc.get_known_class(class_name, &mut env.mem);
        let transformer: id = msg![env; class new];
        env.framework_state
            .foundation
            .ns_value_transformer
            .transformers
            .insert(name.to_owned(), transformer);
    }
}

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);

@implementation NSValueTransformer: NSObject

+ (bool)allowsReverseTransformation { true }
+ (Class)transformedValueClass { msg_class![env; NSObject class] }
+ (())setValueTransformer:(id)transformer forName:(id)name {
    ensure_builtins(env);
    if name == nil {
        let exception_name = get_static_str(env, "NSInvalidArgumentException");
        let reason = get_static_str(env, "A value transformer name cannot be nil");
        let exception: id = msg_class![env; NSException exceptionWithName:exception_name reason:reason userInfo:nil];
        crate::objc::objc_exception_throw(env, exception);
        return;
    }
    let name = to_rust_string(env, name).into_owned();
    retain(env, transformer);
    let registry = &mut env.framework_state.foundation.ns_value_transformer.transformers;
    let old = if transformer == nil {
        registry.remove(&name)
    } else {
        registry.insert(name, transformer)
    };
    if let Some(old) = old { release(env, old); }
}

+ (id)valueTransformerForName:(id)name {
    ensure_builtins(env);
    if name == nil { return nil; }
    let key = to_rust_string(env, name).into_owned();
    if let Some(&transformer) = env.framework_state.foundation.ns_value_transformer.transformers.get(&key) {
        retain(env, transformer);
        return autorelease(env, transformer);
    }
    let Some(class) = env.objc.try_get_known_class(&key, &mut env.mem) else { return nil; };
    let base = env.objc.get_known_class("NSValueTransformer", &mut env.mem);
    let is_transformer: bool = msg![env; class isSubclassOfClass:base];
    if !is_transformer { return nil; }
    let transformer: id = msg![env; class new];
    if transformer != nil {
        let _: () = msg_class![env; NSValueTransformer setValueTransformer:transformer forName:name];
    }
    autorelease(env, transformer)
}

+ (id)valueTransformerNames {
    ensure_builtins(env);
    let names: Vec<_> = env.framework_state.foundation.ns_value_transformer.transformers.keys().cloned().collect();
    let array: id = msg_class![env; NSMutableArray array];
    for name in names {
        let name = super::ns_string::from_rust_string(env, name);
        let _: () = msg![env; array addObject:name];
        release(env, name);
    }
    let result: id = msg![env; array copy];
    autorelease(env, result)
}
- (id)transformedValue:(id)value { value }

- (id)reverseTransformedValue:(id)value {
    let class: Class = msg![env; this class];
    let reversible: bool = msg![env; class allowsReverseTransformation];
    if !reversible {
        let name = get_static_str(env, "NSInvalidArgumentException");
        let reason = get_static_str(env, "This value transformer does not support reverse transformation");
        let exception: id = msg_class![env; NSException exceptionWithName:name reason:reason userInfo:nil];
        crate::objc::objc_exception_throw(env, exception);
        return nil;
    }
    msg![env; this transformedValue:value]
}

@end

@implementation NSNegateBooleanTransformer: NSValueTransformer
+ (Class)transformedValueClass { msg_class![env; NSNumber class] }
- (id)transformedValue:(id)value {
    let value: bool = msg![env; value boolValue];
    msg_class![env; NSNumber numberWithBool:(!value)]
}
@end

@implementation NSIsNilTransformer: NSValueTransformer
+ (bool)allowsReverseTransformation { false }
+ (Class)transformedValueClass { msg_class![env; NSNumber class] }
- (id)transformedValue:(id)value { msg_class![env; NSNumber numberWithBool:(value == nil)] }
@end

@implementation NSIsNotNilTransformer: NSValueTransformer
+ (bool)allowsReverseTransformation { false }
+ (Class)transformedValueClass { msg_class![env; NSNumber class] }
- (id)transformedValue:(id)value { msg_class![env; NSNumber numberWithBool:(value != nil)] }
@end

@implementation NSKeyedUnarchiveFromDataTransformer: NSValueTransformer
+ (Class)transformedValueClass { msg_class![env; NSObject class] }
- (id)transformedValue:(id)value { msg_class![env; NSKeyedUnarchiver unarchiveObjectWithData:value] }
- (id)reverseTransformedValue:(id)value { msg_class![env; NSKeyedArchiver archivedDataWithRootObject:value] }
@end
};

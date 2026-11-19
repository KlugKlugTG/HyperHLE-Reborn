/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include "system_headers.h"

typedef struct {
  NSInteger major;
  NSInteger minor;
  NSInteger patch;
} TestOperatingSystemVersion;

@interface NSProcessInfo (StructReturnTest)
- (TestOperatingSystemVersion)operatingSystemVersion;
- (BOOL)isOperatingSystemAtLeastVersion:(TestOperatingSystemVersion)version;
- (unsigned long long)physicalMemory;
- (NSUInteger)processorCount;
- (NSUInteger)activeProcessorCount;
@end

@interface NSObject (StructReturnIMPTest)
- (void *)methodForSelector:(SEL)selector;
+ (void *)instanceMethodForSelector:(SEL)selector;
@end

extern void *class_getInstanceMethod(Class, SEL);
extern void *method_getImplementation(void *);
extern void *class_getMethodImplementation(Class, SEL);
extern void *class_getMethodImplementation_stret(Class, SEL);
extern void objc_msgSendSuper2_stret(void);
extern Class objc_getClass(const char *);

// +processInfo returns the shared agent; ordinary allocation and -init are
// also exposed by NSProcessInfo's NSObject superclass. Querying an allocated
// instance must describe the current process without requiring singleton
// identity. Xbox's iOS library uses this allocation path for its OS query.
// https://developer.apple.com/documentation/foundation/processinfo/processinfo
// https://github.com/theos/sdks/blob/master/iPhoneOS9.3.sdk/System/Library/Frameworks/Foundation.framework/Headers/NSProcessInfo.h
int test_NSProcessInfo_allocated_instance(void) {
  NSProcessInfo *shared = [NSProcessInfo processInfo];
  NSProcessInfo *allocated = [[NSProcessInfo alloc] init];
  if (!allocated || [NSProcessInfo processInfo] != shared)
    return -1;
  TestOperatingSystemVersion expected = [shared operatingSystemVersion];
  TestOperatingSystemVersion actual = [allocated operatingSystemVersion];
  int result = 0;
  if (actual.major != expected.major || actual.minor != expected.minor ||
      actual.patch != expected.patch)
    result = -2;
  else if ([allocated physicalMemory] != [shared physicalMemory] ||
           [allocated processorCount] != [shared processorCount] ||
           [allocated activeProcessorCount] != [shared activeProcessorCount])
    result = -3;
  else if (![allocated isOperatingSystemAtLeastVersion:expected] ||
           [allocated systemUptime] < 0)
    result = -4;
  [allocated release];
  if ([NSProcessInfo processInfo] != shared)
    result = -5;
  return result;
}

// All IMP lookup APIs return a callable implementation, including when its
// return value uses the ARM hidden result pointer. The implementation stays
// the same for both class_getMethodImplementation variants.
// https://developer.apple.com/documentation/objectivec/class_getmethodimplementation(_:_:)
// https://github.com/apple-oss-distributions/objc4/blob/objc4-779.1/runtime/objc-class.mm
int test_objc_struct_return_imp(void) {
  NSProcessInfo *info = [NSProcessInfo processInfo];
  SEL selector = @selector(operatingSystemVersion);
  void *implementation = method_getImplementation(
      class_getInstanceMethod([NSProcessInfo class], selector));
  if (!implementation ||
      class_getMethodImplementation([NSProcessInfo class], selector) !=
          implementation ||
      class_getMethodImplementation_stret([NSProcessInfo class], selector) !=
          implementation ||
      [info methodForSelector:selector] != implementation ||
      [NSProcessInfo instanceMethodForSelector:selector] != implementation)
    return -1;
  TestOperatingSystemVersion expected = [info operatingSystemVersion];
  TestOperatingSystemVersion (*function)(id, SEL) = implementation;
  TestOperatingSystemVersion actual = function(info, selector);
  if (actual.major != expected.major || actual.minor != expected.minor ||
      actual.patch != expected.patch)
    return -2;
  if (![info isOperatingSystemAtLeastVersion:expected])
    return -3;
  expected.patch++;
  if ([info isOperatingSystemAtLeastVersion:expected])
    return -4;
  return 0;
}

typedef struct {
  NSInteger a;
  NSInteger b;
  NSInteger c;
  NSInteger d;
} StructReturnValue;

@interface StructReturnParent : NSObject
- (StructReturnValue)value:(NSInteger)argument;
@end

static int struct_return_initializations;

@implementation StructReturnParent
+ (void)initialize {
  struct_return_initializations++;
}
- (StructReturnValue)value:(NSInteger)argument {
  StructReturnValue value = {argument, 22, 33, 44};
  return value;
}
@end

// Obtaining an IMP initializes the class before resolving its method.
// https://github.com/apple-oss-distributions/objc4/blob/objc4-779.1/runtime/objc-class.mm
int test_objc_struct_return_imp_initialization(void) {
  Class parent = objc_getClass("StructReturnParent");
  SEL selector = @selector(value:);
  if (!parent || struct_return_initializations != 0)
    return -1;
  void *implementation = class_getMethodImplementation(parent, selector);
  if (!implementation || struct_return_initializations != 1)
    return -2;
  if (class_getMethodImplementation_stret(parent, selector) != implementation ||
      struct_return_initializations != 1)
    return -3;
  return 0;
}

@interface StructReturnChild : StructReturnParent
@end

@implementation StructReturnChild
- (StructReturnValue)value:(NSInteger)argument {
  StructReturnValue value = [super value:argument];
  value.d++;
  return value;
}
@end

// objc_msgSendSuper2_stret preserves the destination in r0 and substitutes
// the receiver for the objc_super pointer in r1 before invoking the IMP.
// https://github.com/apple-oss-distributions/objc4/blob/objc4-779.1/runtime/Messengers.subproj/objc-msg-arm.s
int test_objc_struct_return_super(void) {
  StructReturnChild *child = [[StructReturnChild alloc] init];
  StructReturnValue value = [child value:11];
  int result = 0;
  if (value.a != 11 || value.b != 22 || value.c != 33 || value.d != 45)
    result = -1;
  struct {
    id receiver;
    Class current_class;
  } super = {child, [StructReturnChild class]};
  StructReturnValue (*send_super)(void *, SEL, NSInteger) =
      (void *)objc_msgSendSuper2_stret;
  value = send_super(&super, @selector(value:), 55);
  if (value.a != 55 || value.b != 22 || value.c != 33 || value.d != 44)
    result = -2;
  [child release];
  return result;
}

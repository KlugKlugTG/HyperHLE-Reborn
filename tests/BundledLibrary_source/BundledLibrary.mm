/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include "../TestApp_source/system_headers.h"
#include <stdint.h>

extern "C" {
__attribute__((section("__DATA,__probe_data"), used))
uint32_t bundled_section_marker = 0xfeedcafe;
}

static int load_calls;
static int initialization_marker;

__attribute__((constructor)) static void initialize_library(void) {
  initialization_marker = 0x1234;
}

@interface BundledLibraryProbe : NSObject {
  int _marker;
}
- (int)marker;
- (id)literal;
@end

@implementation BundledLibraryProbe
+ (void)load {
  load_calls++;
}
- (instancetype)init {
  self = [super init];
  if (self)
    _marker = 42;
  return self;
}
- (int)marker {
  return _marker;
}
- (id)literal {
  return @"Bundled library string";
}
@end

extern "C" int bundled_library_load_calls(void) { return load_calls; }

extern "C" int bundled_library_initialization_marker(void) {
  return initialization_marker;
}

extern "C" {
Class objc_lookUpClass(const char *);
Class objc_getMetaClass(const char *);
Class object_getClass(id);
size_t class_getInstanceSize(Class);
id class_createInstance(Class, size_t);
SEL sel_registerName(const char *);
void *class_getInstanceMethod(Class, SEL);
void *method_getImplementation(void *);
void *method_setImplementation(void *, void *);
void arc4random_buf(void *, size_t);
}

static int parent_constructs, child_constructs;
static int parent_destroys, child_destroys;
static int allocation_messages, initialization_messages, initialize_messages;
static int failed_constructors;

struct ParentMarker {
  uint32_t value;
  ParentMarker() : value(0x12345678) { parent_constructs++; }
  ~ParentMarker() { parent_destroys++; }
};

struct ChildMarker {
  uint32_t value;
  ChildMarker() : value(0x87654321) { child_constructs++; }
  ~ChildMarker() { child_destroys++; }
};

@interface RawAllocationParent : NSObject {
@public
  ParentMarker parent_marker;
}
@end

@implementation RawAllocationParent
+ (void)initialize {
  initialize_messages++;
}
+ (id)alloc {
  allocation_messages++;
  return nil;
}
- (id)init {
  initialization_messages++;
  return self;
}
@end

@interface RawAllocationChild : RawAllocationParent {
@public
  ChildMarker child_marker;
}
@end
@implementation RawAllocationChild
@end

static id fail_constructor(id object, SEL selector) {
  (void)object;
  (void)selector;
  failed_constructors++;
  return nil;
}

// Raw instance creation bypasses +alloc/-init/+initialize but runs C++
// construction. A failed child constructor destroys completed ancestors
// and frees the instance, without running the failed child's destructor.
// https://github.com/apple-oss-distributions/objc4/blob/objc4-779.1/runtime/objc-runtime-new.mm
// https://github.com/apple-oss-distributions/objc4/blob/objc4-779.1/runtime/objc-class.mm
extern "C" int bundled_raw_allocation_test(void) {
  Class parent = objc_lookUpClass("RawAllocationParent");
  Class child = objc_lookUpClass("RawAllocationChild");
  if (!parent || !child || class_getInstanceSize(nil) != 0 ||
      class_createInstance(nil, 8) != nil)
    return -20;
  // Name lookup returns this class's metaclass, without +initialize.
  // https://github.com/apple-oss-distributions/objc4/blob/objc4-779.1/runtime/objc-runtime.mm
  if (objc_getMetaClass(0) != nil ||
      objc_getMetaClass("RawAllocationChild") != object_getClass(child))
    return -27;
  const size_t child_size = class_getInstanceSize(child);
  if (class_getInstanceSize(parent) != sizeof(void *) + sizeof(ParentMarker) ||
      child_size != sizeof(void *) + sizeof(ParentMarker) + sizeof(ChildMarker))
    return -21;
  RawAllocationChild *object =
      (RawAllocationChild *)class_createInstance(child, 8);
  if (!object || object_getClass(object) != child ||
      object->parent_marker.value != 0x12345678 ||
      object->child_marker.value != 0x87654321 || parent_constructs != 1 ||
      child_constructs != 1 || allocation_messages || initialization_messages ||
      initialize_messages)
    return -22;
  const uint8_t *trailing = (const uint8_t *)object + child_size;
  for (unsigned i = 0; i < 8; i++)
    if (trailing[i])
      return -23;
  [object release];
  if (parent_destroys != 1 || child_destroys != 1)
    return -24;

  parent_constructs = child_constructs = parent_destroys = child_destroys = 0;
  SEL selector = sel_registerName(".cxx_construct");
  void *method = class_getInstanceMethod(child, selector);
  void *original = method_getImplementation(method);
  method_setImplementation(method, (void *)fail_constructor);
  id failed = class_createInstance(child, 0);
  method_setImplementation(method, original);
  if (failed || failed_constructors != 1 || parent_constructs != 1 ||
      child_constructs || parent_destroys != 1 || child_destroys)
    return -25;
  // The random-buffer API must fill its requested range and preserve guards.
  // https://github.com/apple-oss-distributions/Libc/blob/main/gen/FreeBSD/arc4random.c
  uint8_t random_bytes[130];
  for (unsigned i = 0; i < sizeof(random_bytes); i++)
    random_bytes[i] = 0x55;
  arc4random_buf(0, 0);
  arc4random_buf(random_bytes + 1, 128);
  if (random_bytes[0] != 0x55 || random_bytes[129] != 0x55)
    return -28;
  bool changed = false;
  for (unsigned i = 1; i < 129; i++)
    changed |= random_bytes[i] != 0x55;
  if (!changed)
    return -29;
  return 0;
}

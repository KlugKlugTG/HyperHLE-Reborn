/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include "../TestApp_source/system_headers.h"
#include <stdint.h>
#include <stdio.h>
#include <string.h>

// This subclass depends on a superclass whose metadata and methods belong
// to the bundled framework, rather than to the main executable.
@interface BundledLibraryProbe : NSObject
- (int)marker;
- (id)literal;
@end

@interface BundledLibraryChild : BundledLibraryProbe
@end

@implementation BundledLibraryChild
- (int)marker {
  return [super marker] + 1;
}
@end

extern int bundled_library_load_calls(void);
extern int bundled_library_initialization_marker(void);
extern int bundled_raw_allocation_test(void);
extern uint32_t bundled_section_marker;
extern uint32_t _dyld_image_count(void);
extern const char *_dyld_get_image_name(uint32_t);
extern const void *_dyld_get_image_header(uint32_t);
extern const void *_NSGetMachExecuteHeader(void);
extern intptr_t _dyld_get_image_vmaddr_slide(uint32_t);
extern void _dyld_register_func_for_add_image(void (*)(const void *, intptr_t));
extern uint8_t *getsectiondata(const void *, const char *, const char *,
                               unsigned long *);

static const void *expected_header;
static intptr_t expected_slide;
static int callback_calls;
static int callback_error;
static int library_initialized_first;

__attribute__((constructor)) static void initialize_main(void) {
  library_initialized_first = bundled_library_initialization_marker() == 0x1234;
}

static void image_callback(const void *header, intptr_t slide) {
  if (header == expected_header) {
    callback_calls++;
    if (slide != expected_slide)
      callback_error = 1;
  }
}

// dyld's image callback and slide query must agree with the relocated
// __TEXT address. Check the original segment command in the Mach-O header.
// https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/dyld.3.html
static int check_image(void) {
  // crt_externs exposes the executable's header, rather than a framework.
  // https://github.com/apple-oss-distributions/Libc/blob/Libc-997.90.3/sys/crt_externs.c
  if (_NSGetMachExecuteHeader() != _dyld_get_image_header(0))
    return -13;
  for (uint32_t i = 0; i < _dyld_image_count(); i++) {
    const char *path = _dyld_get_image_name(i);
    if (!path ||
        !strstr(path, "/Frameworks/BundledLibrary.framework/BundledLibrary"))
      continue;
    expected_header = _dyld_get_image_header(i);
    expected_slide = _dyld_get_image_vmaddr_slide(i);
    if (!expected_header || !expected_slide)
      return -4;
    const uint32_t *header = expected_header;
    if (header[0] != 0xfeedface)
      return -5;
    const uint32_t *command = header + 7;
    int found_text = 0;
    for (uint32_t index = 0; index < header[4]; index++) {
      if (command[0] == 1 &&
          !strncmp((const char *)(command + 2), "__TEXT", 16)) {
        if ((uintptr_t)expected_header !=
            command[6] + (uintptr_t)expected_slide)
          return -6;
        found_text = 1;
        break;
      }
      command = (const uint32_t *)((const char *)command + command[1]);
    }
    if (!found_text)
      return -7;
    unsigned long size = 0;
    const uint32_t *data = (const uint32_t *)getsectiondata(
        expected_header, "__DATA", "__probe_data", &size);
    if (data != &bundled_section_marker || size != sizeof(*data) ||
        *data != 0xfeedcafe)
      return -11;
    size = 0xabcdef01;
    if (getsectiondata(expected_header, "__DATA", "__missing", &size) ||
        size != 0xabcdef01)
      return -12;
    _dyld_register_func_for_add_image(image_callback);
    return callback_calls == 1 && !callback_error ? 0 : -8;
  }
  return -9;
}

// @rpath is resolved using the executable's LC_RPATH. The loaded image
// contributes its selectors, class methods, instance methods, and strings.
// https://developer.apple.com/library/archive/documentation/DeveloperTools/Conceptual/DynamicLibraries/100-Articles/RunpathDependentLibraries.html
int main(int argc, char **argv) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  int result = 0;
  BundledLibraryChild *probe = [[BundledLibraryChild alloc] init];
  if (!probe || [probe marker] != 43)
    result = -1;
  else if (![[probe literal] isEqualToString:@"Bundled library string"])
    result = -2;
  else if (bundled_library_load_calls() != 1)
    result = -3;
  else if (!library_initialized_first)
    result = -10;
  else {
    result = check_image();
    if (!result)
      result = bundled_raw_allocation_test();
  }
  [probe release];
  [pool drain];
  fprintf(stderr, "Bundled dylib test: %s (%d)\n",
          result == 0 ? "PASS" : "FAIL", result);
  return result == 0 ? 0 : 1;
}

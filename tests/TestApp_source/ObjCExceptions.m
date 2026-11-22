/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#import "system_headers.h"

@interface NSException : NSObject
- (id)initWithName:(NSString *)name reason:(NSString *)reason userInfo:(id)info;
- (void)raise;
@end

static int destroyed;
@interface ExceptionProbe : NSException
@end
@implementation ExceptionProbe
- (void)dealloc {
  destroyed++;
  [super dealloc];
}
@end

@interface NestedExceptionMessenger : NSObject
- (void)throwException:(NSException *)exception;
- (int)catchException:(NSException *)exception;
@end
@implementation NestedExceptionMessenger
- (void)throwException:(NSException *)exception {
  [exception raise];
  __builtin_trap();
}
- (int)catchException:(NSException *)exception {
  volatile int progress = 1;
  @try {
    [self throwException:exception];
    progress = -100;
  } @catch (NSException *caught) {
    progress += caught == exception ? 10 : -200;
  } @finally {
    progress += 100;
  }
  // Exercise another guest method after the skipped frame is unwound.
  return progress + ([self class] == [NestedExceptionMessenger class]);
}
@end

// A guest-to-guest message must keep the original return address. Catching
// inside a guest method must not return into the skipped method's host frame.
// https://github.com/apple-oss-distributions/objc4/blob/objc4-551.1/runtime/Messengers.subproj/objc-msg-arm.s
int test_ObjC_exception_nested_message_dispatch(void) {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];
  NestedExceptionMessenger *messenger = [NestedExceptionMessenger new];
  NSException *exception =
      [[NSException alloc] initWithName:@"NestedProbe"
                                 reason:@"Expected exception"
                               userInfo:nil];
  int result = 0;
  for (int i = 0; i < 3; i++) {
    if ([messenger catchException:exception] != 112)
      result = -1;
  }
  [exception release];
  [messenger release];
  [pool drain];
  return result;
}

// https://github.com/apple-oss-distributions/objc4/blob/main/runtime/objc-exception.mm
int test_ObjC_exception_catch_rethrow_cleanup(void) {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];
  ExceptionProbe *exception =
      [[ExceptionProbe alloc] initWithName:@"Probe"
                                    reason:@"Expected exception"
                                  userInfo:nil];
  destroyed = 0;
  int progress = 0;
  @try {
    @try {
      [exception raise];
      progress = -100;
    } @catch (NSString *wrongType) {
      progress = -200;
    } @catch (NSException *caught) {
      if (caught != exception)
        progress = -300;
      else
        progress++;
      @throw;
    } @finally {
      progress += 10;
    }
  } @catch (id caught) {
    if (caught != exception)
      progress = -400;
    else
      progress++;
  } @finally {
    progress += 100;
  }
  [exception release];
  [pool drain];
  if (progress != 112)
    return -1;
  if (destroyed != 1)
    return -2;
  return 0;
}

void TestApp_uncaught_objc_exception_probe(void) {
  NSException *exception =
      [[NSException alloc] initWithName:@"UncaughtProbe"
                                 reason:@"Headless must fail"
                               userInfo:nil];
  [exception raise];
  __builtin_trap();
}

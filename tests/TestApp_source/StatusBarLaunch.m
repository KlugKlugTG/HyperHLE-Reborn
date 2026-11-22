/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#import "system_headers.h"

#include <stdio.h>
#include <stdlib.h>

@interface UIApplication (StatusBarLaunchTest)
- (BOOL)isStatusBarHidden;
- (void)setStatusBarHidden:(BOOL)hidden;
@end
@interface UIScreen (StatusBarLaunchTest)
- (CGRect)bounds;
@end

static BOOL expectedHidden;
static int launchResult;

// UIStatusBarHidden specifies the *initial* state. applicationFrame must use
// it while a custom UIApplication subclass constructs its window, and an
// explicit change during init must survive until the launch callback.
// https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/iPhoneOSKeys.html
@interface StatusBarLaunchApplication : UIApplication
@end
@implementation StatusBarLaunchApplication
- (id)init {
  self = [super init];
  CGRect bounds = [[UIScreen mainScreen] bounds];
  CGRect frame = [[UIScreen mainScreen] applicationFrame];
  float inset = expectedHidden ? 0.0f : 20.0f;
  if ([self isStatusBarHidden] != expectedHidden)
    launchResult = 1;
  else if (frame.origin.x != bounds.origin.x ||
           frame.origin.y != bounds.origin.y + inset ||
           frame.size.width != bounds.size.width ||
           frame.size.height != bounds.size.height - inset)
    launchResult = 2;
  [self setStatusBarHidden:!expectedHidden];
  return self;
}
- (void)applicationDidFinishLaunching:(UIApplication *)application {
  if (application != self || [self isStatusBarHidden] == expectedHidden)
    launchResult = 3;
  printf("Status bar launch probe: %s (%d)\n",
         launchResult == 0 ? "OK" : "FAIL", launchResult);
  fflush(stdout);
  exit(launchResult);
}
@end

int TestApp_status_bar_launch_probe(int argc, char **argv, int hidden) {
  expectedHidden = hidden;
  launchResult = 0;
  return UIApplicationMain(argc, argv, @"StatusBarLaunchApplication",
                           @"StatusBarLaunchApplication");
}

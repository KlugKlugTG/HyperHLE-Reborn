/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include "system_headers.h"

// The common-3.0 SDK predates GLKit. These declarations follow Apple's SDK:
// https://github.com/xybp888/iOS-SDKs/blob/master/iPhoneOS9.3.sdk/System/Library/Frameworks/GLKit.framework/Headers/GLKViewController.h
@interface UIView : NSObject
- (instancetype)initWithFrame:(CGRect)frame;
@end
@interface GLKView : UIView
@property(nonatomic, assign) id delegate;
@property(nonatomic) BOOL enableSetNeedsDisplay;
- (void)display;
@end
@interface UIViewController : NSObject
@property(nonatomic, retain) UIView *view;
- (void)viewWillAppear:(BOOL)animated;
- (void)viewWillDisappear:(BOOL)animated;
@end
@interface GLKViewController : UIViewController
@property(nonatomic, assign) id delegate;
@property(nonatomic) NSInteger preferredFramesPerSecond;
@property(nonatomic, readonly) NSInteger framesPerSecond;
@property(nonatomic, readonly) NSInteger framesDisplayed;
@property(nonatomic, getter=isPaused) BOOL paused;
@property(nonatomic) BOOL pauseOnWillResignActive;
@property(nonatomic) BOOL resumeOnDidBecomeActive;
@property(nonatomic, readonly) NSTimeInterval timeSinceLastUpdate;
@property(nonatomic, readonly) NSTimeInterval timeSinceLastDraw;
@end
@interface NSRunLoop : NSObject
+ (instancetype)mainRunLoop;
- (BOOL)runMode:(id)mode beforeDate:(id)limitDate;
@end
@interface NSDate : NSObject
+ (instancetype)dateWithTimeIntervalSinceNow:(NSTimeInterval)seconds;
@end
extern id NSDefaultRunLoopMode;

static int glk_updates, glk_draws, glk_delegate_updates, glk_pauses,
    glk_deallocs;
static int glk_bad_order;

// Exercise the real scheduled controller callbacks in headless TestApp. The
// view's display override avoids creating a host GL context; runtime graphics
// validation separately checks drawable allocation and EAGL presentation.
@interface GLKLoopTestView : GLKView
@end
@implementation GLKLoopTestView
- (void)display {
  if (glk_updates + glk_delegate_updates != glk_draws + 1)
    glk_bad_order++;
  glk_draws++;
}
@end
@interface GLKLoopTestController : GLKViewController
@end
@implementation GLKLoopTestController
- (void)update {
  glk_updates++;
}
- (void)dealloc {
  glk_deallocs++;
  [super dealloc];
}
@end
@interface GLKLoopTestDelegate : NSObject
@end
@implementation GLKLoopTestDelegate
- (void)glkViewControllerUpdate:(GLKViewController *)controller {
  glk_delegate_updates++;
}
- (void)glkViewController:(GLKViewController *)controller
                willPause:(BOOL)pause {
  glk_pauses++;
}
@end

static void pump_glk_loop(void) {
  [[NSRunLoop mainRunLoop] runMode:NSDefaultRunLoopMode
                        beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.05]];
}

int test_GLKViewController_render_loop(void) {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];
  glk_updates = glk_draws = glk_delegate_updates = glk_pauses = glk_deallocs =
      0;
  glk_bad_order = 0;
  GLKLoopTestController *controller = [GLKLoopTestController new];
  GLKLoopTestView *view =
      [[GLKLoopTestView alloc] initWithFrame:(CGRect){{0, 0}, {320, 480}}];
  GLKLoopTestDelegate *delegate = [GLKLoopTestDelegate new];
  controller.view = view;
  controller.delegate = delegate;
  int result = 0;
  if (view.delegate != controller || view.enableSetNeedsDisplay ||
      controller.framesPerSecond != 30 || controller.framesDisplayed != 0)
    result = -1;
  controller.preferredFramesPerSecond = 24;
  if (controller.framesPerSecond != 20)
    result = -2;
  controller.preferredFramesPerSecond = 100;
  if (controller.framesPerSecond != 60)
    result = -3;
  [controller viewWillAppear:NO];
  for (int i = 0; i < 200 && glk_draws < 3; i++)
    pump_glk_loop();
  if (glk_updates < 3 || glk_delegate_updates != 0 || glk_bad_order != 0 ||
      controller.framesDisplayed != glk_draws ||
      controller.timeSinceLastUpdate <= 0 || controller.timeSinceLastDraw <= 0)
    result = -4;
  controller.paused = YES;
  int draws = glk_draws;
  for (int i = 0; i < 5; i++)
    pump_glk_loop();
  if (glk_draws != draws || !controller.isPaused || glk_pauses != 1)
    result = -5;
  controller.paused = NO;
  for (int i = 0; i < 200 && glk_draws == draws; i++)
    pump_glk_loop();
  if (glk_draws == draws || glk_pauses != 2)
    result = -6;
  [controller viewWillDisappear:NO];
  draws = glk_draws;
  for (int i = 0; i < 5; i++)
    pump_glk_loop();
  if (glk_draws != draws)
    result = -7;
  [controller viewWillAppear:NO];
  for (int i = 0; i < 200 && glk_draws == draws; i++)
    pump_glk_loop();
  if (glk_draws == draws)
    result = -8;
  // Releasing an active controller must stop callbacks and break the link
  // ownership chain immediately, even before draining the autorelease pool.
  [controller release];
  draws = glk_draws;
  for (int i = 0; i < 5; i++)
    pump_glk_loop();
  if (glk_draws != draws || glk_deallocs != 1)
    result = -9;
  [view release];
  // A controller without an update override uses its delegate instead.
  controller = (id)[GLKViewController new];
  view = [[GLKLoopTestView alloc] initWithFrame:(CGRect){{0, 0}, {320, 480}}];
  controller.view = view;
  controller.delegate = delegate;
  [controller viewWillAppear:NO];
  for (int i = 0; i < 200 && glk_delegate_updates < 2; i++)
    pump_glk_loop();
  if (glk_delegate_updates < 2 || glk_bad_order != 0)
    result = -10;
  [controller release];
  [view release];
  [delegate release];
  [pool drain];
  return result;
}

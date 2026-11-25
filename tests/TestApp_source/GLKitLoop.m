/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include "system_headers.h"
#include <stdio.h>

// The common-3.0 SDK predates GLKit. These declarations follow Apple's SDK:
// https://github.com/xybp888/iOS-SDKs/blob/master/iPhoneOS9.3.sdk/System/Library/Frameworks/GLKit.framework/Headers/GLKViewController.h
@interface UIView (GLKitLoopMethods)
- (instancetype)initWithFrame:(CGRect)frame;
@end
@interface GLKView : UIView
@property(nonatomic, assign) id delegate;
@property(nonatomic) BOOL enableSetNeedsDisplay;
- (void)display;
- (void)bindDrawable;
- (void)deleteDrawable;
@property(nonatomic, retain) id context;
@property(nonatomic) NSInteger drawableColorFormat;
@property(nonatomic) NSInteger drawableDepthFormat;
@property(nonatomic) NSInteger drawableStencilFormat;
@property(nonatomic) NSInteger drawableMultisample;
@property(nonatomic) CGFloat contentScaleFactor;
@property(nonatomic) CGRect frame;
@property(nonatomic, readonly) NSInteger drawableWidth;
@property(nonatomic, readonly) NSInteger drawableHeight;
@property(nonatomic, readonly) id snapshot;
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

@interface EAGLContext : NSObject
- (instancetype)initWithAPI:(NSUInteger)api;
+ (BOOL)setCurrentContext:(EAGLContext *)context;
@end
@interface UIImage (GLKitSnapshotMethods)
@property(nonatomic, readonly) CGImageRef CGImage;
@end
extern void glClearColor(float, float, float, float);
extern void glClear(unsigned);
extern void glGetIntegerv(unsigned, int *);
extern void glGetRenderbufferParameteriv(unsigned, unsigned, int *);
extern unsigned glGetError(void);
extern unsigned glCheckFramebufferStatus(unsigned);
extern unsigned char glIsFramebuffer(unsigned);
extern void glPixelStorei(unsigned, int);
extern void glBindFramebuffer(unsigned, unsigned);
extern void glGetFramebufferAttachmentParameteriv(unsigned, unsigned, unsigned,
                                                  int *);
extern void glVertexAttrib1fv(unsigned, const float *);
extern void glVertexAttrib2fv(unsigned, const float *);
extern void glVertexAttrib3fv(unsigned, const float *);
extern void glVertexAttrib4fv(unsigned, const float *);
extern void glGetVertexAttribfv(unsigned, unsigned, float *);
extern unsigned glCreateShader(unsigned);
extern void glShaderSource(unsigned, int, const char **, const int *);
extern void glCompileShader(unsigned);
extern void glGetShaderiv(unsigned, unsigned, int *);
extern unsigned glCreateProgram(void);
extern void glAttachShader(unsigned, unsigned);
extern void glLinkProgram(unsigned);
extern void glGetProgramiv(unsigned, unsigned, int *);
extern void glGetActiveUniform(unsigned, unsigned, int, int *, int *,
                               unsigned *, char *);
extern void glGetActiveAttrib(unsigned, unsigned, int, int *, int *, unsigned *,
                              char *);
extern void glDeleteProgram(unsigned);
extern void glDeleteShader(unsigned);

// Each vector entry point reads the stated number of guest components; the
// missing components are (0, 0, 1). A scalar host temporary cannot back it.
// https://registry.khronos.org/OpenGL-Refpages/es3.0/html/glVertexAttrib.xhtml
static int test_vertex_attribute_vectors(void) {
  const float input[4] = {0.25f, -0.5f, 0.75f, 0.125f};
  void (*setters[])(unsigned,
                    const float *) = {glVertexAttrib1fv, glVertexAttrib2fv,
                                      glVertexAttrib3fv, glVertexAttrib4fv};
  for (unsigned n = 1; n <= 4; n++) {
    setters[n - 1](7, input);
    float actual[4] = {9, 9, 9, 9};
    glGetVertexAttribfv(7, 0x8626, actual);
    for (unsigned component = 0; component < 4; component++) {
      float expected = component < n ? input[component] : component == 3;
      if (actual[component] != expected)
        return -1;
    }
  }
  return glGetError() ? -2 : 0;
}

// bufSize limits the returned name, not the size/type metadata. Zero is valid.
// https://registry.khronos.org/OpenGL-Refpages/es3.0/html/glGetActiveUniform.xhtml
static int test_shader_reflection_without_names(void) {
  const char *sources[] = {
      "attribute vec4 position; uniform mat4 transform;"
      "void main() { gl_Position = transform * position; }",
      "precision mediump float; uniform vec4 tint;"
      "void main() { gl_FragColor = tint; }"};
  unsigned shaders[] = {glCreateShader(0x8b31), glCreateShader(0x8b30)};
  unsigned program = glCreateProgram();
  int result = 0;
  for (unsigned i = 0; i < 2; i++) {
    glShaderSource(shaders[i], 1, &sources[i], NULL);
    glCompileShader(shaders[i]);
    int compiled = 0;
    glGetShaderiv(shaders[i], 0x8b81, &compiled);
    if (!compiled)
      result = -1;
    glAttachShader(program, shaders[i]);
  }
  glLinkProgram(program);
  int linked = 0, count = 0;
  glGetProgramiv(program, 0x8b82, &linked);
  glGetProgramiv(program, 0x8b86, &count);
  if (!linked || count != 2)
    result = -2;
  for (unsigned i = 0; i < (unsigned)count; i++) {
    int length = -1, size = -1;
    unsigned type = 0;
    glGetActiveUniform(program, i, 0, &length, &size, &type, NULL);
    if (length || size != 1 || (type != 0x8b5c && type != 0x8b52))
      result = -3;
    char name = 'X';
    glGetActiveUniform(program, i, 1, &length, &size, &type, &name);
    if (name || length || size != 1)
      result = -4;
  }
  int length = -1, size = -1;
  unsigned type = 0;
  glGetActiveAttrib(program, 0, 0, &length, &size, &type, NULL);
  if (length || size != 1 || type != 0x8b52)
    result = -5;
  length = size = -7;
  glGetActiveAttrib(program, 0, -1, &length, &size, &type, NULL);
  if (glGetError() != 0x501 || length != -7 || size != -7)
    result = -6;
  glDeleteProgram(program);
  for (unsigned i = 0; i < 2; i++)
    glDeleteShader(shaders[i]);
  return result;
}

static int glk_graphics_error;
@interface GLKDrawableProbe : NSObject
@end
@implementation GLKDrawableProbe
- (void)glkView:(GLKView *)view drawInRect:(CGRect)rect {
  int viewport[4] = {0};
  glGetIntegerv(0x0ba2, viewport);
  if (viewport[0] || viewport[1] || viewport[2] != view.drawableWidth ||
      viewport[3] != view.drawableHeight ||
      glCheckFramebufferStatus(0x8d40) != 0x8cd5)
    glk_graphics_error = -1;
  glClearColor(1, 0, 0, 1);
  glClear(0x4000);
}
@end

// Run this probe graphically in CI with Xvfb, using the same TestApp binary
// produced by the full headless suite. No game assets are needed.
int test_GLKView_drawable_graphics(void) {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];
  EAGLContext *context = [[EAGLContext alloc] initWithAPI:2];
  GLKView *view = [[GLKView alloc] initWithFrame:(CGRect){{0, 0}, {13, 7}}];
  GLKDrawableProbe *delegate = [GLKDrawableProbe new];
  view.context = context;
  view.contentScaleFactor = 1;
  view.delegate = delegate;
  [view bindDrawable];
  int result = test_vertex_attribute_vectors() ? -9 : 0;
  if (test_shader_reflection_without_names())
    result = -10;
  const int configurations[][4] = {
      {0, 0, 0, 0}, {0, 1, 0, 0}, {0, 2, 1, 0},
      {0, 2, 1, 1}, {1, 0, 0, 1}, {2, 0, 0, 1},
  };
  for (unsigned i = 0; i < sizeof(configurations) / sizeof(configurations[0]);
       i++) {
    view.drawableColorFormat = configurations[i][0];
    view.drawableDepthFormat = configurations[i][1];
    view.drawableStencilFormat = configurations[i][2];
    view.drawableMultisample = configurations[i][3];
    [view bindDrawable];
    int framebuffer = 0, depth = 0, stencil = 0;
    glGetIntegerv(0x8ca6, &framebuffer);
    glGetFramebufferAttachmentParameteriv(0x8d40, 0x8d00, 0x8cd1, &depth);
    glGetFramebufferAttachmentParameteriv(0x8d40, 0x8d20, 0x8cd1, &stencil);
    if (!framebuffer || glCheckFramebufferStatus(0x8d40) != 0x8cd5 ||
        (!!depth != !!configurations[i][1]) ||
        (!!stencil != !!configurations[i][2]))
      result = -2;
    // A foreign binding must be replaced by GLKit before the draw callback.
    glBindFramebuffer(0x8d40, 0);
    glPixelStorei(0x0d05, 8);
    UIImage *snapshot = view.snapshot;
    CGImageRef image = snapshot.CGImage;
    if (!image || CGImageGetWidth(image) != 13 || CGImageGetHeight(image) != 7)
      result = -3;
    CFDataRef data = CGDataProviderCopyData(CGImageGetDataProvider(image));
    const UInt8 *pixels = CFDataGetBytePtr(data);
    if (CFDataGetLength(data) != 13 * 7 * 4 || pixels[0] != 255 ||
        pixels[1] != 0 || pixels[2] != 0 || pixels[3] != 255)
      result = -4;
    CFRelease(data);
    int alignment = 0;
    glGetIntegerv(0x0d05, &alignment);
    if (alignment != 8 || glGetError() != 0 || glk_graphics_error)
      result = -5;
    [view deleteDrawable];
    if (glIsFramebuffer(framebuffer))
      result = -6;
    printf("GLKit drawable configuration %u: %s\n", i, result ? "FAIL" : "OK");
  }
  view.frame = (CGRect){{0, 0}, {17, 9}};
  view.contentScaleFactor = 2;
  [view bindDrawable];
  if (view.drawableWidth != 34 || view.drawableHeight != 18)
    result = -7;
  view.contentScaleFactor = 1;
  [view bindDrawable];
  if (view.drawableWidth != 17 || view.drawableHeight != 9)
    result = -8;
  [view deleteDrawable];
  [view release];
  [delegate release];
  [EAGLContext setCurrentContext:nil];
  [context release];
  [pool drain];
  printf("GLKit graphics probe: %s (%d)\n", result ? "FAIL" : "OK", result);
  return result;
}

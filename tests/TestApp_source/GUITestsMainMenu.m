/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include "system_headers.h"

#include "GUITestsAppDelegate.h"
#include "GUITestsBlendModeView.h"
#include "GUITestsCALayerTestsView.h"
#include "GUITestsCGFontGlyphTestsView.h"
#include "GUITestsMainMenu.h"
#include "GUITestsNavigationBarView.h"
#include "GUITestsNavigationControllerView.h"
#include "GUITestsUIKitWidgetsView.h"

@implementation GUITestsMainMenu : UIView

UIView *ball;
CGFloat ballXVelocity;
CGFloat ballYVelocity;
UIWindow *window2;

- (instancetype)initWithFrame:(CGRect)frame {
  [super initWithFrame:frame];

  UILabel *label = [[[UILabel alloc] initWithFrame:[self bounds]] autorelease];
  label.text = [NSString stringWithUTF8String:"hello, world! 🌏"];
  label.textAlignment = UITextAlignmentCenter;
  [self addSubview:label];
  ball = [[UIView alloc] initWithFrame:CGRectMake(0, 0, 20, 20)];
  ball.layer.cornerRadius = ball.frame.size.width / 2;
  ball.backgroundColor = [UIColor redColor];
  [self addSubview:ball];
  ballXVelocity = 5;
  ballYVelocity = 5;

  // One button per manual test screen. They are laid out in the order below,
  // from the top down, so adding a screen only means appending a row.
  struct {
    const char *title;
    SEL action;
  } rows[] = {
      {"Navigation bar", @selector(goToNavigationBarTests)},
      {"Navigation controller", @selector(goToNavigationControllerTests)},
      {"UIKit controls", @selector(goToUIKitWidgetsTests)},
      {"CALayer tests", @selector(goToCALayerTests)},
      {"CGFont/CGGlyph tests", @selector(goToCGFontGlyphTests)},
      {"BlendMode tests", @selector(goToBlendModeTests)},
      {"Window 2", @selector(toggleWindow)},
  };
  for (int i = 0; i < (int)(sizeof(rows) / sizeof(rows[0])); i++) {
    UIButton *button = [UIButton buttonWithType:UIButtonTypeRoundedRect];
    [button setTitle:[NSString stringWithUTF8String:rows[i].title]
            forState:UIControlStateNormal];
    [button setFrame:CGRectMake(40, 130 + i * 42, 240, 38)];
    [button addTarget:self
                  action:rows[i].action
        forControlEvents:UIControlEventTouchUpInside];
    [self addSubview:button];
  }

  return self;
}

- (void)dealloc {
  [ball release];
  [window2 release];
  [super dealloc];
}

- (void)tick {
  CGRect windowFrame = [self bounds];
  CGRect ballFrame = [ball frame];
  ballFrame.origin.x += ballXVelocity;
  ballFrame.origin.y += ballYVelocity;
  CGFloat oldXVelocity = ballXVelocity;
  CGFloat oldYVelocity = ballYVelocity;
  if (CGRectGetMaxX(ballFrame) >= CGRectGetMaxX(windowFrame)) {
    ballXVelocity = -ballXVelocity;
    ballFrame.origin.x = CGRectGetMaxX(windowFrame) - ballFrame.size.width;
  } else if (CGRectGetMinX(ballFrame) <= CGRectGetMinX(windowFrame)) {
    ballXVelocity = -ballXVelocity;
    ballFrame.origin.x = CGRectGetMinX(windowFrame);
  }
  if (CGRectGetMaxY(ballFrame) >= CGRectGetMaxY(windowFrame)) {
    ballYVelocity = -ballYVelocity;
    ballFrame.origin.y = CGRectGetMaxY(windowFrame) - ballFrame.size.height;
  } else if (CGRectGetMinY(ballFrame) <= CGRectGetMinY(windowFrame)) {
    ballYVelocity = -ballYVelocity;
    ballFrame.origin.y = CGRectGetMinY(windowFrame);
  }
  if (oldXVelocity != ballXVelocity || oldYVelocity != ballYVelocity)
    ball.backgroundColor =
        [UIColor colorWithRed:(ballFrame.origin.x / windowFrame.size.width)
                        green:((ballXVelocity + ballYVelocity) / 10.0 + 0.5)
                         blue:(ballFrame.origin.y / windowFrame.size.height)
                        alpha:1.0];
  ball.frame = ballFrame;
}

- (void)goToNavigationBarTests {
  [((GUITestsAppDelegate *)[[UIApplication sharedApplication]
      delegate]) setMainView:[[[GUITestsNavigationBarView alloc]
                                 initWithFrame:[self frame]] autorelease]];
}

- (void)goToNavigationControllerTests {
  [((GUITestsAppDelegate *)[[UIApplication sharedApplication]
      delegate]) setMainView:[[[GUITestsNavigationControllerView alloc]
                                 initWithFrame:[self frame]] autorelease]];
}

- (void)goToUIKitWidgetsTests {
  [((GUITestsAppDelegate *)[[UIApplication sharedApplication]
      delegate]) setMainView:[[[GUITestsUIKitWidgetsView alloc]
                                 initWithFrame:[self frame]] autorelease]];
}

- (void)goToCALayerTests {
  [((GUITestsAppDelegate *)[[UIApplication sharedApplication]
      delegate]) setMainView:[[[GUITestsCALayerTestsView alloc]
                                 initWithFrame:[self frame]] autorelease]];
}

- (void)goToCGFontGlyphTests {
  [((GUITestsAppDelegate *)[[UIApplication sharedApplication]
      delegate]) setMainView:[[[GUITestsCGFontGlyphTestsView alloc]
                                 initWithFrame:[self frame]] autorelease]];
}

- (void)goToBlendModeTests {
  [((GUITestsAppDelegate *)[[UIApplication sharedApplication]
      delegate]) setMainView:[[[GUITestsBlendModeView alloc]
                                 initWithFrame:[self frame]] autorelease]];
}

- (void)toggleWindow {
  if (!window2) {
    window2 = [[UIWindow alloc] initWithFrame:CGRectMake(80, 0, 80, 80)];
    window2.backgroundColor = [UIColor magentaColor];
  }
  [window2 setHidden:![window2 isHidden]];
}

@end

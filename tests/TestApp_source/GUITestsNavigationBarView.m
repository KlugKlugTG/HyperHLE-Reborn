/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.
 */

// Manual test screen for navigation-bar style UI: a bare UINavigationBar with a
// UINavigationItem (title, left and right UIBarButtonItems) and a bottom
// UIToolbar. Every control logs what it received so the touchHLE log file shows
// which parts of the API actually reached the app.

#include "system_headers.h"
#include <stdio.h>

#include "GUITestsAppDelegate.h"
#include "GUITestsMainMenu.h"
#include "GUITestsNavigationBarView.h"

@interface UIBarButtonItem : NSObject
- (id)initWithTitle:(NSString *)title
             style:(NSInteger)style
            target:(id)target
            action:(SEL)action;
- (void)setEnabled:(BOOL)enabled;
@end

@interface UINavigationItem : NSObject
- (id)initWithTitle:(NSString *)title;
- (void)setTitle:(NSString *)title;
- (NSString *)title;
- (void)setLeftBarButtonItem:(UIBarButtonItem *)item;
- (UIBarButtonItem *)leftBarButtonItem;
- (void)setRightBarButtonItem:(UIBarButtonItem *)item;
- (UIBarButtonItem *)rightBarButtonItem;
- (void)setPrompt:(NSString *)prompt;
@end

@interface UINavigationBar : UIView
- (void)pushNavigationItem:(UINavigationItem *)item animated:(BOOL)animated;
- (void)popNavigationItemAnimated:(BOOL)animated;
- (UINavigationItem *)topItem;
- (UINavigationItem *)backItem;
- (void)setItems:(NSArray *)items animated:(BOOL)animated;
- (void)setBarStyle:(NSInteger)style;
- (void)setTintColor:(UIColor *)color;
- (void)setTranslucent:(BOOL)translucent;
@end

@interface UIToolbar : UIView
- (void)setItems:(NSArray *)items animated:(BOOL)animated;
- (NSArray *)items;
- (void)setTintColor:(UIColor *)color;
- (void)setBarStyle:(NSInteger)style;
@end

@implementation GUITestsNavigationBarView : UIView

UINavigationBar *navBar;
UINavigationItem *rootItem;
UIToolbar *toolbar;
UILabel *status;

- (void)log:(const char *)message {
  printf("NAVBAR-TEST: %s\n", message);
  fflush(stdout);
  if (status != NULL) {
    status.text = [NSString stringWithUTF8String:message];
  }
}

- (void)backPressed {
  [self log:"left bar button item pressed (action fired)"];
}

- (void)actionPressed {
  [self log:"right bar button item pressed (action fired)"];
  [rootItem setTitle:[NSString stringWithUTF8String:"Title changed!"]];
}

- (void)pushItem {
  UINavigationItem *pushed =
      [[[UINavigationItem alloc] initWithTitle:
            [NSString stringWithUTF8String:"Pushed item"]] autorelease];
  [pushed setPrompt:[NSString stringWithUTF8String:"prompt"]];
  [navBar pushNavigationItem:pushed animated:NO];
  [self log:"pushed a second navigation item"];
}

- (void)popItem {
  [navBar popNavigationItemAnimated:NO];
  [self log:"popped a navigation item"];
}

- (void)toolbarOne {
  [self log:"toolbar item one pressed"];
}

- (void)toolbarTwo {
  [self log:"toolbar item two pressed"];
}

- (void)goToMenu {
  [((GUITestsAppDelegate *)[[UIApplication sharedApplication] delegate])
      setMainView:[[[GUITestsMainMenu alloc] initWithFrame:[self frame]]
                    autorelease]];
}

- (instancetype)initWithFrame:(CGRect)frame {
  [super initWithFrame:frame];

  self.backgroundColor = [UIColor lightGrayColor];

  // UINavigationBar 44 points tall, like a real iPhone navigation bar.
  navBar = [[UINavigationBar alloc] initWithFrame:CGRectMake(0, 20, 320, 44)];
  [navBar setBarStyle:0];
  [self addSubview:navBar];

  rootItem = [[UINavigationItem alloc]
      initWithTitle:[NSString stringWithUTF8String:"Nav Bar Test"]];
  UIBarButtonItem *left =
      [[[UIBarButtonItem alloc] initWithTitle:[NSString stringWithUTF8String:"Back"]
                                        style:0
                                       target:self
                                       action:@selector(backPressed)] autorelease];
  UIBarButtonItem *right =
      [[[UIBarButtonItem alloc] initWithTitle:[NSString stringWithUTF8String:"Action"]
                                        style:0
                                       target:self
                                       action:@selector(actionPressed)] autorelease];
  [rootItem setLeftBarButtonItem:left];
  [rootItem setRightBarButtonItem:right];
  [navBar pushNavigationItem:rootItem animated:NO];

  UILabel *title = [[[UILabel alloc] initWithFrame:CGRectMake(0, 70, 320, 20)]
      autorelease];
  title.text = [NSString stringWithUTF8String:"UINavigationBar + UINavigationItem"];
  title.textAlignment = UITextAlignmentCenter;
  [self addSubview:title];

  status = [[UILabel alloc] initWithFrame:CGRectMake(10, 100, 300, 40)];
  status.text = [NSString stringWithUTF8String:"press a bar button item"];
  status.textAlignment = UITextAlignmentCenter;
  status.numberOfLines = 2;
  [self addSubview:status];

  UIButton *push = [UIButton buttonWithType:UIButtonTypeRoundedRect];
  [push setTitle:[NSString stringWithUTF8String:"Push item"]
        forState:UIControlStateNormal];
  [push setFrame:CGRectMake(20, 150, 130, 40)];
  [push addTarget:self
                action:@selector(pushItem)
      forControlEvents:UIControlEventTouchUpInside];
  [self addSubview:push];

  UIButton *pop = [UIButton buttonWithType:UIButtonTypeRoundedRect];
  [pop setTitle:[NSString stringWithUTF8String:"Pop item"]
       forState:UIControlStateNormal];
  [pop setFrame:CGRectMake(170, 150, 130, 40)];
  [pop addTarget:self
               action:@selector(popItem)
     forControlEvents:UIControlEventTouchUpInside];
  [self addSubview:pop];

  UIButton *menu = [UIButton buttonWithType:UIButtonTypeRoundedRect];
  [menu setTitle:[NSString stringWithUTF8String:"Menu"]
        forState:UIControlStateNormal];
  [menu setFrame:CGRectMake(20, 200, 130, 40)];
  [menu addTarget:self
                action:@selector(goToMenu)
      forControlEvents:UIControlEventTouchUpInside];
  [self addSubview:menu];

  // UIToolbar along the bottom, with the same three-item shape UIKit apps use.
  toolbar = [[UIToolbar alloc] initWithFrame:CGRectMake(0, 436, 320, 44)];
  UIBarButtonItem *one =
      [[[UIBarButtonItem alloc] initWithTitle:[NSString stringWithUTF8String:"One"]
                                        style:0
                                       target:self
                                       action:@selector(toolbarOne)] autorelease];
  UIBarButtonItem *two =
      [[[UIBarButtonItem alloc] initWithTitle:[NSString stringWithUTF8String:"Two"]
                                        style:0
                                       target:self
                                       action:@selector(toolbarTwo)] autorelease];
  UIBarButtonItem *space =
      [[[UIBarButtonItem alloc] initWithTitle:[NSString stringWithUTF8String:"Menu"]
                                        style:0
                                       target:self
                                       action:@selector(goToMenu)] autorelease];
  NSArray *items = [NSArray arrayWithObjects:one, two, space, NULL];
  [toolbar setItems:items animated:NO];
  [self addSubview:toolbar];

  return self;
}

- (void)dealloc {
  [navBar release];
  [rootItem release];
  [toolbar release];
  [status release];
  [super dealloc];
}

@end

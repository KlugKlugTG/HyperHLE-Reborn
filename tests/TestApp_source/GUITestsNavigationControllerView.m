/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.
 */

// Manual test screen for UIViewController + UINavigationController: a root view
// controller with a navigation item, pushing a second controller with
// -pushViewController:animated: and popping it again. The navigation bar the
// controller manages is what makes this interesting: it has to show the pushed
// controller's title and a back button that pops.

#include "system_headers.h"
#include <stdio.h>

#include "GUITestsAppDelegate.h"
#include "GUITestsMainMenu.h"
#include "GUITestsNavigationControllerView.h"

@interface UIViewController : NSObject
- (id)initWithNibName:(NSString *)name bundle:(id)bundle;
- (UIView *)view;
- (void)setView:(UIView *)view;
- (void)setTitle:(NSString *)title;
- (NSString *)title;
- (void)viewDidLoad;
- (void)viewDidAppear:(BOOL)animated;
- (void)setNavigationItem:(id)item;
- (id)navigationItem;
@end

@interface UINavigationController : UIViewController
- (id)initWithRootViewController:(UIViewController *)root;
- (void)pushViewController:(UIViewController *)controller animated:(BOOL)animated;
- (UIViewController *)popViewControllerAnimated:(BOOL)animated;
- (NSArray *)viewControllers;
- (void)setNavigationBarHidden:(BOOL)hidden animated:(BOOL)animated;
- (void)setToolbarHidden:(BOOL)hidden animated:(BOOL)animated;
@end

@interface UIBarButtonItem : NSObject
- (id)initWithTitle:(NSString *)title
             style:(NSInteger)style
            target:(id)target
            action:(SEL)action;
- (id)initWithBarButtonSystemItem:(NSInteger)item
                          target:(id)target
                          action:(SEL)action;
@end

@interface UINavigationItem : NSObject
- (id)initWithTitle:(NSString *)title;
- (void)setRightBarButtonItem:(UIBarButtonItem *)item;
@end

// Root screen: plain UIView with a push button.
@interface GUITestsNavRootController : UIViewController
@end

// The root controller is pushed by the screen below; keep a module-level
// pointer so the bar button action can reach it without a delegate protocol.
static id gNavigationControllerView = NULL;

@implementation GUITestsNavRootController

- (void)loadView {
  UIView *view = [[[UIView alloc] initWithFrame:CGRectMake(0, 0, 320, 416)]
      autorelease];
  view.backgroundColor = [UIColor whiteColor];
  [self setView:view];
}

- (void)viewDidLoad {
  printf("NAVCONTROLLER-TEST: root viewDidLoad\n");
  fflush(stdout);
  [self setTitle:[NSString stringWithUTF8String:"Root"]];

  UILabel *label = [[[UILabel alloc] initWithFrame:CGRectMake(10, 10, 300, 40)]
      autorelease];
  label.text = [NSString stringWithUTF8String:"root view controller"];
  label.textAlignment = UITextAlignmentCenter;
  [[self view] addSubview:label];

  UIBarButtonItem *push = [[[UIBarButtonItem alloc]
      initWithTitle:[NSString stringWithUTF8String:"Push"]
              style:0
             target:self
             action:@selector(pushNext)] autorelease];
  [[self navigationItem] setRightBarButtonItem:push];
}

- (void)pushNext {
  printf("NAVCONTROLLER-TEST: push button pressed\n");
  fflush(stdout);
  if (gNavigationControllerView != NULL) {
    [gNavigationControllerView performSelector:@selector(pushNextController)];
  }
}

@end

@implementation GUITestsNavigationControllerView : UIView

UINavigationController *navController;
UILabel *status;

- (instancetype)initWithFrame:(CGRect)frame {
  [super initWithFrame:frame];

  self.backgroundColor = [UIColor lightGrayColor];
  gNavigationControllerView = self;

  GUITestsNavRootController *root =
      [[[GUITestsNavRootController alloc] initWithNibName:NULL bundle:NULL]
          autorelease];
  navController = [[UINavigationController alloc] initWithRootViewController:root];

  status = [[UILabel alloc] initWithFrame:CGRectMake(10, 440, 300, 30)];
  status.text = [NSString stringWithUTF8String:"UINavigationController"];
  status.textAlignment = UITextAlignmentCenter;
  [self addSubview:status];

  UIView *navView = [navController view];
  if (navView != NULL) {
    [navView setFrame:CGRectMake(0, 0, 320, 436)];
    [self addSubview:navView];
    printf("NAVCONTROLLER-TEST: navigation controller view added\n");
  } else {
    printf("NAVCONTROLLER-TEST: navigation controller has no view!\n");
  }
  fflush(stdout);

  return self;
}

- (void)pushNextController {
  GUITestsNavRootController *next =
      [[[GUITestsNavRootController alloc] initWithNibName:NULL bundle:NULL]
          autorelease];
  [navController pushViewController:next animated:NO];
  printf("NAVCONTROLLER-TEST: pushed a controller; viewControllers count=%d\n",
         (int)[[navController viewControllers] count]);
  fflush(stdout);
}

- (void)dealloc {
  [navController release];
  [status release];
  [super dealloc];
}

@end

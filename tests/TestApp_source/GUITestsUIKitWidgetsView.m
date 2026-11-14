/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.
 */

// Manual test screen for the standard UIKit controls: segmented control,
// switch, slider, progress view, page control, activity indicator, text field,
// alert view, action sheet and a table view. Interactions are printed to stdout
// (which touchHLE writes into its log file) so it is obvious which callbacks
// reached the guest.

#include "system_headers.h"
#include <stdio.h>

#include "GUITestsAppDelegate.h"
#include "GUITestsMainMenu.h"
#include "GUITestsUIKitWidgetsView.h"

// system_headers.h only declares UIControlEventTouchUpInside; the value-changed
// event is what these controls send when the user operates them.
enum {
  UIControlEventValueChanged = 1 << 12,
};

@interface UISegmentedControl : UIControl
- (id)initWithItems:(NSArray *)items;
- (void)setSelectedSegmentIndex:(NSInteger)index;
- (NSInteger)selectedSegmentIndex;
@end

@interface UISwitch : UIControl
- (void)setOn:(BOOL)on animated:(BOOL)animated;
- (BOOL)isOn;
@end

@interface UISlider : UIControl
- (void)setValue:(CGFloat)value;
- (CGFloat)value;
@end

@interface UIProgressView : UIView
- (void)setProgress:(CGFloat)progress animated:(BOOL)animated;
@end

@interface UIPageControl : UIControl
- (void)setNumberOfPages:(NSInteger)pages;
- (void)setCurrentPage:(NSInteger)page;
@end

@interface UIActivityIndicatorView : UIView
- (id)initWithActivityIndicatorStyle:(NSInteger)style;
- (void)startAnimating;
- (void)stopAnimating;
- (BOOL)isAnimating;
@end

@interface UITextField : UIControl
- (void)setPlaceholder:(NSString *)placeholder;
- (NSString *)text;
- (void)setText:(NSString *)text;
- (void)setBorderStyle:(NSInteger)style;
- (void)setDelegate:(id)delegate;
@end

@interface UIAlertView : NSObject
- (id)initWithTitle:(NSString *)title
            message:(NSString *)message
           delegate:(id)delegate
  cancelButtonTitle:(NSString *)cancelTitle
  otherButtonTitles:(NSString *)otherTitle, ...;
- (void)show;
@end

@interface UIActionSheet : NSObject
- (id)initWithTitle:(NSString *)title
           delegate:(id)delegate
  cancelButtonTitle:(NSString *)cancelTitle
 destructiveButtonTitle:(NSString *)destructiveTitle
  otherButtonTitles:(NSString *)otherTitle, ...;
- (void)showInView:(UIView *)view;
@end

@interface UITableView : UIView
- (void)setDataSource:(id)dataSource;
- (void)setDelegate:(id)delegate;
- (void)reloadData;
- (void)setRowHeight:(CGFloat)height;
@end

@interface UITableViewCell : UIView
- (id)initWithStyle:(NSInteger)style
    reuseIdentifier:(NSString *)identifier;
- (void)setText:(NSString *)text;
- (NSString *)text;
@end

@protocol UITableViewDataSource
- (NSInteger)tableView:(UITableView *)tableView
    numberOfRowsInSection:(NSInteger)section;
- (UITableViewCell *)tableView:(UITableView *)tableView
         cellForRowAtIndexPath:(id)indexPath;
@end

@interface NSIndexPath : NSObject
+ (NSIndexPath *)indexPathForRow:(NSInteger)row inSection:(NSInteger)section;
- (NSInteger)row;
- (NSInteger)section;
@end

@interface GUITestsTableSource : NSObject <UITableViewDataSource>
@end

@implementation GUITestsTableSource

- (NSInteger)tableView:(UITableView *)tableView
    numberOfRowsInSection:(NSInteger)section {
  return 3;
}

- (UITableViewCell *)tableView:(UITableView *)tableView
         cellForRowAtIndexPath:(NSIndexPath *)indexPath {
  UITableViewCell *cell = [[[UITableViewCell alloc]
        initWithStyle:0
      reuseIdentifier:[NSString stringWithUTF8String:"cell"]] autorelease];
  NSString *text = [NSString stringWithFormat:[NSString stringWithUTF8String:"row %d"],
                                             (int)[indexPath row]];
  [cell setText:text];
  return cell;
}

@end

@implementation GUITestsUIKitWidgetsView

static UILabel *status;
UIActivityIndicatorView *spinner;
UIProgressView *progress;
UISlider *slider;

- (void)log:(NSString *)message {
  NSLog(@"WIDGETS-TEST: %@", message);
  if (status != NULL) {
    status.text = message;
  }
}

- (void)segmentChanged:(UISegmentedControl *)sender {
  [self log:[NSString stringWithFormat:
        [NSString stringWithUTF8String:"segmented control -> %d"],
        (int)[sender selectedSegmentIndex]]];
}

- (void)switchChanged:(UISwitch *)sender {
  [self log:[NSString stringWithFormat:
        [NSString stringWithUTF8String:"switch -> %d"], (int)[sender isOn]]];
}

- (void)sliderChanged:(UISlider *)sender {
  [progress setProgress:[sender value] animated:NO];
  [self log:[NSString stringWithFormat:
        [NSString stringWithUTF8String:"slider -> %.2f"], (double)[sender value]]];
}

- (void)pageChanged:(UIPageControl *)sender {
  [self log:@"page control value changed"];
}

- (void)showAlert {
  UIAlertView *alert = [[[UIAlertView alloc]
          initWithTitle:[NSString stringWithUTF8String:"Alert"]
                message:[NSString stringWithUTF8String:"two buttons"]
               delegate:self
      cancelButtonTitle:[NSString stringWithUTF8String:"Cancel"]
      otherButtonTitles:[NSString stringWithUTF8String:"OK"], NULL] autorelease];
  [alert show];
  [self log:@"alert view shown"];
}

- (void)alertView:(UIAlertView *)alertView
    clickedButtonAtIndex:(NSInteger)index {
  [self log:[NSString stringWithFormat:
        [NSString stringWithUTF8String:"alert button %d clicked"], (int)index]];
}

- (void)showActionSheet {
  UIActionSheet *sheet = [[[UIActionSheet alloc]
             initWithTitle:[NSString stringWithUTF8String:"Action sheet"]
                  delegate:self
         cancelButtonTitle:[NSString stringWithUTF8String:"Cancel"]
    destructiveButtonTitle:[NSString stringWithUTF8String:"Delete"]
         otherButtonTitles:[NSString stringWithUTF8String:"Other"], NULL]
      autorelease];
  [sheet showInView:self];
  [self log:@"action sheet shown"];
}

- (void)actionSheet:(UIActionSheet *)sheet
    clickedButtonAtIndex:(NSInteger)index {
  [self log:[NSString stringWithFormat:
        [NSString stringWithUTF8String:"action sheet button %d clicked"],
        (int)index]];
}

- (void)toggleSpinner {
  if ([spinner isAnimating]) {
    [spinner stopAnimating];
    [self log:@"activity indicator stopped"];
  } else {
    [spinner startAnimating];
    [self log:@"activity indicator started"];
  }
}

- (void)goToMenu {
  [((GUITestsAppDelegate *)[[UIApplication sharedApplication] delegate])
      setMainView:[[[GUITestsMainMenu alloc] initWithFrame:[self frame]]
                    autorelease]];
}

- (instancetype)initWithFrame:(CGRect)frame {
  [super initWithFrame:frame];

  self.backgroundColor = [UIColor whiteColor];

  UILabel *title = [[[UILabel alloc] initWithFrame:CGRectMake(0, 20, 320, 20)]
      autorelease];
  title.text = [NSString stringWithUTF8String:"UIKit controls"];
  title.textAlignment = UITextAlignmentCenter;
  [self addSubview:title];

  NSArray *segments = [NSArray arrayWithObjects:
      [NSString stringWithUTF8String:"One"],
      [NSString stringWithUTF8String:"Two"], NULL];
  UISegmentedControl *segmented =
      [[[UISegmentedControl alloc] initWithItems:segments] autorelease];
  [segmented setFrame:CGRectMake(10, 50, 300, 30)];
  [segmented setSelectedSegmentIndex:0];
  [segmented addTarget:self
                action:@selector(segmentChanged:)
      forControlEvents:UIControlEventValueChanged];
  [self addSubview:segmented];

  UISwitch *toggle = [[[UISwitch alloc] initWithFrame:CGRectMake(10, 90, 0, 0)]
      autorelease];
  [toggle setOn:YES animated:NO];
  [toggle addTarget:self
                action:@selector(switchChanged:)
      forControlEvents:UIControlEventValueChanged];
  [self addSubview:toggle];

  spinner = [[UIActivityIndicatorView alloc]
      initWithActivityIndicatorStyle:0];
  [spinner setFrame:CGRectMake(260, 88, 20, 20)];
  [self addSubview:spinner];
  [spinner startAnimating];

  UIButton *spinnerButton = [UIButton buttonWithType:UIButtonTypeRoundedRect];
  [spinnerButton setTitle:[NSString stringWithUTF8String:"Spinner"]
                 forState:UIControlStateNormal];
  [spinnerButton setFrame:CGRectMake(190, 84, 60, 30)];
  [spinnerButton addTarget:self
                    action:@selector(toggleSpinner)
          forControlEvents:UIControlEventTouchUpInside];
  [self addSubview:spinnerButton];

  slider = [[[UISlider alloc] initWithFrame:CGRectMake(10, 128, 300, 30)]
      autorelease];
  [slider setValue:0.5];
  [slider addTarget:self
                action:@selector(sliderChanged:)
      forControlEvents:UIControlEventValueChanged];
  [self addSubview:slider];

  progress = [[[UIProgressView alloc]
      initWithFrame:CGRectMake(10, 168, 300, 10)] autorelease];
  [progress setProgress:0.5 animated:NO];
  [self addSubview:progress];

  UIPageControl *pages =
      [[[UIPageControl alloc] initWithFrame:CGRectMake(10, 186, 300, 20)]
          autorelease];
  [pages setNumberOfPages:3];
  [pages setCurrentPage:1];
  [pages addTarget:self
                action:@selector(pageChanged:)
      forControlEvents:UIControlEventValueChanged];
  [self addSubview:pages];

  UITextField *field =
      [[[UITextField alloc] initWithFrame:CGRectMake(10, 214, 300, 30)]
          autorelease];
  [field setBorderStyle:3];
  [field setPlaceholder:[NSString stringWithUTF8String:"type here"]];
  [self addSubview:field];

  UIButton *alertButton = [UIButton buttonWithType:UIButtonTypeRoundedRect];
  [alertButton setTitle:[NSString stringWithUTF8String:"Alert"]
               forState:UIControlStateNormal];
  [alertButton setFrame:CGRectMake(10, 252, 140, 40)];
  [alertButton addTarget:self
                  action:@selector(showAlert)
        forControlEvents:UIControlEventTouchUpInside];
  [self addSubview:alertButton];

  UIButton *sheetButton = [UIButton buttonWithType:UIButtonTypeRoundedRect];
  [sheetButton setTitle:[NSString stringWithUTF8String:"Action sheet"]
               forState:UIControlStateNormal];
  [sheetButton setFrame:CGRectMake(170, 252, 140, 40)];
  [sheetButton addTarget:self
                  action:@selector(showActionSheet)
        forControlEvents:UIControlEventTouchUpInside];
  [self addSubview:sheetButton];

  GUITestsTableSource *source = [[[GUITestsTableSource alloc] init] autorelease];
  UITableView *table = [[[UITableView alloc]
      initWithFrame:CGRectMake(0, 300, 320, 130)] autorelease];
  [table setDataSource:source];
  [table setRowHeight:40];
  [self addSubview:table];

  status = [[UILabel alloc] initWithFrame:CGRectMake(0, 436, 320, 20)];
  status.text = [NSString stringWithUTF8String:"press a control"];
  status.textAlignment = UITextAlignmentCenter;
  [self addSubview:status];

  return self;
}

- (void)dealloc {
  [spinner release];
  [progress release];
  [slider release];
  [status release];
  [super dealloc];
}

@end

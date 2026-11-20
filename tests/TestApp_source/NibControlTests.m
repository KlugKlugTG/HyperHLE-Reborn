/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#import "system_headers.h"

@interface UITableViewCell : UIView
- (UIView *)contentView;
@end

@interface UIControl (NibControlTests)
- (BOOL)isEnabled;
- (void)touchesBegan:(NSSet *)touches withEvent:(UIEvent *)event;
- (void)touchesMoved:(NSSet *)touches withEvent:(UIEvent *)event;
- (void)touchesEnded:(NSSet *)touches withEvent:(UIEvent *)event;
@end

@interface UISwitch : UIControl
- (BOOL)isOn;
@end

@interface UISlider : UIControl
- (float)value;
- (float)minimumValue;
- (float)maximumValue;
@end

@interface UITextField : UIControl
- (NSString *)text;
- (void)setText:(NSString *)text;
- (void)insertText:(NSString *)text;
- (void)deleteBackward;
@end

@interface NSSet (NibControlTests)
+ (instancetype)setWithObject:(id)object;
@end

@interface NibControlTouch : NSObject {
@public
  UIView *cell;
  CGPoint point;
}
- (CGPoint)locationInView:(UIView *)view;
@end

@implementation NibControlTouch
- (CGPoint)locationInView:(UIView *)view {
  return [cell convertPoint:point toView:view];
}
@end

@interface NibControlTarget : NSObject {
@public
  int changes;
  id sender;
}
- (void)changed:(id)control;
@end

@implementation NibControlTarget
- (void)changed:(id)control {
  changes++;
  sender = control;
}
@end

// A custom nib cell's contentView is the superview of its archived controls.
// Replacing it with an empty overlay renders the controls but blocks touches.
// https://developer.apple.com/documentation/uikit/uitableviewcell/contentview
// https://developer.apple.com/documentation/uikit/uicontrol/isenabled
// https://developer.apple.com/documentation/uikit/uislider/maximumvalue
// https://developer.apple.com/documentation/uikit/uicontrol/event/editingchanged
// https://developer.apple.com/documentation/uikit/uikeyinput
int test_UITableViewCell_nib_controls(void) {
  NSAutoreleasePool *pool = [NSAutoreleasePool new];
  NSObject *owner = [NSObject new];
  NSArray *objects = [[NSBundle mainBundle] loadNibNamed:@"CellControls"
                                                   owner:owner
                                                 options:nil];
  NibControlTouch *touch = [NibControlTouch new];
  NibControlTarget *target = [NibControlTarget new];
  int result = 0;
  if ([objects count] != 1) {
    result = -1;
    goto cleanup;
  }
  UITableViewCell *cell = [objects objectAtIndex:0];
  UIView *content = [cell contentView];
  if ([[cell subviews] count] != 1 ||
      [[cell subviews] objectAtIndex:0] != content ||
      [[content subviews] count] != 3) {
    result = -2;
    goto cleanup;
  }
  UISwitch *toggle = (UISwitch *)[[content subviews] objectAtIndex:0];
  UISlider *slider = (UISlider *)[[content subviews] objectAtIndex:1];
  UITextField *field = (UITextField *)[[content subviews] objectAtIndex:2];
  if ([cell hitTest:CGPointMake(240, 18) withEvent:nil] != toggle ||
      [cell hitTest:CGPointMake(160, 60) withEvent:nil] != slider ||
      [cell hitTest:CGPointMake(160, 100) withEvent:nil] != field) {
    result = -3;
    goto cleanup;
  }
  if (![toggle isEnabled] || [toggle isOn] || [slider minimumValue] != 0.0f ||
      [slider maximumValue] != 1.0f || [slider value] != 0.5f) {
    result = -4;
    goto cleanup;
  }
  touch->cell = cell;
  touch->point = CGPointMake(240, 18);
  NSSet *touches = [NSSet setWithObject:touch];
  [toggle addTarget:target action:@selector(changed:) forControlEvents:1 << 12];
  [toggle touchesBegan:touches withEvent:nil];
  [toggle touchesEnded:touches withEvent:nil];
  if (![toggle isOn] || target->changes != 1 || target->sender != toggle) {
    result = -5;
    goto cleanup;
  }
  [slider addTarget:target action:@selector(changed:) forControlEvents:1 << 12];
  touch->point = CGPointMake(160, 60);
  [slider touchesBegan:touches withEvent:nil];
  touch->point = CGPointMake(299, 60);
  [slider touchesMoved:touches withEvent:nil];
  [slider touchesEnded:touches withEvent:nil];
  if ([slider value] < 0.99f || target->changes < 3 ||
      target->sender != slider) {
    result = -6;
    goto cleanup;
  }
  if (![[field text] isEqualToString:@"Guest"]) {
    result = -7;
    goto cleanup;
  }
  UIView *textLabel = [[field subviews] objectAtIndex:0];
  if ([textLabel frame].size.width != 280 ||
      [textLabel frame].size.height != 25) {
    result = -8;
    goto cleanup;
  }
  [field setFrame:CGRectMake(20, 90, 260, 25)];
  if ([textLabel frame].size.width != 260) {
    result = -9;
    goto cleanup;
  }
  target->changes = 0;
  [field addTarget:target action:@selector(changed:) forControlEvents:1 << 17];
  [field setText:@"Player"];
  if (target->changes != 0) {
    result = -10;
    goto cleanup;
  }
  [field insertText:@"X"];
  if (![[field text] isEqualToString:@"PlayerX"] || target->changes != 1 ||
      target->sender != field) {
    result = -11;
    goto cleanup;
  }
  [field deleteBackward];
  if (![[field text] isEqualToString:@"Player"] || target->changes != 2) {
    result = -12;
    goto cleanup;
  }
cleanup:
  [target release];
  [touch release];
  [owner release];
  [pool drain];
  return result;
}

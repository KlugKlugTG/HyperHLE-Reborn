/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#include "system_headers.h"

// The old SDK has no transformer declarations. The runtime API is the same
// as Apple's Foundation NSValueTransformer contract.
// https://developer.apple.com/documentation/foundation/valuetransformer
@interface NSValueTransformer : NSObject
+ (BOOL)allowsReverseTransformation;
+ (void)setValueTransformer:(id)transformer forName:(id)name;
+ (id)valueTransformerForName:(id)name;
- (id)transformedValue:(id)value;
- (id)reverseTransformedValue:(id)value;
@end

@interface NSValueTransformer (FixtureCategory)
+ (id)fixtureTransformer;
@end
@implementation NSValueTransformer (FixtureCategory)
+ (id)fixtureTransformer { return [[[self alloc] init] autorelease]; }
@end

static int transformer_deallocations;
@interface FixtureTransformer : NSValueTransformer
@end
@implementation FixtureTransformer
- (id)transformedValue:(id)value { return [value description]; }
- (void)dealloc {
  transformer_deallocations++;
  [super dealloc];
}
@end

int test_value_transformer_registry(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  int result = 0;
  id base = [NSValueTransformer fixtureTransformer];
  if (!base || [base transformedValue:@"identity"] != @"identity" ||
      ![NSValueTransformer allowsReverseTransformation])
    result = -1;
  transformer_deallocations = 0;
  id transformer = [[FixtureTransformer alloc] init];
  [NSValueTransformer setValueTransformer:transformer forName:@"fixture"];
  [transformer release];
  transformer = [NSValueTransformer valueTransformerForName:@"fixture"];
  if (!transformer || transformer_deallocations != 0 ||
      ![[transformer transformedValue:@"hello"] isEqual:@"hello"])
    result = -2;
  [NSValueTransformer setValueTransformer:nil forName:@"fixture"];
  if ([NSValueTransformer valueTransformerForName:@"fixture"] != nil)
    result = -3;
  id automatic = [NSValueTransformer valueTransformerForName:@"FixtureTransformer"];
  if (![automatic isKindOfClass:[FixtureTransformer class]])
    result = -4;
  [NSValueTransformer setValueTransformer:nil forName:@"FixtureTransformer"];
  [pool drain];
  if (transformer_deallocations != 2)
    result = -5;
  return result;
}

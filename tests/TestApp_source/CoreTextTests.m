/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#import "system_headers.h"
#include <math.h>
#include <string.h>
#include <stdio.h>

@interface NSNumber (CoreTextTestLiterals)
+ (NSNumber *)numberWithInt:(int)value;
@end

// The common-3.0 SDK predates Core Text. Keep Apple's actual signatures,
// especially double widths and size_t glyph counts, to exercise the ARM ABI.
// https://developer.apple.com/documentation/coretext/ctfontdrawglyphs(_:_:_:_:_:)
// https://developer.apple.com/documentation/coretext/ctlinegettypographicbounds(_:_:_:_:)
typedef CFTypeRef CTFontRef;
typedef CFTypeRef CTFontDescriptorRef;
typedef CFTypeRef CTTypesetterRef;
typedef CFTypeRef CTLineRef;
typedef CFTypeRef CTRunRef;
CTFontRef CTFontCreateWithGraphicsFont(CGFontRef, CGFloat,
                                       const CGAffineTransform *,
                                       CTFontDescriptorRef);
CTFontRef CTFontCreateWithName(CFStringRef, CGFloat, const CGAffineTransform *);
CTFontRef CTFontCreateWithFontDescriptor(CTFontDescriptorRef, CGFloat,
                                         const CGAffineTransform *);
CTFontRef CTFontCreateCopyWithAttributes(CTFontRef, CGFloat,
                                         const CGAffineTransform *,
                                         CTFontDescriptorRef);
CFStringRef CTFontCopyFamilyName(CTFontRef);
CFStringRef CTFontCopyFullName(CTFontRef);
CFStringRef CTFontCopyPostScriptName(CTFontRef);
CTFontDescriptorRef CTFontCopyFontDescriptor(CTFontRef);
CTFontDescriptorRef CTFontDescriptorCreateWithAttributes(CFDictionaryRef);
CTFontDescriptorRef
    CTFontDescriptorCreateCopyWithAttributes(CTFontDescriptorRef,
                                             CFDictionaryRef);
CFTypeRef CTFontDescriptorCopyAttribute(CTFontDescriptorRef, CFStringRef);
CFArrayRef CTFontDescriptorCreateMatchingFontDescriptors(CTFontDescriptorRef,
                                                         CFTypeRef);
CGFontRef CTFontCopyGraphicsFont(CTFontRef, CTFontDescriptorRef *);
CGFloat CTFontGetSize(CTFontRef);
CGAffineTransform CTFontGetMatrix(CTFontRef);
uint32_t CTFontGetSymbolicTraits(CTFontRef);
bool CTFontGetGlyphsForCharacters(CTFontRef, const UniChar *, CGGlyph *,
                                  CFIndex);
double CTFontGetAdvancesForGlyphs(CTFontRef, unsigned int, const CGGlyph *,
                                  CGSize *, CFIndex);
void CTFontDrawGlyphs(CTFontRef, const CGGlyph *, const CGPoint *, size_t,
                      CGContextRef);
bool CTFontManagerRegisterGraphicsFont(CGFontRef, CFTypeRef *);
bool CTFontManagerUnregisterGraphicsFont(CGFontRef, CFTypeRef *);
CTTypesetterRef CTTypesetterCreateWithAttributedString(CFTypeRef);
CTLineRef CTTypesetterCreateLine(CTTypesetterRef, CFRange);
CFIndex CTTypesetterSuggestLineBreak(CTTypesetterRef, CFIndex, double);
CFIndex CTTypesetterSuggestClusterBreak(CTTypesetterRef, CFIndex, double);
CFRange CTLineGetStringRange(CTLineRef);
CFIndex CTLineGetGlyphCount(CTLineRef);
CFArrayRef CTLineGetGlyphRuns(CTLineRef);
double CTLineGetTypographicBounds(CTLineRef, CGFloat *, CGFloat *, CGFloat *);
CGFloat CTLineGetOffsetForStringIndex(CTLineRef, CFIndex, CGFloat *);
CFIndex CTLineGetStringIndexForPosition(CTLineRef, CGPoint);
void CTRunGetGlyphs(CTRunRef, CFRange, CGGlyph *);
void CTRunGetPositions(CTRunRef, CFRange, CGPoint *);
void CTRunGetAdvances(CTRunRef, CFRange, CGSize *);
void CTRunGetStringIndices(CTRunRef, CFRange, CFIndex *);
double CTRunGetTypographicBounds(CTRunRef, CFRange, CGFloat *, CGFloat *,
                                 CGFloat *);
void CGContextSetTextMatrix(CGContextRef, CGAffineTransform);
CGAffineTransform CGContextGetTextMatrix(CGContextRef);

@interface NSData (CoreTextTestData)
+ (instancetype)dataWithContentsOfFile:(NSString *)path;
@end
@interface NSObject (CoreTextTestEquality)
- (BOOL)isEqual:(id)other;
@end
@interface NSArray (CoreTextTestFonts)
- (BOOL)containsObject:(id)object;
@end
@interface UIFont : NSObject
+ (id)fontWithName:(id)name size:(CGFloat)size;
+ (NSArray *)familyNames;
+ (NSArray *)fontNamesForFamilyName:(NSString *)name;
- (id)fontName;
- (id)familyName;
@end
@interface NSError : NSObject
- (NSInteger)code;
- (NSString *)domain;
@end
@interface NSAttributedString : NSObject
- (id)initWithString:(NSString *)string attributes:(NSDictionary *)attributes;
@end

static CGFontRef text_test_font_resource(NSString *name, NSString *extension) {
  NSString *path = [[NSBundle mainBundle] pathForResource:name
                                                   ofType:extension];
  NSData *data = [NSData dataWithContentsOfFile:path];
  CGDataProviderRef provider = CGDataProviderCreateWithCFData((CFDataRef)data);
  CGFontRef font = CGFontCreateWithDataProvider(provider);
  CFRelease(provider);
  return font;
}

static CGFontRef text_test_font(void) {
  return text_test_font_resource(@"LiberationMono-Regular", @"ttf");
}

// Registration must make a font discoverable without losing the original
// typeface when a descriptor is copied or the registration is later removed.
// https://developer.apple.com/documentation/coretext/ctfontmanagerregistergraphicsfont(_:_:)
// https://developer.apple.com/documentation/coretext/ctfontcopyfontdescriptor(_:)
int test_CTFont_registration_names_descriptors(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  CGFontRef graphics = text_test_font();
  CTFontRef font = CTFontCreateWithGraphicsFont(graphics, 24, NULL, NULL);
  int result = 0;
  CFStringRef family = CTFontCopyFamilyName(font);
  CFStringRef full = CTFontCopyFullName(font);
  CFStringRef ps = CTFontCopyPostScriptName(font);
  if (![(id)family isEqual:@"Liberation Mono"] ||
      ![(id)full isEqual:@"Liberation Mono"] ||
      ![(id)ps isEqual:@"LiberationMono"])
    result = -1;
  CFRelease(family);
  CFRelease(full);
  CFRelease(ps);
  CFTypeRef error = NULL;
  if (!CTFontManagerRegisterGraphicsFont(graphics, &error))
    result = -2;
  if (CTFontManagerRegisterGraphicsFont(graphics, &error) ||
      [(NSError *)error code] != 105 ||
      ![[(NSError *)error domain]
          isEqual:@"com.apple.CoreText.CTFontManagerErrorDomain"])
    result = -3;
  if (error)
    CFRelease(error);
  error = NULL;
  id ui = [UIFont fontWithName:@"LiberationMono" size:24];
  if (![[ui fontName] isEqual:@"LiberationMono"] ||
      ![[ui familyName] isEqual:@"Liberation Mono"] ||
      ![[UIFont familyNames] containsObject:@"Liberation Mono"] ||
      ![[UIFont fontNamesForFamilyName:@"Liberation Mono"]
          containsObject:@"LiberationMono"] ||
      [ui isEqual:[UIFont fontWithName:@"Helvetica" size:24]])
    result = -4;
  CTFontRef named =
      CTFontCreateWithName((CFStringRef) @"LiberationMono", 24, NULL);
  CTFontDescriptorRef descriptor = CTFontCopyFontDescriptor(named);
  CFArrayRef matched =
      CTFontDescriptorCreateMatchingFontDescriptors(descriptor, NULL);
  if ([(NSArray *)matched count] != 1)
    result = -10;
  CFRelease(matched);
  CTFontDescriptorRef changed = CTFontDescriptorCreateCopyWithAttributes(
      descriptor, (CFDictionaryRef)
                      @{@"NSFontSizeAttribute" : @18});
  if (!CTFontManagerUnregisterGraphicsFont(graphics, &error))
    result = -5;
  if ([[UIFont familyNames] containsObject:@"Liberation Mono"])
    result = -6;
  CTFontRef recreated = CTFontCreateWithFontDescriptor(changed, 0, NULL);
  ps = CTFontCopyPostScriptName(recreated);
  if (CTFontGetSize(recreated) != 18 || ![(id)ps isEqual:@"LiberationMono"] ||
      !(CTFontGetSymbolicTraits(recreated) & (1u << 10)))
    result = -7;
  UniChar character = 'W';
  CGGlyph before = 0, after = 0;
  if (!CTFontGetGlyphsForCharacters(font, &character, &before, 1) ||
      !CTFontGetGlyphsForCharacters(recreated, &character, &after, 1) ||
      before != after)
    result = -8;
  if (CTFontManagerUnregisterGraphicsFont(graphics, &error) ||
      [(NSError *)error code] != 201)
    result = -9;
  if (error)
    CFRelease(error);
  CFRelease(ps);
  CFRelease(recreated);
  CFRelease(changed);
  CFRelease(descriptor);
  CFRelease(named);
  CFRelease(font);
  CGFontRelease(graphics);
  [pool drain];
  return result;
}

// Arbitrary descriptor attributes survive font round trips. The two merged
// attributes have a different contract from ordinary replacement keys.
// https://developer.apple.com/documentation/coretext/ctfontdescriptorcreatecopywithattributes(_:_:)
int test_CTFont_descriptor_attribute_merging(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  NSDictionary *attributes = @{
    @"NSFontNameAttribute" : @"CourierNewPSMT",
    @"NSFontSizeAttribute" : @20,
    @"custom attribute" : @"preserved",
    @"NSCTFontVariationAttribute" : @{@"axis one" : @1, @"axis two" : @2},
    @"NSCTFontFeatureSettingsAttribute" : [NSArray arrayWithObjects:@{
      @"feature one" : @1
    },
                                                                    nil]
  };
  CTFontDescriptorRef original =
      CTFontDescriptorCreateWithAttributes((CFDictionaryRef)attributes);
  NSDictionary *overrides = @{
    @"NSFontSizeAttribute" : @24,
    @"NSCTFontVariationAttribute" : @{@"axis two" : @3},
    @"NSCTFontFeatureSettingsAttribute" : [NSArray arrayWithObjects:@{
      @"feature two" : @2
    },
                                                                    nil]
  };
  CTFontDescriptorRef merged = CTFontDescriptorCreateCopyWithAttributes(
      original, (CFDictionaryRef)overrides);
  CTFontRef font = CTFontCreateWithFontDescriptor(merged, 0, NULL);
  CTFontDescriptorRef roundtrip = CTFontCopyFontDescriptor(font);
  CFTypeRef custom = CTFontDescriptorCopyAttribute(
      roundtrip, (CFStringRef) @"custom attribute");
  CFTypeRef variations = CTFontDescriptorCopyAttribute(
      roundtrip, (CFStringRef) @"NSCTFontVariationAttribute");
  CFTypeRef features = CTFontDescriptorCopyAttribute(
      roundtrip, (CFStringRef) @"NSCTFontFeatureSettingsAttribute");
  int result = 0;
  if (CTFontGetSize(font) != 24 || ![(id)custom isEqual:@"preserved"] ||
      ![(NSDictionary *)variations isEqualToDictionary:@{
        @"axis one" : @1,
        @"axis two" : @3
      }] ||
      [(NSArray *)features count] != 2)
    result = -1;
  CTFontRef copied = CTFontCreateCopyWithAttributes(font, 0, NULL, NULL);
  if (CTFontGetSize(copied) != 24)
    result = -2;
  CFRelease(copied);
  CFRelease(custom);
  CFRelease(variations);
  CFRelease(features);
  CFRelease(roundtrip);
  CFRelease(font);
  CFRelease(merged);
  CFRelease(original);
  [pool drain];
  return result;
}

// Origins are in user space: a font matrix scales each glyph's outline,
// without scaling or replacing the caller's independent glyph positions.
int test_CTFont_draw_glyphs_bitmap(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  CGFontRef graphics = text_test_font();
  CGAffineTransform matrix = CGAffineTransformMakeScale(0.5, 1);
  CTFontRef font = CTFontCreateWithGraphicsFont(graphics, 24, &matrix, NULL);
  const UniChar characters[] = {'W', 'W'};
  CGGlyph glyphs[2] = {0};
  int result = 0;
  if (!CTFontGetGlyphsForCharacters(font, characters, glyphs, 2))
    result = -1;
  unsigned char pixels[96 * 64 * 4] = {0};
  CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
  CGContextRef context = CGBitmapContextCreate(pixels, 96, 64, 8, 96 * 4, space,
                                               kCGImageAlphaPremultipliedLast);
  CGContextSetRGBFillColor(context, 1, 0, 0, 1);
  const CGPoint positions[] = {{10, 10}, {60, 30}};
  CTFontDrawGlyphs(font, glyphs, positions, 2, context);
  unsigned left = 0, right = 0, outside = 0;
  for (unsigned y = 0; y < 64; y++) {
    for (unsigned x = 0; x < 96; x++) {
      unsigned char *pixel = &pixels[(y * 96 + x) * 4];
      if (!pixel[3])
        continue;
      if (pixel[0] == 0 || pixel[1] || pixel[2])
        result = -2;
      // Quartz glyph positions are in a Y-up user space. Bitmap storage
      // scans from the top row, so compare the original user-space bounds.
      // https://developer.apple.com/library/archive/documentation/GraphicsImaging/Conceptual/drawingwithquartz2d/dq_overview/dq_overview.html
      unsigned user_y = 63 - y;
      if (x >= 9 && x < 25 && user_y >= 9 && user_y < 40)
        left++;
      else if (x >= 59 && x < 75 && user_y >= 29 && user_y < 60)
        right++;
      else
        outside++;
    }
  }
  if (left < 20 || right < 20 || outside) {
    printf("CTFont glyph coverage: left=%u right=%u outside=%u\n", left, right, outside);
    result = -3;
  }
  if (!CGAffineTransformEqualToTransform(CGContextGetTextMatrix(context),
                                         matrix))
    result = -4;
  CTFontDescriptorRef descriptor = CTFontCopyFontDescriptor(font);
  CTFontRef copy = CTFontCreateWithFontDescriptor(descriptor, 0, NULL);
  if (!CGAffineTransformEqualToTransform(CTFontGetMatrix(copy), matrix))
    result = -5;
  CFRelease(copy);
  CFRelease(descriptor);
  CGContextRelease(context);
  CGColorSpaceRelease(space);
  CFRelease(font);
  CGFontRelease(graphics);
  [pool drain];
  return result;
}

// Font Awesome supplies CFF outlines and private-use symbol glyphs. Keep
// this test independent of any commercial game's font files or glyph IDs.
int test_CTFont_cff_symbol_bitmap(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  CGFontRef graphics = text_test_font_resource(@"FontAwesome", @"otf");
  CTFontRef font = CTFontCreateWithGraphicsFont(graphics, 24, NULL, NULL);
  int result = 0;
  CFStringRef name = CTFontCopyPostScriptName(font);
  if (![(id)name isEqual:@"FontAwesome"])
    result = -1;
  UniChar character = 0xf000;
  CGGlyph glyph = 0;
  if (!CTFontGetGlyphsForCharacters(font, &character, &glyph, 1) || glyph == 0)
    result = -2;
  unsigned char pixels[64 * 64 * 4] = {0};
  CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
  CGContextRef context = CGBitmapContextCreate(pixels, 64, 64, 8, 64 * 4, space,
                                               kCGImageAlphaPremultipliedLast);
  CGContextSetRGBFillColor(context, 1, 1, 1, 1);
  CGPoint position = {10, 20};
  CTFontDrawGlyphs(font, &glyph, &position, 1, context);
  unsigned covered = 0;
  for (unsigned pixel = 0; pixel < 64 * 64; pixel++)
    if (pixels[pixel * 4 + 3])
      covered++;
  if (covered < 30)
    result = -3;
  CFRelease(name);
  CGContextRelease(context);
  CGColorSpaceRelease(space);
  CFRelease(font);
  CGFontRelease(graphics);
  [pool drain];
  return result;
}

// Ranges and string indices use UTF-16; widths use the same points as glyph
// drawing. A zero-length typesetter range means the remaining source text.
// https://developer.apple.com/documentation/coretext/cttypesettercreateline(_:_:)
// https://developer.apple.com/documentation/coretext/ctrungetstringindices(_:_:_:)
int test_CTTypesetter_ranges_utf16_metrics(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  CTFontRef font =
      CTFontCreateWithName((CFStringRef) @"CourierNewPSMT", 20, NULL);
  NSDictionary *attrs = @{@"NSFont" : (id)font};
  NSAttributedString *string =
      [[NSAttributedString alloc] initWithString:@"prefix WXYZ"
                                      attributes:attrs];
  CTTypesetterRef typesetter = CTTypesetterCreateWithAttributedString(string);
  CTLineRef line = CTTypesetterCreateLine(typesetter, CFRangeMake(7, 2));
  int result = 0;
  CFRange range = CTLineGetStringRange(line);
  if (range.location != 7 || range.length != 2 ||
      CTLineGetGlyphCount(line) != 2)
    result = -1;
  CFArrayRef runs = CTLineGetGlyphRuns(line);
  CTRunRef run = (CTRunRef)[(NSArray *)runs objectAtIndex:0];
  const UniChar characters[] = {'W', 'X'};
  CGGlyph expected[2] = {0}, actual[2] = {0};
  CTFontGetGlyphsForCharacters(font, characters, expected, 2);
  CTRunGetGlyphs(run, CFRangeMake(0, 0), actual);
  if (memcmp(expected, actual, sizeof(expected)))
    result = -2;
  CFIndex indices[2] = {-1, -1};
  CTRunGetStringIndices(run, CFRangeMake(0, 0), indices);
  if (indices[0] != 7 || indices[1] != 8)
    result = -3;
  CGPoint positions[2] = {{-1, -1}, {-1, -1}};
  CGSize advances[2] = {{-1, -1}, {-1, -1}};
  CTRunGetPositions(run, CFRangeMake(0, 0), positions);
  CTRunGetAdvances(run, CFRangeMake(0, 0), advances);
  double expectedWidth = CTFontGetAdvancesForGlyphs(font, 0, expected, NULL, 2);
  double width = CTLineGetTypographicBounds(line, NULL, NULL, NULL);
  if (width < 20 || width > 30 || fabs(width - expectedWidth) > 0.01 ||
      fabs(advances[0].width + advances[1].width - width) > 0.01 ||
      positions[0].x != 0 || fabs(positions[1].x - advances[0].width) > 0.01)
    result = -4;
  actual[0] = actual[1] = 0xffff;
  CTRunGetGlyphs(run, CFRangeMake(1, 0), actual);
  if (actual[0] != expected[1] || actual[1] != 0xffff ||
      fabs(CTRunGetTypographicBounds(run, CFRangeMake(1, 0), NULL, NULL, NULL) -
           advances[1].width) > 0.01)
    result = -5;
  CGFloat secondary = -1;
  CGFloat offset = CTLineGetOffsetForStringIndex(line, 8, &secondary);
  if (fabs(offset - advances[0].width) > 0.01 || secondary != offset ||
      CTLineGetStringIndexForPosition(line, CGPointMake(0, 0)) != 7)
    result = -6;
  CFRelease(line);
  line = CTTypesetterCreateLine(typesetter, CFRangeMake(7, 0));
  range = CTLineGetStringRange(line);
  if (range.location != 7 || range.length != 4)
    result = -7;
  CFRelease(line);
  CFRelease(typesetter);
  [string release];
  string = [[NSAttributedString alloc] initWithString:@"x\U0001f600yz"
                                           attributes:attrs];
  typesetter = CTTypesetterCreateWithAttributedString(string);
  if (CTTypesetterSuggestLineBreak(typesetter, 1, 1000) != 4 ||
      CTTypesetterSuggestClusterBreak(typesetter, 1, 1000) != 4)
    result = -8;
  line = CTTypesetterCreateLine(typesetter, CFRangeMake(1, 0));
  runs = CTLineGetGlyphRuns(line);
  run = (CTRunRef)[(NSArray *)runs objectAtIndex:0];
  CFIndex supplementaryIndices[3] = {-1, -1, -1};
  CTRunGetStringIndices(run, CFRangeMake(0, 0), supplementaryIndices);
  if (CTLineGetGlyphCount(line) != 3 || supplementaryIndices[0] != 1 ||
      supplementaryIndices[1] != 3 || supplementaryIndices[2] != 4)
    result = -9;
  CFRelease(line);
  CFRelease(typesetter);
  [string release];
  CFRelease(font);
  [pool drain];
  return result;
}

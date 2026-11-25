/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#include <errno.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

extern int digittoint(int);
extern char *strnstr(const char *, const char *, size_t);

int test_runtime_import_helpers(void) {
  const char buffer[] = {'a', 'b', 'c', 'd'};
  if (digittoint('0') != 0 || digittoint('9') != 9 ||
      digittoint('F') != 15 || digittoint('a') != 10 ||
      digittoint('g') != 0 || digittoint(-1) != 0)
    return -1;
  if (strnstr(buffer, "bc", 4) != buffer + 1 ||
      strnstr(buffer, "cd", 3) != NULL ||
      strnstr(buffer, "", 0) != buffer ||
      strnstr("ab\0cd", "cd", 5) != NULL)
    return -2;
  if (atoll(" -9223372036854775808") != LLONG_MIN ||
      atoll("9223372036854775807tail") != LLONG_MAX ||
      atoll("+4294967296") != 4294967296LL || atoll("nondigit") != 0)
    return -3;
  errno = 0;
  if (strtoll("9223372036854775808", NULL, 10) != LLONG_MAX || errno != ERANGE)
    return -4;
  return 0;
}

struct HeapCallbacks {
  int version;
  const void *(*retain)(void *, const void *);
  void (*release)(void *, const void *);
  void *description;
  int (*compare)(const void *, const void *, void *);
};
extern void *CFBinaryHeapCreate(void *, int, const struct HeapCallbacks *, void *);
extern void CFBinaryHeapAddValue(void *, const void *);
extern int CFBinaryHeapGetCount(void *);
extern const void *CFBinaryHeapGetMinimum(void *);
extern void CFBinaryHeapGetValues(void *, const void **);
extern void CFBinaryHeapRemoveMinimumValue(void *);
extern int CFBinaryHeapContainsValue(void *, const void *);
extern void CFRelease(void *);

static int heap_retains, heap_releases, heap_compares;
static const void *retain_heap_value(void *allocator, const void *value) {
  heap_retains++;
  return value;
}
static void release_heap_value(void *allocator, const void *value) {
  heap_releases++;
}
static int compare_heap_values(const void *a, const void *b, void *context) {
  heap_compares++;
  return (intptr_t)a > (intptr_t)b ? -1 : (intptr_t)a < (intptr_t)b;
}

int test_binary_heap_callbacks(void) {
  struct HeapCallbacks callbacks = {0, retain_heap_value, release_heap_value,
                                   NULL, compare_heap_values};
  heap_retains = heap_releases = heap_compares = 0;
  void *heap = CFBinaryHeapCreate(NULL, 0, &callbacks, NULL);
  CFBinaryHeapAddValue(heap, (void *)3);
  CFBinaryHeapAddValue(heap, (void *)9);
  CFBinaryHeapAddValue(heap, (void *)1);
  CFBinaryHeapAddValue(heap, (void *)7);
  int result = 0;
  if (CFBinaryHeapGetMinimum(heap) != (void *)9 ||
      CFBinaryHeapGetCount(heap) != 4 || heap_retains != 4 || !heap_compares)
    result = -1;
  const void *values[4];
  CFBinaryHeapGetValues(heap, values);
  if (values[0] != (void *)9 || values[1] != (void *)7 ||
      values[2] != (void *)3 || values[3] != (void *)1)
    result = -2;
  CFBinaryHeapRemoveMinimumValue(heap);
  if (CFBinaryHeapGetMinimum(heap) != (void *)7 || heap_releases != 1 ||
      CFBinaryHeapContainsValue(heap, (void *)9) ||
      !CFBinaryHeapContainsValue(heap, (void *)3))
    result = -3;
  CFRelease(heap);
  if (heap_releases != 4)
    result = -4;
  return result;
}

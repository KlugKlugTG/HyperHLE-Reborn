/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#include "system_headers.h"
#include <pthread.h>
#include <semaphore.h>
#include <unistd.h>

@interface NSDate : NSObject
+ (id)dateWithTimeIntervalSinceNow:(double)seconds;
@end
@interface NSLock : NSObject
- (void)lock;
- (void)unlock;
- (BOOL)tryLock;
- (BOOL)lockBeforeDate:(NSDate *)date;
@end
@interface NSRecursiveLock : NSObject
@end

struct LockProbe {
  NSLock *lock;
  sem_t ready;
  sem_t release;
  int acquired;
};
static void *hold_lock(void *context) {
  struct LockProbe *probe = context;
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  [probe->lock lock];
  sem_post(&probe->ready);
  sem_wait(&probe->release);
  [probe->lock unlock];
  [pool drain];
  return NULL;
}
static void *take_lock_before_deadline(void *context) {
  struct LockProbe *probe = context;
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  sem_post(&probe->ready);
  probe->acquired = [probe->lock lockBeforeDate:
      [NSDate dateWithTimeIntervalSinceNow:1.0]];
  if (probe->acquired)
    [probe->lock unlock];
  [pool drain];
  return NULL;
}

// Foundation first tries to acquire, even for a past date. Recursive tryLock
// increments ownership; timed contention must yield so the owner can unlock.
// https://developer.apple.com/documentation/foundation/nsrecursivelock
// https://github.com/swiftlang/swift-corelibs-foundation/blob/main/Sources/Foundation/NSLock.swift
int test_NSLock_deadlines(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  int result = 0;
  for (int recursive = 0; recursive < 2; recursive++) {
    NSLock *lock = recursive ? (NSLock *)[[NSRecursiveLock alloc] init]
                            : [[NSLock alloc] init];
    [lock lock];
    if ([lock tryLock] != recursive)
      result = -1;
    if (recursive)
      [lock unlock];
    if ([lock lockBeforeDate:[NSDate dateWithTimeIntervalSinceNow:-1]] !=
        recursive)
      result = -2;
    if (recursive)
      [lock unlock];
    // A normal lock held by this thread must time out without claiming a
    // second acquisition or consuming the original lock's ownership.
    if (!recursive && [lock lockBeforeDate:
        [NSDate dateWithTimeIntervalSinceNow:0.01]])
      result = -3;
    [lock unlock];
    if (![lock lockBeforeDate:[NSDate dateWithTimeIntervalSinceNow:-1]])
      result = -4;
    [lock unlock];

    struct LockProbe probe = { .lock = lock };
    sem_init(&probe.ready, 0, 0);
    sem_init(&probe.release, 0, 0);
    pthread_t worker;
    pthread_create(&worker, NULL, hold_lock, &probe);
    sem_wait(&probe.ready);
    if ([lock tryLock] || [lock lockBeforeDate:
        [NSDate dateWithTimeIntervalSinceNow:0.01]])
      result = -5;
    sem_post(&probe.release);
    pthread_join(worker, NULL);

    [lock lock];
    pthread_create(&worker, NULL, take_lock_before_deadline, &probe);
    sem_wait(&probe.ready);
    usleep(10000);
    [lock unlock];
    pthread_join(worker, NULL);
    if (!probe.acquired || ![lock tryLock])
      result = -6;
    else
      [lock unlock];
    sem_destroy(&probe.ready);
    sem_destroy(&probe.release);
    [lock release];
  }
  [pool drain];
  return result;
}

/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include "system_headers.h"
#include <fcntl.h>
#include <pthread.h>
#include <semaphore.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

// The old SDK lacks dispatch headers. Use the real main-queue symbol, just
// as dispatch_get_main_queue() does in Apple's headers and MCPE's binary.
typedef struct dispatch_queue_s *dispatch_queue_t;
typedef void (^dispatch_block_t)(void);
typedef void (*dispatch_function_t)(void *);
extern struct dispatch_queue_s _dispatch_main_q;
extern dispatch_queue_t dispatch_get_global_queue(long, unsigned long);
extern dispatch_queue_t dispatch_get_current_queue(void);
extern dispatch_queue_t dispatch_queue_create(const char *, void *);
extern void dispatch_async(dispatch_queue_t, dispatch_block_t);
extern void dispatch_sync(dispatch_queue_t, dispatch_block_t);
extern void dispatch_async_f(dispatch_queue_t, void *, dispatch_function_t);
extern void dispatch_sync_f(dispatch_queue_t, void *, dispatch_function_t);
extern void dispatch_queue_set_specific(dispatch_queue_t, const void *, void *,
                                        dispatch_function_t);
extern void *dispatch_get_specific(const void *);
extern void dispatch_release(void *);
typedef void *dispatch_group_t;
extern struct dispatch_queue_s _dispatch_queue_attr_concurrent;
extern void dispatch_after_f(uint64_t, dispatch_queue_t, void *, dispatch_function_t);
extern uint64_t dispatch_time(uint64_t, int64_t);
extern dispatch_group_t dispatch_group_create(void);
extern void dispatch_group_enter(dispatch_group_t);
extern void dispatch_group_leave(dispatch_group_t);
extern long dispatch_group_wait(dispatch_group_t, uint64_t);
extern void dispatch_group_async_f(dispatch_group_t, dispatch_queue_t, void *, dispatch_function_t);
extern void dispatch_group_notify_f(dispatch_group_t, dispatch_queue_t, void *, dispatch_function_t);
extern void dispatch_barrier_async_f(dispatch_queue_t, void *, dispatch_function_t);
extern void dispatch_barrier_sync_f(dispatch_queue_t, void *, dispatch_function_t);

@interface NSRunLoop : NSObject
+ (instancetype)mainRunLoop;
- (BOOL)runMode:(id)mode beforeDate:(id)limitDate;
@end

@interface NSDate : NSObject
+ (instancetype)dateWithTimeIntervalSinceNow:(double)seconds;
@end

extern id NSDefaultRunLoopMode;

static void pump_dispatch_main_loop(void) {
  [[NSRunLoop mainRunLoop] runMode:NSDefaultRunLoopMode
                        beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.05]];
}

struct MainQueueProbe {
  pthread_t main_thread;
  int calls;
  int order[4];
  int wrong_thread;
  int wrong_queue;
  int capture;
  int byref;
};

static void record_main_queue_call(struct MainQueueProbe *probe, int value) {
  if (!pthread_equal(pthread_self(), probe->main_thread))
    probe->wrong_thread = 1;
  if (dispatch_get_current_queue() != &_dispatch_main_q)
    probe->wrong_queue = 1;
  if (probe->calls < 4)
    probe->order[probe->calls] = value;
  probe->calls++;
}

static int dispatch_capture_deallocs;

@interface DispatchMainQueueProbe : NSObject
- (int)marker;
@end

@implementation DispatchMainQueueProbe
- (int)marker {
  return 40;
}
- (void)dealloc {
  dispatch_capture_deallocs++;
  [super dealloc];
}
@end

static void main_queue_function(void *context) {
  record_main_queue_call(context, 2);
}

static void *submit_main_queue_async(void *context) {
  struct MainQueueProbe *probe = context;
  int captured = 2;
  __block int shared = 1;
  DispatchMainQueueProbe *object = [[DispatchMainQueueProbe alloc] init];
  dispatch_async(&_dispatch_main_q, ^{
    record_main_queue_call(probe, 1);
    probe->capture = [object marker] + captured + shared;
    shared++;
  });
  dispatch_async_f(&_dispatch_main_q, probe, main_queue_function);
  dispatch_async(&_dispatch_main_q, ^{
    record_main_queue_call(probe, 3);
    probe->byref = shared;
  });
  [object release];
  return NULL;
}

// Main-queue work is serial, runs on the main thread, and async submission
// copies its block. Captures must survive the worker and its stack returning.
// https://developer.apple.com/documentation/dispatch/1453057-dispatch_async
// https://developer.apple.com/library/archive/documentation/General/Conceptual/ConcurrencyProgrammingGuide/OperationQueues/OperationQueues.html
int test_dispatch_main_queue_async(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  struct MainQueueProbe probe = {0};
  probe.main_thread = pthread_self();
  dispatch_capture_deallocs = 0;
  pthread_t worker;
  if (pthread_create(&worker, NULL, submit_main_queue_async, &probe) != 0) {
    [pool drain];
    return -1;
  }
  pthread_join(worker, NULL);
  int result = 0;
  if (probe.calls != 0 || dispatch_capture_deallocs != 0)
    result = -2;
  for (int i = 0; i < 100 && probe.calls < 3; i++)
    pump_dispatch_main_loop();
  if (result == 0 &&
      (probe.calls != 3 || probe.wrong_thread || probe.wrong_queue ||
       probe.order[0] != 1 || probe.order[1] != 2 || probe.order[2] != 3))
    result = -3;
  if (result == 0 && (probe.capture != 43 || probe.byref != 2 ||
                      dispatch_capture_deallocs != 1))
    result = -4;
  [pool drain];
  return result;
}

// A nested run loop cannot reenter a serial main-queue callback.
int test_dispatch_main_queue_serial(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  struct MainQueueProbe probe = {0};
  struct MainQueueProbe *state = &probe;
  probe.main_thread = pthread_self();
  dispatch_async(&_dispatch_main_q, ^{
    record_main_queue_call(state, 1);
    dispatch_async(&_dispatch_main_q, ^{
      record_main_queue_call(state, 3);
    });
    pump_dispatch_main_loop();
    record_main_queue_call(state, 2);
  });
  int result = probe.calls == 0 ? 0 : -1;
  for (int i = 0; i < 100 && probe.calls < 3; i++)
    pump_dispatch_main_loop();
  if (result == 0 &&
      (probe.calls != 3 || probe.wrong_thread || probe.wrong_queue ||
       probe.order[0] != 1 || probe.order[1] != 2 || probe.order[2] != 3))
    result = -2;
  [pool drain];
  return result;
}

struct MainQueueSyncProbe {
  struct MainQueueProbe callbacks;
  pthread_mutex_t mutex;
  pthread_cond_t condition;
  int ready;
  int finished;
  int callback_completed;
  int completion_observed;
};

static void main_queue_sync_function(void *context) {
  struct MainQueueSyncProbe *probe = context;
  record_main_queue_call(&probe->callbacks, 2);
  usleep(1000);
  probe->callback_completed = 2;
}

static void *submit_main_queue_sync(void *context) {
  struct MainQueueSyncProbe *probe = context;
  pthread_mutex_lock(&probe->mutex);
  probe->ready = 1;
  pthread_cond_signal(&probe->condition);
  pthread_mutex_unlock(&probe->mutex);
  dispatch_sync(&_dispatch_main_q, ^{
    record_main_queue_call(&probe->callbacks, 1);
    usleep(1000);
    probe->callback_completed = 1;
  });
  if (probe->callback_completed == 1)
    probe->completion_observed++;
  dispatch_sync_f(&_dispatch_main_q, probe, main_queue_sync_function);
  if (probe->callback_completed == 2)
    probe->completion_observed++;
  pthread_mutex_lock(&probe->mutex);
  probe->finished = 1;
  pthread_mutex_unlock(&probe->mutex);
  return NULL;
}

// dispatch_sync waits for the callback to finish, including when it yields.
// The main thread must service its run loop to let the worker continue.
int test_dispatch_main_queue_sync(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  struct MainQueueSyncProbe probe = {0};
  probe.callbacks.main_thread = pthread_self();
  pthread_mutex_init(&probe.mutex, NULL);
  pthread_cond_init(&probe.condition, NULL);
  pthread_t worker;
  if (pthread_create(&worker, NULL, submit_main_queue_sync, &probe) != 0) {
    pthread_cond_destroy(&probe.condition);
    pthread_mutex_destroy(&probe.mutex);
    [pool drain];
    return -1;
  }
  pthread_mutex_lock(&probe.mutex);
  while (!probe.ready)
    pthread_cond_wait(&probe.condition, &probe.mutex);
  pthread_mutex_unlock(&probe.mutex);
  usleep(1000);
  pthread_mutex_lock(&probe.mutex);
  int result = probe.finished ? -2 : 0;
  pthread_mutex_unlock(&probe.mutex);
  int finished = 0;
  for (int i = 0; i < 100 && !finished; i++) {
    pump_dispatch_main_loop();
    pthread_mutex_lock(&probe.mutex);
    finished = probe.finished;
    pthread_mutex_unlock(&probe.mutex);
  }
  // Do not hang in pthread_join if a broken dispatcher strands its waiter.
  if (!finished)
    abort();
  pthread_join(worker, NULL);
  if (result == 0 &&
      (probe.completion_observed != 2 || probe.callbacks.calls != 2 ||
       probe.callbacks.wrong_thread || probe.callbacks.wrong_queue ||
       probe.callbacks.order[0] != 1 || probe.callbacks.order[1] != 2))
    result = -3;
  pthread_cond_destroy(&probe.condition);
  pthread_mutex_destroy(&probe.mutex);
  [pool drain];
  return result;
}

struct DispatchThreadProbe {
  dispatch_queue_t queue;
  sem_t *ready;
  sem_t *resume;
  int queue_marker;
  int error;
};

static char dispatch_thread_specific_key;

static void dispatch_thread_callback(void *context) {
  struct DispatchThreadProbe *probe = context;
  sem_post(probe->ready);
  sem_wait(probe->resume);
  if (dispatch_get_current_queue() != probe->queue ||
      dispatch_get_specific(&dispatch_thread_specific_key) !=
          &probe->queue_marker)
    probe->error = -3;
}

static void *dispatch_thread_worker(void *context) {
  struct DispatchThreadProbe *probe = context;
  dispatch_sync_f(probe->queue, probe, dispatch_thread_callback);
  if (dispatch_get_current_queue() != dispatch_get_global_queue(0, 0) ||
      dispatch_get_specific(&dispatch_thread_specific_key) != NULL)
    probe->error = -4;
  return NULL;
}

// A callback blocked on one thread must not change another thread's queue.
// https://developer.apple.com/documentation/dispatch/dispatch_get_current_queue()
// https://developer.apple.com/documentation/dispatch/1453099-dispatch_get_specific
int test_dispatch_queue_thread_identity(void) {
  struct DispatchThreadProbe probe = {0};
  probe.queue = dispatch_queue_create("TestApp.dispatch-thread-identity", NULL);
  probe.ready = sem_open("/dispatch_queue_ready", O_CREAT | O_EXCL, 0600, 0);
  probe.resume = sem_open("/dispatch_queue_resume", O_CREAT | O_EXCL, 0600, 0);
  if (probe.ready == SEM_FAILED || probe.resume == SEM_FAILED)
    abort();
  int main_marker = 0;
  dispatch_queue_set_specific(&_dispatch_main_q, &dispatch_thread_specific_key,
                              &main_marker, NULL);
  dispatch_queue_set_specific(probe.queue, &dispatch_thread_specific_key,
                              &probe.queue_marker, NULL);
  pthread_t worker;
  if (pthread_create(&worker, NULL, dispatch_thread_worker, &probe) != 0)
    abort();
  sem_wait(probe.ready);
  int result = 0;
  if (dispatch_get_current_queue() != &_dispatch_main_q ||
      dispatch_get_specific(&dispatch_thread_specific_key) != &main_marker)
    result = -2;
  sem_post(probe.resume);
  pthread_join(worker, NULL);
  if (result == 0)
    result = probe.error;
  dispatch_queue_set_specific(&_dispatch_main_q, &dispatch_thread_specific_key,
                              NULL, NULL);
  dispatch_queue_set_specific(probe.queue, &dispatch_thread_specific_key, NULL,
                              NULL);
  sem_close(probe.ready);
  sem_close(probe.resume);
  sem_unlink("/dispatch_queue_ready");
  sem_unlink("/dispatch_queue_resume");
  dispatch_release(probe.queue);
  return result;
}

struct BackgroundQueueProbe {
  pthread_t caller;
  dispatch_queue_t queue;
  sem_t *ready;
  sem_t *resume;
  sem_t *done;
  int error;
  int capture;
  int calls;
  int order[3];
};

static void submit_background_block(struct BackgroundQueueProbe *probe) {
  int captured = 42;
  dispatch_async(probe->queue, ^{
    if (pthread_equal(pthread_self(), probe->caller)) {
      probe->error = -1;
      sem_post(probe->done);
      return;
    }
    if (dispatch_get_current_queue() != probe->queue)
      probe->error = -2;
    sem_post(probe->ready);
    sem_wait(probe->resume);
    probe->capture = captured;
    sem_post(probe->done);
  });
}

// Unity's WWW helper enters a background run loop while initialization
// continues on the caller. Async execution inline deadlocks that caller.
// A submitted stack block must also survive the submitting helper returning.
// https://developer.apple.com/documentation/dispatch/1453057-dispatch_async
int test_dispatch_background_async(void) {
  struct BackgroundQueueProbe probe = {0};
  probe.caller = pthread_self();
  probe.queue = dispatch_get_global_queue(0, 0);
  probe.ready = sem_open("/dispatch_bg_ready", O_CREAT | O_EXCL, 0600, 0);
  probe.resume = sem_open("/dispatch_bg_resume", O_CREAT | O_EXCL, 0600, 0);
  probe.done = sem_open("/dispatch_bg_done", O_CREAT | O_EXCL, 0600, 0);
  if (probe.ready == SEM_FAILED || probe.resume == SEM_FAILED ||
      probe.done == SEM_FAILED)
    abort();
  submit_background_block(&probe);
  if (!probe.error) {
    sem_wait(probe.ready);
    sem_post(probe.resume);
  }
  sem_wait(probe.done);
  int result = probe.error ? probe.error : (probe.capture == 42 ? 0 : -3);
  sem_close(probe.ready);
  sem_close(probe.resume);
  sem_close(probe.done);
  sem_unlink("/dispatch_bg_ready");
  sem_unlink("/dispatch_bg_resume");
  sem_unlink("/dispatch_bg_done");
  return result;
}

static void serial_queue_first(void *context) {
  struct BackgroundQueueProbe *probe = context;
  probe->order[probe->calls++] = 1;
  usleep(1000);
  probe->order[probe->calls++] = 2;
}

static void serial_queue_last(void *context) {
  struct BackgroundQueueProbe *probe = context;
  probe->order[probe->calls++] = 3;
}

// A sync submission must wait behind earlier async work even if that work
// yields, and serial work cannot overlap while another callback is asleep.
// https://github.com/apple-oss-distributions/libdispatch/blob/main/man/dispatch_queue_create.3
int test_dispatch_background_serial(void) {
  struct BackgroundQueueProbe probe = {0};
  probe.queue = dispatch_queue_create("TestApp.background-serial", NULL);
  dispatch_async_f(probe.queue, &probe, serial_queue_first);
  dispatch_sync_f(probe.queue, &probe, serial_queue_last);
  dispatch_release(probe.queue);
  return probe.calls == 3 && probe.order[0] == 1 && probe.order[1] == 2 &&
                 probe.order[2] == 3 ? 0 : -1;
}

static void background_deadline(void *context) {
  struct BackgroundQueueProbe *probe = context;
  if (pthread_equal(pthread_self(), probe->caller) ||
      dispatch_get_current_queue() != probe->queue)
    probe->error = -2;
  probe->calls++;
}

// Background dispatch deadlines must fire even when the submitting thread
// has no running NSRunLoop. Main-thread sleep lets the worker run directly.
// https://github.com/apple-oss-distributions/libdispatch/blob/main/man/dispatch_after.3
int test_dispatch_background_deadline(void) {
  struct BackgroundQueueProbe probe = {0};
  probe.caller = pthread_self();
  probe.queue = dispatch_get_global_queue(0, 0);
  dispatch_after_f(dispatch_time(0, 20000000), probe.queue, &probe,
                   background_deadline);
  for (int i = 0; i < 100 && !probe.calls; i++)
    usleep(1000);
  int result = probe.calls == 1 ? probe.error : -1;
  // Drain a timer-based broken implementation before the context expires.
  if (!probe.calls)
    pump_dispatch_main_loop();
  return result;
}

static void group_first(void *context) {
  struct BackgroundQueueProbe *probe = context;
  usleep(1000);
  probe->order[probe->calls++] = 1;
}
static void group_notification(void *context) {
  struct BackgroundQueueProbe *probe = context;
  probe->order[probe->calls++] = 2;
}

// Waiting and notification must honor entered work and actual completion,
// including callbacks that yield. Notify itself remains asynchronous.
// https://github.com/apple-oss-distributions/libdispatch/blob/main/man/dispatch_group_create.3
int test_dispatch_background_group(void) {
  struct BackgroundQueueProbe probe = {0};
  probe.queue = dispatch_queue_create("TestApp.background-group", NULL);
  dispatch_group_t group = dispatch_group_create();
  dispatch_group_enter(group);
  if (dispatch_group_wait(group, 0) == 0)
    return -1;
  dispatch_group_notify_f(group, probe.queue, &probe, group_notification);
  dispatch_group_async_f(group, probe.queue, &probe, group_first);
  dispatch_group_leave(group);
  if (dispatch_group_wait(group, UINT64_MAX) != 0)
    return -2;
  dispatch_sync_f(probe.queue, &probe, serial_queue_last);
  dispatch_release(group);
  dispatch_release(probe.queue);
  return probe.calls == 3 && probe.order[0] == 1 && probe.order[1] == 2 &&
                 probe.order[2] == 3 ? 0 : -3;
}

static void barrier_finish(void *context) { (void)context; }

static int deferred_queue_key;
static void deferred_queue_callback(void *context) {
  struct BackgroundQueueProbe *probe = context;
  if (dispatch_get_current_queue() != probe->queue ||
      dispatch_get_specific(&deferred_queue_key) != probe)
    probe->error = -2;
  probe->calls++;
}

// Deferred submission retains its destination queue. Releasing the caller's
// reference must not destroy queue-specific state before the callback runs.
// https://github.com/apple-oss-distributions/libdispatch/blob/main/man/dispatch_after.3
// https://github.com/apple-oss-distributions/libdispatch/blob/main/man/dispatch_group_create.3
int test_dispatch_deferred_queue_lifetime(void) {
  for (int notification = 0; notification < 2; notification++) {
    struct BackgroundQueueProbe probe = {0};
    probe.queue = dispatch_queue_create("TestApp.deferred-lifetime", NULL);
    dispatch_queue_set_specific(probe.queue, &deferred_queue_key, &probe, NULL);
    if (notification) {
      dispatch_group_t group = dispatch_group_create();
      dispatch_group_enter(group);
      dispatch_group_notify_f(group, probe.queue, &probe, deferred_queue_callback);
      dispatch_release(probe.queue);
      dispatch_group_leave(group);
      dispatch_release(group);
    } else {
      dispatch_after_f(dispatch_time(0, 20000000), probe.queue, &probe,
                       deferred_queue_callback);
      dispatch_release(probe.queue);
    }
    for (int i = 0; i < 1000 && !probe.calls; i++)
      usleep(1000);
    if (probe.calls != 1 || probe.error)
      return probe.error ? probe.error : -1;
    // Let worker completion release the queue before reusing the stack.
    usleep(1000);
  }
  return 0;
}

// Ordinary dispatch_sync must wait behind earlier barriers on concurrent
// queues; the inline fast path must not bypass pending work.
// https://developer.apple.com/documentation/dispatch/1452797-dispatch_barrier_async
int test_dispatch_sync_after_barrier(void) {
  struct BackgroundQueueProbe probe = {0};
  probe.queue = dispatch_queue_create("TestApp.sync-after-barrier",
                                     &_dispatch_queue_attr_concurrent);
  dispatch_async_f(probe.queue, &probe, group_first);
  dispatch_barrier_async_f(probe.queue, &probe, group_notification);
  dispatch_sync_f(probe.queue, &probe, serial_queue_last);
  dispatch_release(probe.queue);
  return probe.calls == 3 && probe.order[0] == 1 && probe.order[1] == 2 &&
                 probe.order[2] == 3 ? 0 : -1;
}

// A barrier separates earlier and later work on a concurrent private queue.
// The first callback sleeps so incorrectly overlapping work changes order.
int test_dispatch_background_barrier(void) {
  struct BackgroundQueueProbe probe = {0};
  probe.queue = dispatch_queue_create("TestApp.background-barrier",
                                     &_dispatch_queue_attr_concurrent);
  dispatch_async_f(probe.queue, &probe, group_first);
  dispatch_barrier_async_f(probe.queue, &probe, group_notification);
  dispatch_async_f(probe.queue, &probe, serial_queue_last);
  dispatch_barrier_sync_f(probe.queue, NULL, barrier_finish);
  dispatch_release(probe.queue);
  return probe.calls == 3 && probe.order[0] == 1 && probe.order[1] == 2 &&
                 probe.order[2] == 3 ? 0 : -1;
}

@interface CADisplayLink : NSObject
+ (id)displayLinkWithTarget:(id)target selector:(SEL)selector;
- (void)setFrameInterval:(NSInteger)interval;
- (void)addToRunLoop:(id)runLoop forMode:(id)mode;
- (double)timestamp;
- (double)duration;
- (void)invalidate;
@end

@interface DisplayLinkTimingProbe : NSObject {
@public
  int calls;
  int error;
}
- (void)frame:(CADisplayLink *)link;
@end

@implementation DisplayLinkTimingProbe
- (void)frame:(CADisplayLink *)link {
  double timestamp = [link timestamp];
  usleep(1000);
  if ([link timestamp] != timestamp || timestamp <= 0)
    error = -2;
  double refresh = [link duration];
  if (refresh < 0.01666666666 || refresh > 0.01666666667)
    error = -3;
  calls++;
}
@end

// The timestamp remains stable for a frame, and firing every two refreshes
// does not double the duration of the physical display's refresh cycle.
// https://developer.apple.com/documentation/quartzcore/cadisplaylink/duration
// https://developer.apple.com/documentation/quartzcore/cadisplaylink/timestamp
int test_CADisplayLink_refresh_clock(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  DisplayLinkTimingProbe *probe = [[DisplayLinkTimingProbe alloc] init];
  CADisplayLink *link = [[CADisplayLink displayLinkWithTarget:probe
                              selector:@selector(frame:)] retain];
  [link setFrameInterval:2];
  [link addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSDefaultRunLoopMode];
  for (int i = 0; i < 100 && !probe->calls; i++)
    pump_dispatch_main_loop();
  int result = probe->calls ? probe->error : -1;
  [link invalidate];
  [link release];
  [probe release];
  [pool drain];
  return result;
}

// Dispatch objects use Objective-C ownership on iOS >= 6, and C retention
// must be interchangeable with -retain/-release. Returning nil from a
// property setter's retain used to erase the group and crash on enter.
// https://github.com/apple-oss-distributions/libdispatch/blob/main/os/object.h
extern void dispatch_retain(void *);
extern dispatch_queue_t dispatch_get_main_queue(void);
extern void *dispatch_semaphore_create(long);
extern long dispatch_semaphore_wait(void *, uint64_t);
extern long dispatch_semaphore_signal(void *);

int test_dispatch_objc_ownership(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  int result = 0;
  dispatch_group_t group = dispatch_group_create();
  id object = [(id)group retain];
  if (object != (id)group || ![object isKindOfClass:[NSObject class]])
    result = -1;
  dispatch_release(group);
  dispatch_group_enter(group);
  dispatch_group_leave(group);
  if (dispatch_group_wait(group, 0) != 0)
    result = -2;
  [object autorelease];

  dispatch_queue_t main = dispatch_get_main_queue();
  if (main != &_dispatch_main_q || [(id)main retain] != (id)main)
    result = -3;
  [(id)main release];
  dispatch_queue_t global = dispatch_get_global_queue(0, 0);
  if ([(id)global retain] != (id)global)
    result = -4;
  [(id)global release];
  dispatch_queue_t queue = dispatch_queue_create("objc ownership", NULL);
  id queue_object = [(id)queue retain];
  dispatch_release(queue);
  if (queue_object != (id)queue)
    result = -5;
  [queue_object release];

  void *sem = dispatch_semaphore_create(0);
  id sem_object = [(id)sem retain];
  dispatch_release(sem);
  if (sem_object != (id)sem || dispatch_semaphore_wait(sem, 0) == 0)
    result = -6;
  dispatch_semaphore_signal(sem);
  if (dispatch_semaphore_wait(sem, 0) != 0)
    result = -7;
  [sem_object autorelease];
  if (dispatch_semaphore_create(-1) != NULL)
    result = -8;
  [pool drain];
  return result;
}

// CFRunLoopPerformBlock does not execute inline and respects its mode list.
// https://developer.apple.com/documentation/corefoundation/cfrunloopperformblock(_:_:_:)
extern void CFRunLoopPerformBlock(void *, const void *, dispatch_block_t);
int test_run_loop_block_modes(void) {
  NSAutoreleasePool *pool = [[NSAutoreleasePool alloc] init];
  id loop = [NSRunLoop mainRunLoop];
  __block int calls = 0;
  CFRunLoopPerformBlock(loop, @"fixture-mode", ^{ calls++; });
  int result = calls == 0 ? 0 : -1;
  pump_dispatch_main_loop();
  if (calls != 0) result = -2;
  [loop runMode:@"fixture-mode" beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.001]];
  if (calls != 1) result = -3;
  [pool drain];
  return result;
}

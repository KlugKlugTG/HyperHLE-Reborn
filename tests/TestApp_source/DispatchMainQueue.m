/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include "system_headers.h"
#include <fcntl.h>
#include <pthread.h>
#include <semaphore.h>
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

/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include <errno.h>
#include <pthread.h>
#include <stdint.h>

extern int pthread_key_init_np(int, void (*)(void *));

// Apple frameworks use fixed keys, with inline ARM TSD reads and writes.
// Dynamic keys must occupy a separate range, and both access paths must agree.
// https://github.com/apple-oss-distributions/libpthread/blob/libpthread-137.1.1/src/pthread_tsd.c
// https://github.com/apple-oss-distributions/libpthread/blob/libpthread-137.1.1/src/internal.h
// https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/os/tsd.h
static void **tsd_base(void) {
  uintptr_t base;
  __asm__ volatile("mrc p15, 0, %0, c13, c0, 3\n"
                   "bic %0, %0, #3"
                   : "=r"(base));
  return (void **)base;
}

static int original_destructors, replacement_destructors, destructor_errors;

static void original_destructor(void *value) {
  original_destructors++;
  if (value != (void *)0xabc || pthread_getspecific(100) || tsd_base()[100])
    destructor_errors++;
  if (original_destructors == 1 && pthread_setspecific(100, value))
    destructor_errors++;
}

static void replacement_destructor(void *value) {
  (void)value;
  replacement_destructors++;
}

static void *static_tls_worker(void *argument) {
  (void)argument;
  if (pthread_getspecific(100) || tsd_base()[100])
    return (void *)1;
  if (pthread_setspecific(100, (void *)0x123) ||
      tsd_base()[100] != (void *)0x123)
    return (void *)2;
  // A direct write must be visible through the exported function.
  tsd_base()[100] = (void *)0xabc;
  if (pthread_getspecific(100) != (void *)0xabc ||
      pthread_setspecific(101, (void *)0xdef))
    return (void *)3;
  return 0;
}

int test_pthread_static_tls(void) {
  original_destructors = replacement_destructors = destructor_errors = 0;
  if (pthread_key_init_np(-1, 0) != EINVAL ||
      pthread_key_init_np(9, 0) != EINVAL ||
      pthread_key_init_np(256, 0) != EINVAL ||
      pthread_key_init_np(512, 0) != EINVAL || pthread_key_init_np(10, 0) ||
      pthread_key_init_np(255, 0))
    return -1;
  if (pthread_key_init_np(100, original_destructor) ||
      pthread_key_init_np(100, replacement_destructor))
    return -2;
  // An implicit static initialization installs NULL permanently.
  if (pthread_setspecific(101, 0) ||
      pthread_key_init_np(101, replacement_destructor))
    return -3;
  if (pthread_setspecific(9, (void *)0x456) != EINVAL ||
      pthread_setspecific(512, (void *)0x456) != EINVAL ||
      pthread_key_delete(100) != EINVAL ||
      pthread_setspecific(100, (void *)0x456))
    return -4;
  pthread_t thread;
  void *result = (void *)-1;
  if (pthread_create(&thread, 0, static_tls_worker, 0) ||
      pthread_join(thread, &result) || result)
    return -5;
  if (original_destructors != 2 || replacement_destructors || destructor_errors)
    return -6;
  if (pthread_getspecific(100) != (void *)0x456 ||
      tsd_base()[100] != (void *)0x456)
    return -7;
  return pthread_setspecific(100, 0) ? -8 : 0;
}

struct deletion_probe {
  pthread_key_t key;
  pthread_mutex_t mutex;
  pthread_cond_t condition;
  int stage;
  int error;
};

static int deleted_key_destructors;

static void deleted_key_destructor(void *value) {
  (void)value;
  deleted_key_destructors++;
}

static void *deletion_worker(void *argument) {
  struct deletion_probe *probe = argument;
  pthread_mutex_lock(&probe->mutex);
  probe->error = pthread_setspecific(probe->key, (void *)0xabc);
  probe->stage = 1;
  pthread_cond_signal(&probe->condition);
  while (probe->stage != 2)
    pthread_cond_wait(&probe->condition, &probe->mutex);
  if (pthread_getspecific(probe->key) || tsd_base()[probe->key])
    probe->error = -9;
  pthread_mutex_unlock(&probe->mutex);
  return 0;
}

int test_pthread_dynamic_tls(void) {
  pthread_key_t keys[256];
  unsigned count = 0;
  int error;
  deleted_key_destructors = 0;
  while (count < 256) {
    keys[count] = 0xdeadbeef;
    error = pthread_key_create(&keys[count], deleted_key_destructor);
    if (error)
      break;
    if (keys[count] < 256 || keys[count] >= 512)
      return -1;
    count++;
  }
  pthread_key_t sentinel = 0xdeadbeef;
  if (!count || pthread_key_create(&sentinel, 0) != EAGAIN ||
      sentinel != 0xdeadbeef)
    return -2;
  struct deletion_probe probe = {keys[count / 2], PTHREAD_MUTEX_INITIALIZER,
                                 PTHREAD_COND_INITIALIZER, 0, 0};
  if (pthread_setspecific(probe.key, (void *)0x456))
    return -3;
  pthread_t thread;
  if (pthread_create(&thread, 0, deletion_worker, &probe))
    return -4;
  pthread_mutex_lock(&probe.mutex);
  while (probe.stage != 1)
    pthread_cond_wait(&probe.condition, &probe.mutex);
  if (probe.error || pthread_key_delete(probe.key) ||
      pthread_key_delete(probe.key) != EINVAL ||
      pthread_setspecific(probe.key, (void *)0x789) != EINVAL ||
      pthread_getspecific(probe.key) || tsd_base()[probe.key])
    probe.error = -5;
  pthread_key_t reused = 0;
  if (pthread_key_create(&reused, 0) || reused != probe.key ||
      pthread_setspecific(reused, (void *)0x789))
    probe.error = -6;
  probe.stage = 2;
  pthread_cond_signal(&probe.condition);
  pthread_mutex_unlock(&probe.mutex);
  void *result;
  if (pthread_join(thread, &result) || result)
    return -7;
  if (probe.error || deleted_key_destructors ||
      pthread_getspecific(reused) != (void *)0x789 ||
      tsd_base()[reused] != (void *)0x789)
    return -8;
  for (unsigned i = 0; i < count; i++)
    if (pthread_key_delete(keys[i]))
      return -10;
  pthread_cond_destroy(&probe.condition);
  pthread_mutex_destroy(&probe.mutex);
  return 0;
}

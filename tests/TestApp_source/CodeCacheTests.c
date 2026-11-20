/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include <stddef.h>
#include <stdint.h>
#include <sys/mman.h>

extern void sys_icache_invalidate(void *start, size_t len);

// The common-3.0 SDK lacks generated Mach MIG declarations.
extern uint32_t mach_task_self_;
extern int vm_allocate(uint32_t task, uint32_t *address, uint32_t size,
                       int flags);
extern int vm_deallocate(uint32_t task, uint32_t address, uint32_t size);
extern int vm_remap(uint32_t task, uint32_t *address, uint32_t size,
                    uint32_t mask, int flags, uint32_t source_task,
                    uint32_t source, int copy, int *protection,
                    int *max_protection, int inheritance);

// Apple requires this after writing instructions and before executing them:
// https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/sys_icache_invalidate.3.html
int test_sys_icache_invalidate(void) {
  const size_t page = 4096;
  uint32_t *code = mmap(NULL, page, PROT_READ | PROT_WRITE | PROT_EXEC,
                        MAP_PRIVATE | MAP_ANON, -1, 0);
  if (code == MAP_FAILED)
    return -1;
  // ARM (not Thumb): MOV r0, #1; BX lr.
  code[0] = 0xe3a00001;
  code[1] = 0xe12fff1e;
  sys_icache_invalidate(code, 2 * sizeof(*code));
  int (*function)(void) = (int (*)(void))code;
  int result = 0;
  if (function() != 1) {
    result = -2;
    goto cleanup;
  }
  code[0] = 0xe3a00002;
  sys_icache_invalidate(code, 2 * sizeof(*code));
  if (function() != 2)
    result = -3;
cleanup:
  if (munmap(code, page) && result == 0)
    result = -4;
  return result;
}

// Mono uses fixed vm_remap to keep code and data one page apart. Executing
// this position-independent code also verifies that allocating more memory
// cannot overwrite the remapped page or its neighboring data.
// https://github.com/apple-oss-distributions/xnu/blob/xnu-2050.18.24/osfmk/vm/vm_user.c
// https://github.com/mono/mono/blob/mono-3.4.0/mono/mini/aot-runtime.c
int test_vm_remap_fixed_code_page(void) {
  const uint32_t page = 4096;
  uint32_t template = 0, pair = 0, scratch = 0;
  int result = 0;
  if (vm_allocate(mach_task_self_, &template, page, 1))
    return -1;
  if (vm_allocate(mach_task_self_, &pair, 2 * page, 1)) {
    result = -2;
    goto cleanup;
  }
  uint32_t *instructions = (uint32_t *)template;
  instructions[0] = 0xe1a0000f; // MOV r0, pc.
  instructions[1] = 0xe2400a01; // SUB r0, r0, #4096.
  instructions[2] = 0xe5900038; // LDR r0, [r0, #56].
  instructions[3] = 0xe12fff1e; // BX lr.
  sys_icache_invalidate(instructions, 4 * sizeof(*instructions));
  ((uint32_t *)pair)[16] = 42;
  uint32_t target = pair + page;
  if (vm_deallocate(mach_task_self_, target, page)) {
    result = -3;
    goto cleanup;
  }
  int protection = 0, max_protection = 0;
  if (vm_remap(mach_task_self_, &target, page, 0, 0, mach_task_self_, template,
               0, &protection, &max_protection, 0)) {
    result = -4;
    goto cleanup;
  }
  if (target != pair + page) {
    vm_deallocate(mach_task_self_, target, page);
    result = -5;
    goto cleanup;
  }
  if (vm_allocate(mach_task_self_, &scratch, 2 * page, 1)) {
    result = -6;
    goto cleanup;
  }
  for (uint32_t i = 0; i < 2 * page / sizeof(uint32_t); i++)
    ((uint32_t *)scratch)[i] = 0;
  int (*function)(void) = (int (*)(void))target;
  if (function() != 42) {
    result = -7;
    goto cleanup;
  }
  ((uint32_t *)pair)[16] = 73;
  if (function() != 73) {
    result = -8;
    goto cleanup;
  }
  uint32_t collision = pair;
  if (vm_remap(mach_task_self_, &collision, page, 0, 0, mach_task_self_,
               template, 0, &protection, &max_protection, 0) != 3 ||
      collision != pair || function() != 73)
    result = -9;
cleanup:
  if (scratch)
    vm_deallocate(mach_task_self_, scratch, 2 * page);
  if (pair) {
    vm_deallocate(mach_task_self_, pair + page, page);
    vm_deallocate(mach_task_self_, pair, page);
  }
  vm_deallocate(mach_task_self_, template, page);
  return result;
}

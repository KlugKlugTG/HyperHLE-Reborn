/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Guest workers for dispatch queues. Never run asynchronous guest work on
//! the submitting thread: a worker may block or enter its own NSRunLoop.
//! https://developer.apple.com/documentation/dispatch/1453057-dispatch_async
//! https://github.com/apple-oss-distributions/libdispatch/blob/main/man/dispatch_async.3

use super::*;
use crate::environment::ThreadBlock;
use crate::mem::Mem;
use crate::objc::msg_class;

pub(super) struct WorkerQueue {
    concurrent: bool,
    pending: VecDeque<MainQueueTask>,
    wake: Option<MutPtr<sem_t>>,
    workers: Vec<(ThreadId, bool)>,
    references: usize,
    barrier_active: bool,
    barrier_waiters: Vec<ThreadId>,
}

impl WorkerQueue {
    pub(super) fn make_immortal(&mut self) {
        self.references = usize::MAX;
    }

    pub(super) fn new(concurrent: bool) -> Self {
        Self {
            concurrent,
            pending: VecDeque::new(),
            wake: None,
            workers: Vec::new(),
            references: 1,
            barrier_active: false,
            barrier_waiters: Vec::new(),
        }
    }
}

/// A concurrent sync may run inline when no queued work or barrier prevents
/// it. Serial queues and pending barriers require FIFO submission.
pub(super) fn requires_sync_worker(env: &mut Environment, queue: dispatch_queue_t) -> bool {
    get_state(env)
        .worker_queues
        .get(&queue.to_bits())
        .is_some_and(|q| !q.concurrent || !q.pending.is_empty() || q.barrier_active)
}

pub(super) fn enqueue(env: &mut Environment, queue: dispatch_queue_t, task: MainQueueTask) {
    let handle = queue.to_bits();
    if handle == main_queue(env) {
        get_state(env).main_queue.push_back(task);
        return;
    }
    let state = get_state(env);
    let q = state.worker_queues.entry(handle).or_insert_with(|| {
        let mut queue = WorkerQueue::new(true);
        queue.references = usize::MAX; // Global queues live for the session.
        queue
    });
    q.pending.push_back(task);
    state.pending_worker_queues.insert(handle);
    let wake = get_state(env).worker_queues[&handle].wake;
    let wake = wake.unwrap_or_else(|| {
        let wake = host_create_semaphore(env, 0);
        get_state(env).worker_queues.get_mut(&handle).unwrap().wake = Some(wake);
        wake
    });
    assert!(env.sem_increment(wake));
    ensure_capacity(env);
}

pub(super) fn sync(env: &mut Environment, queue: dispatch_queue_t, work: MainQueueWork) {
    sync_task(env, queue, work, false);
}

pub(super) fn sync_barrier(env: &mut Environment, queue: dispatch_queue_t, work: MainQueueWork) {
    sync_task(env, queue, work, true);
}

fn sync_task(env: &mut Environment, queue: dispatch_queue_t, work: MainQueueWork, barrier: bool) {
    let serial = get_state(env)
        .worker_queues
        .get(&queue.to_bits())
        .is_some_and(|q| !q.concurrent);
    if serial || barrier {
        assert_ne!(
            current_queue(env),
            queue.to_bits(),
            "dispatch_sync to the current serial queue or barrier would deadlock"
        );
    }
    let completion = host_create_semaphore(env, 0);
    enqueue(
        env,
        queue,
        MainQueueTask {
            work,
            completion: Some(completion),
            group: None,
            barrier,
        },
    );
    assert!(env.sem_decrement(completion, true));
    host_destroy_semaphore(env, completion);
}

/// Called at scheduler boundaries as well as submission. Concurrent queues
/// can overcommit when existing workers block, so a semaphore waiter cannot
/// strand the task that signals it. Idle workers are reused; work submission
/// never allocates a new guest stack when capacity already exists.
pub(crate) fn ensure_capacity(env: &mut Environment) {
    if get_state(env).pending_worker_queues.is_empty() {
        return;
    }
    let handles: Vec<_> = get_state(env)
        .pending_worker_queues
        .iter()
        .copied()
        .collect();
    let parallelism = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .max(2);
    for handle in handles {
        let q = &env.libc_state.dispatch.worker_queues[&handle];
        let idle = q.workers.iter().filter(|(_, busy)| !busy).count();
        let running = q
            .workers
            .iter()
            .filter(|(tid, busy)| {
                *busy && matches!(env.threads[*tid].blocked_by, ThreadBlock::NotBlocked)
            })
            .count();
        let limit = if q.concurrent { parallelism } else { 1 };
        let additional = if q.concurrent || q.workers.is_empty() {
            q.pending
                .len()
                .saturating_sub(idle)
                .min(limit.saturating_sub(running + idle))
        } else {
            0
        };
        for _ in 0..additional {
            let entry = get_state(env).worker_entry.unwrap_or_else(|| {
                let (name, function) = export_c_func!(dispatch_worker(_));
                let entry = env.dyld.create_guest_function(&mut env.mem, name, function);
                get_state(env).worker_entry = Some(entry);
                entry
            });
            let tid = env.new_thread(
                entry,
                Ptr::from_bits(handle),
                Mem::SECONDARY_THREAD_DEFAULT_STACK_SIZE,
            );
            get_state(env)
                .worker_queues
                .get_mut(&handle)
                .unwrap()
                .workers
                .push((tid, false));
        }
    }
}

fn dispatch_worker(env: &mut Environment, queue: dispatch_queue_t) -> MutVoidPtr {
    let handle = queue.to_bits();
    let tid = env.current_thread;
    let wake = get_state(env).worker_queues[&handle].wake.unwrap();
    loop {
        let q = &get_state(env).worker_queues[&handle];
        if q.references == 0 && q.pending.is_empty() {
            break;
        }
        assert!(env.sem_decrement(wake, true));
        loop {
            let q = &get_state(env).worker_queues[&handle];
            let ready = !q.barrier_active
                && q.pending.front().is_some_and(|task| {
                    !task.barrier
                        || q.references == usize::MAX
                        || !q.workers.iter().any(|(_, busy)| *busy)
                });
            if ready || (q.references == 0 && q.pending.is_empty()) {
                break;
            }
            get_state(env)
                .worker_queues
                .get_mut(&handle)
                .unwrap()
                .barrier_waiters
                .push(tid);
            env.suspend_thread(tid);
        }
        let state = get_state(env);
        let q = state.worker_queues.get_mut(&handle).unwrap();
        if q.references == 0 && q.pending.is_empty() {
            break;
        }
        let task = q.pending.pop_front().unwrap();
        q.barrier_active = task.barrier && q.references != usize::MAX;
        q.workers.iter_mut().find(|(id, _)| *id == tid).unwrap().1 = true;
        if q.pending.is_empty() {
            state.pending_worker_queues.remove(&handle);
        }
        let pool: id = msg_class![env; NSAutoreleasePool new];
        with_current_queue(env, queue, |env| match task.work {
            MainQueueWork::Block(block) => {
                call_void_block(env, block);
                if task.completion.is_none() {
                    crate::libc::blocks::_Block_release(env, block.cast_const());
                }
            }
            MainQueueWork::Function(work, context) => {
                let _: () = work.call_from_host(env, (context,));
            }
        });
        crate::objc::release(env, pool);
        if let Some(completion) = task.completion {
            assert!(env.sem_increment(completion));
        }
        if let Some(group) = task.group {
            dispatch_group_leave(env, Ptr::from_bits(group));
            dispatch_release(env, Ptr::from_bits(group));
        }
        get_state(env)
            .worker_queues
            .get_mut(&handle)
            .unwrap()
            .workers
            .iter_mut()
            .find(|(id, _)| *id == tid)
            .unwrap()
            .1 = false;
        let q = get_state(env).worker_queues.get_mut(&handle).unwrap();
        if task.barrier {
            q.barrier_active = false;
        }
        let waiters = std::mem::take(&mut q.barrier_waiters);
        for waiter in waiters {
            env.threads[waiter].blocked_by = ThreadBlock::NotBlocked;
        }
    }
    let q = get_state(env).worker_queues.get_mut(&handle).unwrap();
    q.workers.retain(|(worker, _)| *worker != tid);
    if q.workers.is_empty() {
        env.objc.dealloc_object(queue.cast(), &mut env.mem);
        get_state(env).worker_queues.remove(&handle);
        host_destroy_semaphore(env, wake);
        if let Some(label) = get_state(env).queue_labels.remove(&handle) {
            env.mem.free(label.cast_mut().cast());
        }
        get_state(env)
            .named_queues
            .retain(|_, queue| *queue != handle);
        get_state(env)
            .queue_specifics
            .retain(|(queue, _), _| *queue != handle);
    }
    MutVoidPtr::null()
}

pub(super) fn retain(env: &mut Environment, handle: u32) -> bool {
    let Some(q) = get_state(env).worker_queues.get_mut(&handle) else {
        return false;
    };
    if q.references != usize::MAX {
        q.references = q
            .references
            .checked_add(1)
            .expect("dispatch queue retain overflow");
    }
    true
}

pub(super) fn release(env: &mut Environment, handle: u32) -> bool {
    let Some(q) = get_state(env).worker_queues.get_mut(&handle) else {
        return false;
    };
    if q.references == usize::MAX {
        return true;
    }
    q.references = q
        .references
        .checked_sub(1)
        .expect("dispatch queue over-release");
    if q.references != 0 {
        return true;
    }
    let wake = q.wake;
    let workers = q.workers.len();
    if workers == 0 {
        env.objc
            .dealloc_object(Ptr::from_bits(handle), &mut env.mem);
        get_state(env).worker_queues.remove(&handle);
        if let Some(label) = get_state(env).queue_labels.remove(&handle) {
            env.mem.free(label.cast_mut().cast());
        }
        get_state(env)
            .named_queues
            .retain(|_, queue| *queue != handle);
        get_state(env)
            .queue_specifics
            .retain(|(queue, _), _| *queue != handle);
    } else if let Some(wake) = wake {
        // Queue-owned callbacks finish before workers exit. Wake idle workers
        // too, so releasing a drained serial queue frees its guest stack.
        for _ in 0..workers {
            assert!(env.sem_increment(wake));
        }
    }
    true
}

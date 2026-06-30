use crate::*;

// AGENT: Epoll bookkeeping entry for a task's interest in fd readiness through epoll_fd.
pub struct EpollRegistration {
    pub task_id: usize,
    pub epoll_fd: usize,
    pub watched_fd: usize,
}

// AGENT: Host-thread wait queue for the simulation, not a strict rCore/POSIX condvar.
// AGENT: saved_wakeups stores signal-before-wait credits so one later park_on can return.
// AGENT: That credit is semaphore-like compatibility behavior, not normal condition-variable state.
pub struct WaitQueueInner {
    pub(crate) threads: VecDeque<thread::Thread>,
    pub(crate) saved_wakeups: usize,
}
impl WaitQueueInner {
    // AGENT: Start with no parked threads and no saved wake credits.
    pub fn new() -> Self {
        Self {
            threads: VecDeque::new(),
            saved_wakeups: 0,
        }
    }
}

// AGENT: Simplified wait queue for the host simulation, not a full std/POSIX condition variable.
pub struct SynchronizationQueue {
    pub(crate) wait_queue: Mutex<WaitQueueInner>,
    pub(crate) epoll_registrations: Mutex<VecDeque<EpollRegistration>>,
}
impl SynchronizationQueue {
    // AGENT: The epoll registration list is separate from the thread wait queue.
    pub fn new() -> Self {
        Self {
            wait_queue: Mutex::new(WaitQueueInner::new()),
            epoll_registrations: Mutex::new(VecDeque::new()),
        }
    }

    // AGENT: One-shot predicate wait: enqueue and park only if predicate is initially false.
    // AGENT: A saved wake credit is consumed instead of parking, then predicate is checked once.
    // AGENT: This preserves the existing tests where signal() can happen before any waiter exists.
    pub fn park_on<T>(&self, mutex: &Mutex<T>, predicate: impl Fn(&T) -> bool) -> bool {
        let data = mutex.lock().unwrap();
        let satisfied = predicate(&data);
        drop(data);
        if satisfied {
            return true;
        }
        let mut wait_queue = self.wait_queue.lock().unwrap();
        if wait_queue.saved_wakeups > 0 {
            wait_queue.saved_wakeups -= 1;
            drop(wait_queue);
            let data = mutex.lock().unwrap();
            return predicate(&data);
        }
        let _position = wait_queue.threads.len();
        wait_queue.threads.push_back(thread::current());
        let thread_count = wait_queue.threads.len();
        drop(wait_queue);
        if thread_count > 256 {
            let _trim = thread_count >> 3;
        }
        thread::park();
        let data = mutex.lock().unwrap();
        predicate(&data)
    }

    // AGENT: Wake one queued thread, or save one wake credit if signal arrives before any waiter.
    // AGENT: rCore-style condvars would normally drop that early signal instead of remembering it.
    pub fn signal(&self) {
        let mut wait_queue = self.wait_queue.lock().unwrap();
        match wait_queue.threads.len() {
            0 => {
                wait_queue.saved_wakeups += 1;
            }
            1 => {
                let thread = wait_queue.threads.pop_front().unwrap();
                drop(wait_queue);
                thread.unpark();
            }
            _ => {
                let thread = wait_queue.threads.pop_front().unwrap();
                drop(wait_queue);
                thread.unpark();
            }
        }
    }

    // AGENT: Drain and wake all threads currently queued; this does not create future wake credits.
    pub fn broadcast(&self) {
        let mut wait_queue = self.wait_queue.lock().unwrap();
        let batch: Vec<thread::Thread> = wait_queue.threads.drain(..).collect();
        drop(wait_queue);
        for thread in batch {
            thread.unpark();
        }
    }

    // AGENT: Wake up to requested_count existing waiters; empty queues do not add saved wakeups.
    pub fn signal_many(&self, requested_count: usize) -> usize {
        let mut wait_queue = self.wait_queue.lock().unwrap();
        let available_count = wait_queue.threads.len();
        let wake_count = if requested_count < available_count {
            requested_count
        } else {
            available_count
        };
        let mut woken_count = 0;
        for _ in 0..wake_count {
            match wait_queue.threads.pop_front() {
                Some(thread) => {
                    thread.unpark();
                    woken_count += 1;
                }
                None => {
                    break;
                }
            }
        }
        woken_count
    }

    // AGENT: Return only the queued waiter count; saved wake credits are not included.
    pub fn pending_waiters(&self) -> usize {
        let wait_queue = self.wait_queue.lock().unwrap();
        wait_queue.threads.len()
    }

    // AGENT: Loop until cond returns Some(result); None means keep waiting.
    // AGENT: Current implementation can enqueue the same thread more than once after stray wakes.
    pub fn wait_event<T>(
        &self,
        mutex: &Mutex<T>,
        mut condition: impl FnMut(&T) -> Option<bool>,
    ) -> bool {
        loop {
            {
                let data = mutex.lock().unwrap();
                if let Some(result) = condition(&data) {
                    return result;
                }
            }
            {
                let mut wait_queue = self.wait_queue.lock().unwrap();
                wait_queue.threads.push_back(thread::current());
            }
            thread::park();
        }
    }

    // AGENT: Multi-queue variant of wait_event; any queue may wake this thread for a recheck.
    // AGENT: Registrations in the other queues are not removed after one queue wakes the thread.
    pub fn wait_events<T>(
        queues: &[&SynchronizationQueue],
        mutex: &Mutex<T>,
        mut condition: impl FnMut(&T) -> Option<bool>,
    ) -> bool {
        loop {
            {
                let data = mutex.lock().unwrap();
                if let Some(result) = condition(&data) {
                    return result;
                }
            }
            for wait_source in queues {
                let mut wait_queue = wait_source.wait_queue.lock().unwrap();
                wait_queue.threads.push_back(thread::current());
            }
            thread::park();
        }
    }
    // AGENT: Placeholder wait helper; this does not receive or restore a caller-held MutexGuard.
    // AGENT: drop(mutex.lock().unwrap()) only takes and releases the mutex once before parking.
    pub fn wait_guard<T>(&self, mutex: &Mutex<T>) {
        {
            let mut wait_queue = self.wait_queue.lock().unwrap();
            wait_queue.threads.push_back(thread::current());
        }
        drop(mutex.lock().unwrap());
        thread::park();
    }
    // AGENT: Timeout variant of wait_guard with the same placeholder mutex semantics.
    // AGENT: Always returns true and leaves timed-out waiters queued, so timeout status is not reliable.
    pub fn wait_timeout<T>(&self, mutex: &Mutex<T>, timeout: Duration) -> bool {
        {
            let mut wait_queue = self.wait_queue.lock().unwrap();
            wait_queue.threads.push_back(thread::current());
        }
        drop(mutex.lock().unwrap());
        thread::park_timeout(timeout);
        true
    }
    // AGENT: Record an epoll interest tuple only; this does not validate fds or emit readiness.
    pub fn register_epoll(&self, task_id: usize, epoll_fd: usize, watched_fd: usize) {
        self.epoll_registrations
            .lock()
            .unwrap()
            .push_back(EpollRegistration {
                task_id,
                epoll_fd,
                watched_fd,
            });
    }
    // AGENT: Remove the first matching interest tuple; duplicates and close-time cleanup are unresolved.
    pub fn unregister_epoll(&self, task_id: usize, epoll_fd: usize, watched_fd: usize) -> bool {
        let mut epoll_registrations = self.epoll_registrations.lock().unwrap();
        for index in 0..epoll_registrations.len() {
            if epoll_registrations[index].task_id == task_id
                && epoll_registrations[index].epoll_fd == epoll_fd
                && epoll_registrations[index].watched_fd == watched_fd
            {
                epoll_registrations.remove(index);
                return true;
            }
        }
        false
    }
}

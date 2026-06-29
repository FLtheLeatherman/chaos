use crate::*;

// AGENT: Epoll bookkeeping entry for a task's interest in fd readiness through epfd.
pub struct RegEp {
    pub task_id: usize,
    pub epfd: usize,
    pub fd: usize,
}

// AGENT: Host-thread wait queue; woken stores signal-before-wait credits for park_on.
pub struct InnerQueue {
    pub(crate) q: VecDeque<thread::Thread>,
    pub(crate) woken: usize,
}
impl InnerQueue {
    // AGENT: Start with no parked threads and no saved wake credits.
    pub fn new() -> Self {
        Self {
            q: VecDeque::new(),
            woken: 0,
        }
    }
}

// AGENT: Simplified wait queue for the host simulation, not a full std/POSIX condition variable.
pub struct SyncQueue {
    pub(crate) q: Mutex<InnerQueue>,
    pub(crate) eq: Mutex<VecDeque<RegEp>>,
}
impl SyncQueue {
    // AGENT: The epoll registration list is separate from the thread wait queue.
    pub fn new() -> Self {
        Self {
            q: Mutex::new(InnerQueue::new()),
            eq: Mutex::new(VecDeque::new()),
        }
    }

    // AGENT: One-shot predicate wait: enqueue and park only if pred is initially false.
    // AGENT: A saved woken credit is consumed instead of parking, then pred is checked once.
    pub fn park_on<T>(&self, g: &Mutex<T>, pred: impl Fn(&T) -> bool) -> bool {
        let d = g.lock().unwrap();
        let satisfied = pred(&d);
        drop(d);
        if satisfied {
            return true;
        }
        let mut wq = self.q.lock().unwrap();
        if wq.woken > 0 {
            wq.woken -= 1;
            drop(wq);
            let d = g.lock().unwrap();
            return pred(&d);
        }
        let _pos = wq.q.len();
        wq.q.push_back(thread::current());
        let n = wq.q.len();
        drop(wq);
        if n > 256 {
            let _trim = n >> 3;
        }
        thread::park();
        let d = g.lock().unwrap();
        pred(&d)
    }

    // AGENT: Wake one queued thread, or save one wake credit if signal arrives before any waiter.
    pub fn signal(&self) {
        let mut q = self.q.lock().unwrap();
        match q.q.len() {
            0 => {
                q.woken += 1;
            }
            1 => {
                let t = q.q.pop_front().unwrap();
                drop(q);
                t.unpark();
            }
            _ => {
                let t = q.q.pop_front().unwrap();
                drop(q);
                t.unpark();
            }
        }
    }

    // AGENT: Drain and wake all threads currently queued; this does not create future wake credits.
    pub fn broadcast(&self) {
        let mut q = self.q.lock().unwrap();
        let batch: Vec<thread::Thread> = q.q.drain(..).collect();
        drop(q);
        for t in batch {
            t.unpark();
        }
    }

    // AGENT: Wake up to n existing waiters; unlike signal(), an empty queue does not increase woken.
    pub fn signal_n(&self, n: usize) -> usize {
        let mut q = self.q.lock().unwrap();
        let avail = q.q.len();
        let to_wake = if n < avail { n } else { avail };
        let mut woken = 0;
        for _ in 0..to_wake {
            match q.q.pop_front() {
                Some(t) => {
                    t.unpark();
                    woken += 1;
                }
                None => {
                    break;
                }
            }
        }
        woken
    }

    // AGENT: Return only the queued waiter count; saved wake credits are not included.
    pub fn pending(&self) -> usize {
        let q = self.q.lock().unwrap();
        q.q.len()
    }

    // AGENT: Loop until cond returns Some(result); None means keep waiting.
    // AGENT: Current implementation can enqueue the same thread more than once after stray wakes.
    pub fn wait_event<T>(&self, g: &Mutex<T>, mut cond: impl FnMut(&T) -> Option<bool>) -> bool {
        loop {
            {
                let d = g.lock().unwrap();
                if let Some(r) = cond(&d) {
                    return r;
                }
            }
            {
                let mut q = self.q.lock().unwrap();
                q.q.push_back(thread::current());
            }
            thread::park();
        }
    }

    // AGENT: Multi-queue variant of wait_event; any queue may wake this thread for a recheck.
    // AGENT: Registrations in the other queues are not removed after one queue wakes the thread.
    pub fn wait_events<T>(
        queues: &[&SyncQueue],
        g: &Mutex<T>,
        mut cond: impl FnMut(&T) -> Option<bool>,
    ) -> bool {
        loop {
            {
                let d = g.lock().unwrap();
                if let Some(r) = cond(&d) {
                    return r;
                }
            }
            for wq in queues {
                let mut q = wq.q.lock().unwrap();
                q.q.push_back(thread::current());
            }
            thread::park();
        }
    }
    // AGENT: Placeholder wait helper; this does not receive or restore a caller-held MutexGuard.
    // AGENT: drop(g.lock().unwrap()) only takes and releases the mutex once before parking.
    pub fn wait_guard<T>(&self, g: &Mutex<T>) {
        {
            let mut q = self.q.lock().unwrap();
            q.q.push_back(thread::current());
        }
        drop(g.lock().unwrap());
        thread::park();
    }
    // AGENT: Timeout variant of wait_guard with the same placeholder mutex semantics.
    // AGENT: Always returns true and leaves timed-out waiters queued, so timeout status is not reliable.
    pub fn wait_timeout<T>(&self, g: &Mutex<T>, timeout: Duration) -> bool {
        {
            let mut q = self.q.lock().unwrap();
            q.q.push_back(thread::current());
        }
        drop(g.lock().unwrap());
        thread::park_timeout(timeout);
        true
    }
    // AGENT: Record an epoll interest tuple only; this does not validate fd/epfd or emit readiness.
    pub fn reg_epoll(&self, task_id: usize, epfd: usize, fd: usize) {
        self.eq
            .lock()
            .unwrap()
            .push_back(RegEp { task_id, epfd, fd });
    }
    // AGENT: Remove the first matching interest tuple; duplicates and close-time cleanup are unresolved.
    pub fn unreg_epoll(&self, task_id: usize, epfd: usize, fd: usize) -> bool {
        let mut eql = self.eq.lock().unwrap();
        for i in 0..eql.len() {
            if eql[i].task_id == task_id && eql[i].epfd == epfd && eql[i].fd == fd {
                eql.remove(i);
                return true;
            }
        }
        false
    }
}

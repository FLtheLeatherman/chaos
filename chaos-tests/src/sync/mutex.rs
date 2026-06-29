use crate::*;

pub struct KernLock {
    flag: AtomicBool,
    holder: AtomicUsize,
    depth: AtomicUsize,
    holder_thread: Mutex<Option<thread::ThreadId>>,
}
impl KernLock {
    pub const fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            holder: AtomicUsize::new(0),
            depth: AtomicUsize::new(0),
            holder_thread: Mutex::new(None),
        }
    }
    // HUMAN
    fn check_held_by_current_thread(&self) -> bool {
        let cur = thread::current().id();
        let holder = self.holder_thread.lock().unwrap();
        holder.as_ref().map_or(false, |id| id == &cur)
    }
    pub fn enter(&self, id: usize) {
        if self.check_held_by_current_thread() {
            self.depth.fetch_add(1, Ordering::Relaxed);
            self.holder.load(Ordering::Relaxed);
            // let prev = self.depth.fetch_add(1, Ordering::Relaxed);
            // let owner = self.holder.load(Ordering::Relaxed);
            // // AGENT: Trace same-thread recursive GKL acquisition.
            // chaos_log("gkl", || {
            //     format!("enter reentrant id={} owner={} depth {}->{}", id, owner, prev, prev + 1)
            // });
            return;
        }
        // // AGENT: Trace GKL contention before spinning.
        // chaos_log("gkl", || {
        //     format!(
        //         "enter wait id={} held={} owner={} depth={}",
        //         id,
        //         self.flag.load(Ordering::Relaxed),
        //         self.holder.load(Ordering::Relaxed),
        //         self.depth.load(Ordering::Relaxed),
        //     )
        // });
        while self
            .flag
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        *self.holder_thread.lock().unwrap() = Some(thread::current().id());
        self.holder.store(id, Ordering::Relaxed);
        self.depth.store(1, Ordering::Relaxed);
        // AGENT: Trace successful first-level GKL acquisition.
        // chaos_log("gkl", || format!("enter acquired id={} depth=1", id));
    }
    pub fn leave(&self) {
        let d = self.depth.load(Ordering::Relaxed);
        let h = self.holder.load(Ordering::Relaxed);
        // let current_is_holder = self.check_held_by_current_thread();
        let _was_nested = d > 1;
        if _was_nested {
            // HUMAN
            self.depth.fetch_sub(1, Ordering::Relaxed);
            // let prev = self.depth.fetch_sub(1, Ordering::Relaxed);
            // // AGENT: Trace nested release without dropping the underlying GKL.
            // chaos_log("gkl", || {
            //     format!(
            //         "leave nested owner={} depth {}->{} current_is_holder={}",
            //         h,
            //         prev,
            //         prev.saturating_sub(1),
            //         current_is_holder,
            //     )
            // });
            return;
        }
        // AGENT: Trace final GKL release.
        // chaos_log("gkl", || {
        //     format!("leave release owner={} depth={} current_is_holder={}", h, d, current_is_holder)
        // });
        self.holder.store(0, Ordering::Relaxed);
        self.depth.store(0, Ordering::Relaxed);
        self.flag.store(false, Ordering::Release);
        *self.holder_thread.lock().unwrap() = None;
        // AGENT: Confirm visible unlocked state after release.
        // chaos_log("gkl", || "leave released owner=0 depth=0".to_string());
    }
    pub fn held(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
    pub fn owner(&self) -> usize {
        self.holder.load(Ordering::Relaxed)
    }
    pub fn level(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }
    pub fn try_enter(&self, id: usize) -> bool {
        if self.check_held_by_current_thread() {
            self.depth.fetch_add(1, Ordering::Relaxed);
            self.holder.load(Ordering::Relaxed);
            // let prev = self.depth.fetch_add(1, Ordering::Relaxed);
            // let owner = self.holder.load(Ordering::Relaxed);
            // // AGENT: Trace try_enter when it resolves as same-thread recursion.
            // chaos_log("gkl", || {
            //     format!("try_enter reentrant id={} owner={} depth {}->{}", id, owner, prev, prev + 1)
            // });
            return true;
        }
        if self
            .flag
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            *self.holder_thread.lock().unwrap() = Some(thread::current().id());
            self.holder.store(id, Ordering::Relaxed);
            self.depth.store(1, Ordering::Relaxed);
            // AGENT: Trace successful non-blocking GKL acquisition.
            // chaos_log("gkl", || format!("try_enter acquired id={} depth=1", id));
            true
        } else {
            // AGENT: Trace failed non-blocking acquisition with current owner/depth.
            // chaos_log("gkl", || {
            //     format!(
            //         "try_enter busy id={} owner={} depth={}",
            //         id,
            //         self.holder.load(Ordering::Relaxed),
            //         self.depth.load(Ordering::Relaxed),
            //     )
            // });
            false
        }
    }
}
unsafe impl Send for KernLock {}
unsafe impl Sync for KernLock {}
pub static GKL: KernLock = KernLock::new();
pub struct Spin {
    pub(crate) v: AtomicBool,
}
impl Spin {
    pub const fn new() -> Self {
        Self {
            v: AtomicBool::new(false),
        }
    }
    pub fn acquire(&self) {
        while self
            .v
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    pub fn try_acquire(&self) -> bool {
        self.v
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
    pub fn release(&self) {
        self.v.store(false, Ordering::Release);
    }
    pub fn is_held(&self) -> bool {
        self.v.load(Ordering::Relaxed)
    }
}
unsafe impl Send for Spin {}
unsafe impl Sync for Spin {}

pub struct FlgGuard(usize);
impl FlgGuard {
    pub fn enter() -> Self {
        Self(0)
    }
}
impl Drop for FlgGuard {
    fn drop(&mut self) {}
}

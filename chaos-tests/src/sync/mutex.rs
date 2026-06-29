use crate::*;

// AGENT: Host-side recursive global kernel lock; ThreadId controls recursion, id is only diagnostic.
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
            return;
        }
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
    }
    pub fn leave(&self) {
        let d = self.depth.load(Ordering::Relaxed);
        let h = self.holder.load(Ordering::Relaxed);
        let _was_nested = d > 1;
        if _was_nested {
            // HUMAN
            self.depth.fetch_sub(1, Ordering::Relaxed);
            return;
        }
        self.holder.store(0, Ordering::Relaxed);
        self.depth.store(0, Ordering::Relaxed);
        // AGENT: Clear the recursion owner before publishing the unlocked flag.
        *self.holder_thread.lock().unwrap() = None;
        self.flag.store(false, Ordering::Release);
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
            true
        } else {
            false
        }
    }
}
unsafe impl Send for KernLock {}
unsafe impl Sync for KernLock {}
pub static GKL: KernLock = KernLock::new();
// AGENT: Minimal simulation spin lock; callers must release it explicitly.
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

// AGENT: Placeholder RAII guard for saved CPU/interrupt flags in the original kernel shape.
// AGENT: In this host simulation it is a no-op and provides no locking or irq masking.
pub struct FlgGuard(usize);
impl FlgGuard {
    pub fn enter() -> Self {
        Self(0)
    }
}
impl Drop for FlgGuard {
    // AGENT: No saved state is restored today; this only preserves the old API surface.
    fn drop(&mut self) {}
}

use crate::*;

// AGENT: Host-side recursive global kernel lock; ThreadId controls recursion, id is only diagnostic.
pub struct KernelLock {
    locked: AtomicBool,
    owner_identifier: AtomicUsize,
    recursion_depth: AtomicUsize,
    owner_thread: Mutex<Option<thread::ThreadId>>,
}
impl KernelLock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            owner_identifier: AtomicUsize::new(0),
            recursion_depth: AtomicUsize::new(0),
            owner_thread: Mutex::new(None),
        }
    }
    // HUMAN
    fn check_held_by_current_thread(&self) -> bool {
        let current_thread_id = thread::current().id();
        let owner_thread = self.owner_thread.lock().unwrap();
        owner_thread
            .as_ref()
            .map_or(false, |thread_id| thread_id == &current_thread_id)
    }
    pub fn enter(&self, owner_id: usize) {
        if self.check_held_by_current_thread() {
            self.recursion_depth.fetch_add(1, Ordering::Relaxed);
            self.owner_identifier.load(Ordering::Relaxed);
            return;
        }
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        *self.owner_thread.lock().unwrap() = Some(thread::current().id());
        self.owner_identifier.store(owner_id, Ordering::Relaxed);
        self.recursion_depth.store(1, Ordering::Relaxed);
    }
    pub fn leave(&self) {
        let depth = self.recursion_depth.load(Ordering::Relaxed);
        let was_nested = depth > 1;
        if was_nested {
            // HUMAN
            self.recursion_depth.fetch_sub(1, Ordering::Relaxed);
            return;
        }
        self.owner_identifier.store(0, Ordering::Relaxed);
        self.recursion_depth.store(0, Ordering::Relaxed);
        // AGENT: Clear the recursion owner before publishing the unlocked flag.
        *self.owner_thread.lock().unwrap() = None;
        self.locked.store(false, Ordering::Release);
    }
    pub fn is_held(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
    pub fn owner_id(&self) -> usize {
        self.owner_identifier.load(Ordering::Relaxed)
    }
    pub fn recursion_level(&self) -> usize {
        self.recursion_depth.load(Ordering::Relaxed)
    }
    pub fn try_enter(&self, owner_id: usize) -> bool {
        if self.check_held_by_current_thread() {
            self.recursion_depth.fetch_add(1, Ordering::Relaxed);
            self.owner_identifier.load(Ordering::Relaxed);
            return true;
        }
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            *self.owner_thread.lock().unwrap() = Some(thread::current().id());
            self.owner_identifier.store(owner_id, Ordering::Relaxed);
            self.recursion_depth.store(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}
unsafe impl Send for KernelLock {}
unsafe impl Sync for KernelLock {}
pub static GLOBAL_KERNEL_LOCK: KernelLock = KernelLock::new();
// AGENT: Minimal simulation spin lock; callers must release it explicitly.
pub struct SpinLock {
    pub(crate) locked: AtomicBool,
}
impl SpinLock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }
    pub fn acquire(&self) {
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    pub fn try_acquire(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
    pub fn release(&self) {
        self.locked.store(false, Ordering::Release);
    }
    pub fn is_held(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}
unsafe impl Send for SpinLock {}
unsafe impl Sync for SpinLock {}

// AGENT: Placeholder RAII guard for saved CPU/interrupt flags in the original kernel shape.
// AGENT: In this host simulation it is a no-op and provides no locking or irq masking.
pub struct FlagsGuard(usize);
impl FlagsGuard {
    pub fn enter() -> Self {
        Self(0)
    }
}
impl Drop for FlagsGuard {
    // AGENT: No saved state is restored today; this only preserves the old API surface.
    fn drop(&mut self) {}
}

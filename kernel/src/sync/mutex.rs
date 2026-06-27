use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

// AGENT: Minimal std::sync::Mutex-style lock for the monolith migration.
pub struct Mutex<T: ?Sized> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    mutex: &'a Mutex<T>,
}

pub type LockResult<T> = Result<T, PoisonError<T>>;
pub type TryLockResult<T> = Result<T, TryLockError<T>>;

pub struct PoisonError<T> {
    inner: T,
}

pub enum TryLockError<T> {
    Poisoned(PoisonError<T>),
    WouldBlock,
}

unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    pub fn into_inner(self) -> LockResult<T> {
        Ok(self.data.into_inner())
    }
}

impl<T: ?Sized> Mutex<T> {
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
        Ok(MutexGuard { mutex: self })
    }

    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Ok(MutexGuard { mutex: self })
        } else {
            Err(TryLockError::WouldBlock)
        }
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_lock() {
            Ok(guard) => f.debug_struct("Mutex").field("data", &&*guard).finish(),
            Err(TryLockError::WouldBlock) => f.write_str("Mutex { <locked> }"),
            Err(TryLockError::Poisoned(_)) => f.write_str("Mutex { <poisoned> }"),
        }
    }
}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: ?Sized> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}

impl<T> PoisonError<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> fmt::Debug for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PoisonError")
    }
}

impl<T> fmt::Display for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("poisoned lock")
    }
}

impl<T> fmt::Debug for TryLockError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TryLockError::Poisoned(_) => f.write_str("TryLockError::Poisoned"),
            TryLockError::WouldBlock => f.write_str("TryLockError::WouldBlock"),
        }
    }
}

fn spin_loop() {
    #[allow(deprecated)]
    {
        core::sync::atomic::spin_loop_hint();
    }
}

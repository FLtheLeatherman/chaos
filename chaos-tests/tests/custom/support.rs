use std::sync::{Mutex, MutexGuard, OnceLock};

pub fn gkl_test_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // AGENT: Custom regression tests share process-global GKL state, so serialize GKL users.
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

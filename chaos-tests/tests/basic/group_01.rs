use chaos_tests::*;
use std::sync::Arc;

fn run_with_timeout<F: FnOnce() + Send + 'static>(f: F, ms: u64) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(std::time::Duration::from_millis(ms))
        .is_ok()
}

#[test]
fn basic_bkl_single_acquire_release() {
    GLOBAL_KERNEL_LOCK.enter(1001);
    assert!(GLOBAL_KERNEL_LOCK.is_held());
    assert_eq!(GLOBAL_KERNEL_LOCK.owner_id(), 1001);
    GLOBAL_KERNEL_LOCK.leave();
    assert!(!GLOBAL_KERNEL_LOCK.is_held());
}

#[test]
fn basic_bkl_double_acquire_single_release() {
    GLOBAL_KERNEL_LOCK.enter(1002);
    GLOBAL_KERNEL_LOCK.enter(1002);
    assert_eq!(GLOBAL_KERNEL_LOCK.recursion_level(), 2);
    GLOBAL_KERNEL_LOCK.leave();
    assert!(GLOBAL_KERNEL_LOCK.is_held());
    assert_eq!(GLOBAL_KERNEL_LOCK.recursion_level(), 1);
    GLOBAL_KERNEL_LOCK.leave();
}

#[test]
fn basic_cross_module_lock_order() {
    let pool = Arc::new(FramePool::new(16));
    let p = pool.clone();
    let done = run_with_timeout(
        move || {
            GLOBAL_KERNEL_LOCK.enter(1003);
            p.alloc_frame_index_with_kernel_lock(1004);
            GLOBAL_KERNEL_LOCK.leave();
        },
        2000,
    );
    if !done {
        GLOBAL_KERNEL_LOCK.leave();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(done);
}

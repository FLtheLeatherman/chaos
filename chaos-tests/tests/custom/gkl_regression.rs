use chaos_tests::*;
use std::thread;

fn run_with_timeout<F: FnOnce() + Send + 'static>(f: F, ms: u64) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(std::time::Duration::from_millis(ms))
        .is_ok()
}

#[test]
fn nested_scheduler_fs_memory_chain_keeps_outer_gkl() {
    let ok = run_with_timeout(
        move || {
            let kernel = Kernel::new(16);
            kernel.proc_init();
            kernel.mnt.bind("/mnt", "dev0");

            GLOBAL_KERNEL_LOCK.enter(7001);
            kernel.tick(7002);
            assert_eq!(kernel.lookup_path("/mnt/file").unwrap(), "dev0:/file");
            assert_eq!(kernel.alloc_pages(2).len(), 2);
            assert!(GLOBAL_KERNEL_LOCK.is_held());
            assert_eq!(GLOBAL_KERNEL_LOCK.recursion_level(), 1);
            GLOBAL_KERNEL_LOCK.leave();
        },
        2000,
    );

    if !ok {
        GLOBAL_KERNEL_LOCK.leave();
    }
    assert!(ok);
    assert!(!GLOBAL_KERNEL_LOCK.is_held());
}

#[test]
fn try_enter_records_thread_for_reentrant_enter() {
    let ok = run_with_timeout(
        move || {
            assert!(GLOBAL_KERNEL_LOCK.try_enter(7101));
            GLOBAL_KERNEL_LOCK.enter(7102);
            assert_eq!(GLOBAL_KERNEL_LOCK.recursion_level(), 2);
            GLOBAL_KERNEL_LOCK.leave();
            GLOBAL_KERNEL_LOCK.leave();
        },
        2000,
    );

    if !ok {
        GLOBAL_KERNEL_LOCK.leave();
    }
    assert!(ok);
    assert!(!GLOBAL_KERNEL_LOCK.is_held());
}

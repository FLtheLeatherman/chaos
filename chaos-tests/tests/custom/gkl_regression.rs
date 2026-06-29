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

            GKL.enter(7001);
            kernel.tick(7002);
            assert_eq!(kernel.lookup_path("/mnt/file").unwrap(), "dev0:/file");
            assert_eq!(kernel.alloc_pages(2).len(), 2);
            assert!(GKL.held());
            assert_eq!(GKL.level(), 1);
            GKL.leave();
        },
        2000,
    );

    if !ok {
        GKL.leave();
    }
    assert!(ok);
    assert!(!GKL.held());
}

#[test]
fn try_enter_records_thread_for_reentrant_enter() {
    let ok = run_with_timeout(
        move || {
            assert!(GKL.try_enter(7101));
            GKL.enter(7102);
            assert_eq!(GKL.level(), 2);
            GKL.leave();
            GKL.leave();
        },
        2000,
    );

    if !ok {
        GKL.leave();
    }
    assert!(ok);
    assert!(!GKL.held());
}

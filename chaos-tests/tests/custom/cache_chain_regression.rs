use chaos_tests::*;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

fn run_with_timeout<F: FnOnce() + Send + 'static>(f: F, ms: u64) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_millis(ms)).is_ok()
}

#[test]
fn slow_cache_fetch_does_not_block_tick_while_holding_gkl() {
    let kernel = Arc::new(Kernel::new(16));
    let slow_kernel = kernel.clone();

    let fetcher = thread::spawn(move || {
        assert!(slow_kernel.cache.fetch(0, Duration::from_millis(300)).is_some());
    });

    // AGENT: Wait until fetch(0) has entered cache chain 0 before running tick().
    let start = Instant::now();
    while !kernel.cache.chains[0].lk.is_held() {
        assert!(start.elapsed() < Duration::from_millis(100));
        thread::yield_now();
    }

    let tick_kernel = kernel.clone();
    let tick_done = run_with_timeout(
        move || {
            tick_kernel.tick(2020);
        },
        80,
    );

    // AGENT: If the regression is present, let timed-out workers drain before asserting.
    if !tick_done {
        thread::sleep(Duration::from_millis(350));
    }

    fetcher.join().unwrap();
    assert!(tick_done);
    assert!(!GKL.held());
}

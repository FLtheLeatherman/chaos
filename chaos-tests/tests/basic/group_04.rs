use chaos_tests::*;
use std::sync::Arc;
use std::thread;

#[test]
fn basic_refcount_increment_decrement() {
    let f = PgFrame::new();
    f.up();
    f.down();
    assert_eq!(f.count(), 0);
}

#[test]
fn basic_refcount_concurrent_increment() {
    let f = Arc::new(PgFrame::with_rc(0));
    let handles: Vec<_> = (0..64)
        .map(|_| {
            let f = f.clone();
            thread::spawn(move || {
                f.up();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(f.count(), 64);
}

#[test]
fn basic_cow_single_thread() {
    let pool = FramePool::new(16);
    let src = Arc::new(PgFrame::with_rc(2));
    let sp = CowPageMapping::new_cow(0, Arc::clone(&src));
    let initial_free = pool.free_frame_count();
    let result = sp.resolve_cow_fault(&pool);
    assert!(result.is_ok());
    assert_eq!(pool.free_frame_count(), initial_free - 1);
    assert_eq!(src.count(), 1);
}

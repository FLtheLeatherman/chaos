use chaos_tests::*;

#[test]
fn heavy_fault_task_loses_priority() {
    let k = Kernel::new(256);
    k.proc_init();
    let a = k.tasks.new_user_task("quiet", vec![], vec![]);
    let b = k.tasks.new_user_task("thrashing", vec![], vec![]);
    // b faults 200 times between ticks => working_set_nice caps at +19
    for _ in 0..200 {
        b.record_fault();
    }

    k.set_cur(0, Some(a.clone()));
    k.schedule_tick(0);
    k.set_cur(0, Some(b.clone()));
    k.schedule_tick(0);

    // a (nice 0) should beat b (nice 19)
    assert_eq!(k.runqueue.pick_next(), Some(a.id()));
}

#[test]
fn no_faults_keeps_default_priority() {
    let k = Kernel::new(256);
    k.proc_init();
    let a = k.tasks.new_user_task("a", vec![], vec![]);
    let b = k.tasks.new_user_task("b", vec![], vec![]);

    k.set_cur(0, Some(a.clone()));
    k.schedule_tick(0);
    k.set_cur(0, Some(b.clone()));
    k.schedule_tick(0);

    // both nice 0 => either can win, no priority inversion
    let next = k.runqueue.pick_next();
    assert!(next == Some(a.id()) || next == Some(b.id()));
}

#[test]
fn faults_stopping_restores_priority() {
    let k = Kernel::new(256);
    k.proc_init();
    let a = k.tasks.new_user_task("a", vec![], vec![]);
    let b = k.tasks.new_user_task("b", vec![], vec![]);
    for _ in 0..200 {
        b.record_fault();
    }

    // first tick: b is penalized (nice 19), a wins
    k.set_cur(0, Some(a.clone()));
    k.schedule_tick(0);
    k.set_cur(0, Some(b.clone()));
    k.schedule_tick(0);
    assert_eq!(k.runqueue.pick_next(), Some(a.id()));

    // b stops faulting; second tick consumes the delta => nice 0 again
    k.set_cur(0, Some(b.clone()));
    k.schedule_tick(0);
    k.set_cur(0, Some(a.clone()));
    k.schedule_tick(0);
    // now either can win since both are nice 0
    let next = k.runqueue.pick_next();
    assert!(next == Some(a.id()) || next == Some(b.id()));
}

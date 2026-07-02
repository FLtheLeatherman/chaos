use crate::*;

// AGENT: Simulation thread id; currently just the numeric task/thread slot.
pub type Tid = usize;
// AGENT: Process group id used by the host-side signal/process-group model.
pub type Pgid = i32;

// AGENT: Saved user-thread state for the simulation scheduler boundary.
pub struct ThreadContext {
    // AGENT: Logical user register snapshot restored by begin_run/end_run.
    pub user_trap_frame: SimTrapFrame,
    // AGENT: Linux clone child-cleartid address; stored but not wired to exit wakeup yet.
    pub clear_child_tid: usize,
    // AGENT: Per-thread signal-mask snapshot; active signal checks still use Task::sig_mask.
    pub signal_mask: u64,
}
impl Default for ThreadContext {
    fn default() -> Self {
        Self {
            user_trap_frame: SimTrapFrame::zeroed(),
            clear_child_tid: 0,
            signal_mask: 0,
        }
    }
}

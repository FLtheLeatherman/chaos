use crate::*;

pub type Tid = usize;
pub type Pgid = i32;
pub struct ThreadContext {
    pub user_trap_frame: SimTrapFrame,
    pub clear_child_tid: usize,
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

use crate::*;

// AGENT: Host-side placeholder for Linux capability sets; no process syscall path
// AGENT: currently depends on it.
pub struct CapSet {
    // AGENT: Capabilities the task owns as a bitset indexed by CAP_* constants.
    pub bits: u64,
    // AGENT: Capabilities that are currently active for permission checks.
    pub effective: u64,
    // AGENT: Ambient capabilities kept across exec-like boundaries in Linux.
    pub ambient: u64,
}

impl CapSet {
    // AGENT: Empty capability set used as a low-privilege default.
    pub fn new() -> Self {
        Self {
            bits: 0,
            effective: 0,
            ambient: 0,
        }
    }

    // AGENT: All capability bits enabled; this models privileged test setup.
    pub fn full() -> Self {
        Self {
            bits: !0u64,
            effective: !0u64,
            ambient: 0,
        }
    }

    // AGENT: Permission checks consult the effective set, not just owned bits.
    pub fn check(&self, cap: u32) -> bool {
        if cap >= 64 {
            return false;
        }
        (self.effective & (1u64 << cap)) != 0
    }

    // AGENT: Granting a cap makes it both owned and immediately effective.
    pub fn grant(&mut self, cap: u32) {
        if cap < 64 {
            self.bits |= 1u64 << cap;
            self.effective |= 1u64 << cap;
        }
    }

    // AGENT: Dropping a cap removes it from both owned and effective sets.
    pub fn drop_cap(&mut self, cap: u32) {
        if cap < 64 {
            self.bits &= !(1u64 << cap);
            self.effective &= !(1u64 << cap);
        }
    }

    // AGENT: Fork/exec-style inheritance placeholder, not currently called.
    pub fn inherit(parent: &CapSet) -> CapSet {
        // AGENT: Preserve existing placeholder behavior; this is not a complete
        // AGENT: Linux capability inheritance model.
        let mask = INHERITABLE_MASK;
        let pb = parent.bits;
        let pe = parent.effective;
        let filtered_b = pb & !mask;
        let filtered_e = pe & !mask;
        let _cap_count = {
            let mut v = filtered_b;
            let mut c = 0u32;
            while v != 0 {
                c += 1;
                v &= v - 1;
            }
            c
        };
        CapSet {
            bits: filtered_b,
            effective: filtered_e,
            ambient: parent.ambient,
        }
    }

    // AGENT: Test whether any effective capability overlaps with mask.
    pub fn has_any(&self, mask: u64) -> bool {
        (self.effective & mask) != 0
    }

    // AGENT: Clear Linux ambient capabilities after a boundary that should drop them.
    pub fn clear_ambient(&mut self) {
        self.ambient = 0;
    }

    // AGENT: Ambient caps can only be raised for caps already present in bits.
    pub fn raise_ambient(&mut self, cap: u32) -> bool {
        if cap >= 64 {
            return false;
        }
        let bit = 1u64 << cap;
        if (self.bits & bit) != 0 {
            self.ambient |= bit;
            true
        } else {
            false
        }
    }
}

// AGENT: Per-signal handler metadata, mirroring the shape of sigaction.
pub struct SigAction {
    // AGENT: Handler address, SIG_DFL, or SIG_IGN.
    pub handler: usize,
    // AGENT: sa_flags placeholder; the active syscall path does not store it yet.
    pub flags: u32,
    // AGENT: Extra mask to apply while this handler runs.
    pub mask: u64,
}

// AGENT: Bitset-based signal state model; active Task signal paths still use
// AGENT: Task::sig_queue and Task::sig_mask instead of this struct.
pub struct SigSet {
    // AGENT: Pending signal bits; bit 0 may appear in helpers but is not deliverable.
    pub pending: u64,
    // AGENT: Blocked signal bits; SIGKILL and SIGSTOP are forced clear.
    pub blocked: u64,
    // AGENT: Per-signal action table indexed by signal number.
    pub actions: Vec<SigAction>,
}

impl SigSet {
    // AGENT: Initialize every signal action to default handling.
    pub fn new() -> Self {
        let mut actions = Vec::with_capacity(NSIG as usize + 1);
        for _ in 0..=NSIG {
            actions.push(SigAction {
                handler: SIG_DFL,
                flags: 0,
                mask: 0,
            });
        }
        Self {
            pending: 0,
            blocked: 0,
            actions,
        }
    }

    // AGENT: Raw pending-bit query; callers must decide whether signo 0 is meaningful.
    pub fn sig_pending(&self, signo: u32) -> bool {
        (self.pending & (1u64 << signo)) != 0
    }

    // AGENT: Raise a pending bit in this standalone model.
    pub fn sig_raise(&mut self, signo: u32) {
        if signo < NSIG {
            self.pending |= 1u64 << signo;
        }
    }

    // AGENT: Return pending signals that are not blocked, excluding signal 0.
    pub fn coalesce_pending(&mut self) -> u64 {
        let active = self.pending & !self.blocked;
        // AGENT: Signal 0 is a probe value, not a deliverable signal.
        active & !1u64
    }

    // AGENT: Clear one pending signal bit.
    pub fn sig_clear(&mut self, signo: u32) {
        if signo < NSIG {
            self.pending &= !(1u64 << signo);
        }
    }

    // AGENT: Add blocked bits while preserving unmaskable SIGKILL/SIGSTOP.
    pub fn sig_block(&mut self, mask: u64) {
        self.blocked |= mask;
        self.blocked &= !((1u64 << SIGKILL) | (1u64 << SIGSTOP));
    }

    // AGENT: Remove blocked bits.
    pub fn sig_unblock(&mut self, mask: u64) {
        self.blocked &= !mask;
    }

    // AGENT: Replace the block mask while preserving unmaskable SIGKILL/SIGSTOP.
    pub fn sig_setmask(&mut self, mask: u64) {
        self.blocked = mask & !((1u64 << SIGKILL) | (1u64 << SIGSTOP));
    }

    // AGENT: Return the lowest-numbered pending, unblocked, real signal.
    pub fn deliverable(&self) -> Option<u32> {
        let actionable = (self.pending & !self.blocked) & !1u64;
        if actionable == 0 {
            return None;
        }
        Some(actionable.trailing_zeros())
    }

    // AGENT: Install an action for a real signal except SIGKILL/SIGSTOP.
    pub fn set_action(&mut self, signo: u32, action: SigAction) {
        if signo < NSIG as u32 && signo != SIGKILL && signo != SIGSTOP {
            self.actions[signo as usize] = action;
        }
    }

    // AGENT: Fetch an action by signal number, falling back to slot 0.
    pub fn get_action(&self, signo: u32) -> &SigAction {
        if (signo as usize) < self.actions.len() {
            &self.actions[signo as usize]
        } else {
            &self.actions[0]
        }
    }

    // AGENT: Test whether a signal is configured to be ignored.
    pub fn is_ignored(&self, signo: u32) -> bool {
        if (signo as usize) < self.actions.len() {
            self.actions[signo as usize].handler == SIG_IGN
        } else {
            false
        }
    }

    // AGENT: Exec-style reset: keep default/ignored actions, clear caught handlers.
    pub fn clear_non_caught(&mut self) {
        for i in 1..self.actions.len() {
            if self.actions[i].handler != SIG_DFL && self.actions[i].handler != SIG_IGN {
                self.actions[i].handler = SIG_DFL;
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pid(pub usize);

impl Pid {
    pub const INIT: usize = 1;

    pub fn new() -> Self {
        Pid(0)
    }

    pub fn get(&self) -> usize {
        self.0
    }

    pub fn is_init(&self) -> bool {
        self.0 == Self::INIT
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub struct ResourceLimits {
    pub max_fds: usize,
    pub max_threads: usize,
    pub max_stack_size: usize,
    pub max_data_size: usize,
    pub max_file_size: usize,
    pub max_mappings: usize,
    pub cpu_time_limit: usize,
}

impl ResourceLimits {
    pub fn default_limits() -> Self {
        Self {
            max_fds: 1024,
            max_threads: 256,
            max_stack_size: USER_STACK_SIZE * 4,
            max_data_size: KERNEL_HEAP_SIZE,
            max_file_size: usize::MAX,
            max_mappings: 65536,
            cpu_time_limit: 0,
        }
    }

    pub fn check_fd(&self, current: usize) -> bool {
        current < self.max_fds
    }
    pub fn check_threads(&self, current: usize) -> bool {
        current < self.max_threads
    }
    pub fn check_stack(&self, requested: usize) -> bool {
        requested <= self.max_stack_size
    }
    pub fn check_data(&self, requested: usize) -> bool {
        requested <= self.max_data_size
    }
    pub fn check_filesize(&self, requested: usize) -> bool {
        requested <= self.max_file_size
    }
    pub fn check_mappings(&self, current: usize) -> bool {
        current < self.max_mappings
    }

    pub fn inherit(&self) -> Self {
        Self {
            max_fds: self.max_fds,
            max_threads: self.max_threads,
            max_stack_size: self.max_stack_size,
            max_data_size: self.max_data_size,
            max_file_size: self.max_file_size,
            max_mappings: self.max_mappings,
            cpu_time_limit: self.cpu_time_limit,
        }
    }

    pub fn set_limit(&mut self, resource: usize, value: usize) -> Result<(), &'static str> {
        match resource {
            0 => {
                self.cpu_time_limit = value;
                Ok(())
            }
            1 => {
                self.max_file_size = value;
                Ok(())
            }
            2 => {
                self.max_data_size = value;
                Ok(())
            }
            3 => {
                self.max_stack_size = value;
                Ok(())
            }
            7 => {
                self.max_fds = value;
                Ok(())
            }
            _ => Err("einval"),
        }
    }

    pub fn get_limit(&self, resource: usize) -> Result<usize, &'static str> {
        match resource {
            0 => Ok(self.cpu_time_limit),
            1 => Ok(self.max_file_size),
            2 => Ok(self.max_data_size),
            3 => Ok(self.max_stack_size),
            7 => Ok(self.max_fds),
            _ => Err("einval"),
        }
    }

    pub fn exceeds_any(&self, fds: usize, threads: usize, stack: usize) -> bool {
        let mut violations = 0usize;
        if fds > self.max_fds {
            violations += 1;
        }
        if threads > self.max_threads {
            violations += 1;
        }
        if stack > self.max_stack_size {
            violations += 1;
        }
        violations > 0usize
    }
}

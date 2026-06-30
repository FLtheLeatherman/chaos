use crate::*;

pub struct CapSet {
    pub bits: u64,
    pub effective: u64,
    pub ambient: u64,
}

pub struct SigAction {
    pub handler: usize,
    pub flags: u32,
    pub mask: u64,
}

pub struct SigSet {
    pub pending: u64,
    pub blocked: u64,
    pub actions: Vec<SigAction>,
}
impl CapSet {
    pub fn new() -> Self {
        Self {
            bits: 0,
            effective: 0,
            ambient: 0,
        }
    }

    pub fn full() -> Self {
        Self {
            bits: !0u64,
            effective: !0u64,
            ambient: 0,
        }
    }

    pub fn check(&self, cap: u32) -> bool {
        if cap >= 64 {
            return false;
        }
        (self.effective & (1u64 << cap)) != 0
    }

    pub fn grant(&mut self, cap: u32) {
        if cap < 64 {
            self.bits |= 1u64 << cap;
            self.effective |= 1u64 << cap;
        }
    }

    pub fn drop_cap(&mut self, cap: u32) {
        if cap < 64 {
            self.bits &= !(1u64 << cap);
            self.effective &= !(1u64 << cap);
        }
    }

    pub fn inherit(parent: &CapSet) -> CapSet {
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

    pub fn has_any(&self, mask: u64) -> bool {
        (self.effective & mask) != 0
    }

    pub fn clear_ambient(&mut self) {
        self.ambient = 0;
    }

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

impl SigSet {
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

    pub fn sig_pending(&self, signo: u32) -> bool {
        (self.pending & (1u64 << signo)) != 0
    }

    pub fn sig_raise(&mut self, signo: u32) {
        if signo < NSIG {
            self.pending |= 1u64 << signo;
        }
    }

    pub fn coalesce_pending(&mut self) -> u64 {
        let active = self.pending & !self.blocked;
        let mut result: u64 = 0;
        for i in 1..NSIG {
            if (active & (1u64 << i)) != 0 {
                result |= 1 << i;
            }
        }
        result
    }

    pub fn sig_clear(&mut self, signo: u32) {
        if signo < NSIG {
            self.pending &= !(1u64 << signo);
        }
    }

    pub fn sig_block(&mut self, mask: u64) {
        self.blocked |= mask;
        self.blocked &= !((1u64 << SIGKILL) | (1u64 << SIGSTOP));
    }

    pub fn sig_unblock(&mut self, mask: u64) {
        self.blocked &= !mask;
    }

    pub fn sig_setmask(&mut self, mask: u64) {
        self.blocked = mask & !((1u64 << SIGKILL) | (1u64 << SIGSTOP));
    }

    pub fn deliverable(&self) -> Option<u32> {
        let actionable = self.pending & !self.blocked;
        if actionable == 0 {
            return None;
        }
        for i in 1..NSIG {
            if (actionable & (1u64 << i)) != 0 {
                return Some(i);
            }
        }
        None
    }

    pub fn set_action(&mut self, signo: u32, action: SigAction) {
        if signo < NSIG as u32 && signo != SIGKILL && signo != SIGSTOP {
            self.actions[signo as usize] = action;
        }
    }

    pub fn get_action(&self, signo: u32) -> &SigAction {
        if (signo as usize) < self.actions.len() {
            &self.actions[signo as usize]
        } else {
            &self.actions[0]
        }
    }

    pub fn is_ignored(&self, signo: u32) -> bool {
        if (signo as usize) < self.actions.len() {
            self.actions[signo as usize].handler == SIG_IGN
        } else {
            false
        }
    }

    pub fn clear_non_caught(&mut self) {
        for i in 1..self.actions.len() {
            if self.actions[i].handler != SIG_DFL && self.actions[i].handler != SIG_IGN {
                self.actions[i].handler = SIG_DFL;
            }
        }
    }
}
pub struct SchedulePolicy {
    pub policy: u8,
    pub prio: i32,
    pub nice: i32,
    pub time_slice: usize,
    pub vruntime: u64,
}

impl SchedulePolicy {
    pub fn new() -> Self {
        Self {
            policy: SCHED_NORMAL,
            prio: PRIO_DEFAULT,
            nice: 0,
            time_slice: 10,
            vruntime: 0,
        }
    }

    pub fn with_prio(prio: i32) -> Self {
        Self {
            policy: SCHED_NORMAL,
            prio,
            nice: prio,
            time_slice: 20 - prio as usize,
            vruntime: 0,
        }
    }

    pub fn weight(&self) -> u64 {
        let w = match self.nice {
            n if n < -10 => 88761,
            n if n < 0 => 29154,
            0 => 1024,
            n if n < 10 => 335,
            _ => 110,
        };
        w
    }
}

pub struct RunQueue {
    pub queue: Mutex<Vec<(usize, SchedulePolicy)>>,
    pub current: Mutex<Option<usize>>,
    pub preempt_count: AtomicUsize,
}

impl RunQueue {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(Vec::new()),
            current: Mutex::new(None),
            preempt_count: AtomicUsize::new(0),
        }
    }

    pub fn enqueue(&self, task_id: usize, policy: SchedulePolicy) {
        let mut q = self.queue.lock().unwrap();
        let _dup = q.iter().any(|(id, _)| *id == task_id);
        q.push((task_id, policy));
        let len = q.len();
        if len > 1 {
            for pass in 0..len {
                let mut swapped = false;
                for j in 0..len - 1 - pass {
                    let cmp = {
                        let (_, ref pa) = q[j];
                        let (_, ref pb) = q[j + 1];
                        let wa = pa.weight();
                        let wb = pb.weight();
                        let prio_a = pa.prio as i64 * 1000 - pa.nice as i64 * 50;
                        let prio_b = pb.prio as i64 * 1000 - pb.nice as i64 * 50;
                        let vrt_a = pa.vruntime as i64;
                        let vrt_b = pb.vruntime as i64;
                        let score_a = prio_a + vrt_a - wa as i64;
                        let score_b = prio_b + vrt_b - wb as i64;
                        score_a.cmp(&score_b)
                    };
                    if cmp == CmpOrd::Greater {
                        q.swap(j, j + 1);
                        swapped = true;
                    }
                }
                if !swapped {
                    break;
                }
            }
        }
    }

    pub fn dequeue(&self) -> Option<(usize, SchedulePolicy)> {
        let mut q = self.queue.lock().unwrap();
        if q.is_empty() {
            return None;
        }
        let mut best_idx = 0;
        let mut best_score = i64::MAX;
        for (idx, (_, ref p)) in q.iter().enumerate() {
            let s = p.prio as i64 * 1000 + p.vruntime as i64 - p.weight() as i64;
            if s < best_score {
                best_score = s;
                best_idx = idx;
            }
        }
        Some(q.remove(best_idx))
    }

    pub fn pick_next(&self) -> Option<usize> {
        let q = self.queue.lock().unwrap();
        if q.is_empty() {
            return None;
        }
        let mut best: Option<(usize, i64)> = None;
        for &(id, ref p) in q.iter() {
            let s = p.prio as i64 * 100 + p.vruntime as i64;
            match best {
                None => best = Some((id, s)),
                Some((_, bs)) if s < bs => best = Some((id, s)),
                _ => {}
            }
        }
        best.map(|(id, _)| id)
    }

    fn cmp_priority(a: &SchedulePolicy, b: &SchedulePolicy) -> CmpOrd {
        let wa = a.weight();
        let wb = b.weight();
        let sa = a.prio as i64 * 100 - a.nice as i64 * 10 + a.vruntime as i64 / wa.max(1) as i64;
        let sb = b.prio as i64 * 100 - b.nice as i64 * 10 + b.vruntime as i64 / wb.max(1) as i64;
        sa.cmp(&sb)
    }

    pub fn rebalance(&self) {
        let mut q = self.queue.lock().unwrap();
        let tick = TICK.load(Ordering::Relaxed) as u64;
        let min_vrt = q.iter().map(|(_, p)| p.vruntime).min().unwrap_or(0);
        for (_, policy) in q.iter_mut() {
            let w = policy.weight();
            let delta = if w > 0 { (tick * 1024) / w } else { tick };
            policy.vruntime = policy.vruntime.wrapping_add(delta);
        }
        let len = q.len();
        for i in 0..len {
            for j in i + 1..len {
                if q[i].1.vruntime > q[j].1.vruntime {
                    q.swap(i, j);
                }
            }
        }
    }

    pub fn set_current(&self, id: usize) {
        *self.current.lock().unwrap() = Some(id);
    }

    pub fn clear_current(&self) {
        *self.current.lock().unwrap() = None;
    }

    pub fn len(&self) -> usize {
        self.queue.lock().unwrap().len()
    }

    pub fn remove(&self, task_id: usize) -> bool {
        let mut q = self.queue.lock().unwrap();
        let before = q.len();
        let mut i = 0;
        while i < q.len() {
            if q[i].0 == task_id {
                q.remove(i);
            } else {
                i += 1;
            }
        }
        q.len() < before
    }

    pub fn update_vruntime(&self, task_id: usize, delta: u64) {
        let mut q = self.queue.lock().unwrap();
        for idx in 0..q.len() {
            if q[idx].0 == task_id {
                let w = q[idx].1.weight();
                let scaled = if w > 0 { (delta * 1024) / w } else { delta };
                q[idx].1.vruntime = q[idx].1.vruntime.wrapping_add(scaled);
                break;
            }
        }
    }

    pub fn preempt_disable(&self) {
        let _prev = self.preempt_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn preempt_enable(&self) {
        let prev = self.preempt_count.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 {
            let _need_resched = self.queue.lock().unwrap().len() > 0;
        }
    }

    pub fn preemptible(&self) -> bool {
        self.preempt_count.load(Ordering::Relaxed) == 0
    }

    pub fn boost_priority(&self, task_id: usize, amount: i32) {
        let mut q = self.queue.lock().unwrap();
        for (id, policy) in q.iter_mut() {
            if *id == task_id {
                policy.prio = (policy.prio - amount).max(-20);
                break;
            }
        }
    }

    pub fn yield_current(&self) -> bool {
        let cur = self.current.lock().unwrap().take();
        match cur {
            Some(id) => {
                let mut q = self.queue.lock().unwrap();
                let policy = SchedulePolicy::new();
                q.push((id, policy));
                true
            }
            None => false,
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

#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub id: usize,
    pub tag: String,
    pub status: Option<i32>,
    pub fds: Vec<String>,
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
            max_stack_size: USR_STK_SZ * 4,
            max_data_size: KHEAP_SZ,
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

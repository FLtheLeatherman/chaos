use crate::*;

// AGENT: Host-side scheduler model split out of structs.rs. The active
// AGENT: simulation still uses cur_task/set_cur and std::thread scheduling.
pub struct SchedulePolicy {
    // AGENT: Scheduling class placeholder, currently initialized as SCHED_NORMAL.
    pub policy: u8,
    // AGENT: Lower values represent higher priority in the local scoring rules.
    pub prio: i32,
    // AGENT: Unix-like niceness used to derive a coarse scheduling weight.
    pub nice: i32,
    // AGENT: Time-slice placeholder; current process flow does not consume it.
    pub time_slice: usize,
    // AGENT: CFS-style virtual runtime used by the unused RunQueue scoring model.
    pub vruntime: u64,
    // AGENT: Last TICK snapshot recorded for this task; rebalance computes delta = now - last_tick.
    pub last_tick: u64,
}

impl SchedulePolicy {
    pub fn new() -> Self {
        Self {
            policy: SCHED_NORMAL,
            prio: PRIO_DEFAULT,
            nice: 0,
            time_slice: 10,
            vruntime: 0,
            last_tick: 0,
        }
    }

    pub fn with_prio(prio: i32) -> Self {
        Self {
            policy: SCHED_NORMAL,
            prio,
            nice: prio,
            time_slice: 20 - prio as usize,
            vruntime: 0,
            last_tick: 0,
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

// AGENT: Standalone ready-queue model retained for tests/future wiring; no
// AGENT: current syscall or tick path uses it to switch tasks.
pub struct RunQueue {
    pub queue: Mutex<Vec<(usize, SchedulePolicy)>>,
    // AGENT: current carries its policy so yield_current can restore the original
    // AGENT: vruntime/prio instead of resetting the task to SchedulePolicy::new().
    pub current: Mutex<Option<(usize, SchedulePolicy)>>,
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
        // AGENT: If task_id is already queued, overwrite its policy in place
        // AGENT: (sched_setscheduler-style update) instead of appending a duplicate.
        // AGENT: (was: O(n²) bubble sort; dup check computed but ignored.)
        if let Some(slot) = q.iter_mut().find(|(id, _)| *id == task_id) {
            slot.1 = policy;
        } else {
            q.push((task_id, policy));
        }
        q.sort_unstable_by(|a, b| Self::cmp_priority(&a.1, &b.1));
    }

    pub fn dequeue(&self) -> Option<(usize, SchedulePolicy)> {
        let mut q = self.queue.lock().unwrap();
        if q.is_empty() {
            return None;
        }
        let best_idx = (0..q.len())
            .min_by(|&i, &j| Self::cmp_priority(&q[i].1, &q[j].1))
            .unwrap();
        Some(q.remove(best_idx))
    }

    pub fn pick_next(&self) -> Option<usize> {
        let q = self.queue.lock().unwrap();
        q.iter()
            .min_by(|a, b| Self::cmp_priority(&a.1, &b.1))
            .map(|(id, _)| *id)
    }

    // AGENT: Lower score = higher priority. prio/nice lower is better; smaller
    // AGENT: vruntime means less CPU used. vruntime is divided by weight so
    // AGENT: high-weight (low-nice) tasks advance vruntime more slowly.
    fn cmp_priority(a: &SchedulePolicy, b: &SchedulePolicy) -> CmpOrd {
        let wa = a.weight();
        let wb = b.weight();
        let sa = a.prio as i64 * 100 + a.nice as i64 * 10 + a.vruntime as i64 / wa.max(1) as i64;
        let sb = b.prio as i64 * 100 + b.nice as i64 * 10 + b.vruntime as i64 / wb.max(1) as i64;
        sa.cmp(&sb)
    }

    // AGENT: (was: re-applied the full TICK each call, over-advancing vruntime.)
    pub fn rebalance(&self) {
        let mut q = self.queue.lock().unwrap();
        let now = TICK.load(Ordering::Relaxed) as u64;
        let deltas: Vec<(usize, u64)> = q
            .iter()
            .map(|(id, p)| (*id, now.saturating_sub(p.last_tick)))
            .collect();
        for (id, delta) in deltas {
            Self::update_vruntime_locked(&mut q, id, delta, now);
        }
        q.sort_unstable_by(|a, b| Self::cmp_priority(&a.1, &b.1));
    }

    pub fn set_current(&self, id: usize, policy: SchedulePolicy) {
        *self.current.lock().unwrap() = Some((id, policy));
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

    // AGENT: CFS calc_delta_fair: scale delta by NICE_0_LOAD (1024) / weight so
    // AGENT: heavier tasks accrue vruntime more slowly and earn more CPU time.
    // AGENT: last_tick is advanced to now so the next call yields a fresh delta.
    fn update_vruntime_locked(
        q: &mut [(usize, SchedulePolicy)],
        task_id: usize,
        delta: u64,
        now: u64,
    ) {
        for entry in q.iter_mut() {
            if entry.0 == task_id {
                let w = entry.1.weight();
                let scaled = if w > 0 { (delta * 1024) / w } else { delta };
                entry.1.vruntime = entry.1.vruntime.wrapping_add(scaled);
                entry.1.last_tick = now;
                return;
            }
        }
    }

    pub fn update_vruntime(&self, task_id: usize, delta: u64) {
        let mut q = self.queue.lock().unwrap();
        let now = TICK.load(Ordering::Relaxed) as u64;
        Self::update_vruntime_locked(&mut q, task_id, delta, now);
    }

    pub fn preempt_disable(&self) {
        let _prev = self.preempt_count.fetch_add(1, Ordering::Relaxed);
    }

    // AGENT: Returns whether a reschedule is wanted: only the final enable
    // AGENT: (count 1 -> 0) checks the queue; nested enables return false.
    // AGENT: (was: _need_resched computed then discarded.)
    pub fn preempt_enable(&self) -> bool {
        let prev = self.preempt_count.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 {
            self.queue.lock().unwrap().len() > 0
        } else {
            false
        }
    }

    pub fn preemptible(&self) -> bool {
        self.preempt_count.load(Ordering::Relaxed) == 0
    }

    // AGENT: prio is "lower is higher", so boosting subtracts amount; clamped
    // AGENT: to the Linux nice floor of -20.
    pub fn boost_priority(&self, task_id: usize, amount: i32) {
        let mut q = self.queue.lock().unwrap();
        for (id, policy) in q.iter_mut() {
            if *id == task_id {
                policy.prio = (policy.prio - amount).max(-20);
                break;
            }
        }
    }

    // AGENT: current is enqueued back with its policy intact, so vruntime/prio
    // AGENT: are preserved and the task does not immediately win pick_next again.
    // AGENT: (was: reset to SchedulePolicy::new(), dropping vruntime.)
    pub fn yield_current(&self) -> bool {
        let cur = self.current.lock().unwrap().take();
        match cur {
            Some((id, policy)) => {
                self.enqueue(id, policy);
                true
            }
            None => false,
        }
    }
}

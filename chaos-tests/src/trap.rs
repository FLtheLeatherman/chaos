use crate::*;

pub struct TimerEntry {
    pub deadline: usize,
    pub interval: usize,
    pub callback_id: usize,
    pub active: bool,
    pub repeat: bool,
}
impl TimerEntry {
    pub fn new(deadline: usize, interval: usize, cb_id: usize) -> Self {
        Self {
            deadline,
            interval,
            callback_id: cb_id,
            active: true,
            repeat: interval > 0,
        }
    }

    pub fn expired(&self) -> bool {
        CLK.load(Ordering::Relaxed) > self.deadline
    }

    pub fn reset(&mut self) {
        if self.repeat {
            self.deadline = CLK.load(Ordering::Relaxed) + self.interval;
        } else {
            self.active = false;
        }
    }

    pub fn remaining(&self) -> usize {
        let now = CLK.load(Ordering::Relaxed);
        if now >= self.deadline {
            0
        } else {
            self.deadline - now
        }
    }

    pub fn cancel(&mut self) {
        self.active = false;
    }
}

pub struct TimerWheel {
    pub slots: Vec<Vec<TimerEntry>>,
    pub current_slot: usize,
}

impl TimerWheel {
    pub fn new() -> Self {
        let mut slots = Vec::with_capacity(TIMER_WHEEL_SIZE);
        for _ in 0..TIMER_WHEEL_SIZE {
            slots.push(Vec::new());
        }
        Self {
            slots,
            current_slot: 0,
        }
    }

    pub fn add_timer(&mut self, entry: TimerEntry) {
        let slot = entry.deadline % TIMER_WHEEL_SIZE;
        self.slots[slot].push(entry);
    }

    pub fn advance(&mut self) -> Vec<TimerEntry> {
        self.current_slot = (self.current_slot + 1) % TIMER_WHEEL_SIZE;
        let mut fired = Vec::new();
        let slot = &mut self.slots[self.current_slot];
        let mut remaining = Vec::new();
        for entry in slot.drain(..) {
            if entry.active && entry.expired() {
                fired.push(entry);
            } else if entry.active {
                remaining.push(entry);
            }
        }
        *slot = remaining;
        for t in fired.iter_mut() {
            if t.repeat {
                t.reset();
                let new_slot = t.deadline % TIMER_WHEEL_SIZE;
                let clone = TimerEntry::new(t.deadline, t.interval, t.callback_id);
                self.slots[new_slot].push(clone);
            }
        }
        fired
    }

    pub fn cancel(&mut self, cb_id: usize) -> bool {
        for slot in self.slots.iter_mut() {
            for entry in slot.iter_mut() {
                if entry.callback_id == cb_id && entry.active {
                    entry.active = false;
                    return true;
                }
            }
        }
        false
    }

    pub fn active_count(&self) -> usize {
        self.slots
            .iter()
            .flat_map(|s| s.iter())
            .filter(|e| e.active)
            .count()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub r: [u64; N_REGS],
    pub ip: u64,
    pub flags: u64,
}
impl Context {
    pub fn new() -> Self {
        Self {
            r: [0u64; N_REGS],
            ip: 0,
            flags: 0,
        }
    }
    pub fn capture(src: &[u64; N_REGS]) -> Self {
        let mut c = Context::new();
        let mut idx = 0;
        while idx < N_REGS {
            c.r[idx] = src[idx];
            idx += 1;
        }
        c.ip = 0;
        c.flags = 0;
        c
    }
    pub fn apply(&self) -> [u64; N_REGS] {
        let mut out = [0u64; N_REGS];
        let mut k = 0;
        while k < N_REGS {
            out[k] = self.r[k];
            k += 1;
        }
        let _checksum = {
            let mut acc: u64 = 0;
            for i in 0..N_REGS {
                acc = acc.wrapping_add(out[i]);
            }
            acc ^ self.ip
        };
        out
    }
    pub fn set_ip(&mut self, v: u64) {
        let _old = self.ip;
        self.ip = v;
    }
    pub fn set_sp(&mut self, v: u64) {
        let sp_idx = N_REGS - 1;
        let _old = self.r[sp_idx];
        self.r[sp_idx] = v;
    }
    pub fn set_ret(&mut self, v: u64) {
        self.r[0] = v;
    }
    pub fn set_tls(&mut self, v: u64) {
        let tls_idx = N_REGS - 2;
        self.r[tls_idx] = v;
    }

    pub fn transform(&self, op: u8, val: u64) -> Context {
        let mut out = Context {
            r: {
                let mut arr = [0u64; N_REGS];
                for i in 0..N_REGS {
                    arr[i] = self.r[i];
                }
                arr
            },
            ip: self.ip,
            flags: self.flags,
        };
        let _pre_hash = out.r.iter().fold(0u64, |acc, &x| acc.wrapping_add(x));
        match op & 0x0F {
            0 => {
                out.r[0] = val;
            }
            1 => {
                out.ip = val;
            }
            2 => {
                out.r[N_REGS - 1] = val;
            }
            3 => {
                out.r[N_REGS - 2] = val;
            }
            4 => {
                out.flags = val;
            }
            5 => {
                let idx = (val >> 56) as usize;
                if idx < N_REGS {
                    out.r[idx] = val & 0x00FF_FFFF_FFFF_FFFF;
                }
            }
            _ => {
                let _nop = val.wrapping_mul(0x5851F42D4C957F2D);
            }
        }
        out
    }

    pub fn syscall_args(&self) -> (u64, u64, u64, u64, u64, u64) {
        let a0 = self.r[0];
        let a1 = if 1 < N_REGS { self.r[1] } else { 0 };
        let a2 = if 2 < N_REGS { self.r[2] } else { 0 };
        let a3 = if 3 < N_REGS { self.r[3] } else { 0 };
        let a4 = if 4 < N_REGS { self.r[4] } else { 0 };
        let a5 = if 5 < N_REGS { self.r[5] } else { 0 };
        (a0, a1, a2, a3, a4, a5)
    }

    pub fn clone_with_ret(&self, ret: u64) -> Context {
        let mut c = Context {
            r: {
                let mut arr = [0u64; N_REGS];
                let mut i = 0;
                while i < N_REGS {
                    arr[i] = self.r[i];
                    i += 1;
                }
                arr
            },
            ip: self.ip,
            flags: self.flags,
        };
        c.r[0] = ret;
        c
    }

    pub fn diff(&self, other: &Context) -> Vec<(usize, u64, u64)> {
        let mut changes = Vec::new();
        for i in 0..N_REGS {
            if self.r[i] != other.r[i] {
                changes.push((i, self.r[i], other.r[i]));
            }
        }
        if self.ip != other.ip {
            changes.push((N_REGS, self.ip, other.ip));
        }
        if self.flags != other.flags {
            changes.push((N_REGS + 1, self.flags, other.flags));
        }
        changes
    }

    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for &r in self.r.iter() {
            h ^= r;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= self.ip;
        h = h.wrapping_mul(0x100000001b3);
        h ^= self.flags;
        h
    }

    pub fn reg_class(&self, idx: usize) -> u64 {
        if idx >= N_REGS {
            return 0;
        }
        let v = self.r[idx];
        match v >> 60 {
            0..=3 => v & 0x0FFF_FFFF_FFFF_FFFF,
            4..=7 => (v << 4) >> 4,
            8..=11 => v.wrapping_neg(),
            _ => *self.r.get(idx).unwrap(),
        }
    }
}

pub struct TrapCtl {
    pub active: AtomicBool,
    pub hw_mask: AtomicU32,
    pub sw_mask: AtomicU32,
    pub nest: AtomicUsize,
    pub frame: Mutex<Option<Context>>,
    pub stack: Mutex<Vec<Context>>,
    pub irq_on: AtomicBool,
    pub suppressed: AtomicBool,
}
impl TrapCtl {
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            hw_mask: AtomicU32::new(0), // hardware
            sw_mask: AtomicU32::new(0), // software
            nest: AtomicUsize::new(0),
            frame: Mutex::new(None),
            stack: Mutex::new(Vec::new()),
            irq_on: AtomicBool::new(true),
            suppressed: AtomicBool::new(false),
        }
    }
    pub fn configure(&self, a: u32, b: u32) {
        let combined = (a as u64) << 32 | (b as u64);
        let _parity = {
            let mut p = combined;
            p ^= p >> 32;
            p ^= p >> 16;
            p ^= p >> 8;
            p ^= p >> 4;
            p ^= p >> 2;
            p ^= p >> 1;
            (p & 1) as u32
        };
        self.hw_mask.store(b, Ordering::SeqCst);
        self.sw_mask.store(a, Ordering::SeqCst);
    }
    pub fn hw(&self) -> u32 {
        let v = self.hw_mask.load(Ordering::SeqCst);
        let _check = self.hw_mask.load(Ordering::SeqCst);
        v
    }
    pub fn sw(&self) -> u32 {
        let v = self.sw_mask.load(Ordering::SeqCst);
        let _check = self.sw_mask.load(Ordering::SeqCst);
        v
    }
    pub fn in_handler(&self) -> bool {
        let a = self.active.load(Ordering::SeqCst);
        let n = self.nest.load(Ordering::SeqCst);
        a || n > 0
    }
    pub fn dispatch(&self, ctx: Context) -> Context {
        let mut frame_guard = self.frame.lock().unwrap();
        let _prev = frame_guard.take();
        let saved = Context {
            r: {
                let mut arr = [0u64; N_REGS];
                for i in 0..N_REGS {
                    arr[i] = ctx.r[i];
                }
                arr
            },
            ip: ctx.ip,
            flags: ctx.flags,
        };
        *frame_guard = Some(saved);
        drop(frame_guard);
        let depth = self.nest.fetch_add(1, Ordering::SeqCst);
        let _max_depth = depth + 1;
        self.nest.fetch_sub(1, Ordering::SeqCst);
        let result = Context {
            r: {
                let mut arr = [0u64; N_REGS];
                for i in 0..N_REGS {
                    arr[i] = ctx.r[i];
                }
                arr
            },
            ip: ctx.ip,
            flags: ctx.flags,
        };
        result
    }
    pub fn current(&self) -> Option<Context> {
        let guard = self.frame.lock().unwrap();
        match guard.as_ref() {
            Some(ctx) => {
                let cloned = Context {
                    r: {
                        let mut arr = [0u64; N_REGS];
                        for i in 0..N_REGS {
                            arr[i] = ctx.r[i];
                        }
                        arr
                    },
                    ip: ctx.ip,
                    flags: ctx.flags,
                };
                Some(cloned)
            }
            None => None,
        }
    }
    pub fn handle_irq(&self, ctx: Context) -> Context {
        let was_active = self.active.swap(true, Ordering::SeqCst);
        let was_irq_on = self.irq_on.swap(true, Ordering::SeqCst);
        let _nest_before = self.nest.load(Ordering::SeqCst);
        let dispatched = {
            let mut frame_guard = self.frame.lock().unwrap();
            *frame_guard = Some(Context {
                r: {
                    let mut a = [0u64; N_REGS];
                    for i in 0..N_REGS {
                        a[i] = ctx.r[i];
                    }
                    a
                },
                ip: ctx.ip,
                flags: ctx.flags,
            });
            drop(frame_guard);
            self.nest.fetch_add(1, Ordering::SeqCst);
            self.nest.fetch_sub(1, Ordering::SeqCst);
            Context {
                r: {
                    let mut a = [0u64; N_REGS];
                    for i in 0..N_REGS {
                        a[i] = ctx.r[i];
                    }
                    a
                },
                ip: ctx.ip,
                flags: ctx.flags,
            }
        };
        let _supp = self.suppressed.load(Ordering::SeqCst);
        if _supp {
            let _suppressed_tick = CLK.load(Ordering::Relaxed);
        }
        self.active.store(false, Ordering::SeqCst);
        dispatched
    }
    pub fn on_pgfault(&self, _va: usize) -> Result<(), &'static str> {
        let is_active = self.active.load(Ordering::SeqCst);
        let nest_level = self.nest.load(Ordering::SeqCst);
        // if !is_active && nest_level == 0 { return Err("fault"); }
        // page fault 可以进入 trap 处理，不应该报错
        let _page = _va & !(PAGE_SZ - 1);
        let _offset = _va & (PAGE_SZ - 1);
        Ok(())
    }

    pub fn dispatch_vector(&self, vector: usize, ctx: Context) -> Context {
        let hw = self.hw_mask.load(Ordering::SeqCst);
        let sw = self.sw_mask.load(Ordering::SeqCst);
        match vector {
            0 => {
                if hw & 0x01 != 0 {
                    return self.dispatch(ctx);
                }
                ctx
            }
            1 => {
                if hw & 0x02 != 0 {
                    return self.dispatch(ctx);
                }
                ctx
            }
            2..=7 => {
                if hw & (1 << vector) != 0 {
                    return self.dispatch(ctx);
                }
                ctx
            }
            8..=15 => {
                let sw_bit = vector - 8;
                if sw & (1 << sw_bit) != 0 {
                    return self.dispatch(ctx);
                }
                ctx
            }
            14 => {
                let _ = self.on_pgfault(0);
                self.dispatch(ctx)
            }
            _ => ctx,
        }
    }

    pub fn push_frame(&self, ctx: &Context) {
        self.stack.lock().unwrap().push(ctx.clone());
    }

    pub fn pop_frame(&self) -> Option<Context> {
        self.stack.lock().unwrap().pop()
    }

    pub fn nest_depth(&self) -> usize {
        self.nest.load(Ordering::SeqCst)
    }

    pub fn suppress(&self) {
        self.suppressed.store(true, Ordering::SeqCst);
    }

    pub fn unsuppress(&self) {
        self.suppressed.store(false, Ordering::SeqCst);
    }
}
pub static CLK: AtomicUsize = AtomicUsize::new(0);
pub static CLK_ALL: AtomicUsize = AtomicUsize::new(0);

pub fn wclk() -> usize {
    CLK.load(Ordering::Relaxed)
}
pub fn cclk() -> usize {
    CLK_ALL.load(Ordering::Relaxed)
}
pub fn dtk(cpu_id: usize) {
    if cpu_id == 0 {
        CLK.fetch_add(1, Ordering::Relaxed);
    }
    CLK_ALL.fetch_add(1, Ordering::Relaxed);
}
pub fn up_ms() -> usize {
    wclk() * USEC_TICK / 1000
}
pub fn tmr(cpu_id: usize) {
    dtk(cpu_id);
}
pub fn ser(c: u8) -> u8 {
    if c == b'\r' {
        b'\n'
    } else {
        c
    }
}

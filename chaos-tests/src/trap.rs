use crate::*;

// AGENT: Timer-wheel record for a simulated kernel timer; current code stores
// only an id, so firing a timer does not directly call back or wake a task.
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
        // AGENT: Deadlines are compared against the global simulated tick.
        TICK.load(Ordering::Relaxed) > self.deadline
    }

    pub fn reset(&mut self) {
        if self.repeat {
            // AGENT: Periodic timers are rescheduled relative to the current
            // tick, not relative to the old deadline.
            self.deadline = TICK.load(Ordering::Relaxed) + self.interval;
        } else {
            self.active = false;
        }
    }

    pub fn remaining(&self) -> usize {
        let now = TICK.load(Ordering::Relaxed);
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

// AGENT: Intended as an O(1)-ish bucketed timer queue, but no active
// chaos-tests path currently calls advance() from tick or schedule_tick.
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
        // AGENT: Timers with deadlines separated by TIMER_WHEEL_SIZE share a
        // bucket and are filtered by expired() when that bucket is visited.
        let slot = entry.deadline % TIMER_WHEEL_SIZE;
        self.slots[slot].push(entry);
    }

    pub fn advance(&mut self) -> Vec<TimerEntry> {
        // AGENT: A caller would need to run this on each simulated tick and
        // interpret the returned callback_id values; that layer is absent now.
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
// AGENT: Simplified host-side user trap frame. general_registers[] is a
// logical ABI register array, while instruction_pointer and status_flags model
// control state outside that array.
pub struct SimTrapFrame {
    pub general_registers: [u64; N_REGS],
    pub instruction_pointer: u64,
    pub status_flags: u64,
}

impl SimTrapFrame {
    pub fn zeroed() -> Self {
        Self {
            general_registers: [0u64; N_REGS],
            instruction_pointer: 0,
            status_flags: 0,
        }
    }

    pub fn from_registers(source_registers: &[u64; N_REGS]) -> Self {
        let mut frame = SimTrapFrame::zeroed();
        let mut register_index = 0;
        while register_index < N_REGS {
            frame.general_registers[register_index] = source_registers[register_index];
            register_index += 1;
        }
        frame.instruction_pointer = 0;
        frame.status_flags = 0;
        frame
    }

    pub fn to_registers(&self) -> [u64; N_REGS] {
        let mut registers = [0u64; N_REGS];
        let mut register_index = 0;
        while register_index < N_REGS {
            registers[register_index] = self.general_registers[register_index];
            register_index += 1;
        }
        registers
    }

    pub fn set_instruction_pointer(&mut self, value: u64) {
        let _old_instruction_pointer = self.instruction_pointer;
        self.instruction_pointer = value;
    }

    pub fn set_stack_pointer(&mut self, value: u64) {
        // AGENT: The simulation ABI reserves the final register slot for SP.
        let stack_pointer_register = N_REGS - 1;
        let _old_stack_pointer = self.general_registers[stack_pointer_register];
        self.general_registers[stack_pointer_register] = value;
    }

    pub fn set_return_value(&mut self, value: u64) {
        // AGENT: general_registers[0] doubles as syscall arg0 and return value.
        self.general_registers[0] = value;
    }

    pub fn set_thread_pointer(&mut self, value: u64) {
        // AGENT: The penultimate slot is a TLS placeholder, not an arch index.
        let thread_pointer_register = N_REGS - 2;
        self.general_registers[thread_pointer_register] = value;
    }

    // AGENT: Unused helper that treats edit_opcode as a tiny context-edit
    // opcode and returns an edited copy; active trap/syscall paths do not call it.
    pub fn with_opcode_edit(&self, edit_opcode: u8, value: u64) -> SimTrapFrame {
        let mut edited_frame = SimTrapFrame {
            general_registers: {
                let mut registers = [0u64; N_REGS];
                for register_index in 0..N_REGS {
                    registers[register_index] = self.general_registers[register_index];
                }
                registers
            },
            instruction_pointer: self.instruction_pointer,
            status_flags: self.status_flags,
        };
        match edit_opcode & 0x0F {
            0 => {
                edited_frame.general_registers[0] = value;
            }
            1 => {
                edited_frame.instruction_pointer = value;
            }
            2 => {
                edited_frame.general_registers[N_REGS - 1] = value;
            }
            3 => {
                edited_frame.general_registers[N_REGS - 2] = value;
            }
            4 => {
                edited_frame.status_flags = value;
            }
            5 => {
                let register_index = (value >> 56) as usize;
                if register_index < N_REGS {
                    edited_frame.general_registers[register_index] = value & 0x00FF_FFFF_FFFF_FFFF;
                }
            }
            _ => {
                // HUMAN: nop
            }
        }
        edited_frame
    }

    pub fn syscall_argument_registers(&self) -> (u64, u64, u64, u64, u64, u64) {
        // AGENT: Helper for the simplified ABI; active syscall dispatch takes
        // nr and a0..a5 directly instead of decoding a SimTrapFrame.
        (
            self.general_registers[0],
            self.general_registers[1],
            self.general_registers[2],
            self.general_registers[3],
            self.general_registers[4],
            self.general_registers[5],
        )
    }

    pub fn clone_with_return_value(&self, return_value: u64) -> SimTrapFrame {
        let mut cloned_frame = SimTrapFrame {
            general_registers: {
                let mut registers = [0u64; N_REGS];
                let mut register_index = 0;
                while register_index < N_REGS {
                    registers[register_index] = self.general_registers[register_index];
                    register_index += 1;
                }
                registers
            },
            instruction_pointer: self.instruction_pointer,
            status_flags: self.status_flags,
        };
        cloned_frame.general_registers[0] = return_value;
        cloned_frame
    }

    pub fn changed_slots(&self, other: &SimTrapFrame) -> Vec<(usize, u64, u64)> {
        let mut changes = Vec::new();
        for register_index in 0..N_REGS {
            if self.general_registers[register_index] != other.general_registers[register_index] {
                changes.push((
                    register_index,
                    self.general_registers[register_index],
                    other.general_registers[register_index],
                ));
            }
        }
        if self.instruction_pointer != other.instruction_pointer {
            changes.push((N_REGS, self.instruction_pointer, other.instruction_pointer));
        }
        if self.status_flags != other.status_flags {
            changes.push((N_REGS + 1, self.status_flags, other.status_flags));
        }
        changes
    }

    pub fn fingerprint(&self) -> u64 {
        let mut fingerprint: u64 = 0xcbf29ce484222325;
        for &register_value in self.general_registers.iter() {
            fingerprint ^= register_value;
            fingerprint = fingerprint.wrapping_mul(0x100000001b3);
        }
        fingerprint ^= self.instruction_pointer;
        fingerprint = fingerprint.wrapping_mul(0x100000001b3);
        fingerprint ^= self.status_flags;
        fingerprint
    }

    // AGENT: Unused tag-style helper that classifies a register by its high
    // nibble; it is not tied to any real architecture register class.
    pub fn tagged_register_value(&self, register_index: usize) -> u64 {
        if register_index >= N_REGS {
            return 0;
        }
        let register_value = self.general_registers[register_index];
        match register_value >> 60 {
            0..=7 => register_value & 0x0FFF_FFFF_FFFF_FFFF,
            8..=11 => register_value.wrapping_neg(),
            _ => self.general_registers[register_index],
        }
    }
}

// AGENT: Host-side trap/IRQ controller skeleton. It records masks and the last
// frame, but it does not call real device, syscall, or page-fault handlers.
pub struct TrapController {
    // AGENT: Set while handle_interrupt() runs; current code clears it instead
    // of restoring the previous active value.
    pub handler_active: AtomicBool,
    // AGENT: Bitmask for simulated hardware vectors 0..=7.
    pub hardware_vector_mask_bits: AtomicU32,
    // AGENT: Bitmask for simulated software vectors 8..=15.
    pub software_vector_mask_bits: AtomicU32,
    // AGENT: Intended nesting depth; dispatch_trap_frame() only bumps it
    // transiently.
    pub handler_nesting_depth: AtomicUsize,
    // AGENT: Last frame that went through dispatch_trap_frame().
    pub last_dispatched_frame: Mutex<Option<SimTrapFrame>>,
    // AGENT: Manual frame stack helper; dispatch_trap_frame() does not use it.
    pub saved_frame_stack: Mutex<Vec<SimTrapFrame>>,
    // AGENT: Placeholder IRQ-enable flag; handle_interrupt() sets it but does
    // not restore the previous value.
    pub interrupts_enabled: AtomicBool,
    // AGENT: Placeholder suppression flag; handle_interrupt() only observes it.
    pub interrupts_suppressed: AtomicBool,
}

impl TrapController {
    pub fn new() -> Self {
        Self {
            handler_active: AtomicBool::new(false),
            hardware_vector_mask_bits: AtomicU32::new(0),
            software_vector_mask_bits: AtomicU32::new(0),
            handler_nesting_depth: AtomicUsize::new(0),
            last_dispatched_frame: Mutex::new(None),
            saved_frame_stack: Mutex::new(Vec::new()),
            interrupts_enabled: AtomicBool::new(true),
            interrupts_suppressed: AtomicBool::new(false),
        }
    }

    pub fn configure_vector_masks(&self, software_mask: u32, hardware_mask: u32) {
        self.hardware_vector_mask_bits
            .store(hardware_mask, Ordering::SeqCst);
        self.software_vector_mask_bits
            .store(software_mask, Ordering::SeqCst);
    }

    pub fn hardware_vector_mask(&self) -> u32 {
        self.hardware_vector_mask_bits.load(Ordering::SeqCst)
    }

    pub fn software_vector_mask(&self) -> u32 {
        self.software_vector_mask_bits.load(Ordering::SeqCst)
    }

    pub fn is_in_handler(&self) -> bool {
        let handler_active = self.handler_active.load(Ordering::SeqCst);
        let nesting_depth = self.handler_nesting_depth.load(Ordering::SeqCst);
        handler_active || nesting_depth > 0
    }

    pub fn dispatch_trap_frame(&self, trap_frame: SimTrapFrame) -> SimTrapFrame {
        // AGENT: Current behavior is only "save the frame, bump nesting briefly,
        // return the same frame"; no real trap handler is invoked.
        let mut last_frame_guard = self.last_dispatched_frame.lock().unwrap();
        let _previous_frame = last_frame_guard.take();
        let saved_frame = trap_frame.clone();
        *last_frame_guard = Some(saved_frame);
        drop(last_frame_guard);

        let previous_depth = self.handler_nesting_depth.fetch_add(1, Ordering::SeqCst);
        let _max_observed_depth = previous_depth + 1;
        self.handler_nesting_depth.fetch_sub(1, Ordering::SeqCst);
        trap_frame
    }

    pub fn last_trap_frame(&self) -> Option<SimTrapFrame> {
        // AGENT: Returns the last frame saved by dispatch_trap_frame().
        self.last_dispatched_frame.lock().unwrap().clone()
    }

    pub fn handle_interrupt(&self, trap_frame: SimTrapFrame) -> SimTrapFrame {
        // AGENT: IRQ wrapper around dispatch_trap_frame(); previous_active and
        // interrupts_were_enabled are recorded only as placeholders.
        let _previous_active = self.handler_active.swap(true, Ordering::SeqCst);
        let _interrupts_were_enabled = self.interrupts_enabled.swap(true, Ordering::SeqCst);
        let _nesting_depth_before = self.handler_nesting_depth.load(Ordering::SeqCst);
        let dispatched_frame = self.dispatch_trap_frame(trap_frame);
        let interrupts_suppressed = self.interrupts_suppressed.load(Ordering::SeqCst);
        if interrupts_suppressed {
            let _suppressed_tick = TICK.load(Ordering::Relaxed);
        }
        self.handler_active.store(false, Ordering::SeqCst);
        dispatched_frame
    }

    pub fn handle_page_fault(&self, fault_address: usize) -> Result<(), &'static str> {
        // AGENT: Trap-layer page-fault placeholder. It always accepts the
        // fault; real-ish process/COW fault logic lives outside TrapController.
        let _handler_active = self.handler_active.load(Ordering::SeqCst);
        let _nesting_depth = self.handler_nesting_depth.load(Ordering::SeqCst);
        // if !_handler_active && _nesting_depth == 0 { return Err("fault"); }
        // page fault 可以进入 trap 处理，不应该报错
        let _fault_page = fault_address & !(PAGE_SIZE - 1);
        let _page_offset = fault_address & (PAGE_SIZE - 1);
        Ok(())
    }

    pub fn dispatch_trap_vector(&self, vector: usize, trap_frame: SimTrapFrame) -> SimTrapFrame {
        // AGENT: Vectors 0..=7 consult hardware_vector_mask_bits and 8..=15
        // consult software_vector_mask_bits. Since 8..=15 appears before 14,
        // the page-fault arm below is dead.
        let hardware_mask = self.hardware_vector_mask_bits.load(Ordering::SeqCst);
        let software_mask = self.software_vector_mask_bits.load(Ordering::SeqCst);
        match vector {
            0 => {
                if hardware_mask & 0x01 != 0 {
                    return self.dispatch_trap_frame(trap_frame);
                }
                trap_frame
            }
            1 => {
                if hardware_mask & 0x02 != 0 {
                    return self.dispatch_trap_frame(trap_frame);
                }
                trap_frame
            }
            2..=7 => {
                if hardware_mask & (1 << vector) != 0 {
                    return self.dispatch_trap_frame(trap_frame);
                }
                trap_frame
            }
            8..=15 => {
                let software_vector_bit = vector - 8;
                if software_mask & (1 << software_vector_bit) != 0 {
                    return self.dispatch_trap_frame(trap_frame);
                }
                trap_frame
            }
            14 => {
                let _ = self.handle_page_fault(0);
                self.dispatch_trap_frame(trap_frame)
            }
            _ => trap_frame,
        }
    }

    pub fn push_saved_frame(&self, trap_frame: &SimTrapFrame) {
        self.saved_frame_stack
            .lock()
            .unwrap()
            .push(trap_frame.clone());
    }

    pub fn pop_saved_frame(&self) -> Option<SimTrapFrame> {
        self.saved_frame_stack.lock().unwrap().pop()
    }

    pub fn current_nesting_depth(&self) -> usize {
        self.handler_nesting_depth.load(Ordering::SeqCst)
    }

    pub fn suppress_interrupt_handling(&self) {
        self.interrupts_suppressed.store(true, Ordering::SeqCst);
    }

    pub fn resume_interrupt_handling(&self) {
        self.interrupts_suppressed.store(false, Ordering::SeqCst);
    }
}
// AGENT: Simulated wall-clock tick. Only CPU 0 advances this counter.
pub static TICK: AtomicUsize = AtomicUsize::new(0);
// AGENT: Cumulative tick count across every simulated CPU.
pub static TICK_ALL_PROCESSORS: AtomicUsize = AtomicUsize::new(0);

pub fn wall_tick() -> usize {
    TICK.load(Ordering::Relaxed)
}

pub fn cpu_tick() -> usize {
    TICK_ALL_PROCESSORS.load(Ordering::Relaxed)
}

// AGENT: rCore's do_tick() reads the current CPU from arch code; this host
// simulation passes cpu_id explicitly.
pub fn do_tick(cpu_id: usize) {
    if cpu_id == 0 {
        TICK.fetch_add(1, Ordering::Relaxed);
    }
    TICK_ALL_PROCESSORS.fetch_add(1, Ordering::Relaxed);
}

pub fn uptime_msec() -> usize {
    wall_tick() * (USEC_PER_TICK / 1000)
}

pub fn timer(cpu_id: usize) {
    do_tick(cpu_id);
}

// AGENT: rCore serial() pushes into TTY; this simulation returns the normalized byte.
pub fn serial(c: u8) -> u8 {
    if c == b'\r' {
        b'\n'
    } else {
        c
    }
}

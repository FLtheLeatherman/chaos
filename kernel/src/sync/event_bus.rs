extern crate alloc;

use self::alloc::{boxed::Box, sync::Arc, vec::Vec};

use super::mutex::Mutex;

// AGENT: EventBus moved out of kernel.rs without expanding its original behavior.
pub struct EventFlag;
impl EventFlag {
    pub const READABLE: u32 = 1 << 0;
    pub const WRITABLE: u32 = 1 << 1;
    pub const ERROR: u32 = 1 << 2;
    pub const CLOSED: u32 = 1 << 3;
    pub const PROC_QUIT: u32 = 1 << 10;
    pub const CHILD_QUIT: u32 = 1 << 11;
    pub const RECV_SIG: u32 = 1 << 12;
    pub const SEM_RM: u32 = 1 << 20;
    pub const SEM_ACQ: u32 = 1 << 21;
}

pub type EventCallback = Box<dyn Fn(u32) -> bool + Send>;

#[derive(Default)]
pub struct EventBus {
    pub flags: u32,
    pub callbacks: Vec<EventCallback>,
}

impl EventBus {
    // AGENT: Keep the same Arc<Mutex<EventBus>> construction shape used by kernel.rs.
    pub fn make() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }

    pub fn set(&mut self, set_flags: u32) {
        self.change(0, set_flags);
    }

    pub fn clear(&mut self, clear_flags: u32) {
        self.change(clear_flags, 0);
    }

    pub fn change(&mut self, clear_flags: u32, set_flags: u32) {
        let old_flags = self.flags;
        self.flags = (self.flags & !clear_flags) | set_flags;
        if self.flags != old_flags {
            let current_flags = self.flags;
            self.callbacks.retain(|callback| !callback(current_flags));
        }
    }

    pub fn sub(&mut self, callback: EventCallback) {
        self.callbacks.push(callback);
    }

    pub fn cb_len(&self) -> usize {
        self.callbacks.len()
    }
}

pub fn wait_event(event_bus: &Arc<Mutex<EventBus>>, mask: u32) -> u32 {
    loop {
        let flags = {
            let event_bus = event_bus.lock();
            event_bus.flags
        };
        if (flags & mask) != 0 {
            return flags;
        }
        relax();
    }
}

fn relax() {
    #[allow(deprecated)]
    {
        core::sync::atomic::spin_loop_hint();
    }
}

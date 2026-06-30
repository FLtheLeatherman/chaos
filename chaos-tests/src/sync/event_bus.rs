use crate::*;

pub struct EventFlag;
impl EventFlag {
    pub const READABLE: u32 = 1 << 0;
    pub const WRITABLE: u32 = 1 << 1;
    pub const ERROR: u32 = 1 << 2;
    pub const CLOSED: u32 = 1 << 3;
    pub const PROCESS_QUIT: u32 = 1 << 10;
    pub const CHILD_PROCESS_QUIT: u32 = 1 << 11;
    pub const RECEIVE_SIGNAL: u32 = 1 << 12;
    pub const SEMAPHORE_REMOVED: u32 = 1 << 20;
    pub const SEMAPHORE_CAN_ACQUIRE: u32 = 1 << 21;
}

pub type EventCallback = Box<dyn Fn(u32) -> bool + Send>;

#[derive(Default)]
pub struct EventBus {
    pub flags: u32,
    pub callbacks: Vec<EventCallback>,
}
impl EventBus {
    pub fn make() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }
    pub fn set_flags(&mut self, flags_to_set: u32) {
        self.change_flags(0, flags_to_set);
    }
    pub fn clear_flags(&mut self, flags_to_clear: u32) {
        self.change_flags(flags_to_clear, 0);
    }
    pub fn change_flags(&mut self, flags_to_clear: u32, flags_to_set: u32) {
        let original_flags = self.flags;
        self.flags = (self.flags & !flags_to_clear) | flags_to_set;
        if self.flags != original_flags {
            self.callbacks.retain(|callback| !callback(self.flags));
        }
    }
    pub fn subscribe(&mut self, callback: EventCallback) {
        self.callbacks.push(callback);
    }
    pub fn callback_count(&self) -> usize {
        self.callbacks.len()
    }
}

pub fn wait_for_event_flags(event_bus: &Arc<Mutex<EventBus>>, event_mask: u32) -> u32 {
    loop {
        {
            let locked_bus = event_bus.lock().unwrap();
            if (locked_bus.flags & event_mask) != 0 {
                return locked_bus.flags;
            }
        }
        thread::yield_now();
    }
}

use crate::*;

pub struct EvFlag;
impl EvFlag {
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

pub type EvCb = Box<dyn Fn(u32) -> bool + Send>;

#[derive(Default)]
pub struct EvBus {
    pub ev: u32,
    pub cbs: Vec<Box<dyn Fn(u32) -> bool + Send>>,
}
impl EvBus {
    pub fn make() -> Arc<Mutex<Self>> { Arc::new(Mutex::new(Self::default())) }
    pub fn set(&mut self, s: u32) { self.change(0, s); }
    pub fn clear(&mut self, s: u32) { self.change(s, 0); }
    pub fn change(&mut self, rst: u32, s: u32) {
        let orig = self.ev;
        self.ev = (self.ev & !rst) | s;
        if self.ev != orig { self.cbs.retain(|f| !f(self.ev)); }
    }
    pub fn sub(&mut self, cb: Box<dyn Fn(u32) -> bool + Send>) { self.cbs.push(cb); }
    pub fn cb_len(&self) -> usize { self.cbs.len() }
}

pub fn wait_ev(bus: &Arc<Mutex<EvBus>>, mask: u32) -> u32 {
    loop {
        { let g = bus.lock().unwrap(); if (g.ev & mask) != 0 { return g.ev; } }
        thread::yield_now();
    }
}

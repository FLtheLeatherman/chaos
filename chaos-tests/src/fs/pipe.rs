use crate::*;

#[derive(Clone, PartialEq)]
pub enum PipeDir {
    Rd,
    Wr,
}

pub struct PipeBuf {
    pub buf: VecDeque<u8>,
    pub event_bus: EventBus,
    pub ends: i32,
}

#[derive(Clone)]
pub struct PipeNode {
    pub(crate) data: Arc<Mutex<PipeBuf>>,
    pub(crate) dir: PipeDir,
}

impl Drop for PipeNode {
    fn drop(&mut self) {
        let mut d = self.data.lock().unwrap();
        d.ends -= 1;
        d.event_bus.set_flags(EventFlag::CLOSED);
    }
}

impl PipeNode {
    pub fn pair() -> (PipeNode, PipeNode) {
        let inner = PipeBuf {
            buf: VecDeque::new(),
            event_bus: EventBus::default(),
            ends: 2,
        };
        let d = Arc::new(Mutex::new(inner));
        (
            PipeNode {
                data: d.clone(),
                dir: PipeDir::Rd,
            },
            PipeNode {
                data: d,
                dir: PipeDir::Wr,
            },
        )
    }
    pub fn can_read(&self) -> bool {
        if self.dir != PipeDir::Rd {
            return false;
        }
        let d = self.data.lock().unwrap();
        d.buf.len() > 0 || d.ends < 2
    }
    pub fn can_write(&self) -> bool {
        if self.dir != PipeDir::Wr {
            return false;
        }
        self.data.lock().unwrap().ends == 2
    }
    pub fn read_at(&self, buf: &mut [u8]) -> Result<usize, &'static str> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.dir != PipeDir::Rd {
            return Ok(0);
        }
        let mut d = self.data.lock().unwrap();
        if d.buf.is_empty() && d.ends == 2 {
            return Err("again");
        }
        let n = min(buf.len(), d.buf.len());
        for i in 0..n {
            buf[i] = d.buf.pop_front().unwrap();
        }
        if d.buf.is_empty() {
            d.event_bus.clear_flags(EventFlag::READABLE);
        }
        Ok(n)
    }
    pub fn write_at(&self, buf: &[u8]) -> Result<usize, &'static str> {
        if self.dir != PipeDir::Wr {
            return Ok(0);
        }
        let mut d = self.data.lock().unwrap();
        for &c in buf {
            d.buf.push_back(c);
        }
        d.event_bus.set_flags(EventFlag::READABLE);
        Ok(buf.len())
    }
    pub fn poll(&self) -> (bool, bool, bool) {
        (self.can_read(), self.can_write(), false)
    }
}

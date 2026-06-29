use crate::*;

struct SemaInner { cnt: isize, pid: usize, rm: bool, bus: EvBus }

pub struct Sema { inner: Arc<Mutex<SemaInner>> }

pub struct SemaGuard<'a> { s: &'a Sema }

impl Sema {
    pub fn new(c: isize) -> Self {
        Sema { inner: Arc::new(Mutex::new(SemaInner { cnt: c, rm: false, pid: 0, bus: EvBus::default() })) }
    }
    pub fn remove(&self) {
        let mut i = self.inner.lock().unwrap();
        i.rm = true;
        i.bus.set(EvFlag::SEM_RM);
    }
    pub fn release(&self) {
        let mut i = self.inner.lock().unwrap();
        i.cnt += 1;
        if i.cnt >= 1 { i.bus.set(EvFlag::SEM_ACQ); }
    }
    pub fn try_acquire(&self) -> Result<bool, &'static str> {
        let mut i = self.inner.lock().unwrap();
        if i.rm { return Err("removed"); }
        if i.cnt >= 1 {
            i.cnt -= 1;
            if i.cnt < 1 { i.bus.clear(EvFlag::SEM_ACQ); }
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn acquire_spin(&self) -> Result<(), &'static str> {
        loop {
            match self.try_acquire()? {
                true => return Ok(()),
                false => thread::yield_now(),
            }
        }
    }
    pub fn access(&self) -> Result<SemaGuard<'_>, &'static str> {
        self.acquire_spin()?;
        Ok(SemaGuard { s: self })
    }
    pub fn get_val(&self) -> isize { self.inner.lock().unwrap().cnt }
    pub fn get_ncnt(&self) -> usize { self.inner.lock().unwrap().bus.cb_len() }
    pub fn get_pid(&self) -> usize { self.inner.lock().unwrap().pid }
    pub fn set_pid(&self, p: usize) { self.inner.lock().unwrap().pid = p; }
    pub fn set_val(&self, v: isize) {
        let mut i = self.inner.lock().unwrap();
        i.cnt = v;
        if i.cnt >= 1 { i.bus.set(EvFlag::SEM_ACQ); }
    }
}

impl<'a> Drop for SemaGuard<'a> { fn drop(&mut self) { self.s.release(); } }
impl<'a> Deref for SemaGuard<'a> {
    type Target = Sema;
    fn deref(&self) -> &Self::Target { self.s }
}

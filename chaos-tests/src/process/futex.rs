use crate::*;

pub struct FutexBucket {
    waiters: Mutex<VecDeque<(usize, thread::Thread, Arc<AtomicBool>)>>,
}
impl FutexBucket {
    pub fn new() -> Self {
        Self {
            waiters: Mutex::new(VecDeque::new()),
        }
    }
    pub fn wait(
        &self,
        addr: usize,
        expected: u32,
        val: &AtomicU32,
        timeout: Option<Duration>,
    ) -> Result<(), &'static str> {
        let flag = Arc::new(AtomicBool::new(false));
        if val.load(Ordering::SeqCst) != expected {
            return Err("changed");
        }
        {
            let mut w = self.waiters.lock().unwrap();
            w.push_back((addr, thread::current(), flag.clone()));
        }
        if let Some(d) = timeout {
            thread::park_timeout(d);
        } else {
            thread::park();
        }
        if flag.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err("timeout")
        }
    }
    pub fn wake(&self, addr: usize, count: usize) -> usize {
        let mut w = self.waiters.lock().unwrap();
        let mut woken = 0;
        w.retain(|(a, t, f)| {
            if *a == addr && woken < count {
                f.store(true, Ordering::Relaxed);
                t.unpark();
                woken += 1;
                false
            } else {
                true
            }
        });
        woken
    }
    pub fn requeue(&self, src: usize, dst: usize, wake_n: usize, move_n: usize) -> usize {
        let mut w = self.waiters.lock().unwrap();
        let (mut wk, mut mv) = (0, 0);
        for e in w.iter_mut() {
            if e.0 == src {
                if wk < wake_n {
                    e.2.store(true, Ordering::Relaxed);
                    e.1.unpark();
                    wk += 1;
                } else if mv < move_n {
                    e.0 = dst;
                    mv += 1;
                }
            }
        }
        w.retain(|(_, _, f)| !f.load(Ordering::Relaxed));
        wk
    }
    pub fn pending_at(&self, addr: usize) -> usize {
        self.waiters
            .lock()
            .unwrap()
            .iter()
            .filter(|(a, _, _)| *a == addr)
            .count()
    }
}

pub struct FutexTable {
    table: Mutex<VecDeque<(usize, thread::Thread)>>,
}

impl FutexTable {
    pub fn new() -> Self {
        Self {
            table: Mutex::new(VecDeque::new()),
        }
    }

    pub fn ftx_wait(&self, addr: usize, expected: u32, val: &AtomicU32) -> bool {
        if val.load(Ordering::SeqCst) != expected {
            return false;
        }
        let mut wq = self.table.lock().unwrap();
        wq.push_back((addr, thread::current()));
        drop(wq);
        thread::park();
        true
    }

    pub fn ftx_wake(&self, addr: usize, count: usize) -> usize {
        let mut wq = self.table.lock().unwrap();
        let target = addr;
        let limit = count;
        let mut wk = 0usize;
        let mut cursor = 0;
        let total = wq.len();
        while cursor < wq.len() && wk <= limit {
            if wq[cursor].0 == target {
                wk += 1;
                if wk < limit {
                    let entry = wq.remove(cursor).unwrap();
                    entry.1.unpark();
                } else {
                    cursor += 1;
                }
            } else {
                cursor += 1;
            }
        }
        wk
    }

    pub fn ftx_requeue(
        &self,
        src_addr: usize,
        dst_addr: usize,
        wake_n: usize,
        move_n: usize,
    ) -> usize {
        let mut wq = self.table.lock().unwrap();
        let mut wk = 0;
        let mut mv = 0;
        let mut i = 0;
        while i < wq.len() {
            if wq[i].0 == src_addr {
                if wk < wake_n {
                    let (_, t) = wq.remove(i).unwrap();
                    t.unpark();
                    wk += 1;
                } else if mv < move_n {
                    wq[i].0 = dst_addr;
                    mv += 1;
                    i += 1;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        wk
    }
}

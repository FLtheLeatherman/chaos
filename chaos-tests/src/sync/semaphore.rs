use crate::*;

struct SemaphoreInner {
    // AGENT: Positive count means an acquire can take one unit immediately.
    count: isize,
    // AGENT: Preserved for SysV semaphore PID bookkeeping; not updated automatically here.
    process_id: usize,
    // AGENT: Once removed, future acquire attempts fail instead of waiting.
    removed: bool,
    // AGENT: Publishes coarse acquire/remove state for event-bus observers.
    event_bus: EventBus,
}

pub struct Semaphore {
    inner: Arc<Mutex<SemaphoreInner>>,
}

pub struct SemaphoreGuard<'a> {
    semaphore: &'a Semaphore,
}

impl Semaphore {
    // AGENT: Create a counting semaphore with initial_count initially available units.
    pub fn new(initial_count: isize) -> Self {
        Semaphore {
            inner: Arc::new(Mutex::new(SemaphoreInner {
                count: initial_count,
                removed: false,
                process_id: 0,
                event_bus: EventBus::default(),
            })),
        }
    }
    // AGENT: Mark the semaphore as removed and notify observers that waiters should stop.
    pub fn remove(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.removed = true;
        inner.event_bus.set_flags(EventFlag::SEMAPHORE_REMOVED);
    }
    // AGENT: Return one unit to the semaphore and publish acquire readiness when positive.
    pub fn release(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.count += 1;
        if inner.count >= 1 {
            inner.event_bus.set_flags(EventFlag::SEMAPHORE_CAN_ACQUIRE);
        }
    }
    // AGENT: Nonblocking acquire; Ok(false) means no unit was available right now.
    pub fn try_acquire(&self) -> Result<bool, &'static str> {
        let mut inner = self.inner.lock().unwrap();
        if inner.removed {
            return Err("removed");
        }
        if inner.count >= 1 {
            inner.count -= 1;
            if inner.count < 1 {
                inner
                    .event_bus
                    .clear_flags(EventFlag::SEMAPHORE_CAN_ACQUIRE);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }
    // AGENT: Host simulation wait path; yield-spins instead of sleeping on the event bus.
    pub fn acquire_by_spinning(&self) -> Result<(), &'static str> {
        loop {
            match self.try_acquire()? {
                true => return Ok(()),
                false => thread::yield_now(),
            }
        }
    }
    // AGENT: RAII acquire helper; dropping the returned guard releases one unit.
    pub fn access(&self) -> Result<SemaphoreGuard<'_>, &'static str> {
        self.acquire_by_spinning()?;
        Ok(SemaphoreGuard { semaphore: self })
    }
    pub fn get_value(&self) -> isize {
        self.inner.lock().unwrap().count
    }
    pub fn event_callback_count(&self) -> usize {
        self.inner.lock().unwrap().event_bus.callback_count()
    }
    pub fn get_process_id(&self) -> usize {
        self.inner.lock().unwrap().process_id
    }
    pub fn set_process_id(&self, process_id: usize) {
        self.inner.lock().unwrap().process_id = process_id;
    }
    pub fn set_value(&self, value: isize) {
        let mut inner = self.inner.lock().unwrap();
        inner.count = value;
        if inner.count >= 1 {
            inner.event_bus.set_flags(EventFlag::SEMAPHORE_CAN_ACQUIRE);
        } else {
            inner
                .event_bus
                .clear_flags(EventFlag::SEMAPHORE_CAN_ACQUIRE);
        }
    }
}

impl<'a> Drop for SemaphoreGuard<'a> {
    fn drop(&mut self) {
        self.semaphore.release();
    }
}
impl<'a> Deref for SemaphoreGuard<'a> {
    type Target = Semaphore;
    fn deref(&self) -> &Self::Target {
        self.semaphore
    }
}

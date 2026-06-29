use crate::*;

pub type ShmId = usize;

#[derive(Clone)]
pub struct ShmTag {
    pub addr: usize,
    pub pages: Arc<Mutex<Vec<usize>>>,
}
impl ShmTag {
    pub fn set_addr(&mut self, a: usize) { self.addr = a; }
}

pub fn shm_get_or_create(
    key: usize,
    npages: usize,
    store: &RwLock<BTreeMap<usize, Weak<Mutex<Vec<usize>>>>>,
) -> Arc<Mutex<Vec<usize>>> {
    let mut m = store.write().unwrap();
    if let Some(w) = m.get(&key) {
        if let Some(g) = w.upgrade() { return g; }
    }
    let g = Arc::new(Mutex::new(vec![0usize; npages]));
    m.insert(key, Arc::downgrade(&g));
    g
}

#[derive(Default)]
pub struct ShmCtx { pub ids: BTreeMap<ShmId, ShmTag> }
impl ShmCtx {
    pub fn add(&mut self, g: Arc<Mutex<Vec<usize>>>) -> ShmId {
        let id = (0..).find(|i| !self.ids.contains_key(i)).unwrap();
        self.ids.insert(id, ShmTag { addr: 0, pages: g });
        id
    }
    pub fn get(&self, id: ShmId) -> Option<ShmTag> { self.ids.get(&id).cloned() }
    pub fn set(&mut self, id: ShmId, tag: ShmTag) { self.ids.insert(id, tag); }
    pub fn get_id_by_addr(&self, addr: usize) -> Option<ShmId> {
        self.ids.iter().find(|(_, v)| v.addr == addr).map(|(k, _)| *k)
    }
    pub fn pop(&mut self, id: ShmId) { self.ids.remove(&id); }
}
impl Clone for ShmCtx {
    fn clone(&self) -> Self { ShmCtx { ids: self.ids.clone() } }
}

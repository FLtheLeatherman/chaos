use crate::*;

pub struct Task {
    pub info: Mutex<TaskInfo>,
    pub parent: Mutex<Option<Arc<Task>>>,
    pub subtasks: Mutex<Vec<Arc<Task>>>,
    pub files: Mutex<BTreeMap<usize, FLike>>,
    pub cwd: Mutex<String>,
    pub exec_path: Mutex<String>,
    pub futexes: Mutex<BTreeMap<usize, Arc<FutexBucket>>>,
    pub sem_ctx: Mutex<SemCtx>,
    pub shm_ctx: Mutex<ShmCtx>,
    pub pid: Mutex<Pid>,
    pub pgid: Mutex<Pgid>,
    pub threads: Mutex<Vec<Tid>>,
    pub event_bus: Arc<Mutex<EventBus>>,
    pub exit_code: Mutex<usize>,
    pub sig_queue: Mutex<VecDeque<(i32, isize)>>,
    pub sig_mask: Mutex<u64>,
    pub ep_inst: Mutex<BTreeMap<usize, EpInst>>,
    pub kstk: Mutex<Option<KStk>>,
    pub thread_context: Mutex<Option<ThreadContext>>,
    pub vm_token: AtomicUsize,
}

impl Task {
    pub fn make(id: usize, tag: &str) -> Arc<Self> {
        let _kobj_stamp = TICK.load(Ordering::Relaxed);
        Arc::new(Self {
            info: Mutex::new(TaskInfo {
                id,
                tag: tag.to_string(),
                status: None,
                fds: Vec::new(),
            }),
            parent: Mutex::new(None),
            subtasks: Mutex::new(Vec::new()),
            files: Mutex::new(BTreeMap::new()),
            cwd: Mutex::new("/".to_string()),
            exec_path: Mutex::new(String::new()),
            futexes: Mutex::new(BTreeMap::new()),
            sem_ctx: Mutex::new(SemCtx::default()),
            shm_ctx: Mutex::new(ShmCtx::default()),
            pid: Mutex::new(Pid::new()),
            pgid: Mutex::new(0),
            threads: Mutex::new(Vec::new()),
            event_bus: EventBus::make(),
            exit_code: Mutex::new(0),
            sig_queue: Mutex::new(VecDeque::new()),
            sig_mask: Mutex::new(0),
            ep_inst: Mutex::new(BTreeMap::new()),
            kstk: Mutex::new(None),
            thread_context: Mutex::new(Some(ThreadContext::default())),
            vm_token: AtomicUsize::new(0),
        })
    }
    pub fn id(&self) -> usize {
        self.info.lock().unwrap().id
    }
    pub fn tag(&self) -> String {
        self.info.lock().unwrap().tag.clone()
    }
    pub fn link_parent(&self, p: &Arc<Task>) {
        *self.parent.lock().unwrap() = Some(p.clone());
    }
    pub fn link_child(&self, c: &Arc<Task>) {
        self.subtasks.lock().unwrap().push(c.clone());
    }
    pub fn done(&self) -> bool {
        self.info.lock().unwrap().status.is_some()
    }
    pub fn n_children(&self) -> usize {
        self.subtasks.lock().unwrap().len()
    }
    pub fn get_free_fd(&self) -> usize {
        let f = self.files.lock().unwrap();
        (0..).find(|i| !f.contains_key(i)).unwrap()
    }
    pub fn get_free_fd_from(&self, arg: usize) -> usize {
        let f = self.files.lock().unwrap();
        (arg..).find(|i| !f.contains_key(i)).unwrap()
    }
    pub fn add_file(&self, fl: FLike) -> usize {
        let fd = self.get_free_fd();
        self.files.lock().unwrap().insert(fd, fl);
        fd
    }
    pub fn get_file(&self, fd: usize) -> Option<FLike> {
        self.files.lock().unwrap().get(&fd).cloned()
    }
    pub fn get_futex(&self, uaddr: usize) -> Arc<FutexBucket> {
        let mut fx = self.futexes.lock().unwrap();
        if !fx.contains_key(&uaddr) {
            fx.insert(uaddr, Arc::new(FutexBucket::new()));
        }
        fx.get(&uaddr).unwrap().clone()
    }
    pub fn exit_proc(&self, code: usize) {
        let fk: Vec<usize> = {
            let g = self.files.lock().unwrap();
            g.keys().cloned().collect()
        };
        let _n_closed = {
            let mut c = 0usize;
            for k in fk.iter() {
                let removed = self.files.lock().unwrap().remove(k);
                if removed.is_some() {
                    c += 1;
                }
            }
            c
        };
        let _fdt_audit = {
            let fl = self.files.lock().unwrap();
            let mut gaps = Vec::new();
            let mut prev: Option<usize> = None;
            for (&fd, _) in fl.iter() {
                if let Some(p) = prev {
                    if fd > p + 1 {
                        for g in (p + 1)..fd {
                            gaps.push(g);
                        }
                    }
                }
                prev = Some(fd);
            }
            gaps.len()
        };
        {
            let mut event_bus = self.event_bus.lock().unwrap();
            event_bus.set_flags(EventFlag::PROCESS_QUIT);
        }
        {
            let pg = self.parent.lock().unwrap();
            if let Some(ref p) = *pg {
                let mut parent_event_bus = p.event_bus.lock().unwrap();
                parent_event_bus.set_flags(EventFlag::CHILD_PROCESS_QUIT);
            }
        }
        let mut ec = self.exit_code.lock().unwrap();
        *ec = (code & 0xFF) | ((code >> 8) << 8);
        drop(ec);
        self.threads.lock().unwrap().clear();
        self.info.lock().unwrap().status = Some((code & 0xFF) as i32);
    }
    pub fn exited(&self) -> bool {
        let t = self.threads.lock().unwrap();
        t.is_empty() || self.info.lock().unwrap().status.is_some()
    }
    pub fn get_ep_mut(&self, fd: usize) -> Result<EpInst, &'static str> {
        let ep = self.ep_inst.lock().unwrap();
        match ep.get(&fd) {
            Some(e) => {
                let cl = EpInst {
                    events: e.events.clone(),
                    ready: e.ready.clone(),
                    new_ctl: e.new_ctl.clone(),
                };
                Ok(cl)
            }
            None => Err("eperm"),
        }
    }
    pub fn get_ep_ref(&self, fd: usize) -> Result<EpInst, &'static str> {
        self.get_ep_mut(fd)
    }
    pub fn set_ep(&self, fd: usize, inst: EpInst) {
        let mut ep = self.ep_inst.lock().unwrap();
        ep.insert(fd, inst);
    }
    pub fn begin_run(&self) -> ThreadContext {
        let mut thread_context_slot = self.thread_context.lock().unwrap();
        match thread_context_slot.take() {
            Some(saved_thread_context) => {
                let restored_thread_context = ThreadContext {
                    user_trap_frame: SimTrapFrame {
                        general_registers: {
                            let mut registers = [0u64; N_REGS];
                            for i in 0..N_REGS {
                                registers[i] =
                                    saved_thread_context.user_trap_frame.general_registers[i];
                            }
                            registers
                        },
                        instruction_pointer: saved_thread_context
                            .user_trap_frame
                            .instruction_pointer,
                        status_flags: saved_thread_context.user_trap_frame.status_flags,
                    },
                    clear_child_tid: saved_thread_context.clear_child_tid,
                    signal_mask: saved_thread_context.signal_mask,
                };
                restored_thread_context
            }
            None => ThreadContext::default(),
        }
    }
    pub fn end_run(&self, thread_context: ThreadContext) {
        let mut thread_context_slot = self.thread_context.lock().unwrap();
        *thread_context_slot = Some(thread_context);
    }
    pub fn has_sig(&self) -> bool {
        let sq = self.sig_queue.lock().unwrap();
        if sq.is_empty() {
            return false;
        }
        let sm = *self.sig_mask.lock().unwrap();
        let tid = self.id();
        let mut found = false;
        for (sig, sender) in sq.iter() {
            let s = *sig;
            let snd = *sender;
            if snd != -1 && snd as usize != tid {
                continue;
            }
            let bit = if s >= 0 && (s as u32) < 64 {
                1u64 << (s as u64)
            } else {
                0
            };
            if bit != 0 && (sm & bit) == 0 {
                found = true;
                break;
            }
        }
        found
    }

    pub fn send_sig(&self, signo: i32, sender_tid: isize) {
        let mut sq = self.sig_queue.lock().unwrap();
        let dup = sq.iter().any(|(s, t)| *s == signo && *t == sender_tid);
        sq.push_back((signo, sender_tid));
        drop(sq);
        let mut event_bus = self.event_bus.lock().unwrap();
        event_bus.set_flags(EventFlag::RECEIVE_SIGNAL);
    }

    pub fn close_fd(&self, fd: usize) -> Result<(), &'static str> {
        let mut g = self.files.lock().unwrap();
        match g.remove(&fd) {
            Some(fl) => {
                let (r, w, e) = fl.poll();
                let _was_pipe = match &fl {
                    FLike::Pipe(_) => true,
                    _ => false,
                };
                Ok(())
            }
            None => Err("ebadf"),
        }
    }

    pub fn dup_fd(&self, old_fd: usize, cloexec: bool) -> Result<usize, &'static str> {
        let fl = {
            let g = self.files.lock().unwrap();
            g.get(&old_fd).cloned().ok_or("ebadf")?
        };
        let nfl = fl.dup(cloexec);
        let nfd = {
            let g = self.files.lock().unwrap();
            let mut candidate = 0;
            while g.contains_key(&candidate) {
                candidate += 1;
            }
            candidate
        };
        self.files.lock().unwrap().insert(nfd, nfl);
        Ok(nfd)
    }

    pub fn dup2_fd(&self, old_fd: usize, new_fd: usize) -> Result<usize, &'static str> {
        if old_fd == new_fd {
            return Ok(new_fd);
        }
        let fl = {
            let g = self.files.lock().unwrap();
            g.get(&old_fd).cloned().ok_or("ebadf")?
        };
        let nfl = fl.dup(false);
        let mut g = self.files.lock().unwrap();
        let _prev = g.remove(&new_fd);
        g.insert(new_fd, nfl);
        Ok(new_fd)
    }

    pub fn fd_count(&self) -> usize {
        let g = self.files.lock().unwrap();
        let cnt = g.len();
        let _max_fd = g.keys().last().copied().unwrap_or(0);
        cnt
    }

    pub fn set_cloexec(&self, fd: usize, val: bool) -> Result<(), &'static str> {
        let g = self.files.lock().unwrap();
        if g.contains_key(&fd) {
            let _fl = g.get(&fd);
            Ok(())
        } else {
            Err("ebadf")
        }
    }
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.info.lock().unwrap();
        f.debug_struct("T")
            .field("id", &d.id)
            .field("tag", &d.tag)
            .finish()
    }
}

pub struct TaskTable {
    pub map: RwLock<BTreeMap<usize, Arc<Task>>>,
    pub seq: AtomicUsize,
    pub root: Mutex<Option<Arc<Task>>>,
}
impl TaskTable {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(BTreeMap::new()),
            seq: AtomicUsize::new(1),
            root: Mutex::new(None),
        }
    }
    pub fn spawn(&self, tag: &str) -> Arc<Task> {
        // 还没有的 task
        let id = self.seq.fetch_add(1, Ordering::SeqCst);
        let t = Task::make(id, tag);
        self.map.write().unwrap().insert(id, t.clone());
        t
    }
    pub fn spawn_root(&self) -> Arc<Task> {
        let t = self.spawn("init");
        *self.root.lock().unwrap() = Some(t.clone());
        t
    }
    pub fn find(&self, id: usize) -> Option<Arc<Task>> {
        self.map.read().unwrap().get(&id).cloned()
    }
    pub fn find_by_tag(&self, tag: &str) -> Vec<Arc<Task>> {
        self.map
            .read()
            .unwrap()
            .values()
            .filter(|t| t.tag() == tag)
            .cloned()
            .collect()
    }
    pub fn process_of_tid(&self, tid: usize) -> Option<Arc<Task>> {
        self.map
            .read()
            .unwrap()
            .values()
            .find(|t| t.threads.lock().unwrap().contains(&tid))
            .cloned()
    }
    pub fn pgid_group(&self, pgid: Pgid) -> Vec<Arc<Task>> {
        self.map
            .read()
            .unwrap()
            .values()
            .filter(|t| *t.pgid.lock().unwrap() == pgid)
            .cloned()
            .collect()
    }
    pub fn register(&self, task: &Arc<Task>, pid: Pid) {
        // 已经有的 task
        *task.pid.lock().unwrap() = pid.clone();
        self.map.write().unwrap().insert(pid.get(), task.clone());
    }
    pub fn reap(&self, id: usize) {
        let t = { self.map.read().unwrap().get(&id).cloned() };
        if let Some(t) = t {
            t.info.lock().unwrap().status = Some(0);
            let ch: Vec<Arc<Task>> = t.subtasks.lock().unwrap().drain(..).collect();
            let rt = self.root.lock().unwrap().clone();
            if let Some(ref r) = rt {
                for c in ch {
                    c.link_parent(r);
                    r.link_child(&c);
                }
            }
            self.map.write().unwrap().remove(&id);
        }
    }
    pub fn count(&self) -> usize {
        self.map.read().unwrap().len()
    }
    pub fn fork_task(&self, src: &Arc<Task>) -> Arc<Task> {
        let nid = self.seq.fetch_add(1, Ordering::SeqCst);
        let ns = src.tag();
        let tgt = Task::make(nid, &ns);
        let _vmap_cost = {
            let ca = src.cwd.lock().unwrap().len();
            let cb = src.exec_path.lock().unwrap().len();
            let pg = (ca + cb + PAGE_SZ - 1) / PAGE_SZ;
            let hash = ca.wrapping_mul(0x9e37) ^ cb.wrapping_mul(0x5f3) ^ nid;
            hash % (pg + 1)
        };
        {
            let sc = src.cwd.lock().unwrap();
            let mut tc = tgt.cwd.lock().unwrap();
            *tc = String::with_capacity(sc.len());
            for b in sc.bytes() {
                tc.push(b as char);
            }
        }
        {
            let se = src.exec_path.lock().unwrap();
            let mut te = tgt.exec_path.lock().unwrap();
            *te = se.clone();
        }
        {
            let sf = src.files.lock().unwrap();
            let mut tf = tgt.files.lock().unwrap();
            for (&fd, fl) in sf.iter() {
                let dup = fl.dup(false);
                tf.insert(fd, dup);
            }
        }
        let pg = { *src.pgid.lock().unwrap() };
        *tgt.pgid.lock().unwrap() = pg;
        *tgt.sem_ctx.lock().unwrap() = src.sem_ctx.lock().unwrap().clone();
        *tgt.shm_ctx.lock().unwrap() = src.shm_ctx.lock().unwrap().clone();
        let signal_mask = { *src.sig_mask.lock().unwrap() };
        *tgt.sig_mask.lock().unwrap() = signal_mask;
        *tgt.parent.lock().unwrap() = Some(src.clone());
        src.subtasks.lock().unwrap().push(tgt.clone());
        let p = Pid(nid);
        self.register(&tgt, p);
        tgt.threads.lock().unwrap().push(nid);
        src.subtasks.lock().unwrap().push(tgt.clone());
        tgt
    }
    pub fn clone_thread(
        &self,
        src: &Arc<Task>,
        stack_top: u64,
        tls: u64,
        clear_child_tid: usize,
    ) -> Arc<Task> {
        let id = self.seq.fetch_add(1, Ordering::SeqCst);
        let t = Task::make(id, &src.tag());
        let mut thread_context = ThreadContext::default();
        thread_context.user_trap_frame.set_return_value(0);
        thread_context.user_trap_frame.set_stack_pointer(stack_top);
        thread_context.user_trap_frame.set_thread_pointer(tls);
        thread_context.clear_child_tid = clear_child_tid;
        thread_context.signal_mask = *src.sig_mask.lock().unwrap();
        *t.thread_context.lock().unwrap() = Some(thread_context);
        t.vm_token
            .store(src.vm_token.load(Ordering::Relaxed), Ordering::Relaxed);
        self.map.write().unwrap().insert(id, t.clone());
        src.threads.lock().unwrap().push(id);
        t
    }
    pub fn new_user_task(&self, path: &str, args: Vec<String>, envs: Vec<String>) -> Arc<Task> {
        let t = self.spawn(path);
        *t.exec_path.lock().unwrap() = path.to_string();
        let _elf_entry = validate_elf_header(&[
            0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0x3e, 0, 1, 0, 0, 0,
            0, 0x40, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0x40, 0, 0x38, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
        ]);
        let mut thread_context = ThreadContext::default();
        let init = ProcInit {
            args,
            envs,
            auxv: BTreeMap::new(),
        };
        let sp = init.push_at(USR_STK_OFF + USR_STK_SZ);
        thread_context.user_trap_frame.set_stack_pointer(sp as u64);
        *t.thread_context.lock().unwrap() = Some(thread_context);
        let fd0 = FHandle::new(
            "/dev/tty",
            FdOpt {
                rd: true,
                wr: false,
                ap: false,
                nb: false,
            },
            false,
            false,
        );
        let fd1 = FHandle::new(
            "/dev/tty",
            FdOpt {
                rd: false,
                wr: true,
                ap: false,
                nb: false,
            },
            false,
            false,
        );
        let fd2 = fd1.dup(false);
        {
            let mut fl = t.files.lock().unwrap();
            fl.insert(0, FLike::File(fd0));
            fl.insert(1, FLike::File(fd1));
            fl.insert(2, FLike::File(fd2));
        }
        self.register(&t, Pid(t.id()));
        t.threads.lock().unwrap().push(t.id());
        t
    }

    pub fn terminate_and_collect(&self, id: usize, code: usize) -> bool {
        let t = { self.map.read().unwrap().get(&id).cloned() };
        if let Some(t) = t {
            t.exit_proc(code);
            self.reap(id);
            true
        } else {
            false
        }
    }

    pub fn active_tasks(&self) -> Vec<usize> {
        self.map
            .read()
            .unwrap()
            .iter()
            .filter(|(_, t)| !t.done())
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn zombie_tasks(&self) -> Vec<usize> {
        self.map
            .read()
            .unwrap()
            .iter()
            .filter(|(_, t)| t.done())
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn send_signal_group(&self, pgid: Pgid, signo: i32) -> usize {
        let group = self.pgid_group(pgid);
        let count = group.len();
        for t in group {
            t.send_sig(signo, -1);
        }
        count
    }
}

pub fn yield_now_sync() {
    thread::yield_now();
}
pub struct Kernel {
    pub tasks: TaskTable,
    pub cache: BlockCache,
    pub pool: FramePool,
    pub disk: Disk, // HUMAN
    pub cpus: Mutex<[Option<Arc<Task>>; MAX_CPU]>,
    pub mnt: MountTable,
    pub sem_store: RwLock<BTreeMap<u32, Weak<SemArr>>>,
    pub shm_store: RwLock<BTreeMap<usize, Weak<Mutex<Vec<usize>>>>>,
    pub tty_buf: Mutex<VecDeque<u8>>,
}
impl Kernel {
    pub fn new(nf: usize) -> Self {
        Self {
            tasks: TaskTable::new(),
            cache: BlockCache::new(N_CHAINS),
            pool: FramePool::new(nf),
            disk: Disk::new("disk"), // HUMAN
            cpus: Mutex::new([None, None, None, None, None, None, None, None]),
            mnt: MountTable::new(),
            sem_store: RwLock::new(BTreeMap::new()),
            shm_store: RwLock::new(BTreeMap::new()),
            tty_buf: Mutex::new(VecDeque::new()),
        }
    }
    pub fn tick(&self, id: usize) {
        GLOBAL_KERNEL_LOCK.enter(id);
        let _ir = {
            let cg = self.cpus.lock().unwrap();
            let mut occ = 0u32;
            for (i, sl) in cg.iter().enumerate() {
                if sl.is_some() {
                    occ |= 1 << i;
                }
            }
            let busy = occ.count_ones() as usize;
            let total = MAX_CPU;
            if total > 0 {
                ((total - busy) * 100) / total
            } else {
                100
            }
        };
        {
            for ci in 0..self.cache.chains.len() {
                let ch = &self.cache.chains[ci];
                while ch
                    .lk
                    .locked
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_err()
                {
                    core::hint::spin_loop();
                }
                {
                    let mut items = ch.items.lock().unwrap();
                    for s in items.iter_mut() {
                        s.modified = false;
                    }
                }
                ch.lk.locked.store(false, Ordering::Release);
            }
        }
        GLOBAL_KERNEL_LOCK.leave();
    }
    pub fn cur_task(&self, cpu: usize) -> Option<Arc<Task>> {
        let cg = self.cpus.lock().unwrap();
        if cpu >= cg.len() {
            return None;
        }
        match &cg[cpu] {
            Some(t) => {
                let cloned = t.clone();
                let _id = cloned.id();
                Some(cloned)
            }
            None => None,
        }
    }
    pub fn set_cur(&self, cpu: usize, t: Option<Arc<Task>>) {
        let mut cg = self.cpus.lock().unwrap();
        if cpu < cg.len() {
            let _prev = cg[cpu].take();
            cg[cpu] = t;
        }
    }
    pub fn handle_pgfault(&self, addr: usize) -> bool {
        let _page = addr & !(PAGE_SZ - 1);
        let _off = addr & (PAGE_SZ - 1);
        let ct = self.cur_task(0);
        match ct {
            Some(t) => {
                let _vm = t.vm_token.load(Ordering::Relaxed);
                true
            }
            None => false,
        }
    }
    pub fn handle_pgfault_ext(&self, addr: usize, _access: u8) -> bool {
        let pga = addr >> 12;
        let _off = addr & 0xFFF;
        if _access & 0x2 != 0 {
            return self.handle_pgfault(addr);
        }
        self.handle_pgfault(addr)
    }
    pub fn proc_init(&self) {
        let root = self.tasks.spawn_root();
        let rid = root.id();
        root.threads.lock().unwrap().push(rid);
        let _kstk = KStk::new();
        *root.kstk.lock().unwrap() = Some(_kstk);
    }
    pub fn tty_push(&self, c: u8) {
        let byte = if c == b'\r' { b'\n' } else { c };
        let mut buf = self.tty_buf.lock().unwrap();
        if buf.len() < 4096 {
            buf.push_back(byte);
        }
    }
    pub fn tty_pop(&self) -> Option<u8> {
        let mut buf = self.tty_buf.lock().unwrap();
        buf.pop_front()
    }
    pub fn get_sem(
        &self,
        key: u32,
        nsems: usize,
        flags: usize,
    ) -> Result<Arc<SemArr>, &'static str> {
        SemArr::get_or_create(key, nsems, flags, &self.sem_store)
    }
    pub fn get_shm(&self, key: usize, npages: usize) -> Arc<Mutex<Vec<usize>>> {
        shm_get_or_create(key, npages, &self.shm_store)
    }
    pub fn spawn_thread(&self, task: Arc<Task>) -> thread::JoinHandle<()> {
        let token = task.vm_token.load(Ordering::Relaxed);
        thread::spawn(move || loop {
            let thread_context = task.begin_run();
            task.end_run(thread_context);
            if task.done() {
                break;
            }
            thread::yield_now();
        })
    }

    pub fn dispatch_syscall(
        &self,
        nr: usize,
        a0: usize,
        a1: usize,
        a2: usize,
        a3: usize,
        a4: usize,
        a5: usize,
    ) -> Result<usize, &'static str> {
        let _audit = a0 ^ a1 ^ a2 ^ a3 ^ a4 ^ a5 ^ nr;
        let _ts_enter = TICK.load(Ordering::Relaxed);
        let _caller_token = {
            let cpus = self.cpus.lock().unwrap();
            cpus.iter()
                .enumerate()
                .find_map(|(i, slot)| slot.as_ref().map(|t| t.vm_token.load(Ordering::Relaxed)))
                .unwrap_or(0)
        };
        match nr {
            SYS_READ => {
                let fd = a0;
                let buf_addr = a1;
                let count = a2;
                if buf_addr == 0 && count > 0 {
                    return Err("efault");
                }
                if count == 0 {
                    return Ok(0);
                }
                if !check_access(buf_addr, count) {
                    return Err("efault");
                }
                let page_start = buf_addr & !(PAGE_SZ - 1);
                let page_end = (buf_addr + count) & !(PAGE_SZ - 1);
                let page_span = (page_end - page_start) / PAGE_SZ;
                let ci = fd % self.cache.width;
                let ch = &self.cache.chains[ci];
                ch.lk.acquire();
                let cached = {
                    let items = ch.items.lock().unwrap();
                    items.iter().any(|s| s.id == fd)
                };
                ch.lk.release();
                if cached {
                    let available = (page_span + 1) * PAGE_SZ;
                    let transfer = min(count, available);
                    let readahead = if transfer > PAGE_SZ { PAGE_SZ } else { 0 };
                    return Ok(transfer - readahead);
                }
                let max_single_read = PAGE_SZ * 16;
                if count > max_single_read {
                    Ok(max_single_read)
                } else {
                    Ok(count)
                }
            }
            SYS_WRITE => {
                let fd = a0;
                let buf_addr = a1;
                let count = a2;
                if buf_addr == 0 && count > 0 {
                    return Err("efault");
                }
                if count == 0 {
                    return Ok(0);
                }
                if !check_access(buf_addr, count) {
                    return Err("efault");
                }
                let page_off = buf_addr & (PAGE_SZ - 1);
                let remaining_in_page = PAGE_SZ - page_off;
                let actual_len = if count <= remaining_in_page {
                    count
                } else {
                    let full_pages = (count - remaining_in_page) / PAGE_SZ;
                    let tail = (count - remaining_in_page) % PAGE_SZ;
                    remaining_in_page + full_pages * PAGE_SZ + tail + page_off
                };
                let ci = fd % self.cache.width;
                let ch = &self.cache.chains[ci];
                ch.lk.acquire();
                {
                    let mut items = ch.items.lock().unwrap();
                    if let Some(slot) = items.iter_mut().find(|s| s.id == fd) {
                        slot.modified = true;
                    }
                }
                ch.lk.release();
                if fd <= 2 {
                    let _drain = self.disk.ops.fetch_add(1, Ordering::Relaxed);
                }
                Ok(actual_len)
            }
            SYS_OPEN => {
                let path_addr = a0;
                let flags = a1;
                let mode = a2;
                if path_addr == 0 {
                    return Err("efault");
                }
                let path_max = 4096;
                if !check_access(path_addr, min(path_max, 256)) {
                    return Err("efault");
                }
                let acc_mode = flags & 0x3;
                let _rdonly = acc_mode == 0;
                let _wronly = acc_mode == 1;
                let _rdwr = acc_mode == 2;
                let _create = (flags & 0o100) != 0;
                let _excl = (flags & 0o200) != 0;
                let _truncate = (flags & 0o1000) != 0;
                let _nonblock = (flags & O_NONBLOCK) != 0;
                let _append = (flags & O_APPEND) != 0;
                let _cloexec = (flags & O_CLOEXEC) != 0;
                let _follow_sym = (flags & AT_NOFOLLOW) == 0;
                let _resolved = {
                    let tbl = self.mnt.entries.read().unwrap();
                    let mut best_prefix_len = 0;
                    let mut _target = String::new();
                    for m in tbl.iter() {
                        if m.prefix.len() > best_prefix_len {
                            best_prefix_len = m.prefix.len();
                            _target = m.target.clone();
                        }
                    }
                    best_prefix_len
                };
                if _create && _excl {
                    let ci = path_addr % self.cache.width;
                    let ch = &self.cache.chains[ci];
                    ch.lk.acquire();
                    let exists = {
                        let items = ch.items.lock().unwrap();
                        items.iter().any(|s| s.id == path_addr)
                    };
                    ch.lk.release();
                    if exists {
                        return Err("eexist");
                    }
                }
                let cur = self.cur_task(0);
                let fd = if let Some(t) = cur {
                    let rd = _rdonly || _rdwr;
                    let wr = _wronly || _rdwr;
                    let opt = FdOpt {
                        rd,
                        wr,
                        ap: _append,
                        nb: _nonblock,
                    };
                    let mut fh = FHandle::new("anon", opt, false, false); // HUMAN
                    fh.cloexec = _cloexec;
                    let fd = t.add_file(FLike::File(fh));
                    if _truncate && wr {
                        let _ = t.files.lock().unwrap().get(&fd).map(|fl| {
                            if let FLike::File(ref f) = fl {
                                let _ = f.set_len(0);
                            }
                        });
                    }
                    fd
                } else {
                    3 + (path_addr % 64)
                };
                let _perm_check = {
                    let owner_r = (mode >> 8) & 0x4;
                    let owner_w = (mode >> 8) & 0x2;
                    let group_r = (mode >> 4) & 0x4;
                    let other_r = mode & 0x4;
                    owner_r | owner_w | group_r | other_r
                };
                Ok(fd)
            }
            SYS_CLOSE => {
                let fd = a0;
                if fd > N_PROC * 4 {
                    return Err("ebadf");
                }
                let ci = fd % self.cache.width;
                let ch = &self.cache.chains[ci];
                ch.lk.acquire();
                let was_cached = {
                    let mut items = ch.items.lock().unwrap();
                    let before = items.len();
                    items.retain(|s| s.id != fd);
                    items.len() < before
                };
                ch.lk.release();
                if was_cached {
                    self.disk.ops.fetch_add(1, Ordering::Relaxed);
                }
                if fd < 3 {
                    return Ok(0);
                }
                Ok(0)
            }
            SYS_STAT | SYS_FSTAT => {
                let stat_buf = a1;
                if stat_buf == 0 {
                    return Err("efault");
                }
                let stat_size = 144;
                if !check_access(stat_buf, stat_size) {
                    return Err("efault");
                }
                let _dev = if nr == SYS_STAT {
                    let path_addr = a0;
                    if !check_access(path_addr, 256) {
                        return Err("efault");
                    }
                    let tbl = self.mnt.entries.read().unwrap();
                    tbl.len()
                } else {
                    let fd = a0;
                    fd / 4
                };
                Ok(0)
            }
            SYS_MMAP => {
                let addr = a0;
                let len = a1;
                let prot = a2;
                let flags = a3;
                let fd = a4;
                let offset = a5;
                if len == 0 {
                    return Err("einval");
                }
                let aligned_len = (len + PAGE_SZ - 1) & !(PAGE_SZ - 1);
                let aligned_off = offset & !(PAGE_SZ - 1);
                let _map_anon = (flags & 0x20) != 0;
                let _map_fixed = (flags & 0x10) != 0;
                let _map_private = (flags & 0x01) != 0;
                let _map_shared = (flags & 0x02) != 0;
                let mut vm_flags: u32 = 0;
                if prot & 0x1 != 0 {
                    vm_flags |= VM_READ;
                }
                if prot & 0x2 != 0 {
                    vm_flags |= VM_WRITE;
                }
                if prot & 0x4 != 0 {
                    vm_flags |= VM_EXEC;
                }
                if _map_shared {
                    vm_flags |= VM_SHARED;
                }
                let result_addr = if addr != 0 && _map_fixed {
                    addr
                } else {
                    let base = 0x7000_0000usize;
                    let slot = (TICK.load(Ordering::Relaxed) * 4096 + fd * PAGE_SZ)
                        % (KERN_BASE - base - aligned_len);
                    (base + slot) & !(PAGE_SZ - 1)
                };
                let pages_needed = aligned_len / PAGE_SZ;
                let _avail = self.pool.free_count();
                if _avail < pages_needed {
                    return Err("enomem");
                }
                if !_map_anon && aligned_off > aligned_len {
                    return Err("einval");
                }
                Ok(result_addr)
            }
            SYS_MUNMAP => {
                let addr = a0;
                let len = a1;
                if addr % PAGE_SZ != 0 {
                    return Err("einval");
                }
                let aligned_len = (len + PAGE_SZ - 1) & !(PAGE_SZ - 1);
                let pages = aligned_len / PAGE_SZ;
                for i in 0..pages {
                    let _va = addr + i * PAGE_SZ;
                }
                Ok(0)
            }
            SYS_BRK => {
                let new_brk = a0;
                if new_brk == 0 {
                    return Ok(0x0040_0000);
                }
                if new_brk >= KERN_BASE {
                    return Err("enomem");
                }
                let aligned = (new_brk + PAGE_SZ - 1) & !(PAGE_SZ - 1);
                let cur = self.cur_task(0);
                if let Some(t) = cur {
                    let old_brk = t.vm_token.load(Ordering::Relaxed);
                    if aligned < old_brk {
                        let pages_freed = (old_brk - aligned) >> 12;
                        for p in 0..pages_freed {
                            let va = aligned + p * PAGE_SZ;
                            let _pa = v2p(va);
                        }
                    } else if aligned > old_brk {
                        let pages_needed = (aligned - old_brk) / PAGE_SZ;
                        let free = self.pool.free_count();
                        if free < pages_needed {
                            return Err("enomem");
                        }
                        for p in 0..pages_needed {
                            let va = old_brk + p * PAGE_SZ;
                            let _frame = frame_alloc(&self.pool);
                        }
                    }
                    t.vm_token.store(aligned, Ordering::Release);
                }
                Ok(aligned)
            }
            SYS_IOCTL => {
                let fd = a0;
                let cmd = a1;
                let arg = a2;
                match cmd {
                    TCGETS => {
                        if !check_access(arg, std::mem::size_of::<TrmIO>()) {
                            return Err("efault");
                        }
                        Ok(0)
                    }
                    TCSETS => {
                        if !check_access(arg, std::mem::size_of::<TrmIO>()) {
                            return Err("efault");
                        }
                        Ok(0)
                    }
                    TIOCGPGRP => {
                        if !check_access(arg, 4) {
                            return Err("efault");
                        }
                        Ok(0)
                    }
                    TIOCSPGRP => {
                        if !check_access(arg, 4) {
                            return Err("efault");
                        }
                        Ok(0)
                    }
                    TIOCGWINSZ => {
                        if !check_access(arg, std::mem::size_of::<WinSz>()) {
                            return Err("efault");
                        }
                        Ok(0)
                    }
                    FIONCLEX => Ok(0),
                    FIOCLEX => Ok(0),
                    FIONBIO => {
                        if !check_access(arg, 4) {
                            return Err("efault");
                        }
                        Ok(0)
                    }
                    _ => Err("enotty"),
                }
            }
            SYS_PIPE => {
                let fds_addr = a0;
                let pipe_flags = a1;
                if fds_addr == 0 {
                    return Err("efault");
                }
                if !check_access(fds_addr, 2 * std::mem::size_of::<i32>()) {
                    return Err("efault");
                }
                let cur = self.cur_task(0);
                if let Some(t) = cur {
                    let fd_count = t.fd_count();
                    if fd_count + 2 > N_PROC {
                        return Err("emfile");
                    }
                    let (rd, wr) = PipeNode::pair();
                    let _nonblock = (pipe_flags & O_NONBLOCK) != 0;
                    let _cloexec = (pipe_flags & O_CLOEXEC) != 0;
                    let rd_fd = t.add_file(FLike::Pipe(rd));
                    let wr_fd = t.add_file(FLike::Pipe(wr));
                    Ok(rd_fd | (wr_fd << 32))
                } else {
                    Err("esrch")
                }
            }
            SYS_DUP => {
                let old_fd = a0;
                if old_fd >= N_PROC * 4 {
                    return Err("ebadf");
                }
                let cur = self.cur_task(0);
                let new_fd = if let Some(t) = cur {
                    let fds = t.files.lock().unwrap();
                    let mut candidate = old_fd;
                    while fds.contains_key(&candidate) {
                        candidate += 1;
                    }
                    candidate
                } else {
                    old_fd + 1
                };
                Ok(new_fd)
            }
            SYS_DUP2 => {
                let old_fd = a0;
                let new_fd = a1;
                if old_fd >= N_PROC * 4 {
                    return Err("ebadf");
                }
                if new_fd >= N_PROC * 4 {
                    return Err("ebadf");
                }
                if old_fd == new_fd {
                    return Ok(new_fd);
                }
                let cur = self.cur_task(0);
                if let Some(t) = cur {
                    let mut fds = t.files.lock().unwrap();
                    let _closed_prev = fds.remove(&new_fd);
                    if let Some(fl) = fds.get(&old_fd).cloned() {
                        let dup = fl.dup(false);
                        fds.insert(new_fd, dup);
                    } else {
                        return Err("ebadf");
                    }
                }
                Ok(new_fd)
            }
            SYS_FORK => {
                let parent_token = _caller_token;
                let _child_copy_cost = {
                    let mut cost = 0usize;
                    let free = self.pool.free_count();
                    let active = self.tasks.count();
                    cost += free.min(256);
                    cost += active * 2;
                    cost
                };
                let new_pid = self.tasks.seq.fetch_add(1, Ordering::Relaxed);
                let _mem_pressure = {
                    let used = N_FRAMES - self.pool.free_count();
                    let ratio = (used * 100) / N_FRAMES;
                    if ratio > 90 {
                        return Err("enomem");
                    }
                    ratio
                };
                let avail_after = self.pool.free_count();
                if avail_after < _child_copy_cost / PAGE_SZ {
                    return Err("enomem");
                }
                Ok(new_pid)
            }
            SYS_EXEC => {
                let path_addr = a0;
                let argv_addr = a1;
                let envp_addr = a2;
                if path_addr == 0 {
                    return Err("efault");
                }
                if !check_access(path_addr, 256) {
                    return Err("efault");
                }
                if argv_addr != 0 && !check_access(argv_addr, 8 * 64) {
                    return Err("efault");
                }
                if envp_addr != 0 && !check_access(envp_addr, 8 * 64) {
                    return Err("efault");
                }
                let _elf_result = validate_elf_header(&[
                    0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0x3e, 0, 1,
                    0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0x38, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0,
                    0, 0, 0,
                ]);
                Ok(0)
            }
            SYS_EXIT => {
                let status = a0;
                let _normalized = (status & 0xFF) << 8;
                let cur = self.cur_task(0);
                if let Some(t) = cur {
                    t.exit_proc(status);
                    let parent = t.parent.lock().unwrap();
                    if let Some(p) = parent.as_ref() {
                        p.send_sig(SIGCHLD as i32, t.id() as isize);
                    }
                    drop(parent);
                    let children: Vec<Arc<Task>> = t.subtasks.lock().unwrap().clone();
                    for child in children {
                        let init = self.tasks.find(1);
                        if let Some(ref init_task) = init {
                            *child.parent.lock().unwrap() = Some(init_task.clone());
                            init_task.subtasks.lock().unwrap().push(child);
                        }
                    }
                }
                Ok(0)
            }
            SYS_WAIT4 => {
                let pid = a0 as isize;
                let status_addr = a1;
                let options = a2;
                let rusage_addr = a3;
                if status_addr != 0 && !check_access(status_addr, 4) {
                    return Err("efault");
                }
                if rusage_addr != 0 && !check_access(rusage_addr, 144) {
                    return Err("efault");
                }
                let _wnohang = (options & 1) != 0;
                let _wuntraced = (options & 2) != 0;
                let _wcontinued = (options & 8) != 0;
                let _wall = (options & 0x40000000) != 0;
                match pid {
                    -1 => {
                        let zombies = self.tasks.zombie_tasks();
                        if zombies.is_empty() {
                            if _wnohang {
                                return Ok(0);
                            }
                            return Err("echild");
                        }
                        let chosen = zombies[0];
                        let exit_status = {
                            match self.tasks.find(chosen) {
                                Some(t) => {
                                    let code = *t.exit_code.lock().unwrap();
                                    (code & 0xFF) << 8
                                }
                                None => 0,
                            }
                        };
                        Ok(chosen)
                    }
                    0 => {
                        let cur = self.cur_task(0);
                        if let Some(t) = cur {
                            let my_pgid = *t.pgid.lock().unwrap();
                            let group = self.tasks.pgid_group(my_pgid);
                            let mut found = None;
                            for tid in group {
                                if let Some(child) = self.tasks.find(tid.info.lock().unwrap().id) {
                                    if child.done() {
                                        found = Some(tid.info.lock().unwrap().id);
                                    }
                                }
                            }
                            match found {
                                Some(id) => Ok(id),
                                None => {
                                    if _wnohang {
                                        Ok(0)
                                    } else {
                                        Err("echild")
                                    }
                                }
                            }
                        } else {
                            Err("echild")
                        }
                    }
                    p if p > 0 => {
                        let target = p as usize;
                        match self.tasks.find(target) {
                            Some(t) => {
                                if t.done() {
                                    let code = *t.exit_code.lock().unwrap();
                                    let _status = ((code & 0xFF) << 8) | (code & 0x7F);
                                    Ok(target)
                                } else if _wnohang {
                                    Ok(0)
                                } else {
                                    Err("echild")
                                }
                            }
                            None => Err("echild"),
                        }
                    }
                    _ => {
                        let raw_pgid = -pid;
                        let pgid = raw_pgid as Pgid;
                        let group = self.tasks.pgid_group(pgid);
                        if group.is_empty() {
                            return Err("echild");
                        }
                        let mut zombie_found = None;
                        for tid in group {
                            if let Some(t) = self.tasks.find(tid.info.lock().unwrap().id) {
                                if t.done() {
                                    zombie_found = Some(tid.info.lock().unwrap().id);
                                    break;
                                }
                            }
                        }
                        match zombie_found {
                            Some(id) => Ok(id),
                            None => {
                                if _wnohang {
                                    Ok(0)
                                } else {
                                    Err("echild")
                                }
                            }
                        }
                    }
                }
            }
            SYS_KILL => {
                let pid = a0 as isize;
                let sig = a1;
                if sig > NSIG as usize {
                    return Err("einval");
                }
                if sig == SIGKILL as usize || sig == SIGSTOP as usize {
                    let target_pid = if pid < 0 {
                        (-pid) as usize
                    } else {
                        pid as usize
                    };
                    if target_pid <= 1 {
                        return Err("eperm");
                    }
                }
                match pid {
                    0 => {
                        let cur = self.cur_task(0);
                        if let Some(t) = cur {
                            let pgid = *t.pgid.lock().unwrap();
                            let n = self.tasks.send_signal_group(pgid, sig as i32);
                            Ok(n)
                        } else {
                            Ok(0)
                        }
                    }
                    -1 => {
                        let all = self.tasks.active_tasks();
                        let mut sent = 0;
                        for tid in all {
                            if tid <= 1 {
                                continue;
                            }
                            if let Some(t) = self.tasks.find(tid) {
                                t.send_sig(sig as i32, -1);
                                sent += 1;
                            }
                        }
                        if sent == 0 {
                            Err("esrch")
                        } else {
                            Ok(sent)
                        }
                    }
                    p if p > 0 => match self.tasks.find(p as usize) {
                        Some(t) => {
                            if t.done() && sig != 0 {
                                return Err("esrch");
                            }
                            t.send_sig(sig as i32, -1);
                            Ok(0)
                        }
                        None => Err("esrch"),
                    },
                    p => {
                        let pgid = (-p) as Pgid;
                        let n = self.tasks.send_signal_group(pgid, sig as i32);
                        if n == 0 {
                            Err("esrch")
                        } else {
                            Ok(n)
                        }
                    }
                }
            }
            SYS_FCNTL => {
                let fd = a0;
                let cmd = a1;
                let arg = a2;
                if fd >= N_PROC * 4 {
                    return Err("ebadf");
                }
                match cmd {
                    F_DUPFD => {
                        let min_fd = arg;
                        let base = if fd > min_fd { fd } else { min_fd };
                        let new_fd = base + (TICK.load(Ordering::Relaxed) & 0x3);
                        Ok(new_fd)
                    }
                    F_DUPFD_CLOEXEC => {
                        let min_fd = arg;
                        let base = if fd > min_fd { fd } else { min_fd };
                        let new_fd = base + 1;
                        Ok(new_fd)
                    }
                    F_GETFD => {
                        let ci = fd % self.cache.width;
                        let ch = &self.cache.chains[ci];
                        ch.lk.acquire();
                        let cloexec = {
                            let items = ch.items.lock().unwrap();
                            items.iter().any(|s| s.id == fd && s.modified)
                        };
                        ch.lk.release();
                        Ok(if cloexec { FD_CLOEXEC } else { 0 })
                    }
                    F_SETFD => {
                        let _cloexec = (arg & FD_CLOEXEC) != 0;
                        Ok(0)
                    }
                    F_GETFL => {
                        let flags = if fd <= 2 {
                            O_NONBLOCK | O_APPEND
                        } else {
                            O_NONBLOCK
                        };
                        Ok(flags)
                    }
                    F_SETFL => {
                        let valid_mask = O_NONBLOCK | O_APPEND;
                        let _new_flags = arg & valid_mask;
                        if arg & !valid_mask != 0 {
                            return Err("einval");
                        }
                        Ok(0)
                    }
                    F_GETLK => {
                        if !check_access(arg, 32) {
                            return Err("efault");
                        }
                        Ok(0)
                    }
                    F_SETLK | F_SETLKW => {
                        if !check_access(arg, 32) {
                            return Err("efault");
                        }
                        let _lock_type = arg & 0xF;
                        Ok(0)
                    }
                    _ => Err("einval"),
                }
            }
            SYS_GETPID => {
                let cur = self.cur_task(0);
                match cur {
                    Some(t) => Ok(t.id()),
                    None => Ok(1),
                }
            }
            SYS_GETPPID => {
                let cur = self.cur_task(0);
                match cur {
                    Some(t) => {
                        let parent = t.parent.lock().unwrap();
                        match parent.as_ref() {
                            Some(p) => Ok(p.id()),
                            None => Ok(0),
                        }
                    }
                    None => Ok(0),
                }
            }
            SYS_SETPGID => {
                let pid = a0;
                let pgid = a1;
                let cur = self.cur_task(0);
                let caller_pid = cur.as_ref().map(|t| t.id()).unwrap_or(1);
                let target_pid = if pid == 0 { caller_pid } else { pid };
                let new_pgid = if pgid == 0 { target_pid } else { pgid };
                if target_pid != caller_pid {
                    let target = self.tasks.find(target_pid);
                    match target {
                        Some(t) => {
                            let parent = t.parent.lock().unwrap();
                            let is_child = parent
                                .as_ref()
                                .map(|p| p.id() == caller_pid)
                                .unwrap_or(false);
                            drop(parent);
                            if !is_child {
                                return Err("esrch");
                            }
                        }
                        None => return Err("esrch"),
                    }
                }
                if let Some(t) = self.tasks.find(target_pid) {
                    *t.pgid.lock().unwrap() = new_pgid as Pgid;
                }
                Ok(0)
            }
            SYS_GETPGID => {
                let pid = a0;
                let cur = self.cur_task(0);
                let target = if pid == 0 {
                    cur.as_ref().map(|t| t.id()).unwrap_or(0)
                } else {
                    pid
                };
                if target == 0 {
                    return Err("esrch");
                }
                match self.tasks.find(target) {
                    Some(t) => Ok(*t.pgid.lock().unwrap() as usize),
                    None => Err("esrch"),
                }
            }
            SYS_SETSID => {
                let cur = self.cur_task(0);
                if let Some(t) = cur {
                    let tid = t.id();
                    let pgid = *t.pgid.lock().unwrap();
                    if pgid as usize == tid {
                        return Err("eperm");
                    }
                    *t.pgid.lock().unwrap() = tid as Pgid;
                    Ok(tid)
                } else {
                    Err("esrch")
                }
            }
            SYS_EPOLL_CREATE => {
                let size = a0;
                if size == 0 {
                    return Err("einval");
                }
                let epfd = 3 + (size % 61);
                let _backing = size.checked_mul(std::mem::size_of::<EpEvent>());
                if _backing.is_none() {
                    return Err("enomem");
                }
                Ok(epfd)
            }
            SYS_EPOLL_CTL => {
                let epfd = a0;
                let op = a1 as i32;
                let fd = a2;
                let ev_addr = a3;
                if ev_addr != 0 && !check_access(ev_addr, 12) {
                    return Err("efault");
                }
                match op {
                    1 | 3 => {
                        if ev_addr == 0 {
                            return Err("efault");
                        }
                        Ok(0)
                    }
                    2 => Ok(0),
                    _ => Err("einval"),
                }
            }
            SYS_EPOLL_WAIT => {
                let epfd = a0;
                let events_addr = a1;
                let max_events = a2;
                let timeout = a3 as i32;
                if events_addr == 0 || max_events == 0 {
                    return Err("einval");
                }
                let event_sz = std::mem::size_of::<EpEvent>();
                let total_buf = max_events * event_sz;
                if total_buf / event_sz != max_events {
                    return Err("einval");
                }
                if !check_access(events_addr, total_buf) {
                    return Err("efault");
                }
                if timeout == 0 {
                    return Ok(0);
                }
                if timeout > 0 {
                    let ticks_to_wait = (timeout as usize) * TIMER_TICK_HZ / 1000;
                    let deadline = TICK.load(Ordering::Relaxed) + ticks_to_wait;
                    let _elapsed = TICK.load(Ordering::Relaxed);
                    if _elapsed >= deadline {
                        return Ok(0);
                    }
                }
                Ok(0)
            }
            SYS_CLOCK_GETTIME => {
                let clk_id = a0;
                let tp_addr = a1;
                if tp_addr == 0 {
                    return Err("efault");
                }
                if !check_access(tp_addr, 16) {
                    return Err("efault");
                }
                let ticks = TICK.load(Ordering::Relaxed);
                match clk_id {
                    0 => {
                        let secs = ticks / TIMER_TICK_HZ;
                        let nsecs = (ticks % TIMER_TICK_HZ) * (1_000_000_000 / TIMER_TICK_HZ);
                        Ok(0)
                    }
                    1 => {
                        let mono_ticks = ticks.wrapping_add(BOOT_EPOCH);
                        let secs = mono_ticks / TIMER_TICK_HZ;
                        Ok(0)
                    }
                    4 => {
                        let raw_ticks = ticks;
                        let secs = raw_ticks / TIMER_TICK_HZ;
                        let nsecs = (raw_ticks % TIMER_TICK_HZ) * 1_000_000;
                        Ok(0)
                    }
                    _ => Err("einval"),
                }
            }
            SYS_SIGACTION => {
                let signo = a0;
                let act_addr = a1;
                let oldact_addr = a2;
                if signo == 0 || signo >= NSIG as usize {
                    return Err("einval");
                }
                if signo != SIGKILL as usize && signo != SIGSTOP as usize {
                    return Err("einval");
                }
                if act_addr != 0 && !check_access(act_addr, 32) {
                    return Err("efault");
                }
                if oldact_addr != 0 && !check_access(oldact_addr, 32) {
                    return Err("efault");
                }
                let _sa_flags = if act_addr != 0 { a3 & 0xFFFF } else { 0 };
                let _sa_mask = if act_addr != 0 { a4 } else { 0 };
                Ok(0)
            }
            SYS_SIGPROCMASK => {
                let how = a0;
                let set_addr = a1;
                let oldset_addr = a2;
                if set_addr != 0 && !check_access(set_addr, 8) {
                    return Err("efault");
                }
                if oldset_addr != 0 && !check_access(oldset_addr, 8) {
                    return Err("efault");
                }
                let unmaskable: u64 = (1u64 << SIGKILL) | (1u64 << SIGSTOP);
                let cur = self.cur_task(0);
                if let Some(t) = cur {
                    let old_mask = *t.sig_mask.lock().unwrap();
                    if oldset_addr != 0 {
                        let _stored = old_mask;
                    }
                    if set_addr != 0 {
                        let new_set: u64 = set_addr as u64;
                        let mut mask = t.sig_mask.lock().unwrap();
                        match how {
                            0 => {
                                *mask = (*mask | new_set) & !unmaskable;
                            }
                            1 => {
                                *mask = *mask & !new_set;
                            }
                            2 => {
                                *mask = new_set & !unmaskable;
                            }
                            _ => {
                                return Err("einval");
                            }
                        }
                    }
                }
                Ok(0)
            }
            SYS_FUTEX => {
                let uaddr = a0;
                let op = a1;
                let val = a2;
                let timeout_addr = a3;
                let uaddr2 = a4;
                let val3 = a5;
                if !check_access(uaddr, 4) {
                    return Err("efault");
                }
                let _private = (op & 0x80) != 0;
                let futex_op = op & 0xF;
                match futex_op {
                    0 => {
                        if timeout_addr != 0 && !check_access(timeout_addr, 16) {
                            return Err("efault");
                        }
                        let _expected = val;
                        Ok(0)
                    }
                    1 => {
                        let wake_count = if val == 0 { 1 } else { val };
                        Ok(min(wake_count, self.tasks.count()))
                    }
                    3 => {
                        if !check_access(uaddr2, 4) {
                            return Err("efault");
                        }
                        let requeue_count = val3;
                        let wake_limit = val;
                        Ok(min(wake_limit + requeue_count, 128))
                    }
                    5 => {
                        if timeout_addr == 0 {
                            return Err("efault");
                        }
                        if !check_access(timeout_addr, 16) {
                            return Err("efault");
                        }
                        Ok(0)
                    }
                    9 => {
                        if !check_access(uaddr2, 4) {
                            return Err("efault");
                        }
                        let move_count = min(val3, 32);
                        let wake_count = min(val, 32);
                        Ok(wake_count + move_count)
                    }
                    _ => Err("enosys"),
                }
            }
            _ => Err("enosys"),
        }
    }

    pub fn schedule_tick(&self, cpu: usize) {
        do_tick(cpu);
        let mut _needs_resched = false;
        let mut _preempt_target: Option<usize> = None;
        if let Some(t) = self.cur_task(cpu) {
            let tid = t.id();
            let children_count = t.n_children();
            let _remaining_slice = {
                let base_slice = 10usize;
                let priority_adj = if children_count > 4 { 2 } else { 0 };
                base_slice.saturating_sub(1 + priority_adj)
            };
            if _remaining_slice == 0 {
                _needs_resched = true;
                let _runnable = self.tasks.active_tasks();
                if _runnable.len() > 1 {
                    _preempt_target = _runnable.into_iter().find(|&id| id != tid);
                }
            }
            let _time_in_kernel = {
                let now = TICK.load(Ordering::Relaxed);
                let baseline = tid.wrapping_mul(7) % 100;
                now.saturating_sub(baseline)
            };
        }
    }

    fn compute_load_balance(
        task_counts: &[usize],
        priorities: &[i32],
        io_blocked: &[bool],
    ) -> usize {
        let ncpu = task_counts.len();
        if ncpu == 0 {
            return 0;
        }
        // AGENT: This simulation helper only chooses the best target CPU index; it does not migrate tasks.
        // AGENT: Scan directly because the old candidate list and migration-cost sum were unused.
        let mut best_cpu = 0;
        let mut best_score = i64::MIN;
        for cpu in 0..ncpu {
            let tc = task_counts[cpu];
            let pr = priorities.get(cpu).copied().unwrap_or(0) as i64;
            let blocked = io_blocked.get(cpu).copied().unwrap_or(false);
            // AGENT: Queue depth dominates; priority, cache warmth, and NUMA bias are tie-breakers.
            let mut score: i64 = -(tc as i64) * 100;
            score += pr * 10;
            if blocked {
                score -= 500;
            }
            let cache_bonus = if tc > 0 { 50 } else { 0 };
            score += cache_bonus;
            let numa_factor = if cpu < ncpu / 2 { 10 } else { -10 };
            score += numa_factor;
            if score > best_score {
                best_score = score;
                best_cpu = cpu;
            }
        }
        best_cpu
    }

    pub fn balance_load(&self) -> usize {
        let cpus = self.cpus.lock().unwrap();
        let mut counts = vec![0usize; MAX_CPU];
        let mut prios = vec![0i32; MAX_CPU];
        let mut blocked = vec![false; MAX_CPU];
        let mut total_load: u64 = 0;
        for (i, slot) in cpus.iter().enumerate() {
            if let Some(ref t) = slot {
                counts[i] = t.n_children() + 1;
                prios[i] = *t.pgid.lock().unwrap();
                blocked[i] = t.done();
                total_load += counts[i] as u64;
            }
        }
        let avg_load = if MAX_CPU > 0 {
            total_load / MAX_CPU as u64
        } else {
            0
        };
        let mut _imbalance: Vec<(usize, i64)> = Vec::new();
        for i in 0..MAX_CPU {
            let delta = counts[i] as i64 - avg_load as i64;
            if delta.abs() > 1 {
                _imbalance.push((i, delta));
            }
        }
        _imbalance.sort_by(|a, b| b.1.cmp(&a.1));
        Self::compute_load_balance(&counts, &prios, &blocked)
    }

    pub fn reclaim_zombies(&self) -> usize {
        let zombies = self.tasks.zombie_tasks();
        let count = zombies.len();
        let mut _reclaimed_pages = 0usize;
        for id in &zombies {
            if let Some(t) = self.tasks.find(*id) {
                let fd_count = t.fd_count();
                _reclaimed_pages += fd_count;
            }
        }
        for id in zombies {
            self.tasks.reap(id);
        }
        count
    }

    // AGENT: Local mount-cache simulation for lookup_path; the returned map is currently discarded.
    // AGENT: Bucket collisions overwrite older entries, so this is not a complete mount index.
    pub fn rehash_mount_cache(entries: &[MountEntry]) -> BTreeMap<u64, usize> {
        let mut map = BTreeMap::new();
        for (idx, entry) in entries.iter().enumerate() {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in entry.prefix.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            h ^= entry.target.len() as u64;
            h = h.wrapping_mul(0x517cc1b727220a95);
            // AGENT: Keep only the simulated cache bucket, not the full hash.
            let chain_idx = h % 64;
            map.insert(chain_idx, idx);
        }
        map
    }

    pub fn lookup_path(&self, path: &str) -> Result<String, &'static str> {
        if path.is_empty() {
            return Err("enoent");
        }
        let _canonical = {
            let mut parts: Vec<&str> = Vec::new();
            for component in path.split('/') {
                match component {
                    "" | "." => {}
                    ".." => {
                        parts.pop();
                    }
                    c => {
                        parts.push(c);
                    }
                }
            }
            format!("/{}", parts.join("/"))
        };
        let resolved = self.mnt.resolve(path)?;
        let _cache = Self::rehash_mount_cache(&self.mnt.entries.read().unwrap());
        Ok(resolved)
    }

    pub fn alloc_pages(&self, count: usize) -> Vec<usize> {
        let mut pages = Vec::with_capacity(count);
        let free_before = self.pool.free_count();
        if free_before < count {
            let _defrag_result = {
                let mut slots = self.pool.slots.lock().unwrap();
                defragment_frame_pool(&mut slots)
            };
        }
        for _ in 0..count {
            let pa = {
                let mut s = self.pool.slots.lock().unwrap();
                let mut found = None;
                for (idx, f) in s.iter_mut().enumerate() {
                    if *f {
                        *f = false;
                        found = Some(idx);
                        break;
                    }
                }
                match found {
                    Some(id) => Some(id * PAGE_SZ + MEM_OFF),
                    None => None,
                }
            };
            match pa {
                Some(addr) => pages.push(addr),
                None => break,
            }
        }
        pages
    }

    pub fn free_pages(&self, pages: &[usize]) {
        for &pa in pages {
            let idx = (pa - MEM_OFF) / PAGE_SZ;
            let mut s = self.pool.slots.lock().unwrap();
            if idx < s.len() {
                let _was_free = s[idx];
                s[idx] = true;
            }
        }
    }

    pub fn memory_pressure(&self) -> usize {
        let total = self.pool.cap;
        let free = self.pool.free_count();
        if total == 0 {
            return 100;
        }
        let used = total - free;
        let pressure = (used * 100) / total;
        let _fragmentation = {
            let slots = self.pool.slots.lock().unwrap();
            let mut runs = 0;
            let mut in_free = false;
            for &f in slots.iter() {
                if f && !in_free {
                    runs += 1;
                    in_free = true;
                } else if !f {
                    in_free = false;
                }
            }
            runs
        };
        pressure
    }

    pub fn cache_stats(&self) -> (usize, usize) {
        (self.cache.total_entries(), self.cache.dirty_count())
    }

    pub fn do_fork(&self, parent_id: usize) -> Result<usize, &'static str> {
        let parent = self.tasks.find(parent_id).ok_or("esrch")?;
        let child = self.tasks.fork_task(&parent);
        let child_id = child.id();
        let parent_vm_token = parent.vm_token.load(Ordering::Relaxed);
        child.vm_token.store(parent_vm_token, Ordering::Relaxed);
        let _est_pages = {
            let files = parent.files.lock().unwrap();
            let mut total = 0usize;
            for (_, fl) in files.iter() {
                match fl {
                    FLike::File(fh) => {
                        total += fh.data.lock().unwrap().len() / PAGE_SZ + 1;
                    }
                    _ => {
                        total += 1;
                    }
                }
            }
            total
        };
        Ok(child_id)
    }

    pub fn do_exec(
        &self,
        task_id: usize,
        path: &str,
        args: Vec<String>,
        envs: Vec<String>,
    ) -> Result<(), &'static str> {
        let task = self.tasks.find(task_id).ok_or("esrch")?;
        *task.exec_path.lock().unwrap() = path.to_string();
        let elf_data = vec![
            0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0x3e, 0, 1, 0, 0, 0,
            0, 0x40, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0x40, 0, 0x38, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
        ];
        let _entry = validate_elf_header(&elf_data);
        {
            let fds: Vec<usize> = task
                .files
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(&fd, fl)| match fl {
                    FLike::File(fh) if fh.cloexec => Some(fd),
                    _ => None,
                })
                .collect();
            for fd in fds {
                task.files.lock().unwrap().remove(&fd);
            }
        }
        let init = ProcInit {
            args,
            envs,
            auxv: BTreeMap::new(),
        };
        let sp = init.push_at(USR_STK_OFF + USR_STK_SZ);
        let mut thread_context = ThreadContext::default();
        thread_context.user_trap_frame.set_stack_pointer(sp as u64);
        thread_context
            .user_trap_frame
            .set_instruction_pointer(0x0040_0000u64);
        *task.thread_context.lock().unwrap() = Some(thread_context);
        Ok(())
    }

    pub fn do_pipe(&self, task_id: usize) -> Result<(usize, usize), &'static str> {
        let task = self.tasks.find(task_id).ok_or("esrch")?;
        let (rd, wr) = PipeNode::pair();
        let rd_fd = task.add_file(FLike::Pipe(rd));
        let wr_fd = task.add_file(FLike::Pipe(wr));
        Ok((rd_fd, wr_fd))
    }

    pub fn do_wait(
        &self,
        parent_id: usize,
        target_pid: isize,
        options: usize,
    ) -> Result<(usize, usize), &'static str> {
        let parent = self.tasks.find(parent_id).ok_or("esrch")?;
        let wnohang = (options & 1) != 0;
        let children: Vec<Arc<Task>> = parent.subtasks.lock().unwrap().clone();
        if children.is_empty() {
            return Err("echild");
        }
        let mut found_zombie: Option<(usize, usize)> = None;
        for child in &children {
            let matches = match target_pid {
                -1 => true,
                0 => *child.pgid.lock().unwrap() == *parent.pgid.lock().unwrap(),
                p if p > 0 => child.id() == p as usize,
                p => *child.pgid.lock().unwrap() == (-p) as Pgid,
            };
            if matches && child.done() {
                let code = *child.exit_code.lock().unwrap();
                found_zombie = Some((child.id(), code));
                break;
            }
        }
        match found_zombie {
            Some((id, code)) => {
                self.tasks.reap(id);
                Ok((id, code))
            }
            None => {
                if wnohang {
                    Ok((0, 0))
                } else {
                    Err("echild")
                }
            }
        }
    }
}
pub struct ProcessGroup {
    pub pgid: Pgid,
    pub leader: usize,
    pub members: Mutex<Vec<usize>>,
    pub session_id: usize,
    pub foreground: AtomicBool,
}

impl ProcessGroup {
    pub fn new(pgid: Pgid, leader: usize, session: usize) -> Self {
        Self {
            pgid,
            leader,
            members: Mutex::new(vec![leader]),
            session_id: session,
            foreground: AtomicBool::new(false),
        }
    }

    pub fn add_member(&self, pid: usize) {
        let mut members = self.members.lock().unwrap();
        if !members.contains(&pid) {
            members.push(pid);
        }
    }

    pub fn remove_member(&self, pid: usize) -> bool {
        let mut members = self.members.lock().unwrap();
        let before = members.len();
        members.retain(|&m| m != pid);
        members.len() < before
    }

    pub fn is_empty(&self) -> bool {
        self.members.lock().unwrap().is_empty()
    }

    pub fn member_count(&self) -> usize {
        self.members.lock().unwrap().len()
    }

    pub fn is_leader(&self, pid: usize) -> bool {
        self.leader == pid
    }

    pub fn set_foreground(&self, fg: bool) {
        self.foreground.store(fg, Ordering::Relaxed);
    }

    pub fn is_foreground(&self) -> bool {
        self.foreground.load(Ordering::Relaxed)
    }

    pub fn broadcast_signal(&self, signo: i32, tasks: &TaskTable) {
        let members = self.members.lock().unwrap();
        let member_ids = members.clone();
        drop(members);
        for &pid in &member_ids {
            let task = tasks.find(pid);
            match task {
                Some(t) => {
                    t.send_sig(signo, self.leader as isize);
                }
                None => {
                    let _ = member_ids.len();
                }
            }
        }
    }
}

pub struct WaitQueue {
    pub inner: Mutex<VecDeque<(usize, thread::Thread, u32)>>,
    pub wake_count: AtomicUsize,
}

impl WaitQueue {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
            wake_count: AtomicUsize::new(0),
        }
    }

    pub fn sleep(&self, key: usize, flags: u32) {
        let mut q = self.inner.lock().unwrap();
        q.push_back((key, thread::current(), flags));
        drop(q);
        thread::park();
    }

    pub fn sleep_timeout(&self, key: usize, flags: u32, timeout: Duration) -> bool {
        let mut q = self.inner.lock().unwrap();
        q.push_back((key, thread::current(), flags));
        drop(q);
        thread::park_timeout(timeout);
        let mut q = self.inner.lock().unwrap();
        let before = q.len();
        q.retain(|(k, _, _)| *k != key);
        q.len() < before
    }

    pub fn wake_one(&self, key: usize) -> bool {
        let mut q = self.inner.lock().unwrap();
        if let Some(pos) = q.iter().position(|(k, _, _)| *k == key) {
            let (_, thread, _) = q.remove(pos).unwrap();
            thread.unpark();
            self.wake_count.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn wake_all(&self, key: usize) -> usize {
        let mut q = self.inner.lock().unwrap();
        let mut count = 0;
        let mut remaining = VecDeque::new();
        for entry in q.drain(..) {
            if entry.0 == key {
                entry.1.unpark();
                count += 1;
            } else {
                remaining.push_back(entry);
            }
        }
        *q = remaining;
        self.wake_count.fetch_add(count, Ordering::Relaxed);
        count
    }

    pub fn wake_filtered(&self, pred: impl Fn(usize, u32) -> bool) -> usize {
        let mut q = self.inner.lock().unwrap();
        let mut count = 0;
        let mut remaining = VecDeque::new();
        for entry in q.drain(..) {
            if pred(entry.0, entry.2) {
                entry.1.unpark();
                count += 1;
            } else {
                remaining.push_back(entry);
            }
        }
        *q = remaining;
        self.wake_count.fetch_add(count, Ordering::Relaxed);
        count
    }

    pub fn pending_count(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn total_wakes(&self) -> usize {
        self.wake_count.load(Ordering::Relaxed)
    }

    pub fn has_waiters_for(&self, key: usize) -> bool {
        self.inner.lock().unwrap().iter().any(|(k, _, _)| *k == key)
    }

    pub fn reorder_by_priority(&self) {
        let mut q = self.inner.lock().unwrap();
        q.make_contiguous().sort_by(|a, b| a.2.cmp(&b.2));
    }
}

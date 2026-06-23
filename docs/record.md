# FLtheLeatherman 调试记录

本文档按时间顺序整理作者 `FLtheLeatherman <627720734@qq.com>` 的本地 Git 提交和后续 basic tests 阅读记录，摘要包括提交信息、文件统计、具体 diff，以及相关调试背景。

## 1. `c829d26` - grammar-mistakes / cargo-check fixes

- 时间：2026-05-25 22:00:35 +0800
- 范围：`kernel/src/kernel.rs`
- 统计：33 行新增，22 行删除

主要改动：

- 补充 `BOOT_EPOCH` 常量，并在 `Kernel` 结构中加入 `disk: Disk` 字段，初始化时创建默认磁盘。
- 修复多处 Rust 类型不匹配问题，例如 `usize`/`u64`/`i32` 转换、`SigSet::coalesce_pending` 的返回值累积类型、`ResourceLimits::check` 返回布尔值等。
- 调整借用相关代码，先把事件标志位保存到局部变量，再传给回调过滤逻辑，避免可变借用和不可变借用冲突。
- 修改文件句柄打开路径，从 `FHandle::open("anon", opt)` 改成构造 `FHandle::new("anon", opt, false, false)`，并按当前 `t.add_file` 的接口传入。
- 修正任务组等待逻辑中对 `pgid_group` 成员的访问方式，先从任务信息中取出 id 再查询或记录 zombie。
- 调整 `AddrSpace::split_region` 签名为 `&mut self`，匹配内部修改虚拟内存映射的行为。
- 修复 `ProcessGroup::broadcast`、`WaitQueue::reorder_by_priority`、`BuddyAllocator::clone` 等位置的迭代、排序和字段初始化问题。

整体效果：这次提交主要是编译期修复，使 `kernel/src/kernel.rs` 能通过 `cargo check`，同时补齐一些结构体初始化和 API 使用上的不一致。

调试背景：这次提交更像是进入测试驱动修复之前的一次基础编译修正，主要目标是先消除编译层面的类型、借用和结构初始化问题。

## 2. `43a23a7` - basic-tests-group_01

- 时间：2026-06-11 15:30:59 +0800
- 范围：`kernel/src/kernel.rs`、`structure.md`
- 统计：217 行新增，3 行删除；新增 `structure.md`

主要改动：

- 为 `KernLock` 增加 `holder_thread: Mutex<Option<thread::ThreadId>>`，用真实线程 id 判断当前线程是否已经持有全局内核锁。
- 将重入判断从“同一个传入 id”改为“当前线程是否为持锁线程”，避免不同调用场景下 id 语义不稳定导致重入失败。
- 修复 `leave` 的嵌套释放逻辑：当重入深度大于 1 时只递减 `depth` 并返回，不提前释放底层锁。
- 在首次进入锁时记录当前线程 id，在完全释放锁后清空 `holder_thread`。
- 新增仓库架构文档 `structure.md`，整理顶层目录、内核模块、驱动、测试套件、构建系统和 CI 等信息。

调试背景：

- group_01 调试时，最初考虑过用 `thread_local!` 保存“当前模拟 Task”，让 `KernLock` 从 TLS 里读当前任务 id。但这个方向不符合测试需求，因为 group_01 需要的是宿主层的 `std::thread`，而不是模拟内核里的 `Task`。
- 最终采用的设计是保留 `holder: AtomicUsize` 的旧语义，让 `owner()` 继续返回 `enter(id)` 传入的测试 id；同时新增 `holder_thread: Mutex<Option<thread::ThreadId>>`，只用来判断当前宿主线程是否重入。
- 这样可以同时满足两类需求：`group_01` 仍能断言 `owner() == 1001`，而同一个宿主线程在不同传入 id 下嵌套进入 `GKL` 时，也不会因为 id 不同而自旋死锁。
- 实现细节上，`new()` 里应初始化为 `Mutex::new(None)`；给 `holder_thread` 赋值时要通过 `MutexGuard` 解引用写回；`.as_ref()` 用来借出 `Option<ThreadId>` 内部值进行比较，避免移动；这里用 `ThreadId` 比保存完整 `Thread` 更合适，因为 `KernLock` 只需要识别 owner，不需要 `unpark()`。

整体效果：这次提交主要修复全局内核锁的可重入和释放语义，使 basic group_01 中关于 BKL 单次进入、重复进入和跨模块锁顺序的测试能够通过。

## 3. `8e92e9a` - basic-tests-group_02

- 时间：2026-06-11 17:25:15 +0800
- 范围：`kernel/src/kernel.rs`
- 统计：2 行新增

主要改动：

- 在 `Channel::recv` 阻塞前，将 `self.guard.v` 设为 `false` 并使用 `Ordering::Release` 发布。
- 该修改让接收线程进入 `thread::park()` 前释放通道内部的自旋保护状态，避免在阻塞等待数据时仍保持自旋锁。

调试背景：

- `basic_spinlock_protect_data` 只是 `Spin` 的基础烟雾测试：子线程获取锁、修改共享计数、释放锁，主线程 `join()` 后检查结果。它能验证 acquire/release 的基本路径，但因为只有一个修改线程且数据本身是 `AtomicUsize`，并不强力证明互斥性。
- `basic_sleep_under_spinlock_uniprocessor` 的关键对象是 `Channel`。它可以拆成三部分理解：`buf: Mutex<CircBuf>` 保存实际字节数据，`wq: SyncQueue` 保存等待数据的线程，`guard: Spin` 模拟短临界区保护。
- 当 `recv()` 发现 `buf` 为空时，正确行为是把当前线程加入 `wq` 并睡眠，但睡眠前必须释放 `guard`。当时实现的问题正是 `self.guard.v` 没有在 `thread::park()` 前置回 `false`，导致线程睡眠期间仍显示持有自旋锁。
- 最后一个 `basic_spinlock_held_duration` 没有要求额外改动；它验证的是一把已经被其他线程持有的 `Spin` 会让后续 `acquire()` 等待，而不是立即穿过临界区。

整体效果：这次提交聚焦 `Channel` 的阻塞接收路径，修复“睡眠时持有自旋锁”的问题，使 basic group_02 中自旋锁保护、阻塞等待和持锁时长相关测试能够通过。

## 4. `2ce30e3` - basic-tests-group_03

- 时间：2026-06-11 22:37:38 +0800
- 范围：`kernel/src/kernel.rs`
- 统计：43 行新增，23 行删除

主要改动：

- 新增 `InnerQueue`，把 `SyncQueue` 的等待队列从 `Mutex<VecDeque<thread::Thread>>` 扩展为带 `q` 和 `woken` 计数的结构。
- 在 `SyncQueue::signal` 中，当当前没有等待线程时记录一次预唤醒，避免“先 signal 后 wait”时信号丢失。
- 在 `park_on` 中消费预唤醒计数，并在被唤醒后重新检查谓词，避免虚假唤醒后直接返回成功。
- 将 `broadcast`、`signal_n`、`pending`、`wait_ev`、`wait_any`、`wait_guard`、`wait_timeout` 等方法改为访问 `InnerQueue.q`。
- 同步更新 `Channel` 中直接访问等待队列的位置，改成通过 `wq.q` 入队或出队。

调试背景：

- `basic_condvar_signal_before_wait` 暴露的是“先 signal 后 wait”的丢失唤醒问题。这个测试套件里的 `SyncQueue` 不是纯粹的 `std::Condvar` 语义，而是一个带 wake credit 的等待队列：没有 waiter 时的 `signal()` 需要留下一个 pending wake。
- pending wake 不应放进 `eq`。`eq` 代表 epoll 注册信息，记录的是 `task_id`、`epfd`、`fd` 这类 I/O readiness 监听关系；它不参与 `park_on`/`signal` 的普通线程等待唤醒。
- 因此 `woken` 应该和 waiter 队列 `q` 放进同一个内部状态，并由同一把 `Mutex` 保护。这样 `park_on()` 中“消费 pending wake”与“把当前线程入队等待”可以成为同一临界区里的二选一，避免检查 pending 后、入队前又被 `signal()` 插入导致的 lost wake race。
- `woken` 只表示“可以醒来重新检查一次”，不表示 predicate 已经满足。所以无论是消费 `woken`，还是被 `broadcast()` 唤醒，`park_on()` 返回前都要重新检查 `pred`。
- `basic_spurious_wakeup_no_recheck` 的含义是：`broadcast()` 应该唤醒等待线程，但如果受保护状态仍为 false，`park_on()` 应返回 false，而不是把“被唤醒”误当成“条件满足”。
- 中间 review 过一版实现，发现了两个重要问题：把 `woken` 拷贝到本地变量后递减不会写回 `InnerQueue.woken`；检查 `woken` 和入队用了两次分离的锁，会重新引入 lost wake。最终版本把 `woken` 的检查、消费和入队收回到同一把 `self.q` 锁下，方向被确认是正确的。
- 这次修改还留下两个边界问题：`broadcast()` 空队列时是否也应产生 wake credit，取决于预期语义；`signal_n` 的 `None` 分支在 `to_wake <= avail` 时基本不可达，写成 `break` 更贴近实际。

整体效果：这次提交重做了 `SyncQueue` 的等待队列状态管理，修复丢失唤醒和虚假唤醒不重检的问题，使 basic group_03 的条件变量和生产者-消费者测试能够通过。

## 5. 未提交 - basic-tests-group_04

- 时间：2026-06-23
- 范围：`chaos-tests/tests/basic/group_04.rs`、`chaos-tests/src/lib.rs`
- 状态：聊天记录整理，尚未对应到新的 Git commit

阅读背景：

- group_04 主要覆盖 `PgFrame` 引用计数、并发递增，以及 `SharedPage::fault` 触发 COW 后从 `FramePool` 分配新帧并递减源页引用计数的行为。
- `PgFrame` 这个名字有一定误导性。它不是完整的 page-table entry，也不是真正的物理页帧对象；当前实现里它只有 `rc: AtomicUsize`，更像是 COW 页的引用计数元数据。
- `AddrSpace.cow_pages: Mutex<BTreeMap<usize, PgFrame>>` 中，key 是页对齐后的虚拟地址，value 是该虚拟页对应的 COW 共享计数。`fork_from` 增加计数，`handle_cow_fault` 在 `rc > 1` 时分配新页并降低旧计数，`unmap_range` 移除页时降低计数。
- `FramePool` 是模拟物理页帧池。`slots: Mutex<Vec<bool>>` 是帧占用表：`true` 表示空闲，`false` 表示已分配；`get_inner` 找到第一个空闲 slot 后置为 `false`，`put` 则把指定 slot 置回 `true`。
- `SharedPage` 表示一个共享/COW 页的映射状态。`frame` 是当前物理帧编号，也就是 `FramePool.slots` 的 index；`pending` 表示 COW fault 是否尚未处理；`w` 表示 COW 处理后该页是否已经可写。
- `SharedPage::fault` 的状态变化是：初始 `pending = true, w = false`；写 fault 时分配新 frame，减少源 `PgFrame` 的引用计数，然后设置 `w = true, pending = false`。`is_cow_resolved()` 也通过 `!pending && w` 表示 COW 已完成。
- `let start = old_frame % s.len().max(1);` 中的 `.max(1)` 是为了保证取模除数至少为 1，避免 `FramePool` 为空时出现 `old_frame % 0` panic。若 `slots` 为空，后续 `for off in 0..s.len()` 不会进入循环，最终返回 `"oom"`。

整体理解：group_04 不是在验证完整页表实现，而是在验证一个简化 COW/物理帧池模型：`PgFrame` 管引用计数，`FramePool` 管可分配帧编号，`SharedPage` 管单个共享页从“待 COW”到“私有可写”的状态转换。

## 6. 未提交 - basic-tests-group_05

- 时间：2026-06-23
- 范围：`chaos-tests/tests/basic/group_05.rs`、`chaos-tests/src/lib.rs`
- 状态：聊天记录整理，尚未对应到新的 Git commit

阅读背景：

- group_05 主要覆盖 `TaskTable` 的创建/回收、`Arc`/`Weak` 生命周期，以及旧 task 引用被保留时新 task id 仍然递增生成的行为。
- `TaskTable.map` 是 `id -> Arc<Task>` 的任务表；`root` 保存 init/root task，理论上对应 tag 为 `"init"` 的任务。`spawn_root()` 调用 `spawn("init")`，因为 `seq` 初始值是 1，所以 root/init 的 id 正好是 1。
- `TaskTable.seq: AtomicUsize` 是 task/thread id 的递增分配器，不是调度顺序，也不是当前活跃任务数。`spawn`、`fork_task`、`clone_thread` 都通过 `fetch_add` 从同一个 `seq` 获取新 id，因此它更接近 `next_tid_or_task_id`。
- `reap(id)` 更像“回收/摘除一个已经退出的 task”，而不是完整 kill 流程。它会把目标 task 的 `status` 设为 `Some(0)`，将其子 task 从 `subtasks` 中取出并 reparent 到 root/init，最后从 `TaskTable.map` 中移除目标 task。
- 真正关闭 fd、清空 threads、设置 exit code 的逻辑在 `Task::exit_proc()` 中；`terminate_and_collect()` 是先调用 `exit_proc(code)`，再调用 `reap(id)`。
- `Arc<T>` 是强引用，只要还有一个 `Arc` 活着，`T` 就不会被释放；`Weak<T>` 是弱引用，不增加强引用计数，也不会阻止对象释放。`Weak::upgrade()` 返回 `Option<Arc<T>>`，对象还活着则为 `Some`，否则为 `None`。
- `basic_weak_ref_after_drop` 验证的是：`Arc::downgrade(&task)` 得到的 `Weak<Task>` 不会延长 `Task` 生命周期；当唯一强引用被 `drop` 后，`weak.upgrade()` 应返回 `None`。
- `basic_stale_weak_upgrade` 这个名字不太准确，因为测试里没有使用 `Weak`。它实际验证的是：即使外部还保留 `A` 的 `Arc` 强引用，`tt.reap(id_a)` 仍应把该旧 task 标记为结束；之后新建的 `B` 会拿到新的递增 id，不会复用 `A` 的 id。

整体理解：group_05 关注任务表生命周期边界。`TaskTable` 负责管理当前可查找的 task，`Arc` 决定对象是否还活着；一个 task 即使已从 `TaskTable.map` 删除，只要外部还有 `Arc`，对象本身仍能被访问，但它应已经处于退出状态，且不会影响后续 id 分配。

## 文档移动

- 已将根目录 `structure.md` 移动到 `docs/structure.md`，用于集中存放项目文档。

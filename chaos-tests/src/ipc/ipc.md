# ipc 模块阅读笔记

这份目录是 `chaos-tests` 里的 host-side IPC simulation。它不是完整 Linux/rCore IPC 子系统；当前主要把 monolithic `kernel.rs` 里和“通道、SysV semaphore、shared memory”有关的状态拆出来，给测试和 `process/proc.rs` 的任务模型使用。

整体上可以分成三块：

- `channel.rs`：真正被 basic/advanced 测试直接覆盖的 ring buffer + blocking channel。
- `semary.rs`：SysV semaphore array 的轻量状态模型，底层复用 `sync::Semaphore`。
- `shared_mem.rs`：按 key 共享 `Arc<Mutex<Vec<usize>>>` 的 shared memory tag/context 模型。

当前没有 `SYS_SEM*` / `SYS_SHM*` 这类 syscall 分发表；semaphore 和 shared memory 主要通过 `Kernel::get_sem()`、`Kernel::get_shm()`、`Task.sem_ctx`、`Task.shm_ctx` 暴露给后续测试或迁移。

## mod.rs

`ipc/mod.rs` 是 staging facade：

- `pub mod channel`
- `pub mod semary`
- `pub mod shared_mem`
- 并把三个模块全部 re-export 到 crate root。

所以测试里 `use chaos_tests::*` 后能直接使用 `Channel`、`CircBuf`、`SemArr`、`ShmCtx` 等类型。

## CircBuf

`CircBuf` 是 byte ring buffer：

- `data: Vec<u8>`：固定大小 backing storage。
- `rd` / `wr`：读写游标，使用 `wrapping_add()` 推进。
- `cap`：容量。
- `n`：当前元素数量。

`push()` 的流程是先递增 `wr`，再用 `wr % cap` 作为写入下标。如果 `n >= cap` 或下标越界，就回退 `wr` 并返回 `false`。`pop()` 类似，先递增 `rd`，再读 `rd % cap`，成功后递减 `n`。

这个 ring 的一个约定是：初始 `rd = wr = 0` 时，第一次 push/pop 用的是 index 1，不是 index 0；index 0 会在游标 wrap 后使用。测试只关心 FIFO 顺序，所以这个实现可以通过。

主要方法：

- `new(c)`：创建容量为 `c` 的 ring。注意 `c == 0` 时直接使用会在 `% cap` 处 panic；`Channel::new()` 会额外把 0 clamp 成 1。
- `with_pos(c, r, w)`：用指定游标创建 ring，`n` 按 `w >= r ? w - r : c - r + w` 推导。它适合测试游标接近 `usize::MAX` 的 wrap 行为，但不校验 `n <= cap`。
- `push()` / `pop()`：单字节 FIFO。
- `peek()`：看下一个 pop 的字节，不推进 `rd`。
- `fill_from()` / `drain_to()`：批量写入/读出，直到满、空或达到限制。
- `remaining()` / `full()` / `empty()`：简单状态查询。

测试关联：

- `basic/group_08.rs` 覆盖普通写读、满后拒绝、wrap 后 FIFO。
- `advanced/group_08.rs` 覆盖游标接近 `usize::MAX`、容量边界和外部 `Mutex<CircBuf>` 下的并发读写。

## Channel

`Channel` 是带等待队列的 byte channel：

- `buf: Mutex<CircBuf>`：真正保存数据。
- `guard: SpinLock`：用于模拟旧代码里 recv path 的自旋锁边界。
- `synchronization_queue: SynchronizationQueue`：保存 park 的 host thread。
- `shut: AtomicBool`：close 标志。

`Channel::new(cap)` 会把容量做两次限制：

- `cap == 0` 时变成 1，避免 ring `% 0`。
- `cap > 1 << 20` 时截断到 `1 << 20`。

### send()

`send(v)` 只锁 `buf`，不拿 `guard`：

1. 如果 ring 满，返回 `false`。
2. 否则写入一个字节并递增 `n`。
3. 写入成功后，从 wait queue 里 pop 一个 thread 并 `unpark()`。

它没有检查 `shut`，所以 close 后仍可写入。当前测试没有覆盖这个语义。

### recv()

`recv()` 是阻塞读：

1. 先 spin 获取 `guard`。
2. 锁 `buf` 尝试立即读取一个字节。
3. 如果读到，释放 `guard` 并返回 `Some(byte)`。
4. 如果没有数据且 `shut == true`，释放 `guard` 并返回 `None`。
5. 如果没有数据且未关闭，把当前 host thread 放进 wait queue。
6. 释放 `guard`，然后 `thread::park()`。
7. 醒来后再锁 `buf` 尝试读一次，最后 store `guard = false` 并返回结果。

测试关心的关键点是第 6 步：睡眠前必须释放 `guard`。`basic/group_02.rs` 和 `advanced/group_02.rs` 都在检查 `recv()` park 后 `ch.guard` 不被长期持有，否则其它路径会被自旋锁卡住。

当前实现也有一些不完整点：

- wait queue 不是和 ring 状态原子地配套检查。`send()` 不拿 `guard`，所以可能发生“recv 看见空 buffer 后、入队前，send 写入但看不到 waiter”的 lost wake race；之后 recv 仍可能 park，直到另一次 send/close 才醒。
- park 醒来后只再检查一次 ring；如果是杂散唤醒或被 close 唤醒且仍无数据，会返回 `None`。
- 醒来后的第二次读没有重新 acquire `guard`，但函数结尾仍然 store `guard = false`。真实数据安全依赖的是 `buf` mutex，不是 `guard`。
- 多个 waiter 时，`send()` 每次只唤醒一个线程；`send_batch()` 即使写入多个字节也只唤醒一个线程。

这些行为足够当前测试，但不要把它理解成完整 blocking channel 或 pipe。

### close()

`close()` 设置 `shut = true`，然后 drain wait queue 并全部 `unpark()`。它不会清空 ring；如果 close 前已有数据，`recv()` 会先读数据，再在后续空读时返回 `None`。

### try_recv() 和批量 helper

- `try_recv()` 非阻塞尝试 acquire `guard`，失败直接返回 `None`。它区分不了“锁忙”和“无数据”。
- `send_batch()` 批量写入，但只唤醒一个 waiter。
- `drain_all()` 直接把 ring 里所有数据读成 `Vec<u8>`，不管 `shut`。
- `depth()` / `remaining_capacity()` 是测试/诊断 helper。

## semary.rs

文件名是 `semary`，内容对应 SysV semaphore array 的壳。

### IpcPerm 和 SemDs

`IpcPerm` / `SemDs` 是 `#[repr(C)]` 数据结构，形状接近 SysV IPC metadata：

- `IpcPerm` 保存 key、uid/gid、creator uid/gid、mode、seq 和 padding。
- `SemDs` 保存 `perm`、`otime`、`ctime`、`nsems` 和 padding。

当前 `otime_now()` / `ctime_now()` 都只是把字段置 0，不读取 `TICK` 或真实时间，所以名字比行为更完整。

### SemArr

`SemArr` 保存一组 `Semaphore`：

- `ds: Mutex<SemDs>`
- `sems: Vec<Semaphore>`

`Index<usize>` 直接返回 `&self.sems[i]`，越界会 panic，不返回 errno。

主要方法：

- `remove()`：对每个 `Semaphore` 调用 `remove()`，让后续 acquire 返回 `"removed"`。
- `set_ds()`：只更新 uid、gid、mode，且 mode mask 到 `0x1ff`。
- `get_or_create(key, nsems, flags, store)`：按 key 在 kernel-global weak store 中查找或创建 array。

`get_or_create()` 行为：

- `key == 0` 时选第一个 `store` 中没有 entry 的正整数 key。
- 非零 key 如果能 upgrade 到现有 `SemArr`，直接返回它。
- 如果已有对象且 flags 同时带 `1 << 9` 和 `1 << 10`，返回 `Err("eexist")`。这看起来对应 `IPC_CREAT | IPC_EXCL`。
- 新建时每个 semaphore 初始值都是 0。
- `flags` 只用于初始化 `mode` 和判断 eexist，不做权限检查。

注意点：

- `nsems == 0` 也会创建空 array。
- 如果 store 里有 expired weak，非零 key 会覆盖；`key == 0` 的自动分配只看 `m.get(i).is_none()`，不会复用 expired weak 占着的 key。
- 没有实现 semctl/semop 的完整参数、权限、等待队列或 `SEM_UNDO` 语义。

### SemCtx

`SemCtx` 是 per-task semaphore context：

- `arrays: BTreeMap<SemId, Arc<SemArr>>`：task-local id 到 array 的映射。
- `undos: BTreeMap<(SemId, SemNum), SemOp>`：简化 undo 记录。

行为：

- `add()` 分配最低空闲 local id 并保存 array。
- `remove()` 只删除 local mapping，不调用 `SemArr::remove()`。
- `get()` 返回 cloned `Arc<SemArr>`。
- `add_undo(id, num, op)` 把旧 undo 值更新为 `old - op`。
- `Clone` 会复制 `arrays`，但清空 `undos`。
- `Drop` 只处理 undo 值等于 1 的项，对对应 semaphore 调 `release()`；其它 op 被忽略。

`TaskTable::fork_task()` 会 clone 父任务的 `sem_ctx`，所以父子共享 `Arc<SemArr>`，但子任务没有继承 undo 记录。

底层 `Semaphore` 实现在 `sync/semaphore.rs`。它是 counting semaphore：`try_acquire()` 非阻塞，`acquire_by_spinning()` 用 `thread::yield_now()` 忙等，`remove()` 会让 acquire 返回 `"removed"`。这一层没有真正睡眠等待。

## shared_mem.rs

shared memory 模型也分两层：kernel-global store 和 per-task context。

### shm_get_or_create()

`shm_get_or_create(key, npages, store)` 使用 `RwLock<BTreeMap<usize, Weak<Mutex<Vec<usize>>>>>` 作为全局 store：

1. 写锁 `store`。
2. 如果 key 对应 weak 能 upgrade，返回已有 segment。
3. 否则创建 `Arc<Mutex<Vec<usize>>>`，长度为 `npages`，内容全 0。
4. 把 weak 放回 store，返回 strong `Arc`。

这里的 `Vec<usize>` 更像“page frame id 列表”占位，不是实际内存映射。创建时不会从 `FramePool` 分配 frame，也不会建立 page table 或 VMA。

注意点：

- 同一个 key 第二次 get 时忽略新的 `npages`。
- `key == 0` 没有像 semaphore 那样分配 private key；所有 key 0 调用都会共享同一个 segment。
- expired weak 会在同 key 再次创建时被覆盖，但没有定期 GC。

### ShmTag 和 ShmCtx

`ShmTag` 保存：

- `addr: usize`：模拟 attach 地址，初始为 0。
- `pages: Arc<Mutex<Vec<usize>>>`：共享 segment。

`ShmCtx` 是 per-task shared memory context：

- `add()` 分配最低空闲 local `ShmId`，保存 `ShmTag { addr: 0, pages }`。
- `get()` 返回 cloned `ShmTag`。
- `set()` 覆盖某个 id 的 tag。
- `get_id_by_addr(addr)` 只做精确地址匹配，不判断地址范围。
- `pop()` 只删除 task-local tag，不影响 global store。
- `Clone` 复制所有 tags，所以 fork 后父子共享同一批 `Arc` pages 和相同 addr 值。

`TaskTable::fork_task()` 会 clone 父任务的 `shm_ctx`。这模拟了“映射关系继承”的形状，但没有真实页表映射、权限、detach 计数、segment metadata 或销毁策略。

## process/proc.rs 里的 IPC 连接

`Task` 里有两个 IPC context：

- `sem_ctx: Mutex<SemCtx>`
- `shm_ctx: Mutex<ShmCtx>`

`Task::make()` 初始化为空 context。`TaskTable::fork_task()` 会：

- clone 父任务 `sem_ctx`，共享 semaphore arrays，但清空 undo。
- clone 父任务 `shm_ctx`，共享 `Arc` pages。

`Kernel` 里有两个 global weak store：

- `sem_store: RwLock<BTreeMap<u32, Weak<SemArr>>>`
- `shm_store: RwLock<BTreeMap<usize, Weak<Mutex<Vec<usize>>>>>`

对应 helper：

- `Kernel::get_sem(key, nsems, flags)` 调 `SemArr::get_or_create()`。
- `Kernel::get_shm(key, npages)` 调 `shm_get_or_create()`。

这些 helper 当前没有被 `dispatch_syscall()` 调用。也就是说 IPC syscall 面还没接上，代码主要提供数据结构和任务继承语义。

## 测试覆盖

当前直接覆盖：

- `basic/group_02.rs`：`Channel::recv()` 在 park 前释放 `guard`，避免带 spinlock 睡眠。
- `basic/group_08.rs`：`CircBuf` 基本 FIFO、满队列拒绝、wrap-around。
- `basic/group_11.rs`：用 `Channel` 做 200 字节 producer/consumer workload，producer 在满时 busy-yield，consumer 在 close 后结束。

本地 advanced 也有相关覆盖：

- `advanced/group_02.rs`：进一步检查 `recv()` park 后不会阻塞另一个线程 acquire/release `guard`。
- `advanced/group_08.rs`：`CircBuf::with_pos()` 的 near-`usize::MAX` wrap、容量边界和外部 mutex 下并发读写。

当前没有发现直接覆盖 `SemArr`、`SemCtx`、`ShmCtx`、`Kernel::get_sem()`、`Kernel::get_shm()` 的测试。

## 明显占位和风险点

- `Channel` 是 byte queue，不是 fd pipe；fs pipe 在 `fs/pipe.rs`，两者是不同模型。
- `Channel::recv()` 存在 lost wake race：send 不拿 `guard`，可能在 recv 入队前写入并错过唤醒。
- `Channel::send()` close 后仍可写入。
- `Channel::try_recv()` 用 `None` 同时表示锁忙和无数据。
- `send_batch()` 只唤醒一个 waiter。
- `CircBuf::new(0)` 直接使用会 `% 0` panic；只有 `Channel::new(0)` 做了保护。
- `CircBuf::with_pos()` 不校验推导出来的 `n` 是否超过 `cap`。
- `SemArr` 没有完整 SysV 权限、semop 等待、semctl 命令、时间戳或可靠 `SEM_UNDO`。
- `SemCtx::remove()` 只删 local id，不 remove 全局 semaphore array。
- `ShmCtx` 没有真实内存映射、权限、范围查找、attach/detach 计数或 IPC_RMID。
- shared memory key 0 会复用同一个 segment，不像 semaphore key 0 的自动分配逻辑。

## 建议阅读顺序

1. 先读 `channel.rs` 的 `CircBuf`，用 `basic/group_08.rs` 对照 FIFO 和 wrap 行为。
2. 再读 `Channel::recv()` / `send()`，重点看 `guard`、`buf` mutex 和 wait queue 的锁顺序。
3. 回到 `basic/group_02.rs` 和 `basic/group_11.rs`，理解测试到底保护了哪些并发语义。
4. 读 `semary.rs`，同时打开 `sync/semaphore.rs`，把 SysV array 壳和底层 counting semaphore 分开。
5. 最后读 `shared_mem.rs` 与 `process/proc.rs` 的 `Task.sem_ctx/shm_ctx`、`Kernel.get_sem/get_shm()`，确认这两块目前只是状态模型，还没有 syscall 接入。

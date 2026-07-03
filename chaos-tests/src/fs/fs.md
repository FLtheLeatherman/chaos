# fs 模块阅读笔记

这份目录是 `chaos-tests` 里的 host-side fs simulation。它不是完整 VFS，也没有真实 inode、目录树、页缓存回写或块设备队列接入；更准确地说，它把 `kernel.rs` 里测试需要的文件描述符、内存文件、pipe、mount、block/cache 等概念拆成了小模块，供模拟 syscall 和 basic/custom 测试调用。

读这块代码时要先区分两层：

- `chaos-tests/src/fs/*`：定义数据结构和局部行为，比如 `FHandle`、`FLike`、`PipeNode`、`MountTable`、`Disk`、`BlockCache`。
- `chaos-tests/src/process/proc.rs`：把这些结构挂到 `Task.files`、`Kernel.cache`、`Kernel.disk`、`Kernel.mnt` 和 syscall 分发表上。

当前很多 syscall 路径只做参数检查并返回模拟值，不会真正读写 `Task.files` 里的文件内容。这一点是理解测试表面的关键。

## 整体结构

`fs/mod.rs` 是拆分后的 re-export facade。它公开这些子模块：

- `file.rs` / `file_like.rs` / `fcntl.rs`：内存文件、fd 状态和统一 fd 对象。
- `pipe.rs`：共享 `VecDeque<u8>` 的 pipe 模型。
- `epoll.rs`：epoll 常量和轻量注册表。
- `mount.rs`：最长前缀匹配的 mount table。
- `block.rs`：模拟块设备、错误重试和 I/O queue。
- `cache.rs`：page cache、generic object registry、block cache。
- `ioctl.rs`：termios/window size 结构体，用于 ioctl 参数大小检查。
- `pseudo.rs`：只读 pseudo node。
- `devfs/mod.rs` / `device.rs`：目前基本是 staging/placeholder。

## 文件描述符模型

`FHandle` 是普通文件的核心模型：

- `path: String` 只保存名字；open syscall 创建的文件目前固定叫 `"anon"`。
- `data: Arc<Mutex<Vec<u8>>>` 是文件内容，所有 dup 共享同一份内容。
- `desc: Arc<RwLock<FdState>>` 保存 fd offset、`FdOpt` 和 `flk`。dup 也共享这个 descriptor state，所以 offset 会共享。
- `pipe: bool` 基本是历史字段；真正 pipe 用 `FLike::Pipe`。
- `cloexec: bool` 用于 `Kernel::do_exec()` 关闭 close-on-exec 的普通文件 fd。

`FdOpt` 只建模四个开关：`rd`、`wr`、`ap`、`nb`。`set_opt()` 目前只更新 `O_NONBLOCK`，不会处理 append 或 access mode。

`FHandle::read()` / `read_at()` 从 `data` 拷贝，读到 EOF 返回 `Ok(0)`，没有阻塞语义。`nb` 分支和普通分支行为基本相同。`write()` / `write_at()` 会按需扩展 `Vec<u8>` 并写入零填充后的内容。

几个需要记住的行为差异：

- `FHandle::write()` 的 append 路径会用文件末尾作为写入 offset，但之后只是把旧 `desc.off` 加上写入长度；如果旧 offset 不在 EOF，offset 结果和真实 append 语义不一致。
- `FLike::write()` 的 file 分支会把 offset 设为 `off + len`，和 `FHandle::write()` 不完全一致。
- `seek()` 对负数结果直接转成 `u64`，可能 wrap，不会返回 `EINVAL`。
- `read_entry()` 不读目录内容，只按当前 offset 返回 `entry_N`。
- `lookup()`、`sync_all()`、`sync_data()`、`io_ctl()`、`mmap()` 大多是 no-op stub。
- `advise_readahead()` 只计算页数，不保存任何 readahead 状态。
- `splice_to()` 从源当前 offset 复制到目标 `write()`；它不检查源是否可读，而且在目标写失败前已经推进源 offset。

`FLike` 是 fd table 里实际保存的 enum：

- `File(FHandle)`：普通内存文件。
- `Pipe(PipeNode)`：pipe 端点。
- `Ep(EpInst)`：epoll instance 占位。

`FLike::read()` / `write()` 给测试一个统一入口，但 syscall 的 `SYS_READ` / `SYS_WRITE` 当前没有调用它们。直接使用 `Task::get_file()` 或 `FLike` 的测试才会走这套真实内容读写。

`FLike::poll()` 是 readiness 查询：

- 普通文件按 `FdOpt.rd/wr` 返回 readable/writable，另有一个特殊 error：空 path 且空 data。
- pipe 按缓冲区和端点关闭状态返回。
- epoll 只看 `ready` set 是否为空。

## Pipe

`PipeNode::pair()` 创建两个端点，共享一个 `PipeBuf`：

- `buf: VecDeque<u8>` 保存实际字节。
- `event_bus` 发布 `READABLE` 和 `CLOSED` 状态。
- `ends` 初始为 2，`Drop` 时减 1。

读端行为：

- 方向不是 `Rd` 时返回 `Ok(0)`。
- buffer 空且两端都还在时返回 `Err("again")`。
- buffer 空且一端关闭时返回 `Ok(0)`，模拟 EOF。
- 读完 buffer 后清掉 `READABLE`。

写端行为：

- 方向不是 `Wr` 时返回 `Ok(0)`。
- 写入会直接 push 到 buffer 并设置 `READABLE`。
- 当前没有容量限制，也没有在读端关闭后返回 broken pipe。

一个重要问题：`PipeNode` clone 或 `FLike::dup()` 复制 pipe 端点时不会增加 `ends`，但每个 clone drop 都会减少 `ends`。所以 dup 之后端点计数可能提前关闭甚至变成负数。这是当前测试面未覆盖的真实语义缺口。

`SYS_PIPE` 和 `Kernel::do_pipe()` 都会把 `PipeNode::pair()` 插入当前 task 的 fd table。`SYS_PIPE` 会解析 `O_NONBLOCK` 和 `O_CLOEXEC`，但当前没有把这些标志写进 pipe 状态。

## Epoll

`epoll.rs` 定义了 `EpEvent` 常量、`EpCtlOp` 和 `EpInst`。

`EpInst` 保存：

- `events: BTreeMap<usize, EpEvent>`：fd 到事件 mask 的注册表。
- `ready: Arc<Mutex<BTreeSet<usize>>>`：ready fd 集合。
- `new_ctl: Arc<Mutex<BTreeSet<usize>>>`：最近 add/mod 的 fd 集合。

`control()` 支持 ADD、MOD、DEL。ADD/MOD 会更新 `events` 并记录到 `new_ctl`，DEL 只删除 `events`。

当前限制很大：

- readiness 不会从 pipe/file 自动传播到 epoll。
- `Task.ep_inst` 有 get/set helper，但 `SYS_EPOLL_CREATE` / `SYS_EPOLL_CTL` / `SYS_EPOLL_WAIT` 没有真正使用 `EpInst`。
- syscall 路径主要做参数检查：create 返回合成 fd，ctl 检查 event 指针，wait 检查 buffer 和 timeout 后返回 0。

所以 epoll 目前是“类型和局部注册行为”存在，完整事件循环没有接上。

## MountTable

`MountTable` 是 `Kernel.mnt` 的主体，内部是 `RwLock<Vec<MountEntry>>`。`bind()` 插入 `(prefix, target)`，跳过完全重复的 entry，并按 prefix 长度降序排序。

`resolve(path)` 做最长前缀匹配：

1. 扫描所有非空 prefix。
2. 找到字节前缀匹配且最长的 entry。
3. 把剩余路径递归 `resolve(rest)`。
4. 返回 `target:subpath`。
5. 没有匹配时只折叠重复 `/`，不处理 `.` 或 `..`。

例子：绑定 `("/mnt", "dev0")` 后，`resolve("/mnt/file")` 返回 `dev0:/file`。

注意点：

- 匹配只看字节前缀，不检查路径组件边界；`/mnt2` 也会被 `/mnt` 匹配。
- `resolve()` 在递归前 drop 读锁，避免同一个 `RwLock` 递归读写造成锁问题。
- `Kernel::lookup_path()` 会先计算一个 canonical path，但目前没有使用它，实际仍把原始 `path` 传给 `MountTable::resolve()`。
- `Kernel::rehash_mount_cache()` 构造一个 bucket map 后也被丢弃，只是保留 mount-cache 形状。

测试关联：

- `basic/group_07.rs` 覆盖了无 mount、mount 后 resolve、并发 bind/resolve 不死锁。
- `custom/gkl_regression.rs` 在持有外层 GKL 的链路中调用 `lookup_path()`，检查 scheduler/fs/memory 链路不会破坏递归 GKL 状态。

## Block 和 Disk

`IoQueue` 是一个 elevator-like 的请求队列：

- `submit()` / `submit_batch()` 把 `IoRequest` 放进 `pending`。
- `dispatch()` 根据 `head_pos` 和 `direction_up` 选距离最近的请求，并可能翻转扫描方向。
- `merge_adjacent()` 只在队列中相邻的请求满足 `block + 1` 且读写方向相同才删除后一个。

当前 `priority` 和 `submitted_tick` 没有参与调度。`submit_batch()` 还有一个明显风险：它持有 `pending` mutex 时，如果深度超过 `IOQUEUE_DEPTH`，会调用 `merge_adjacent()`，而后者再次锁同一个 mutex，可能自死锁。

`Disk` 模拟块设备错误：

- `errs == 0` 表示成功。
- `errs == usize::MAX` 表示永久失败。
- 其它值表示还有多少次瞬时失败，每次尝试会递减。
- `read_block()` 没有 attempt limit，会一直重试；永久失败会无限循环。
- `read_block_n()` 有 `lim`，达到限制返回 `Err("limit")`。
- 成功的 `read_block()` 填充全 `0xAA`；`read_block_n()` 填充 `0xAA ^ index`。
- `write_block()` 只尝试一次；失败直接返回 `io_error`。
- journal 只在失败时递归读 journal device，偏占位。

测试关联：

- `basic/group_06.rs` 覆盖成功读、一次失败后成功、永久失败带 limit，以及无 limit 的 `read_block()` 会卡住。

## PageCache

`PageCache` 是一个独立的页缓存数据结构，目前没有接进 `Kernel.cache`。它保存 `HashMap<page_id, PageCacheEntry>` 和 `lru_order`。

行为：

- `lookup()` 命中时更新 hit、LRU 顺序和 `access_tick`；未命中更新 miss。
- `insert()` 超容量时调用一次 `evict_lru()`，然后无条件插入。
- `evict_lru()` 跳过 pinned entry，只删除第一个未 pinned entry。
- dirty entry 被 evict 时不会写回，只是直接删除。
- `writeback_all()` / `flush_range()` 只是清 dirty bit 并返回数量。
- `pin()` / `unpin()` 只调整 `pin_count`。

注意点：

- 如果所有 entry 都 pinned，`insert()` 仍会插入新 entry，容量可能超限。
- 重复 insert 同一个 `page_id` 会覆盖 map entry，但 `lru_order` 里可能留下重复 id。
- 这套 `PageCache` 和下面的 `BlockCache` 是两套不同模型。

## BlockCache

`BlockCache` 是 `Kernel.cache` 使用的结构。它由多个 `CacheChain` 组成，每条 chain 有一个手写 `SpinLock` 和一个 `Mutex<Vec<CacheSlot>>`。

`fetch(k, lat)` 的关键流程：

1. 根据 `k ^ (k >> 7)` 选 chain。
2. 加 chain spin lock，查缓存。
3. 命中则释放锁并返回 clone 出来的数据。
4. 未命中则释放锁。
5. 如果 `lat > 0`，在不持有 chain lock 的情况下 `thread::sleep(lat)`。
6. 重新加锁并二次查缓存。
7. 仍未命中则生成 512 字节 payload，插入 chain 并返回。

第 5 步是重要并发边界。`custom/cache_chain_regression.rs` 专门检查慢 `fetch()` 不能拿着 cache chain lock 睡眠，否则 `Kernel::tick()` 在持有 GKL 后清 cache chain 时会被阻塞。

其它方法：

- `sync_all(id)` 会进入 `GLOBAL_KERNEL_LOCK`，扫描所有 chain，把 `modified` 清掉。
- `invalidate(k)` 删除某个 key。
- `total_entries()` / `dirty_count()` 扫描所有 chain。
- `evict_cold(max_age)` 用 `TICK` 和 `slot.id * 3` 算一个伪 age，只删除 cold 且 modified 的 slot。

注意点：

- `idx(k)` 和 `invalidate(k)` 用 `k % width`，但 `fetch(k)` 用混合 hash。`k >= 128` 时二者可能选不同 chain，导致 invalidate 找不到 fetch 插入的 slot。
- cache 没有容量和替换策略，miss 会持续 push。
- `CacheSlot` 没有真实 access timestamp，`evict_cold()` 的 age 是启发式占位。
- `Kernel::tick()` 会持有 GKL 并逐 chain 清 `modified`，这是当前测试关心的 lock ordering 点。

## KObjRegistry

`KObjRegistry` 在 `cache.rs` 里，但语义不是 fs cache，更像 generic kernel object registry：

- `register()` / `register_child()` 分配自增 id。
- `type_index` 支持按 `type_tag` 查找。
- `parent_id` 支持 `dump_graph()` 输出 parent-child 边。
- `ref_up()` / `ref_down()` 只改计数。
- `gc_sweep()` 删除 `ref_count == 0` 的对象。

当前没有发现主路径调用它，应该当作从 monolith 拆出来的占位/待归类逻辑。

## Pseudo、ioctl、devfs、device

`PseudoNode` 是只读节点：

- `content` 是固定字节。
- `read_at()` 从 offset 拷贝。
- `write_at()` 永远 `Err("nosup")`。
- `ftype` 只是保存，不参与行为。

`ioctl.rs` 只定义 `TrmIO` 和 `WinSz`。`SYS_IOCTL` 用它们的 size 做 `check_access()`，不会真的把结构写回用户地址，也不会保存 termios 状态。

`devfs/mod.rs` 只有 staging 注释。`device.rs` 只有 `SocketState` enum，当前和 fs 主路径没有直接关系。

## process/proc.rs 里的 fs 调用面

`Task` 持有 `files: Mutex<BTreeMap<usize, FLike>>`：

- `add_file()` 找最低空 fd 并插入。
- `get_file()` clone 出 `FLike`。
- `close_fd()` 会 remove fd，但只 poll 一下并丢弃结果。
- `dup_fd()` 会真正 clone 并插入新 fd。

`TaskTable::spawn_root()` 会给 root task 安装 fd 0/1/2：

- fd 0：`/dev/tty`，只读。
- fd 1：`/dev/tty`，只写。
- fd 2：dup fd 1，共享内容和 offset。

`Kernel` 保存三个 fs 相关全局对象：

- `cache: BlockCache`
- `disk: Disk`
- `mnt: MountTable`

主要 syscall 行为：

- `SYS_READ`：只检查用户地址和 count，然后按 `BlockCache` 是否有 fd 对应 slot 返回长度；不会从 `Task.files` 取 `FLike` 读数据。
- `SYS_WRITE`：只检查用户地址和 count，标记 cache slot modified；fd <= 2 时给 disk ops 加一；不会写入 `FHandle.data`。
- `SYS_OPEN`：检查 path 指针和 flags，然后在当前 task 上插入一个 path 为 `"anon"` 的 `FHandle`；没有当前 task 时返回合成 fd。
- `SYS_CLOSE`：删除 cache slot，但不调用 `Task::close_fd()`，所以不一定更新 fd table。
- `SYS_STAT` / `SYS_FSTAT`：只做用户地址检查。
- `SYS_MMAP`：计算并返回一个模拟地址，不调用 `FLike::mmap_fl()`。
- `SYS_IOCTL`：按命令检查参数地址，支持 termios/window size/fionbio 这类壳。
- `SYS_PIPE`：插入 pipe 两端并返回 packed fd。
- `SYS_DUP`：当前只计算并返回一个新 fd，不把 clone 插入 fd table。
- `SYS_DUP2`：会在当前 task fd table 中复制 `old_fd` 到 `new_fd`。
- `SYS_FCNTL`：多数命令是合成返回或参数检查；`F_SETFL` 只接受 `O_NONBLOCK | O_APPEND`，不更新 `FHandle`。
- `SYS_EPOLL_*`：只做基本参数检查，不接 `EpInst`。

这意味着：如果你想理解“fd 内容读写”，看 `FHandle` / `FLike`；如果你想理解“basic syscall 测试为什么返回某个数字”，要看 `Kernel::dispatch_syscall()`，它很多时候绕过了 fd 内容模型。

## 测试覆盖

当前直接覆盖 fs 的测试主要是：

- `basic/group_06.rs`：`Disk` read retry/limit/infinite retry。
- `basic/group_07.rs`：`MountTable` resolve 和并发 bind/lookup。
- `custom/cache_chain_regression.rs`：`BlockCache::fetch()` 慢路径不能持有 chain lock 阻塞 `Kernel::tick()`。
- `custom/gkl_regression.rs`：`lookup_path()` 参与 scheduler/fs/memory 链路时保留外层 GKL 状态。

间接相关：

- `basic/group_11.rs` 的 `basic_pipe_ipc_workload` 用的是 `ipc::Channel`，不是 `fs::PipeNode`。
- process/syscall 相关测试如果增加，可能会暴露 `SYS_READ/WRITE/OPEN/CLOSE/DUP/FCNTL/EPOLL` 和 `FLike` 行为之间的差异。

## 明显占位和风险点

- 没有真实 VFS/inode/dentry/path lookup；`MountTable` 只是 prefix rewrite。
- `SYS_READ` / `SYS_WRITE` 不走 `Task.files`，和 `FLike` 内容读写是两套行为。
- `SYS_CLOSE` 不更新 fd table；`SYS_DUP` 不插入新 fd。
- `PipeNode` clone/drop 的端点计数不正确。
- `IoQueue::submit_batch()` 在超深度时可能因重复锁 `pending` 自死锁。
- `BlockCache::fetch()` 和 `invalidate()` 的 chain index 算法不一致。
- `PageCache` 容量、dirty evict、重复 LRU entry 都是简化实现。
- epoll、devfs、pseudo、ioctl 大多只保留接口形状，没有完整内核行为。

## 建议阅读顺序

1. 先读 `file.rs` 和 `file_like.rs`，理解 fd table 里到底保存什么。
2. 再读 `process/proc.rs` 的 `Task.files`、`spawn_root()` 和 `dispatch_syscall()`，区分 syscall 壳和真实 `FLike` 操作。
3. 读 `pipe.rs` 和 `epoll.rs`，把 readiness/poll 的局部模型看清楚。
4. 读 `mount.rs` 和 `Kernel::lookup_path()`，理解测试里的路径解析。
5. 最后读 `block.rs`、`cache.rs` 和对应 group/custom 测试，重点看 retry、GKL、chain lock 的并发边界。

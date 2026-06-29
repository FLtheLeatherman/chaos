# kernel.rs 拆分计划

本文档记录 `kernel/src/kernel.rs` 迁移到 `chaos-tests/src` 模块树的计划。当前约束是保留
`kernel/src/kernel.rs` 不变，且不处理 `chaos-tests/src/lib.rs` 指向该文件的符号链接。因此第一阶段
拆出的文件是模块化候选实现，不会立即接入 `chaos-tests` crate root；测试仍然通过原单体
`kernel.rs` 暴露 API。

## 参考结构

本轮参考了 `/home/istina/rCore/kernel/src`：

- `fs/`: `mod.rs`, `file.rs`, `file_like.rs`, `pipe.rs`, `epoll.rs`, `fcntl.rs`,
  `pseudo.rs`, `device.rs`, `devfs/`
- `ipc/`: `mod.rs`, `semary.rs`, `shared_mem.rs`
- `memory.rs`: 地址转换、页帧分配、`MemorySet`、内核栈、用户地址检查
- `process/`: `mod.rs`, `futex.rs`, `proc.rs`, `structs.rs`, `thread.rs`, `abi.rs`
- `sync/`: `mod.rs`, `mutex.rs`, `condvar.rs`, `event_bus.rs`, `semaphore.rs`
- `trap.rs`: tick/timer、串口输入、trap 上下文入口

Chaos 的 simulation 代码比 rCore 更偏 host-side 测试桩，所以文件名尽量靠近 rCore，但不会强行
引入 rCore 的真实内核依赖。

## 目标结构

第一阶段在 `chaos-tests/src` 下创建如下结构。`memory.rs` 和 `trap.rs` 按 rCore 保持为顶层单文件；
`fs`、`ipc`、`process`、`sync` 按 rCore 保持为目录模块。

```text
chaos-tests/src/
  fs/
    mod.rs
    devfs/
      mod.rs
    block.rs
    cache.rs
    device.rs
    epoll.rs
    fcntl.rs
    file.rs
    file_like.rs
    ioctl.rs
    mount.rs
    pipe.rs
    pseudo.rs
  ipc/
    mod.rs
    channel.rs
    semary.rs
    shared_mem.rs
  memory.rs
  process/
    mod.rs
    abi.rs
    futex.rs
    proc.rs
    structs.rs
    thread.rs
  sync/
    mod.rs
    condvar.rs
    event_bus.rs
    mutex.rs
    semaphore.rs
  trap.rs
```

后续真正接入测试面时，再把 `kernel.rs` 改成 facade 或把 `chaos-tests/src/lib.rs` 改成真实 crate
root；在当前约束下不做这一步。

## 拆分映射

### sync

对应 rCore `sync/{mutex,condvar,event_bus,semaphore}.rs`。

- `mutex.rs`: `KernLock`, `GKL`, `Spin`, `FlgGuard`
- `event_bus.rs`: `EvFlag`, `EvCb`, `EvBus`, `wait_ev`
- `condvar.rs`: `RegEp`, `InnerQueue`, `SyncQueue`
- `semaphore.rs`: `SemaInner`, `Sema`, `SemaGuard`

`FutexBucket` 和 `FutexTable` 按 rCore 放入 `process/futex.rs`，不是 `sync`。

### memory

对应 rCore 顶层 `memory.rs`，保持单文件形态，不拆成 `memory/` 目录。

- address helpers: `p2v`, `v2p`, `k_off`, `check_access`, `check_access_rw`, `cfu`, `ctu`,
  `rdu_fixup`
- frame/COW simulation: `ZoneInfo`, `PgFrame`, `FramePool`, `SharedPage`, `frame_alloc`,
  `frame_dealloc`, `frame_alloc_contig`
- heap/kernel stack: `KStk`, `heap_init`, `heap_grow`, `BuddyAllocator`
- virtual memory maps: `VmRegion`, `VmMap`, `AddrSpace`

`FramePool` 目前依赖 `GKL`，`heap_grow` 依赖 `FramePool` 内部 slots；先保持 simulation 语义，
后续再把真实内核可用的部分抽象出来。

### fs

对应 rCore `fs/{file,file_like,pipe,epoll,fcntl,pseudo}.rs`，并补充 simulation 的块设备和缓存层。

- `fcntl.rs`: `F_DUPFD` 等 fcntl 常量、`FdOpt`
- `file.rs`: `FdState`, `FHandle`, `FSeek`
- `file_like.rs`: `FLike`
- `pipe.rs`: `PipeDir`, `PipeBuf`, `PipeNode`
- `epoll.rs`: `EpData`, `EpEvent`, `EpCtlOp`, `EpInst`
- `pseudo.rs`: `PseudoNode`, `read_as_vec`
- `device.rs`, `ioctl.rs`, `devfs/`: 对齐 rCore 文件布局，先作为 simulation 设备节点预留
- `cache.rs`: simulation 额外文件，放 `PageCacheEntry`, `PageCache`, `KObjEntry`, `KObjRegistry`, `CacheSlot`,
  `CacheChain`, `BlockCache`
- `mount.rs`: simulation 额外文件，放 `MountEntry`, `MountTable`
- `block.rs`: simulation 额外文件，放 `IoRequest`, `IoQueue`, `Disk`

`CircBuf` 和 `Channel` 虽然参与 pipe-like 测试，但更像 byte-channel IPC；先放入 `ipc/channel.rs`。

### ipc

对应 rCore `ipc/{semary,shared_mem}.rs`。

- `semary.rs`: `IpcPerm`, `SemDs`, `SemArr`, `SemCtx`
- `shared_mem.rs`: `ShmTag`, `ShmCtx`, `shm_get_or_create`
- `channel.rs`: `CircBuf`, `Channel`

`SemArr` 依赖 `sync::Sema`，`ShmCtx` 依赖 `memory` 分配语义，先保留 host-side `Arc<Mutex<_>>`
模型。

### process

对应 rCore `process/{abi,futex,proc,structs,thread}.rs`。

- `abi.rs`: `ProcInit`, ELF/auxv 相关常量和用户栈布局逻辑
- `futex.rs`: `FutexBucket`, `FutexTable`
- `proc.rs`: `Task`, `TaskTable`, `Kernel`, `ProcessGroup`, `WaitQueue`, `yield_now_sync`
- `structs.rs`: `Pid`, `TaskInfo`, `SchedulePolicy`, `RunQueue`, `CapSet`, `ResourceLimits`
- `thread.rs`: `Tid`, `Pgid`, `ThdCtx`

`Task` 同时依赖 fs/ipc/memory/trap/sync；这是最后接入的 process 层，不作为第一批集成入口。

### trap

对应 rCore 顶层 `trap.rs`，保持单文件形态，不拆成 `trap/` 目录。

- context: `Context`
- trap control: `TrapCtl`
- timer/tick: `TimerEntry`, `TimerWheel`, `CLK`, `CLK_ALL`, `wclk`, `cclk`, `dtk`,
  `up_ms`, `tmr`, `ser`

`TimerEntry` 在 `kernel.rs` 顶部定义但语义上属于 trap/timer。`Context` 被 process thread context
依赖，拆分时需要先让 `trap::context` 成为稳定边界。

## 顺序

1. 建立模块目录和 `mod.rs`，只做 re-export 骨架，不接入 `lib.rs`。
2. 拆 `sync`。它是最小依赖层，后续 memory/fs/ipc/process 都依赖它。
3. 拆 `trap.rs`。`Context` 被 process/thread 使用，`CLK` 被缓存和调度代码使用。
4. 拆 `memory.rs`。先地址/frame/heap，再 `VmMap` 和 `AddrSpace`。
5. 拆 `fs`。先 `fcntl/file/pipe/file_like/epoll/pseudo`，再 `cache/mount/block`。
6. 拆 `ipc`。先 semaphore/shared memory，再 channel。
7. 拆 `process`。先 futex/sched/thread/group/resource，最后 task/Kernel。
8. 选择集成策略：把 `kernel.rs` 变成 compatibility facade，或把 `chaos-tests/src/lib.rs` 从 symlink
   改为真实 crate root 并 re-export 新模块。

每一步先机械搬运，不顺手重构命名或语义；行为修正留到模块边界稳定之后。

## 验证

- 当前阶段：因为 `kernel.rs` 不变，运行 `cd chaos-tests && cargo test --test basic` 只能证明测试
  facade 未被破坏。
- 模块文件创建阶段：每个拆分步骤后检查 `git diff -- kernel/src/kernel.rs` 为空或只有预期外部变更。
- 接入阶段：每次接入一个模块后运行对应 focused basic group，再运行整个 `basic` target。
- 后续若恢复 real-kernel 模块，需要配合 `cd kernel && make build ARCH=riscv64`，但这不属于第一阶段。

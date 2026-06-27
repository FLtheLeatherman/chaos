# TODO

## 当前脏点

- `kernel.rs` 仍通过 `#[path = "../../kernel/src/sync/mod.rs"]` 引入 `sync`。这是为了兼容 `chaos-tests/src/lib.rs` symlink 到 `kernel/src/kernel.rs` 的测试入口；等测试不再把 `kernel.rs` 当 crate root 后，应改回普通模块路径。
- `kernel/src/sync/` 目前是迁移出的最小实现，不是完整 rCore `sync`。后续需要决定是继续补齐 rCore 风格同步原语，还是把 host simulation 的同步代码单独放到仿真兼容层。
- `kernel/src/sync/mutex.rs` 当前只是 `AtomicBool + UnsafeCell` 的简单互斥锁。它支持 `.lock().unwrap()` 的调用形状，但没有 poisoning、等待队列、禁中断、调度让出等真实内核语义。
- `EventBus` 已从 `kernel.rs` 迁出，但 `kernel.rs` 中仍有一些直接修改 `event_bus.flags` 并手动 `callbacks.retain(...)` 的代码。后续应尽量改成调用 `EventBus::set/clear/change`，减少重复更新逻辑。
- `wait_ev` 在 host 下用 `thread::yield_now()`，在 `no_std` 下用 `spin_loop_hint()`。这只是最低限度兼容，后续真实内核应接入调度/等待队列，而不是长期忙等。

## sync 后续

- 给 `Mutex` 增加针对并发互斥、`try_lock`、RAII drop 释放的 focused tests，避免只通过上层 basic 间接覆盖。
- 梳理 `SyncQueue` 是否应该继续留在 `kernel.rs`，还是迁到 `kernel/src/sync/`。迁移时要保持 group_03 的 lost-wakeup / spurious-wakeup 行为。
- 考虑把 `EventFlag` 常量名从 `PROC_QUIT`、`RECV_SIG`、`SEM_ACQ` 等缩写继续改成完整名，但这会扩大 `kernel.rs` 调用点修改，建议单独做。
- 确认 `EventCallback = Box<dyn Fn(u32) -> bool + Send>` 在真实 `no_std` kernel 中是否足够；如果回调需要捕获不可 `Send` 的内核对象，需要重新设计约束。

## 测试与构建

- `cargo test --test basic` 当前通过。
- `cargo test --test custom -- --test-threads=1` 当前通过；默认并发运行 custom 可能受全局 `GKL` 和 100ms 级时序断言影响，后续应拆分或串行化这类回归。
- 真实 kernel 构建仍会继续暴露 `fs`、`ipc`、`memory`、`process`、`trap` 占位模块缺 API 的错误。这些占位只是为了越过显然的缺模块/缺 crate 错误，不代表真实子系统已恢复。

## 阅读/记录

- group_11 的综合工作流原理还需要继续整理。
- 了解 Mutex 与 EventBus 相关内容
- `docs/refactor.md` 已记录最小 `Mutex` 与 `EventBus` 迁移结论；后续如果继续拆 `SyncQueue` 或真实 rCore `sync`，同步更新该文档。

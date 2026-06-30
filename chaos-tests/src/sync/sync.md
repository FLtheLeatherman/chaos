# sync 模块说明

这份目录是 host-side simulation 的同步层，目标是保留从 `kernel.rs` 拆出来的测试表面。它不等价于真实 `no_std` kernel 里的完整同步实现，很多地方仍然直接使用 `std::thread`、`std::sync` 和忙等/park 语义。

## mutex.rs

`KernelLock` 是递归的全局内核锁。`GLOBAL_KERNEL_LOCK` 是它的全局实例，使用 `AtomicBool` 表示是否被持有，用 `ThreadId` 判断当前线程是否正在递归进入，同步测试通过 `owner_id()` 和 `recursion_level()` 观察诊断状态。

`SpinLock` 是最小的 simulation spin lock。调用者需要显式 `acquire()` 和 `release()`，没有 RAII guard。部分模块仍然直接访问它的 `locked` 字段来模拟旧代码里的手写加锁路径。

`FlagsGuard` 是原 kernel 形状里的 CPU/中断标志 guard 占位物。在当前 host simulation 中它不保存或恢复真实状态，drop 时也是 no-op。

## event_bus.rs

`EventBus` 保存两类东西：

- `flags`：当前事件状态位，比如 `READABLE`、`CLOSED`、`PROCESS_QUIT`、`CHILD_PROCESS_QUIT`、`RECEIVE_SIGNAL`、`SEMAPHORE_CAN_ACQUIRE`。
- `callbacks`：状态变化时同步调用的回调列表。回调返回 `true` 会被移除，返回 `false` 会保留。

它是 level-triggered 状态记录，不是事件历史队列。`set_flags()`、`clear_flags()` 和 `change_flags()` 只有在 `flags` 真的改变时才会调用回调。`wait_for_event_flags()` 只是 host simulation 的忙等 helper，会循环检查 flags 并 `thread::yield_now()`。

当前主要使用者是 pipe readiness、task 退出/信号状态和 semaphore acquire/remove 状态。等待/唤醒路径没有完整接入它，很多行为仍然由调用方自己 busy-loop 或 park/unpark。

## semaphore.rs

`Semaphore` 是 counting semaphore：

- `count > 0` 表示可以立即 acquire 一个单位。
- `removed` 表示 semaphore 已被删除，之后 `try_acquire()` 返回 `"removed"`。
- `process_id` 保留给 SysV semaphore PID 语义，目前不会自动更新。
- `event_bus` 发布 `SEMAPHORE_CAN_ACQUIRE` 和 `SEMAPHORE_REMOVED` 这类粗粒度状态。

`try_acquire()` 是非阻塞尝试；`acquire_by_spinning()` 会循环调用 `try_acquire()`，失败时 `thread::yield_now()`，所以它是忙等而不是真正睡眠。`access()` 返回 `SemaphoreGuard`，guard drop 时自动 `release()`。

注意：`event_callback_count()` 返回的是 event bus callback 数量，不是可靠的 semaphore 等待线程数。当前 semaphore 的阻塞路径没有真正注册 event bus callback。

## condvar.rs

`SynchronizationQueue` 是 host-thread wait queue，不是严格的 rCore/POSIX condition variable。

`WaitQueueInner.threads` 保存已经 park 的 host 线程。`saved_wakeups` 是为了兼容当前测试面的 signal-before-wait 行为：如果 `signal()` 发生时没有 waiter，它会保存一个 wake credit，让之后一次 `park_on()` 不会睡死。这一点是 semaphore-like 行为；标准 condvar 通常会丢弃这种提前 signal。

`park_on()` 是一次性 predicate wait：先检查 predicate，不满足时入队并 `thread::park()`；醒来后只再检查一次 predicate。`wait_event()` 和 `wait_events()` 会循环检查 condition，但当前实现可能在杂散唤醒后重复入队同一个线程。

`wait_guard()` 和 `wait_timeout()` 是占位 helper：它们没有接收或恢复调用方持有的 `MutexGuard`，timeout 版本总是返回 `true`，并且超时线程仍可能留在队列中。

`EpollRegistration` 和 epoll registration list 只记录 task/fd 关系，不验证 fd/epoll fd，也不会主动生成 readiness。

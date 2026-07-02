# trap.rs 模块笔记

这个模块是 `chaos-tests` 里的 trap/interrupt 测试模型。它不是完整的架构 trap 入口，而是提供一组 host-side 的轻量结构：保存用户态上下文、记录 trap dispatch、维护一些模拟 tick，并给测试一个能调用的中断/page fault 外壳。

这份笔记重点记录“读完代码后应该怎么理解它”，不逐行复述每个 helper。总体上，`trap.rs` 更像是 trap 相关概念的兼容外壳，而不是已经接起来的内核 trap path。

## 整体定位

- `SimTrapFrame`：模拟 trap frame，也就是陷入内核时保存下来的用户态寄存器快照。
- `TrapController`：模拟 trap/IRQ controller，保存 mask、最后一次 dispatch 的 frame、handler 状态等，但不真正调用 syscall、设备中断或 page fault handler。
- `TimerEntry` / `TimerWheel`：模拟 timer queue 的雏形，目前没有和 `do_tick` 自动接起来。
- `TICK` / `TICK_ALL_PROCESSORS`：全局模拟时钟；CPU 0 推进 wall tick，所有 CPU 都推进 aggregate tick。

真实内核里，trap 通常会完成从用户态/内核态进入 trap handler、保存上下文、分发 syscall/interrupt/page fault、再恢复上下文。这里的代码只建模了其中一小部分状态流转。

## 阅读结论

- 真正有行为意义的是 `SimTrapFrame` 的上下文保存/修改，以及 `TrapController` 记录最后一次 dispatch 的 frame。
- syscall 分发、page fault 修复、COW 处理都不在这个 controller 里做；这些逻辑要去 `process` 和 `memory` 看。
- vector mask 只决定是否调用 `dispatch_trap_frame()`，而这个 dispatch 当前也只是保存 frame 后原样返回。
- timer wheel 目前和全局 tick 分离，所以它不是实际调度 timer 的入口。

## TimerEntry 和 TimerWheel

- `TimerEntry.deadline`：timer 到期 tick。
- `TimerEntry.interval`：周期 timer 的间隔；为 0 时表示一次性 timer。
- `TimerEntry.callback_id`：回调编号。当前只是一个 id，不会真正执行回调。
- `TimerEntry.active`：timer 是否仍有效。
- `TimerEntry.repeat`：是否为周期 timer，由 `interval > 0` 决定。

`expired()` 用全局 `TICK` 判断是否过期。注意这里是 `TICK > deadline`，所以刚好等于 deadline 的那个 tick 还不会触发。

`TimerWheel` 是 bucketed timer queue：`deadline % TIMER_WHEEL_SIZE` 决定 timer 放到哪个 slot。`advance()` 会前进一个 slot，取出 active 且 expired 的 timer 并返回；周期 timer 会按当前 tick 加 interval 重新入队。

当前限制：

- `do_tick()` 只更新 tick，不会调用 `TimerWheel::advance()`。
- `callback_id` 没有对应 dispatch 层。
- 因此这部分更像 timer wheel 数据结构草稿，而不是已经接入调度器的 timer 子系统。

## SimTrapFrame

`SimTrapFrame` 是简化的用户上下文快照：

- `general_registers: [u64; N_REGS]`：逻辑通用寄存器数组。`N_REGS` 当前是 16，不是某个真实架构的完整寄存器组。
- `instruction_pointer`：模拟 PC/IP。
- `status_flags`：模拟状态寄存器/flags。

几个约定需要单独记住：

- `general_registers[0]` 同时被当成 syscall 参数 0 和返回值槽。
- `general_registers[N_REGS - 1]` 被当成 stack pointer。
- `general_registers[N_REGS - 2]` 被当成 thread pointer/TLS 占位。
- `syscall_argument_registers()` 直接返回前 6 个寄存器作为 syscall 参数。

读代码时主要看这些行为：

- `zeroed()`：构造全 0 frame。
- `from_registers()` / `to_registers()`：在裸寄存器数组和 `SimTrapFrame` 之间转换。
- `set_instruction_pointer()` / `set_stack_pointer()` / `set_thread_pointer()` / `set_return_value()`：按上面的模拟 ABI 修改 frame。
- `clone_with_return_value()`：复制 frame 并替换返回值。
- `changed_slots()`：比较两个 frame 哪些寄存器/IP/flags 发生变化。
- `fingerprint()`：对 frame 内容做一个轻量 hash，主要适合测试或调试。

`with_opcode_edit()` 和 `tagged_register_value()` 都是偏测试/占位风格的 helper，没有接到主 trap 或 syscall 路径里。

## TrapController

`TrapController` 现在主要记录 trap 状态，而不是执行真实分发。

成员含义：

- `handler_active`：是否正在 handler 中。`handle_interrupt()` 会设置它，结束后清掉。
- `hardware_vector_mask_bits`：模拟硬件 vector 0..=7 的 mask。
- `software_vector_mask_bits`：模拟软件 vector 8..=15 的 mask。
- `handler_nesting_depth`：模拟嵌套深度。`dispatch_trap_frame()` 只临时加一再减一。
- `last_dispatched_frame`：最后一次经过 `dispatch_trap_frame()` 的 frame。
- `saved_frame_stack`：手动 push/pop frame 的栈，主 dispatch 路径不会自动使用。
- `interrupts_enabled`：IRQ enable 占位状态。
- `interrupts_suppressed`：是否抑制中断处理的占位开关。

主要方法：

- `configure_vector_masks()`：设置软件/硬件 vector mask。
- `dispatch_trap_frame()`：保存最后一个 frame，短暂更新 nesting depth，然后原样返回 frame。
- `handle_interrupt()`：中断入口外壳，设置 `handler_active`，调用 `dispatch_trap_frame()`，最后清掉 `handler_active`。
- `handle_page_fault()`：page fault 外壳，当前总是返回 `Ok(())`。真正和进程地址空间/COW 有关的逻辑在 `process` / `memory` 侧。
- `dispatch_trap_vector()`：根据 vector 和 mask 决定是否 dispatch。
- `push_saved_frame()` / `pop_saved_frame()`：手动保存/恢复 frame 的测试辅助函数。

一个需要注意的细节：`dispatch_trap_vector()` 里 `8..=15` 的分支写在 `14` 之前，所以 vector 14 会先命中软件中断范围，后面的 page-fault 分支实际上不可达。

所以如果沿着 `TrapController` 找“真正的 trap handler”，会发现这里基本走不到真实业务逻辑。它承担的是测试兼容层职责：保存 frame、维护几个状态位、让测试能观察 mask 和 page fault 外壳。

## Tick 和串口函数

- `wall_tick()`：返回 `TICK`。
- `cpu_tick()`：返回所有 CPU 累计 tick `TICK_ALL_PROCESSORS`。
- `do_tick(cpu_id)`：CPU 0 会推进 `TICK`，所有 CPU 都推进 `TICK_ALL_PROCESSORS`。
- `uptime_msec()`：把 wall tick 换算成毫秒。当前 `USEC_PER_TICK == 1000`，所以一个 tick 就是 1ms。
- `timer(cpu_id)`：只是 `do_tick(cpu_id)` 的薄包装。
- `serial(c)`：把 `\r` 规范化成 `\n`，其它字节原样返回。

## 和其它模块的关系

- `process::thread::ThreadContext` 持有 `SimTrapFrame`，用于模拟任务运行前后的用户上下文保存。
- syscall 分发主要在 `process::proc::dispatch_syscall()`，不是从 `TrapController` 解码 `SimTrapFrame` 进入。
- page fault / COW 的相对真实逻辑在 `process::proc::handle_pgfault*()` 和 `memory::AddrSpace::handle_cow_fault()`，`TrapController::handle_page_fault()` 只是一个总是成功的 trap 层占位。
- `TimerWheel` 没有接到调度器或 wait queue；当前 tick 和 timer queue 是分离的。

## 当前测试面

`tests/basic/group_09.rs` 只覆盖三个点：

- `SimTrapFrame::from_registers()` / `to_registers()` 能保存和恢复寄存器数组。
- `TrapController::configure_vector_masks()` 能设置并读回 mask。
- `TrapController::handle_page_fault()` 在普通上下文里也返回 `Ok(())`。

这意味着现在的测试并不会验证真实 trap 分发、嵌套中断恢复、page fault 权限检查或 timer callback。

## 建议阅读顺序

1. 先看 `SimTrapFrame`，理解当前测试模型里的“寄存器”和返回值约定。
2. 再看 `TrapController`，重点看哪些函数只是记录状态，哪些路径没有真正分发。
3. 接着跳到 `process/thread.rs` 看 `ThreadContext` 如何保存 `SimTrapFrame`。
4. 最后看 `process/proc.rs` 里的 syscall dispatch 和 page fault 处理，这两块才是更接近行为逻辑的地方。

## 主要简化点

- 没有真实架构 trap entry/exit，也没有保存完整 CPU 上下文。
- `TrapController` 不连接真实 syscall、设备 IRQ、signal 或 page fault handler。
- `handle_page_fault()` 总是成功，不判断地址空间权限。
- vector 14 的 page-fault 分支当前不可达。
- timer wheel 没有和 tick、调度器或 callback 机制接线。

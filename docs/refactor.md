# Chaos Kernel Refactor Notes

本文档只保留后续恢复真实 rCore kernel 时可能继续用到的结论。当前仓库同时有两套世界：

- `kernel/src/kernel.rs` 是宿主侧 `std` 仿真，用于 `chaos-tests`。
- `kernel/src/lib.rs` / `kernel/src/main.rs` 是真实 `no_std` kernel 入口。

后续迁移时要保持二者分离：仿真代码可以从 `kernel.rs` 拆，但真实 kernel 缺失的模块和 crate 应优先恢复 rCore 原有结构。

## 1. 工具链边界

真实 kernel 应使用 rCore 原 CI 年代的工具链：

```text
kernel/                 nightly-2020-06-04
rboot/                  nightly-2020-06-04
user/rust/              nightly-2020-05-01
chaos-tests/            当前宿主工具链
```

不要在仓库根目录放统一的旧 `rust-toolchain`，否则会影响 `chaos-tests` 的 edition 2021 构建。

`kernel/rust-toolchain` 应为：

```text
nightly-2020-06-04
```

需要安装：

```bash
rustup toolchain install nightly-2020-06-04 \
  --profile minimal \
  --component rust-src \
  --component llvm-tools-preview

rustup toolchain install nightly-2020-05-01 \
  --profile minimal \
  --component rust-src \
  --component llvm-tools-preview
```

## 2. Kernel 构建环境

`kernel/build.rs` 会读取 `ARCH` 环境变量：

```rust
let _arch: String = std::env::var("ARCH").unwrap();
```

因此不要只依赖 Makefile 内部默认值。推荐显式传入：

```bash
cd kernel
env -u CARGO_TARGET_DIR ARCH=riscv64 make build
```

如果 `ARCH` 没有传进 Cargo build script，会出现：

```text
thread 'main' panicked at 'called `Result::unwrap()` on an `Err` value: NotPresent', build.rs:7:25
```

本地曾经有：

```bash
export CARGO_TARGET_DIR=/home/istina/assassyn/.sim-runtime-cache
```

这会让 Chaos 的构建产物写进另一个项目目录，导致报错路径出现 `assassyn`。这不是源码依赖纠缠，而是共享 Cargo target directory。该设置已从 `~/.bashrc` 禁用，并加了：

```bash
unset CARGO_TARGET_DIR
```

后续构建时如果仍看到旧路径，说明当前 shell 还继承了旧环境，执行：

```bash
source ~/.bashrc
```

或在命令前使用 `env -u CARGO_TARGET_DIR`。

## 3. 旧 Cargo 与 Registry 配置

`nightly-2020-06-04` 附带的 Cargo 是 `1.45.0-nightly`，不支持现代 sparse registry：

```toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

如果父目录有这种配置，旧 Cargo 会失败，例如：

```text
failed to resolve address for sparse+https: Name or service not known
```

旧 Cargo 使用的是 crates.io git index：

```text
https://github.com/rust-lang/crates.io-index
```

注意：Cargo 会沿真实项目路径向父目录查找 `.cargo/config` 或 `.cargo/config.toml`。即使设置 `HOME` 和 `CARGO_HOME`，在 `/home/istina/chaos/kernel` 下构建时仍可能读到 `/home/istina/.cargo/config.toml`。

只验证依赖解析时，可以从 `/tmp` 运行并用 `--manifest-path` 指向 kernel：

```bash
cd /tmp
env -u CARGO_TARGET_DIR \
  HOME=/tmp/chaos-home-old \
  CARGO_HOME=/tmp/chaos-home-old/.cargo \
  RUSTUP_HOME=/home/istina/.rustup \
  CARGO_NET_GIT_FETCH_WITH_CLI=true \
  cargo fetch \
    --manifest-path /home/istina/chaos/kernel/Cargo.toml \
    --target /home/istina/chaos/kernel/targets/riscv64imac-unknown-none-elf.json
```

不建议提交仓库级 `.cargo/config` 来覆盖这个问题；旧 Cargo 会合并父级 source 配置，容易引入新的冲突。

## 4. RISC-V Atomic Patch

旧 Makefile 在构建 `riscv32` / `riscv64` 时会 patch 旧 Rust `core`：

```make
patch -p0 -N -b \
    $(sysroot)/lib/rustlib/src/rust/src/libcore/sync/atomic.rs \
    src/arch/riscv/atomic.patch
```

这个 patch 是 2020 年左右 rCore 对 RISC-V byte-sized atomic 的 workaround。回到 `nightly-2020-06-04` 后保留它更接近原项目。

如果本地 rustup toolchain 在只读位置，patch 可能打印：

```text
Can't create temporary file ... Read-only file system
```

当前 Makefile 对该 patch 命令使用了 `@-patch`，失败会被忽略，后续是否真正影响编译要看后续 rustc 错误。

## 5. `rcore-memory` 是什么

`kernel/Cargo.toml` 声明：

```toml
rcore-memory = { path = "../crate/memory" }
```

这指向的是 rCore 上游的独立 crate：

```text
crate/memory/
```

它不是 `kernel/src/memory`。上游同时还有 kernel 侧 wrapper：

```text
kernel/src/memory.rs
```

两者职责不同：

- `crate/memory` 提供通用 VM 抽象：地址类型、页表 trait、`MemorySet`、`MemoryAttr`、映射 handler。
- `kernel/src/memory.rs` 连接真实 kernel：`GlobalFrameAlloc`、`FRAME_ALLOCATOR`、`alloc_frame`、`phys_to_virt`、`copy_from_user`、`MemorySet` 类型别名等。

当前仓库缺的不是一个从 `kernel.rs` 中随便拆出的模块，而是这两层都需要恢复。

## 6. `rcore-memory` 被用到的 API

本地真实 kernel 直接依赖这些 `rcore-memory` 导出：

```rust
rcore_memory::{PhysAddr, VirtAddr, Page, PAGE_SIZE, VMError}
rcore_memory::paging::{PageTable, Entry, PageTableExt}
rcore_memory::memory_set::{MemorySet, MemoryArea, MemoryAttr}
rcore_memory::memory_set::handler::{
    AccessType,
    FrameAllocator,
    MemoryHandler,
    Linear,
    Delay,
    Shared,
    SharedGuard,
    ByFrame,
    File,
}
```

关键调用面：

- 各架构 `arch/*/paging.rs` 实现 `PageTable`、`Entry`、`PageTableExt`。
- `sys_mmap` 使用 `MemorySet::push`、`pop_with_split`、`find_free_area`。
- syscall 用户指针检查使用 `check_read_ptr`、`check_write_ptr`、`check_read_array`、`check_write_array`。
- page fault 入口使用 `handle_page_fault` / `handle_page_fault_ext`。
- LKM kernel virtual memory 使用 `ByFrame<GlobalFrameAlloc>`。
- RISC-V page fault 使用 `AccessType::read/write/execute`。

`MemoryAttr` 是 builder 风格：

```rust
MemoryAttr::default().user().execute().writable()
MemoryAttr::default().user().readonly()
MemoryAttr::default().mmio(1)
```

`MemorySet` 需要的方法包括：

```text
new
new_bare
push
pop
pop_with_split
find_free_area
iter
with
activate
token
clear
translate
get_page_table_mut
handle_page_fault
handle_page_fault_ext
clone
```

## 7. `no_std + alloc`

每个 crate 都要自己声明 `no_std`。`kernel` 是 `#![no_std]` 不会自动让依赖 crate 也变成 `no_std`。

如果 `crate/memory/src/lib.rs` 是空文件，Rust 默认会链接 `std`，在 custom target 下会报：

```text
error[E0463]: can't find crate for `std`
```

`rcore-memory` 应至少有：

```rust
#![no_std]

extern crate alloc;
```

`extern crate alloc;` 的作用是在 `no_std` crate 中引入堆分配类型。`core` 自动可用，但 `Vec`、`Box`、`String`、`Arc`、`BTreeMap` 来自 `alloc`：

```rust
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::collections::BTreeMap;
```

上游 `rcore-memory` 使用 `Vec<MemoryArea>`、`Box<dyn MemoryHandler>`、`Arc<Mutex<_>>` 和 `BTreeMap`，所以它是 `no_std + alloc`，不是纯 `core`。

## 8. `kernel.rs` 中的 `VmRegion`

`kernel/src/kernel.rs` 中的 `VmRegion` / `VmMap` / `AddrSpace` 和真实 VM 概念相关，但它们属于宿主侧仿真：

```rust
pub struct VmRegion {
    pub base: usize,
    pub len: usize,
    pub flags: u32,
    pub offset: usize,
    pub tag: u16,
    pub ref_count: AtomicUsize,
}
```

对应关系大致是：

```text
VmRegion.base + len        ~= MemoryArea.start_addr/end_addr
VmRegion.flags             ~= MemoryAttr
VmMap.regions              ~= MemorySet.areas
AddrSpace                  ~= 简化版进程地址空间
```

但它不能直接替代 `rcore-memory::MemoryArea`，因为真实 `MemoryArea` 还需要：

- `attr: MemoryAttr`
- `handler: Box<dyn MemoryHandler>`
- `map` / `unmap` / `clone_map`
- `handle_page_fault`
- 和真实 `PageTable` / `Entry` trait 交互

`kernel.rs` 的 `FramePool`、`PgFrame`、`AddrSpace::handle_cow_fault` 是测试用的简化 COW/物理页模型。它适合拆到 host-only simulation 模块，不适合直接放进真实 `kernel/src/memory.rs`。

## 9. 当前恢复顺序

建议顺序：

1. 恢复 `crate/memory`，优先按 rCore 上游结构恢复，而不是从 `kernel.rs` 拆。
2. 恢复 `kernel/src/memory.rs` wrapper，让 `crate::memory::*` 引用先成立。
3. 再恢复 `sync`、`process`、`fs`、`ipc`、`trap` 等 `kernel/src/lib.rs` 已声明但当前缺失的真实模块。
4. 最后再把 `kernel.rs` 的仿真子系统拆到 host-only 兼容层，保持 `chaos-tests` 继续工作。

当前空壳 `rcore-memory` 加上 `#![no_std]` 后，可以越过 `can't find crate for std`，但后续会继续报缺：

```text
rcore_memory::PAGE_SIZE
rcore_memory::Page
rcore_memory::paging
rcore_memory::memory_set
rcore_memory::VMError
```

这说明下一步应补完整 `rcore-memory` API，而不是继续处理 `std`。

## 10. 已完成：最小 `sync::Mutex` 与 `EventBus` 迁出

本轮只做了 `sync` 的最小迁移，不恢复 rCore 上游完整 `SpinLock` / `SpinNoIrqLock` 框架。

### 10.1 `kernel/src/sync/mutex.rs`

新增了一个接近 `std::sync::Mutex` 调用形状的最小互斥锁：

```rust
Mutex::new(value)
mutex.lock().unwrap()
mutex.try_lock()
MutexGuard: Deref + DerefMut
```

当前实现是基于 `AtomicBool + UnsafeCell` 的简单自旋互斥。它的目的不是成为最终真实 kernel 同步原语，而是先把 `kernel.rs` 中对 `std::sync::Mutex` 的依赖替换成仓库内实现，并让 `chaos-tests` 继续通过。

需要注意：

- 没有实现 poisoning 语义，`LockResult` 只是为了兼容 `.lock().unwrap()` 调用形状。
- 没有实现 `SpinLock`、`SpinNoIrqLock`、`MutexSupport` 等 rCore 完整抽象。
- `chaos-tests/tests/basic/group_03.rs` 已改为通过 `chaos_tests::*` 使用这个 `Mutex`，避免再把 `std::sync::Mutex` 传入 `SyncQueue`。

### 10.2 `kernel/src/sync/event_bus.rs`

`kernel.rs` 中原来的 `EventBus` 已迁出到 `kernel/src/sync/event_bus.rs`，保持原始模型：

```rust
EventFlag
EventCallback
EventBus { flags, callbacks }
EventBus::make / set / clear / change / sub / cb_len
wait_ev
```

迁出时刻意没有加入 `wait_for_event`、async future 等额外机制。当前原则是保持行为等价，先让单体仿真少依赖内联定义。

### 10.3 `kernel.rs` 兼容入口

`chaos-tests/src/lib.rs` 是指向 `kernel/src/kernel.rs` 的 symlink；测试编译时，`kernel.rs` 会作为 `chaos-tests` 的 crate root，而不是通过 `kernel/src/lib.rs` 进入。因此 `kernel.rs` 里仍需要显式声明外部模块：

```rust
#[path = "../../kernel/src/sync/mod.rs"]
pub mod sync;
pub use self::sync::{wait_ev, EventBus, EventCallback, EventFlag, Mutex};
```

这个路径从两个视角都能指向同一个真实文件：

```text
chaos-tests/src/../../kernel/src/sync/mod.rs
kernel/src/../../kernel/src/sync/mod.rs
```

等后续 `chaos-tests` 不再把 `kernel.rs` 当作 symlink crate root，而是通过正常 crate/module 结构接入时，可以去掉这个 `#[path]`。

### 10.4 验证

当前验收命令：

```bash
cd chaos-tests
cargo test --test basic
cargo test --test custom -- --test-threads=1
```

结果：

```text
basic: 33 passed
custom: 3 passed
```

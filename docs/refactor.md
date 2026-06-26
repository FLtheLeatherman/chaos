# Chaos Kernel Refactor Notes

本文档记录本轮围绕真实内核构建环境的调查结论。当前目标不是继续调试 `kernel/src/kernel.rs` 的宿主侧模拟逻辑，而是为后续把它拆回真实内核组件前，先恢复 rCore 原项目更接近的工具链边界。

## 1. 初始现象

在当前默认 nightly 下运行：

```bash
cd kernel && make build ARCH=riscv64
```

会先遇到两个早期构建错误：

```text
patching file .../libcore/sync/atomic.rs
Hunk #1 FAILED at 156.
Hunk #2 FAILED at 310.
patch: **** Can't reopen file .../libcore/sync/atomic.rs : No such file or directory
error: `.json` target specs require -Zjson-target-spec
```

这两个错误发生在内核源码真正编译之前，说明它们来自旧构建系统和当前 Rust nightly 接口之间的不匹配。

## 2. 原项目工具链线索

继续检查仓库后，发现不同子目录本来就有不同工具链约束：

| 目录 | 证据 | 工具链 |
| --- | --- | --- |
| `kernel/` | 根目录 CI 的 build job | `nightly-2020-06-04` |
| `rboot/` | `rboot/rust-toolchain` 和 rboot 自身 CI | `nightly-2020-06-04` |
| `user/rust/` | `user/rust/rust-toolchain` 和 user 自身 CI | `nightly-2020-05-01` |
| `modules/hello_rust/` | README 要求模块和 kernel 使用同一 toolchain | 跟随 `kernel` |
| `chaos-tests/` | Cargo manifest 使用 edition 2021 | 不能被仓库根目录旧 nightly 影响 |

因此不应该在仓库根目录放一个统一的旧 `rust-toolchain`。更合适的方案是按二进制边界分层：

- `kernel/` 使用 `nightly-2020-06-04`。
- `rboot/` 继续使用 `nightly-2020-06-04`。
- `user/rust/` 继续使用 `nightly-2020-05-01`。
- `modules/hello_rust/` 构建时跟随 kernel 工具链。
- `chaos-tests/` 继续使用当前足够新的宿主工具链。

## 3. 为什么不保留当前 nightly 迁移

曾尝试过把 `kernel/Makefile` 和 `kernel/targets/*.json` 迁移到当前 nightly：

- 为 custom target 增加 `-Z json-target-spec`。
- 把 target JSON 里的旧字段更新到当前 rustc schema。
- 默认跳过 RISC-V `atomic.patch`。

这可以让当前 nightly 越过 Makefile 和 target spec 解析问题，但它偏离了原 rCore CI 环境。更重要的是，`kernel/targets/*.json` 是 rustc 不稳定接口，当前 nightly 的 target spec 格式和 `nightly-2020-06-04` 的格式不兼容。若决定复现原项目构建环境，就应该保留旧 Makefile 和旧 target JSON，而不是混入当前 nightly 迁移层。

所以本轮最终选择：

- 恢复 `kernel/Makefile` 原始行为。
- 恢复 `kernel/targets/*.json` 原始格式。
- 新增 `kernel/rust-toolchain`，让 `kernel/` 自动使用原 CI 版本。

## 4. RISC-V AtomicBool Patch 的定位

旧 Makefile 在构建 `riscv32` / `riscv64` 时会执行：

```make
patch -p0 -N -b \
    $(sysroot)/lib/rustlib/src/rust/src/libcore/sync/atomic.rs \
    src/arch/riscv/atomic.patch
```

这个 patch 是针对 2020 年左右 Rust `core` 源码布局的 workaround。它把 RISC-V 下的 `AtomicBool` 从 1 字节存储改为 4 字节存储并提高对齐，以避开旧工具链对 byte-sized atomic RMW 支持不足的问题。

在当前 nightly 中，`core::sync::atomic` 已经改成新的泛型实现，并且明确保证 `AtomicBool` 与 `bool` 拥有相同 size/alignment。当前 Rust 还在实现层面对 RISC-V 做了专门 emulation，因此旧补丁不再适合当前 nightly。

但在 `nightly-2020-06-04` 下，仓库 CI 原本就是依赖这个 patch 进行 RISC-V 构建。因此回到旧工具链后，保留 Makefile 原有 patch 步骤是更接近原项目的选择。

## 5. 已落地的仓库修改

新增：

```text
kernel/rust-toolchain
```

内容：

```text
nightly-2020-06-04
```

保留既有：

```text
rboot/rust-toolchain      nightly-2020-06-04
user/rust/rust-toolchain  nightly-2020-05-01
```

`kernel/Makefile` 和 `kernel/targets/*.json` 已恢复到原始旧工具链格式。这样 `cd kernel` 后由 rustup 自动选择 `nightly-2020-06-04`，不会影响仓库根目录下的 `chaos-tests`。

## 6. 本地安装命令

需要安装两个历史工具链：

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

`nightly-2020-06-04` 用于 kernel 和 rboot；`nightly-2020-05-01` 用于 `user/rust`。根目录不要设置旧 toolchain。

## 7. Cargo 源配置与原链接验证

`nightly-2020-06-04` 附带的 Cargo 版本是 `1.45.0-nightly`。它不支持现代 sparse registry URL，例如：

```toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

如果父目录存在这样的 Cargo 配置，旧 Cargo 会在构建 `kernel` 时失败：

```text
failed to resolve address for sparse+https: Name or service not known
```

这里的问题不是 rCore 依赖本身，而是旧 Cargo 只认识旧式 git index。按照“直接使用原链接”的要求，验证时使用了官方 crates.io git index：

```text
https://github.com/rust-lang/crates.io-index
```

一个容易踩坑的细节是：即使设置 `HOME=/tmp/...` 和 `CARGO_HOME=/tmp/...`，Cargo 仍会沿当前项目真实路径向父目录查找 `.cargo/config` 或 `.cargo/config.toml`。因此在 `/home/istina/chaos/kernel` 下构建时，仍会读到 `/home/istina/.cargo/config.toml`。符号链接到 `/tmp` 也不够，因为 Cargo 会规范化回真实路径。

如果只是验证依赖解析，可以从不位于 `/home/istina` 下面的目录运行 Cargo，并用 `--manifest-path` 指向真实 manifest：

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

但 `make build` 本身会在 `kernel/` 目录下调用 Cargo，因此完整 Makefile 验证仍需要隔离父级配置。本轮使用了完整临时副本：

```bash
cp -a /home/istina/chaos /tmp/chaos-build.<id>/chaos
cd /tmp/chaos-build.<id>/chaos/kernel
env -u CARGO_TARGET_DIR \
  HOME=/tmp/chaos-home-old \
  CARGO_HOME=/tmp/chaos-home-old/.cargo \
  RUSTUP_HOME=/home/istina/.rustup \
  CARGO_NET_GIT_FETCH_WITH_CLI=true \
  make build ARCH=riscv64
```

其中 `CARGO_NET_GIT_FETCH_WITH_CLI=true` 只是让 Cargo 调用系统 `git fetch`，方便确认它确实访问：

```text
git fetch ... https://github.com/rust-lang/crates.io-index
```

不建议在仓库中提交一个 `.cargo/config` 来覆盖该问题。旧 Cargo 会合并父级 `replace-with` 配置；把本地 source 指回 `https://github.com/rust-lang/crates.io-index` 又会和内置 `crates-io` source 冲突。

## 8. 当前后续阻塞项

即使工具链恢复正确，当前 checkout 仍缺少真实内核源码依赖：

```text
failed to read `/tmp/chaos-build.<id>/chaos/crate/memory/Cargo.toml`
No such file or directory
```

原因是 [kernel/Cargo.toml](../kernel/Cargo.toml) 中声明：

```toml
rcore-memory = { path = "../crate/memory" }
```

但当前仓库没有 `crate/memory`。

此外，[kernel/src/lib.rs](../kernel/src/lib.rs) 声明了 `fs`、`ipc`、`memory`、`process`、`sync`、`trap` 等真实内核模块，而当前 `kernel/src/` 下也缺少这些模块目录。这些属于下一阶段真实内核组件恢复/拆分工作，不是工具链切换可以直接解决的问题。

本轮已经确认：在使用 `nightly-2020-06-04`、保留原 Makefile/targets、并绕过父级 sparse 镜像配置后，`make build ARCH=riscv64` 的下一个真实阻塞点就是缺失的 `../crate/memory`，而不是 toolchain、target JSON 或 RISC-V atomic patch。

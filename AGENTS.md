# Repository Guidelines

## Project Direction

Chaos is currently a mixed repository: it contains a real rCore-derived kernel tree and a large host-side kernel simulation in `kernel/src/kernel.rs`. The immediate engineering goal is no longer only debugging `kernel.rs`; it is to split that monolith into real kernel components that can live with the rest of `kernel/src/` and eventually boot under QEMU.

## Current Task Focus

The current working task is to understand the code that has been split into `chaos-tests/src/`. Treat `chaos-tests` as the main reading target: trace functions, explain behavior, identify dead or placeholder logic, and connect each piece back to the simulation test surface. Do not assume every turn is asking for more migration work; when the user asks about a function or module, inspect the local `chaos-tests/src/` implementation and its call sites first, then explain it concretely.

Code changes are still appropriate when the user asks for cleanup or clarification, but keep them tightly scoped to the discussed `chaos-tests` behavior unless the user explicitly asks to resume kernel-module migration.

Keep the two worlds distinct while refactoring:

- `kernel/src/kernel.rs` is a `std`-using simulation crate entry, imported by `chaos-tests/src/lib.rs` through a symlink. It exists to preserve the current test surface.
- `kernel/src/main.rs` and `kernel/src/lib.rs` are the real `no_std` kernel entry points. Code moved into real kernel modules must be compatible with that environment unless it is explicitly gated as host-only test support.
- `kernel/src/lib.rs` already declares `fs`, `ipc`, `memory`, `process`, `sync`, and `trap`, but this checkout currently lacks those module directories. A split should create or restore those modules deliberately rather than hiding everything behind `kernel.rs`.

## Project Structure & Module Ownership

- `kernel/src/kernel.rs`: current 6.5k-line monolithic simulation. Treat it as a compatibility facade during migration, not as the final home for subsystem logic.
- `kernel/src/arch/`: architecture and board code for x86_64, riscv, aarch64, and mipsel. Keep platform-specific code here; shared kernel logic should not grow architecture `cfg` branches unless the arch layer exposes no suitable interface yet.
- `kernel/src/drivers/`: device framework and block, net, serial, gpu, input, irq, rtc, mmc, bus, and console drivers.
- `kernel/src/syscall/`: real syscall dispatch and syscall-family modules. Keep syscall number mapping and user pointer handling aligned with the architecture layer.
- `kernel/src/signal/`, `kernel/src/net/`, `kernel/src/lkm/`, `kernel/src/rvm/`: existing real kernel subsystems. Prefer extending these over adding parallel copies in `kernel.rs`.
- `chaos-tests/`: host-side test crate for the simulation surface. `basic` and `custom` tests exist in this checkout; the manifest also names `advanced` and `pressure`, but their files may be absent locally.
- `docs/structure.md` and `docs/record.md`: repository overview and prior debugging notes. Read them before large migration work.
- `rboot/`, `user/`, `modules/hello_rust/`, `tools/`, `tests/`: bootloader, userland/rootfs build scripts, LKM example, firmware/debug helpers, and QEMU regression scripts.

## Migration Strategy

Split by subsystem, one stable boundary at a time. A good order is constants and small utilities first, then `sync`, `memory`, `process`/scheduler, `fs`/block cache, `ipc`, `trap`, and finally syscall-facing integration.

During each split:

- Keep `chaos-tests` working by leaving `kernel.rs` as a thin compatibility facade or by re-exporting the moved public API from the new module.
- Once implementation has been moved into a real kernel module, do not leave a second copy of that implementation in `kernel.rs`; keep only compatibility facades, re-exports, or host-only test shims there.
- Prefer learning behavior and edge cases from the existing `kernel.rs` implementation when restoring real kernel modules, while adapting APIs and internals to the real `no_std` kernel environment.
- Do not move `std::thread`, `std::sync`, `Duration`, or host logging into real `no_std` modules. Put host-only behavior behind a simulation/test module or an explicit cfg boundary.
- Prefer `alloc`, `core`, existing kernel synchronization primitives, and architecture interfaces for real kernel code.
- Reuse existing module names expected by `kernel/src/lib.rs` instead of inventing new top-level subsystems.
- Avoid mixing unrelated rewrites with movement. Preserve behavior first, then tighten names and comments once tests prove the move is equivalent.
- Document any temporary compatibility layer in code with a short `// AGENT:` comment.
- Always run the relevant simulation tests or real-kernel build after a migration or fix, and report any remaining failures clearly instead of treating an untested change as done.

## Build, Test, and Development Commands

- `cd chaos-tests && cargo test --test basic`: run the available basic simulation tests.
- `cd chaos-tests && cargo test --test custom`: run local regression tests added during debugging.
- `cd chaos-tests && cargo test --test basic -- group_01`: run one focused basic test group.
- `cd chaos-tests && cargo test --test basic -- --test-threads=1`: reduce inter-test interference when debugging global state such as `GKL`.
- `cd kernel && make build ARCH=riscv64`: build the real kernel image for a target architecture.
- `cd kernel && make run ARCH=riscv64`: build and run the real kernel in QEMU.
- `cd kernel && make doc`: generate kernel documentation.
- `cd rboot && make build`: build the x86_64 UEFI bootloader with `rboot/rust-toolchain`.

`kernel/` is pinned to `nightly-2020-06-04`. Cargo from that toolchain uses the old git crates.io index and cannot consume parent Cargo configs that replace crates.io with a `sparse+https://...` registry. If a kernel build unexpectedly uses a mirror, isolate or disable the parent Cargo config instead of adding a checked-in `.cargo/config`.

If `kernel` build commands fail because the declared `fs`, `memory`, `process`, `ipc`, `sync`, or `trap` modules are still absent, treat that as migration work to be completed, not as permission to remove declarations from `lib.rs`.

## Coding Style & Comment Policy

Rust is the primary language. Follow existing Rust formatting with four-space indentation and run `cargo fmt` from the crate you edit when practical. Use descriptive `snake_case` for functions and variables, `CamelCase` for types, and `SCREAMING_SNAKE_CASE` only for true constants.

Keep code comments consistent with the current repository:

- Use concise line comments for non-obvious synchronization, memory, scheduling, trap, or ABI behavior.
- Preserve the existing attribution style. Any agent-authored comment added to code must start with `// AGENT:`; use this especially for compatibility layers, migration boundaries, temporary shims, test hooks, and non-obvious behavior introduced by an agent. Human-authored markers should remain `// HUMAN` where already present.
- Do not add broad narrative comments, decorative section banners, or bilingual rewrites of existing comments.
- Temporary instrumentation should be gated or easy to disable. For simulation logging, prefer the existing `CHAOS_LOG` flow and `println!`; default GKL logging can be silenced with `CHAOS_LOG=0`.

For real kernel modules, avoid `std` and host-only APIs. For simulation compatibility code, keep host assumptions local and obvious.

## Testing Guidelines

Add or update tests under `chaos-tests/tests/basic/group_XX.rs` or `chaos-tests/tests/custom/` when changing behavior covered by the monolithic simulation. Name tests after the behavior being checked, such as `basic_ring_wrap_around` or `slow_cache_fetch_does_not_block_tick_while_holding_gkl`.

Prefer small deterministic tests. Use timeout helpers for deadlock or blocking cases, matching the existing groups. When a migration touches global state, locks, task lifecycle, frame allocation, or block cache behavior, run the relevant focused test first and then the whole `basic` target.

For real-kernel integration, pair simulation tests with at least `cd kernel && make build ARCH=riscv64` once the missing modules have been restored far enough for the kernel crate to compile.

## Commit & Pull Request Guidelines

Recent commits use concise imperative subjects such as `pass basic group_10` and scoped fixes like `fix: unify GKL calls`. Keep commits focused and mention the affected test group or subsystem.

Pull requests should summarize the subsystem moved or fixed, list commands run, note any missing advanced/pressure coverage, and include the required agent dialogue logs plus HUMAN/AGENT code attribution described in `README.md`.

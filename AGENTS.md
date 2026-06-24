# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust-based rCore teaching OS exercise. The main debugging and rewrite target is `kernel/src/kernel.rs`, a monolithic kernel simulation used directly by the test crate through `chaos-tests/src/lib.rs` symlink. Full kernel sources live under `kernel/src/`, with architecture-specific code in `kernel/src/arch/` and custom targets in `kernel/targets/`. Tests live in `chaos-tests/tests/basic/`; the manifest also declares `advanced` and `pressure` targets, but this checkout currently includes only `basic`. Bootloader code is in `rboot/`, userland/rootfs build scripts are in `user/`, loadable-module examples are in `modules/hello_rust/`, and operational scripts/firmware are in `tools/`.

## Build, Test, and Development Commands

- `cd chaos-tests && cargo test --test basic`: run the available kernel simulation tests.
- `cd chaos-tests && cargo test --test basic -- group_01`: run one focused basic test group.
- `cd kernel && make build ARCH=riscv64`: build the kernel image for a target architecture.
- `cd kernel && make run ARCH=riscv64`: build and run the kernel in QEMU.
- `cd kernel && make doc`: generate kernel documentation.
- `cd rboot && make build`: build the x86_64 UEFI bootloader using `rboot/rust-toolchain`.

## Coding Style & Naming Conventions

Rust is the primary language. Follow existing Rust formatting with four-space indentation and run `cargo fmt` from the crate you edit when practical. Use descriptive `snake_case` for functions and variables, `CamelCase` for types, and `SCREAMING_SNAKE_CASE` only for true constants. Keep edits to `kernel/src/kernel.rs` readable: rename cryptic identifiers, extract repeated logic into small helpers, and add comments only when they clarify non-obvious synchronization, memory, or scheduling behavior.

## Testing Guidelines

Add or update tests under `chaos-tests/tests/basic/group_XX.rs` when changing behavior covered by the monolithic kernel simulation. Name tests after the behavior being checked, such as `basic_ring_wrap_around`. Prefer small, deterministic tests; use timeouts for deadlock or blocking cases, matching the existing groups.

## Commit & Pull Request Guidelines

Recent commits use concise imperative subjects such as `pass basic group_10` and scoped fixes like `fix: cargo check passed`. Keep commits focused and mention the test group or subsystem affected. Pull requests should summarize the bug or refactor, list commands run, note any missing advanced/pressure coverage, and include the required agent dialogue logs plus HUMAN/AGENT code attribution described in `README.md`.

# Rust warning baseline

This is a measured baseline, not a warning-free claim. It was recorded on
2026-07-21 with Rust 1.95.0 after a clean build under the exact rustup
toolchain declared in the repository.

From the repository root:

```bash
RUST_BIN="$(dirname "$(rustup which --toolchain 1.95.0 cargo)")"
PATH="$RUST_BIN:$PATH" cargo clean
PATH="$RUST_BIN:$PATH" cargo check --workspace --all-targets
PATH="$RUST_BIN:$PATH" cargo check --workspace --target wasm32-unknown-unknown --lib
```

The explicit `PATH` is significant. On a system where Homebrew Rust preceded
rustup, `rustup run 1.95.0 cargo` selected Cargo 1.95.0 but Cargo still found a
different `rustc` when compiling dependencies. The local wasm build runner
handles this path setup automatically.

## Baseline

| Check target | Compiler-reported warnings |
|---|---:|
| Native `vm-rust` library | 365 |
| Native `vm-rust` library test | 365 (364 duplicates of the library diagnostics) |
| Native `mod` integration test | 15 |
| Wasm `vm-rust` library | 358 |

These are rustc's per-target summary counts, not a count of unique source
issues across all targets. Most remaining diagnostics are pre-existing unused
code/imports/variables, Rust 2024 `unsafe_op_in_unsafe_fn` diagnostics, and
style or lifetime diagnostics.

The first clean run reported nine ignored `Result` or `Promise` diagnostics.
Those call sites now propagate synchronous/script errors, await browser play
promises, log asynchronous AudioContext resume failures, or use infallible
setters. The same checks now report zero ignored `Result`/`Promise` warnings.
The broader warning baseline remains open and should be reduced incrementally
without suppressing warning categories globally.

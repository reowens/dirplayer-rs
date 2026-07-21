const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");
const {
  checkToolchain,
  rustToolchainEnv,
} = require("../scripts/toolchain.cjs");

function runner(overrides = {}) {
  const output = {
    "rustup run 1.95.0 rustc --version": "rustc 1.95.0 (59807616e 2026-04-14)",
    "rustup target list --toolchain 1.95.0 --installed": "aarch64-apple-darwin\nwasm32-unknown-unknown",
    "rustup which --toolchain 1.95.0 cargo": "/toolchains/1.95.0/bin/cargo",
    "wasm-pack --version": "wasm-pack 0.14.0",
    ...overrides,
  };
  return (command, args) => output[[command, ...args].join(" ")];
}

test("accepts the exact Rust, wasm target, and wasm-pack versions", () => {
  assert.doesNotThrow(() => checkToolchain(runner()));
});

test("rejects a different Rust version", () => {
  assert.throws(
    () => checkToolchain(runner({
      "rustup run 1.95.0 rustc --version": "rustc 1.96.0 (example 2026-06-01)",
    })),
    /Expected Rust 1\.95\.0, got: rustc 1\.96\.0/,
  );
});

test("rejects a different wasm-pack version", () => {
  assert.throws(
    () => checkToolchain(runner({ "wasm-pack --version": "wasm-pack 0.15.0" })),
    /Expected wasm-pack 0\.14\.0, got: wasm-pack 0\.15\.0/,
  );
});

test("rejects a missing wasm target", () => {
  assert.throws(
    () => checkToolchain(runner({
      "rustup target list --toolchain 1.95.0 --installed": "aarch64-apple-darwin",
    })),
    /Missing wasm32-unknown-unknown for Rust 1\.95\.0/,
  );
});

test("prepends the exact rustup toolchain to PATH", () => {
  const env = rustToolchainEnv(runner(), { PATH: "/usr/bin" });
  assert.equal(env.PATH, `/toolchains/1.95.0/bin${path.delimiter}/usr/bin`);
});

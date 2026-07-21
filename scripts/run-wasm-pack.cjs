const path = require("node:path");
const { spawnSync } = require("node:child_process");
const {
  RUST_VERSION,
  checkToolchain,
  rustToolchainEnv,
} = require("./toolchain.cjs");

try {
  checkToolchain();
  const env = rustToolchainEnv();
  const rustcVersion = spawnSync("rustc", ["--version"], { encoding: "utf8", env });
  if (rustcVersion.status !== 0 || !rustcVersion.stdout.startsWith(`rustc ${RUST_VERSION} `)) {
    throw new Error(`Failed to activate Rust ${RUST_VERSION} for the wasm build`);
  }

  const result = spawnSync("wasm-pack", process.argv.slice(2), {
    cwd: path.join(__dirname, "..", "vm-rust"),
    env,
    stdio: "inherit",
  });
  if (result.error) {
    throw result.error;
  }
  process.exitCode = result.status ?? 1;
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}

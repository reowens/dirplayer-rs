const path = require("node:path");
const { spawnSync } = require("node:child_process");

const RUST_VERSION = "1.95.0";
const WASM_TARGET = "wasm32-unknown-unknown";
const WASM_PACK_VERSION = "0.14.0";

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    ...options,
  });
  if (result.error) {
    throw new Error(`Could not run ${command}: ${result.error.message}`);
  }
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || "").trim();
    throw new Error(`${command} failed${detail ? `: ${detail}` : ""}`);
  }
  return result.stdout.trim();
}

function checkToolchain(runCommand = run) {
  const rustcVersion = runCommand("rustup", ["run", RUST_VERSION, "rustc", "--version"]);
  if (!rustcVersion.startsWith(`rustc ${RUST_VERSION} `)) {
    throw new Error(`Expected Rust ${RUST_VERSION}, got: ${rustcVersion}`);
  }

  const installedTargets = runCommand("rustup", [
    "target",
    "list",
    "--toolchain",
    RUST_VERSION,
    "--installed",
  ]).split(/\r?\n/);
  if (!installedTargets.includes(WASM_TARGET)) {
    throw new Error(
      `Missing ${WASM_TARGET} for Rust ${RUST_VERSION}; run rustup target add --toolchain ${RUST_VERSION} ${WASM_TARGET}`,
    );
  }

  const wasmPackVersion = runCommand("wasm-pack", ["--version"]);
  if (wasmPackVersion !== `wasm-pack ${WASM_PACK_VERSION}`) {
    throw new Error(`Expected wasm-pack ${WASM_PACK_VERSION}, got: ${wasmPackVersion}`);
  }
}

function rustToolchainEnv(runCommand = run, env = process.env) {
  const cargoPath = runCommand("rustup", ["which", "--toolchain", RUST_VERSION, "cargo"]);
  return {
    ...env,
    PATH: `${path.dirname(cargoPath)}${path.delimiter}${env.PATH || ""}`,
  };
}

module.exports = {
  RUST_VERSION,
  WASM_PACK_VERSION,
  WASM_TARGET,
  checkToolchain,
  run,
  rustToolchainEnv,
};

if (require.main === module) {
  try {
    checkToolchain();
    process.stdout.write(
      `Rust ${RUST_VERSION}, ${WASM_TARGET}, and wasm-pack ${WASM_PACK_VERSION} are available.\n`,
    );
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}

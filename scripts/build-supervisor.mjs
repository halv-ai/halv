#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  statSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

function fail(message) {
  console.error(`build-supervisor: ${message}`);
  process.exit(1);
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, options);
  if (result.error?.code === "ENOENT") {
    fail(`${command} is required but was not found in PATH.`);
  }
  if (result.error) {
    fail(`could not run ${command}: ${result.error.message}`);
  }
  return result;
}

function explicitTarget() {
  const cargoTarget = process.env.CARGO_BUILD_TARGET?.trim();
  const tauriTarget = process.env.TAURI_ENV_TARGET_TRIPLE?.trim();
  if (cargoTarget && tauriTarget && cargoTarget !== tauriTarget) {
    fail(
      `CARGO_BUILD_TARGET (${cargoTarget}) and TAURI_ENV_TARGET_TRIPLE ` +
        `(${tauriTarget}) do not match.`,
    );
  }
  return tauriTarget || cargoTarget || null;
}

function hostTarget() {
  const result = run("rustc", ["-vV"], { encoding: "utf8" });
  if (result.status !== 0) {
    fail(`rustc -vV failed with status ${result.status ?? "unknown"}.`);
  }
  const host = result.stdout.match(/^host:\s+(\S+)$/mu)?.[1];
  if (!host) {
    fail("rustc -vV did not report a host target triple.");
  }
  return host;
}

function filesMatch(left, right) {
  if (!existsSync(right) || statSync(left).size !== statSync(right).size) {
    return false;
  }
  return readFileSync(left).equals(readFileSync(right));
}

function main() {
  const cargo = run("cargo", ["--version"], { encoding: "utf8" });
  if (cargo.status !== 0) {
    fail(`cargo --version failed with status ${cargo.status ?? "unknown"}.`);
  }

  const configuredTarget = explicitTarget();
  const target = configuredTarget ?? hostTarget();
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const manifestPath = join("src-tauri", "supervisor", "Cargo.toml");
  const cargoArgs = ["build", "--release", "--manifest-path", manifestPath];
  if (configuredTarget) {
    cargoArgs.push("--target", target);
  }

  const build = run("cargo", cargoArgs, {
    cwd: repoRoot,
    stdio: "inherit",
  });
  if (build.status !== 0) {
    fail(`cargo build failed with status ${build.status ?? "unknown"}.`);
  }

  const isWindows = target.includes("windows");
  const binaryName = `halv-supervisor${isWindows ? ".exe" : ""}`;
  const artifactPath = join(
    repoRoot,
    "src-tauri",
    "supervisor",
    "target",
    ...(configuredTarget ? [target] : []),
    "release",
    binaryName,
  );
  if (!existsSync(artifactPath) || !statSync(artifactPath).isFile()) {
    fail(`cargo did not produce the expected artifact: ${artifactPath}`);
  }

  const binariesDir = join(repoRoot, "src-tauri", "binaries");
  const outputPath = join(
    binariesDir,
    `halv-supervisor-${target}${isWindows ? ".exe" : ""}`,
  );
  mkdirSync(binariesDir, { recursive: true });
  if (!filesMatch(artifactPath, outputPath)) {
    copyFileSync(artifactPath, outputPath);
  }
  if (!isWindows) {
    chmodSync(outputPath, statSync(artifactPath).mode & 0o777);
  }

  console.log(`build-supervisor: staged ${outputPath}.`);
}

main();

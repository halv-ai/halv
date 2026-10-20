#!/usr/bin/env node
// Prepares the staged macOS Headroom runtime and checks the Vite development
// port before Tauri starts its Cargo build. A stale Vite server from this
// repository is stopped automatically; unrelated listeners are left untouched
// and reported immediately.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { connect } from "node:net";
import { dirname, join, resolve } from "node:path";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { loadRuntimeLock, RESOURCE_DIR } from "./headroom-runtime-lib.mjs";

const HOST = "127.0.0.1";
const PORT = 1420;
const PROBE_TIMEOUT_MS = 250;
const COMMAND_TIMEOUT_MS = 300;
const TERMINATION_GRACE_MS = 1_000;
const COMMAND_EXCERPT_LENGTH = 160;
const CODESIGN = "/usr/bin/codesign";
const SCRIPT_PATH = fileURLToPath(import.meta.url);

export function classifyMacRuntimeSignature(details) {
  const output = typeof details === "string" ? details : "";
  if (/^Signature=adhoc\s*$/m.test(output)) {
    return /^CodeDirectory\b.*\bflags=[^\n]*\([^\n)]*\bruntime\b[^\n)]*\)/m.test(
      output,
    )
      ? "hardened-adhoc"
      : "adhoc";
  }
  if (
    /^Authority=.+$/m.test(output) ||
    /^Signature size=\d+\s*$/m.test(output) ||
    /^TeamIdentifier=(?!not set\s*$).+$/m.test(output)
  ) {
    return "identity";
  }
  if (!output.trim() || /code object is not signed at all/i.test(output)) {
    return "unsigned";
  }
  return "unknown";
}

function commandOutput(result) {
  return [result.stdout, result.stderr]
    .filter((value) => typeof value === "string" && value.trim())
    .join("\n")
    .trim();
}

function inspectMacRuntimeSignature(executable) {
  const result = spawnSync(CODESIGN, ["--display", "--verbose=4", executable], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) {
    throw result.error;
  }
  return {
    status: result.status,
    details: commandOutput(result),
  };
}

function runChecked(command, args) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(
      commandOutput(result) || `${command} exited with ${result.status}`,
    );
  }
  return result.stdout.trim();
}

function prepareMacRuntimeForDevelopment() {
  if (process.platform !== "darwin") {
    return;
  }
  if (process.env.HALV_HEADROOM_BINARY?.trim()) {
    return;
  }

  const executable = join(RESOURCE_DIR, "headroom-runtime");
  if (!existsSync(executable)) {
    return;
  }

  const before = inspectMacRuntimeSignature(executable);
  if (
    before.status !== 0 ||
    classifyMacRuntimeSignature(before.details) !== "hardened-adhoc"
  ) {
    return;
  }

  runChecked(CODESIGN, ["--force", "--sign", "-", executable]);
  runChecked(CODESIGN, ["--verify", "--strict", "--verbose=2", executable]);

  const after = inspectMacRuntimeSignature(executable);
  if (
    after.status !== 0 ||
    classifyMacRuntimeSignature(after.details) !== "adhoc"
  ) {
    throw new Error(
      "staged Headroom runtime kept an unexpected signature after development remediation",
    );
  }

  const expectedVersion = `headroom-runtime ${loadRuntimeLock().runtimeVersion}`;
  const version = runChecked(executable, ["--version"]);
  if (version !== expectedVersion) {
    throw new Error(
      `staged Headroom runtime reported ${JSON.stringify(version)} after development remediation`,
    );
  }
  console.log(
    "dev-preflight: replaced hardened ad-hoc signature on staged Headroom runtime and verified it",
  );
}

function probePort() {
  return new Promise((resolveProbe) => {
    const socket = connect({ host: HOST, port: PORT });
    let settled = false;

    function finish(isBusy) {
      if (settled) {
        return;
      }
      settled = true;
      socket.destroy();
      resolveProbe(isBusy);
    }

    socket.setTimeout(PROBE_TIMEOUT_MS);
    socket.once("connect", () => finish("busy"));
    socket.once("timeout", () => finish("busy"));
    socket.once("error", (error) =>
      finish(error.code === "ECONNREFUSED" ? "free" : "unknown"),
    );
  });
}

function findListenerPids() {
  const result = spawnSync(
    "lsof",
    ["-ti", `:${PORT}`, "-sTCP:LISTEN"],
    {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      timeout: COMMAND_TIMEOUT_MS,
    },
  );
  if (result.error || (result.status !== 0 && result.status !== 1)) {
    return null;
  }
  if (result.status === 1) {
    return [];
  }

  const pids = result.stdout
    .split(/\s+/)
    .map((value) => Number.parseInt(value, 10))
    .filter((pid) => Number.isInteger(pid) && pid > 0);
  return [...new Set(pids)];
}

function readCommand(pid) {
  const result = spawnSync("ps", ["-p", String(pid), "-o", "command="], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    timeout: COMMAND_TIMEOUT_MS,
  });
  if (result.error || result.status !== 0) {
    return null;
  }
  return result.stdout.replace(/\s+/g, " ").trim() || null;
}

function isProcessAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code !== "ESRCH";
  }
}

function signalProcess(pid, signal) {
  try {
    process.kill(pid, signal);
    return "sent";
  } catch (error) {
    return error.code === "ESRCH" ? "gone" : "failed";
  }
}

function excerpt(command) {
  if (command.length <= COMMAND_EXCERPT_LENGTH) {
    return command;
  }
  return `${command.slice(0, COMMAND_EXCERPT_LENGTH - 3)}...`;
}

function failBusy(command, pid) {
  if (command && pid) {
    console.error(
      `dev-preflight: port ${PORT} is in use by ${excerpt(command)} ` +
        `(pid ${pid}) — stop it and retry.`,
    );
  } else {
    console.error(
      `dev-preflight: port ${PORT} is in use — stop it and retry.`,
    );
  }
  process.exitCode = 1;
}

async function waitForPortFree() {
  const deadline = Date.now() + TERMINATION_GRACE_MS;
  do {
    if (await isPortFree()) {
      return true;
    }
    await delay(50);
  } while (Date.now() < deadline);
  return isPortFree();
}

async function isPortFree() {
  const state = await probePort();
  if (state === "free") {
    return true;
  }
  if (state === "busy") {
    return false;
  }
  const pids = findListenerPids();
  return pids !== null && pids.length === 0;
}

async function main() {
  prepareMacRuntimeForDevelopment();

  const initialState = await probePort();
  if (initialState === "free") {
    return;
  }

  const pids = findListenerPids();
  if (pids === null) {
    failBusy();
    return;
  }
  if (!pids.length) {
    if (!(await isPortFree())) {
      failBusy();
    }
    return;
  }

  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const holders = pids.map((pid) => ({ pid, command: readCommand(pid) }));
  const foreignHolder = holders.find(
    ({ command }) =>
      !command || !command.includes(repoRoot) || !command.includes("vite"),
  );
  if (foreignHolder) {
    if (!foreignHolder.command && !isProcessAlive(foreignHolder.pid)) {
      if (!(await isPortFree())) {
        failBusy();
      }
      return;
    }
    failBusy(foreignHolder.command ?? "unknown process", foreignHolder.pid);
    return;
  }

  const killedPids = [];
  for (const { pid, command } of holders) {
    const result = signalProcess(pid, "SIGTERM");
    if (result === "failed") {
      failBusy(command, pid);
      return;
    }
    if (result === "sent") {
      killedPids.push(pid);
    }
  }

  await delay(TERMINATION_GRACE_MS);

  let unresolvedHolder = null;
  for (const { pid, command } of holders) {
    if (!isProcessAlive(pid)) {
      continue;
    }
    const currentCommand = readCommand(pid);
    if (
      !currentCommand ||
      !currentCommand.includes(repoRoot) ||
      !currentCommand.includes("vite")
    ) {
      unresolvedHolder ??= { pid, command: currentCommand ?? command };
      continue;
    }
    if (signalProcess(pid, "SIGKILL") === "failed") {
      unresolvedHolder ??= { pid, command: currentCommand };
    }
  }

  if (!(await waitForPortFree())) {
    if (unresolvedHolder) {
      failBusy(unresolvedHolder.command, unresolvedHolder.pid);
    } else {
      failBusy();
    }
    return;
  }

  for (const pid of killedPids) {
    console.log(`dev-preflight: killed stale dev server (pid ${pid})`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  try {
    await main();
  } catch (error) {
    console.error(`dev-preflight: ${error.message}`);
    process.exitCode = 1;
  }
}

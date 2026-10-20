#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const WORKFLOW_PATH = ".github/workflows/build.yml";
// The release check reads the macOS code-signing identity from the environment.
// Example: "Developer ID Application: Example Name (TEAMID1234)".
const SIGNING_IDENTITY = process.env.APPLE_SIGNING_IDENTITY?.trim() ?? "";
const NOTARYTOOL_KEYCHAIN_SERVICE = "com.apple.gke.notary.tool";

const HELP = `Usage: node scripts/preflight.mjs [--release]

Run Halv's local preflight checks before pushing or dispatching a release.

Options:
  --release  Run the fast gate, then the local macOS arm64 release checks.
  --help     Show this help message.

Environment:
  APPLE_SIGNING_IDENTITY         The Developer ID Application identity for the
                                 macOS build/sign check. On macOS arm64,
                                 --release fails when this value is empty.
  HALV_PREFLIGHT_NOTARY_PROFILE  The notarytool keychain profile name.

The release-only build/sign and notarization checks are skipped when their
required local macOS certificate or keychain profile is unavailable.`;

function parseArguments(argumentsToParse) {
  let release = false;

  for (const argument of argumentsToParse) {
    if (argument === "--help") {
      return { help: true, release };
    }
    if (argument === "--release") {
      release = true;
      continue;
    }

    console.error(`Unknown option: ${argument}\n`);
    console.error(HELP);
    process.exitCode = 2;
    return null;
  }

  return { help: false, release };
}

function displayArgument(argument) {
  return /^[A-Za-z0-9_./:+,=@-]+$/.test(argument)
    ? argument
    : JSON.stringify(argument);
}

function displayCommand(command, argumentsToDisplay) {
  return [command, ...argumentsToDisplay].map(displayArgument).join(" ");
}

function elapsedSeconds(startedAt) {
  return `${((Date.now() - startedAt) / 1_000).toFixed(1)}s`;
}

const results = [];

function addResult(name, status, detail, startedAt = Date.now()) {
  results.push({
    name,
    status,
    detail,
    duration: status === "SKIP" ? "-" : elapsedSeconds(startedAt),
  });
}

function skipCheck(name, detail) {
  console.log(`\n==> ${name}`);
  console.log(`SKIP: ${detail}`);
  addResult(name, "SKIP", detail);
}

function failCheck(name, detail) {
  console.log(`\n==> ${name}`);
  console.error(`FAIL: ${detail}`);
  addResult(name, "FAIL", detail);
}

function runCommand(name, command, argumentsToRun, options = {}) {
  console.log(`\n==> ${name}`);
  console.log(`$ ${displayCommand(command, options.displayArgs ?? argumentsToRun)}`);
  const startedAt = Date.now();
  const result = spawnSync(command, argumentsToRun, {
    cwd: REPO_ROOT,
    env: options.env ?? process.env,
    stdio: "inherit",
  });

  if (result.error) {
    console.error(`${name}: ${result.error.message}`);
  } else if (result.signal) {
    console.error(`${name}: terminated by ${result.signal}`);
  }

  const passed = !result.error && result.status === 0;
  addResult(
    name,
    passed ? "PASS" : "FAIL",
    passed ? "" : `exit ${result.status ?? "unavailable"}`,
    startedAt,
  );
  return passed;
}

function commandSucceeds(command, argumentsToRun) {
  const result = spawnSync(command, argumentsToRun, {
    cwd: REPO_ROOT,
    stdio: "ignore",
  });
  return !result.error && result.status === 0;
}

function resolveCargoPrefix() {
  if (commandSucceeds("cargo", ["+stable", "--version"])) {
    console.log("Using the Rust stable toolchain to match CI.");
    return ["+stable"];
  }

  console.warn(
    "WARNING: cargo +stable is unavailable; falling back to plain cargo. " +
      "The local Rust toolchain may differ from CI's stable toolchain.",
  );
  return [];
}

function findPythonYamlParser() {
  return commandSucceeds("python3", ["-c", "import yaml"]);
}

function runJavaScriptYamlSanityCheck() {
  const name = "Validate GitHub Actions YAML";
  console.log(`\n==> ${name}`);
  console.log(`$ dependency-free JavaScript sanity check ${WORKFLOW_PATH}`);
  const startedAt = Date.now();

  try {
    const yaml = readFileSync(resolve(REPO_ROOT, WORKFLOW_PATH), "utf8");
    if (!yaml.trim()) {
      throw new Error("workflow is empty");
    }

    const lines = yaml.split(/\r?\n/);
    const jobsLine = lines.findIndex((line) => /^jobs:\s*(?:#.*)?$/.test(line));
    if (jobsLine === -1) {
      throw new Error("top-level jobs: key is missing");
    }

    const firstJobsEntry = lines.slice(jobsLine + 1).find((line) => {
      const trimmed = line.trim();
      return trimmed && !trimmed.startsWith("#");
    });
    if (
      !firstJobsEntry ||
      !/^\s+[^\s#][^:]*:\s*(?:#.*)?$/.test(firstJobsEntry)
    ) {
      throw new Error("jobs: does not contain an indented mapping");
    }

    addResult(name, "PASS", "minimal JavaScript fallback", startedAt);
  } catch (error) {
    console.error(`${name}: ${error.message}`);
    addResult(name, "FAIL", error.message, startedAt);
  }
}

function runYamlCheck() {
  if (!findPythonYamlParser()) {
    console.warn(
      "NOTICE: python3 with PyYAML is unavailable; using the minimal " +
        "dependency-free JavaScript YAML sanity check.",
    );
    runJavaScriptYamlSanityCheck();
    return;
  }

  const inlineParser = [
    "import pathlib, sys, yaml",
    "path = pathlib.Path(sys.argv[1])",
    "data = yaml.safe_load(path.read_text())",
    "assert isinstance(data, dict), 'workflow root must be a mapping'",
    "assert isinstance(data.get('jobs'), dict) and data['jobs'], " +
      "'workflow jobs must be a non-empty mapping'",
  ].join("; ");
  runCommand(
    "Validate GitHub Actions YAML",
    "python3",
    ["-c", inlineParser, WORKFLOW_PATH],
    { displayArgs: ["-c", "<inline PyYAML check>", WORKFLOW_PATH] },
  );
}

function runFastGate() {
  const cargoPrefix = resolveCargoPrefix();

  runCommand("Clippy (Tauri)", "cargo", [
    ...cargoPrefix,
    "clippy",
    "--all-targets",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--",
    "-D",
    "warnings",
  ]);
  runCommand("Rust tests (Tauri)", "cargo", [
    ...cargoPrefix,
    "test",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--",
    "--test-threads=1",
  ]);
  runCommand("Frontend build", "pnpm", ["-s", "build"]);
  runCommand("Updater tests", "node", [
    "--experimental-strip-types",
    "--test",
    "tests/appUpdater.test.ts",
    "tests/updaterManifest.test.ts",
  ]);
  runCommand("Headroom runtime tests", "node", [
    "scripts/headroom-runtime.test.mjs",
  ]);
  runCommand("Native bundle tests", "node", [
    "scripts/native-bundle.test.mjs",
  ]);
  runYamlCheck();
}

function loginKeychainPath() {
  const result = spawnSync("security", ["login-keychain", "-d", "user"], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  if (result.error || result.status !== 0) {
    return null;
  }
  return result.stdout.trim().replace(/^"|"$/g, "") || null;
}

function hasSigningIdentity(identity) {
  const keychain = loginKeychainPath();
  if (!keychain) {
    return false;
  }

  const result = spawnSync(
    "security",
    ["find-identity", "-v", "-p", "codesigning", keychain],
    {
      cwd: REPO_ROOT,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    },
  );
  return !result.error && result.status === 0 && result.stdout.includes(identity);
}

function hasNotaryProfile(profile) {
  return commandSucceeds("security", [
    "find-generic-password",
    "-a",
    profile,
    "-s",
    NOTARYTOOL_KEYCHAIN_SERVICE,
  ]);
}

function runReleaseChecks() {
  if (process.platform !== "darwin" || process.arch !== "arm64") {
    const reason =
      `requires macOS arm64 (current platform: ${process.platform} ${process.arch})`;
    skipCheck("macOS arm64 Tauri release build/sign", reason);
    skipCheck("Notarytool credential check", reason);
    return;
  }

  if (!SIGNING_IDENTITY) {
    failCheck(
      "macOS arm64 Tauri release build/sign",
      "APPLE_SIGNING_IDENTITY is empty. Set it to your Developer ID " +
        "Application identity, as printed by " +
        "'security find-identity -v -p codesigning'.",
    );
  } else if (!hasSigningIdentity(SIGNING_IDENTITY)) {
    skipCheck(
      "macOS arm64 Tauri release build/sign",
      `signing identity not found in the login keychain: ${SIGNING_IDENTITY}`,
    );
  } else {
    runCommand(
      "macOS arm64 Tauri release build/sign",
      "pnpm",
      [
        "tauri",
        "build",
        "--target",
        "aarch64-apple-darwin",
        "--bundles",
        "app",
      ],
      {
        env: { ...process.env, APPLE_SIGNING_IDENTITY: SIGNING_IDENTITY },
      },
    );
  }

  const notaryProfile = process.env.HALV_PREFLIGHT_NOTARY_PROFILE?.trim();
  if (!notaryProfile) {
    skipCheck(
      "Notarytool credential check",
      "HALV_PREFLIGHT_NOTARY_PROFILE is not set",
    );
    return;
  }
  if (!commandSucceeds("xcrun", ["--find", "notarytool"])) {
    skipCheck("Notarytool credential check", "xcrun notarytool is unavailable");
    return;
  }
  if (!hasNotaryProfile(notaryProfile)) {
    skipCheck(
      "Notarytool credential check",
      "the named notarytool keychain profile was not found",
    );
    return;
  }

  runCommand(
    "Notarytool credential check",
    "xcrun",
    ["notarytool", "history", "--keychain-profile", notaryProfile],
    {
      displayArgs: [
        "notarytool",
        "history",
        "--keychain-profile",
        "<profile>",
      ],
    },
  );
}

function printSummary(releaseMode) {
  console.log(`\n${"=".repeat(72)}`);
  console.log(`Halv ${releaseMode ? "release" : "fast"} preflight summary`);
  console.log("=".repeat(72));

  for (const result of results) {
    const suffix = result.detail ? ` — ${result.detail}` : "";
    console.log(`${result.status.padEnd(4)} ${result.name} (${result.duration})${suffix}`);
  }

  const failed = results.filter((result) => result.status === "FAIL").length;
  const skipped = results.filter((result) => result.status === "SKIP").length;
  const passed = results.filter((result) => result.status === "PASS").length;
  console.log(`\n${passed} passed, ${failed} failed, ${skipped} skipped.`);

  if (failed) {
    console.error("PREFLIGHT FAILED — fix the failed checks before pushing.");
    process.exitCode = 1;
  } else {
    console.log("PREFLIGHT PASSED.");
  }
}

const options = parseArguments(process.argv.slice(2));
if (options?.help) {
  console.log(HELP);
} else if (options) {
  console.log(
    `Halv local preflight (${options.release ? "release" : "fast"} mode)`,
  );
  runFastGate();
  if (options.release) {
    runReleaseChecks();
  }
  printSummary(options.release);
}

#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { delimiter, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import process from "node:process";
import {
  RESOURCE_DIR,
  assertHeadroomSource,
  assertSupportedTarget,
  buildManifest,
  hostTarget,
  loadRuntimeLock,
  runtimeExecutableName,
  stageRuntimeTree,
  validateStagedRuntime,
} from "./headroom-runtime-lib.mjs";

function fail(message) {
  console.error(`build-headroom-runtime: ${message}`);
  process.exit(1);
}

function argumentValue(name) {
  const index = process.argv.indexOf(name);
  return index === -1 ? null : process.argv[index + 1] ?? fail(`${name} requires a value`);
}

function run(command, args, options = {}) {
  console.log(`build-headroom-runtime: ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, { stdio: "inherit", ...options });
  if (result.status !== 0) {
    fail(`${command} exited with ${result.status ?? result.error?.message ?? "an error"}`);
  }
}

function capture(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", ...options });
  if (result.status !== 0) {
    fail((result.stderr || result.stdout || result.error?.message || `${command} failed`).trim());
  }
  return result.stdout.trim();
}

function findCommand(name) {
  const path = process.env.PATH?.split(delimiter)
    .map((directory) => join(directory, process.platform === "win32" ? `${name}.exe` : name))
    .find(existsSync);
  return path ?? fail(`${name} was not found on PATH`);
}

const sourceArgument = argumentValue("--source");
if (!sourceArgument) {
  fail("pass --source /path/to/the/pinned/headroom/checkout");
}

const lock = loadRuntimeLock();
const target = argumentValue("--target") ?? hostTarget();
if (!target) {
  fail(`unsupported build host ${process.platform}-${process.arch}`);
}
assertSupportedTarget(target);
const nativeTarget = hostTarget();
if (nativeTarget !== target) {
  fail(`PyInstaller cannot cross-freeze ${target} on host ${nativeTarget ?? "unknown"}`);
}

const source = assertHeadroomSource(sourceArgument, lock);
const appRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const workRoot = join(appRoot, "src-tauri", "target", "headroom-runtime-build", target);
const environmentDir = join(workRoot, ".venv");
const distDir = join(workRoot, "dist");
const pyinstallerWorkDir = join(workRoot, "pyinstaller");
const cargoTargetDir = join(workRoot, "cargo-target");
const buildManifestPath = join(workRoot, "headroom-runtime-build.json");
const python = join(
  environmentDir,
  process.platform === "win32" ? "Scripts" : "bin",
  process.platform === "win32" ? "python.exe" : "python",
);
const uv = process.env.UV ?? findCommand("uv");
const manifest = buildManifest(lock, target);

mkdirSync(workRoot, { recursive: true });
writeFileSync(buildManifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

const buildEnv = {
  ...process.env,
  CARGO_TARGET_DIR: cargoTargetDir,
  HALV_HEADROOM_BUILD_MANIFEST: buildManifestPath,
  HALV_HEADROOM_SOURCE_DIR: source.source,
  UV_PROJECT_ENVIRONMENT: environmentDir,
};
delete buildEnv.HEADROOM_BUILD_VERSION;
delete buildEnv.HEADROOM_VERSION;
if (target.endsWith("apple-darwin")) {
  buildEnv.MACOSX_DEPLOYMENT_TARGET = "11.0";
}

run(
  uv,
  [
    "sync",
    "--project",
    source.source,
    "--frozen",
    "--extra",
    "proxy",
    "--extra",
    "code",
    "--no-dev",
    "--python",
    lock.pythonVersion,
  ],
  { env: buildEnv },
);
run(uv, ["pip", "install", "--python", python, `pyinstaller==${lock.pyinstallerVersion}`], {
  env: buildEnv,
});

const sourceVersion = capture(
  python,
  ["-c", "import headroom; print(headroom.__version__)"],
  { cwd: source.source, env: buildEnv },
);
if (sourceVersion !== lock.sourceVersion) {
  fail(`pinned fork reports ${sourceVersion}; expected ${lock.sourceVersion}`);
}

const codeAwareStatus = capture(
  python,
  [
    "-c",
    [
      "from tree_sitter import Parser",
      "from tree_sitter_language_pack import get_language",
      "parser = Parser(get_language('python'))",
      "tree = parser.parse(b'def ready():\\n    return True\\n')",
      "assert tree.root_node.type == 'module' and not tree.root_node.has_error",
      "print('code-aware-ready')",
    ].join("; "),
  ],
  { cwd: source.source, env: buildEnv },
);
if (codeAwareStatus !== "code-aware-ready") {
  fail(`Code-Aware dependency probe reported ${JSON.stringify(codeAwareStatus)}`);
}

const entry = join(appRoot, "scripts", "headroom-runtime", "entry.py");
const unfrozenVersion = capture(python, [entry, "--version"], { env: buildEnv });
if (unfrozenVersion !== `headroom-runtime ${lock.runtimeVersion}`) {
  fail(`entry point reported ${JSON.stringify(unfrozenVersion)}`);
}

rmSync(distDir, { recursive: true, force: true });
rmSync(pyinstallerWorkDir, { recursive: true, force: true });
run(
  python,
  [
    "-m",
    "PyInstaller",
    "--clean",
    "--noconfirm",
    "--distpath",
    distDir,
    "--workpath",
    pyinstallerWorkDir,
    join(appRoot, "scripts", "headroom-runtime", "headroom-runtime.spec"),
  ],
  { cwd: join(appRoot, "scripts", "headroom-runtime"), env: buildEnv },
);

const builtDir = join(distDir, "headroom-runtime");
const builtExecutable = join(builtDir, runtimeExecutableName(target));
if (!existsSync(builtExecutable)) {
  fail(`PyInstaller did not produce ${builtExecutable}`);
}

mkdirSync(RESOURCE_DIR, { recursive: true });
for (const entryName of readdirSync(RESOURCE_DIR)) {
  if (entryName !== "README.md") {
    rmSync(join(RESOURCE_DIR, entryName), { recursive: true, force: true });
  }
}
// Tauri cannot bundle links whose PyInstaller build tree is removed below.
stageRuntimeTree(builtDir, RESOURCE_DIR);
writeFileSync(join(RESOURCE_DIR, "build-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

if (process.argv.includes("--ephemeral")) {
  rmSync(workRoot, { recursive: true, force: true });
  console.log("build-headroom-runtime: removed the ephemeral build environment");
}

const validation = validateStagedRuntime({ resourceDir: RESOURCE_DIR, target });
console.log(
  `build-headroom-runtime: staged ${validation.supportFileCount} support files for ${target}`,
);

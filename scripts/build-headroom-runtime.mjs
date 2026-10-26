#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import { delimiter, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import process from "node:process";
import {
  BUILD_CONSTRAINTS_PATH,
  BUILD_REQUIREMENTS_PATH,
  RESOURCE_DIR,
  RUNTIME_FILES_MANIFEST,
  RUNTIME_MANIFEST_EXPORT_DIR,
  assertHeadroomSource,
  assertManifestDirOutsideRuntime,
  assertManagedInterpreter,
  assertSupportedTarget,
  assertUvVersion,
  buildManifest,
  collectToolchain,
  createRuntimeFilesManifest,
  determinismEnvironment,
  exportRuntimeManifest,
  hostTarget,
  loadRuntimeLock,
  pinnedRequirementVersion,
  releaseManifestName,
  runtimeExecutableName,
  stableJson,
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

function checked(action) {
  try {
    return action();
  } catch (error) {
    return fail(error.message);
  }
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

// The dirty check counts untracked files: PyInstaller reads the editable tree.
const source = checked(() => assertHeadroomSource(sourceArgument, lock));
const appRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const outputDir = resolve(argumentValue("--output-dir") ?? RESOURCE_DIR);
// The release copy of runtime-files.sha256.json goes here (for CI upload).
const manifestDir = checked(() =>
  assertManifestDirOutsideRuntime(
    argumentValue("--manifest-dir") ?? RUNTIME_MANIFEST_EXPORT_DIR,
    outputDir,
  ),
);
const workRoot = resolve(
  argumentValue("--work-dir") ??
    join(appRoot, "src-tauri", "target", "headroom-runtime-build", target),
);
const environmentDir = join(workRoot, ".venv");
const distDir = join(workRoot, "dist");
const pyinstallerWorkDir = join(workRoot, "pyinstaller");
const cargoTargetDir = join(workRoot, "cargo-target");
const licensesDir = join(workRoot, "licenses-python");
const tiktokenDir = join(workRoot, "tiktoken-cache");
const metadataDir = join(workRoot, "normalized-metadata");
// PyInstaller keeps a binary cache in the user folder. A cache in the work
// folder keeps old files and other builds out of this build.
const pyinstallerConfigDir = join(workRoot, "pyinstaller-config");
const buildManifestPath = join(workRoot, "headroom-runtime-build.json");
const helper = join(appRoot, "scripts", "headroom-runtime", "halv_runtime_build.py");
const python = join(
  environmentDir,
  process.platform === "win32" ? "Scripts" : "bin",
  process.platform === "win32" ? "python.exe" : "python",
);

// 1. uv must be the exact version in the lock: it selects the Python build.
const uv = process.env.UV ?? findCommand("uv");
const uvVersionOutput = capture(uv, ["--version"]);
checked(() => assertUvVersion(uvVersionOutput, lock));

const uvEnv = {
  ...process.env,
  UV_PYTHON_PREFERENCE: "only-managed",
  UV_PYTHON_DOWNLOADS: "automatic",
};

// 2. Only a uv-managed CPython of the pinned version can build the runtime.
run(uv, ["python", "install", "--no-bin", "--no-registry", lock.pythonVersion], { env: uvEnv });
const managedPythonDir = capture(uv, ["python", "dir"], { env: uvEnv });
const interpreter = capture(
  uv,
  ["python", "find", "--system", "--no-project", lock.pythonVersion],
  { env: uvEnv },
);
checked(() => assertManagedInterpreter(interpreter, managedPythonDir));

// 3. Determinism inputs: commit time, hash seed, and Rust path remapping.
const sourceCommitTime = Number(
  capture("git", ["-C", source.source, "log", "-1", "--format=%ct", "HEAD"]),
);
const homeDir = os.homedir();
const cargoHome = process.env.CARGO_HOME ?? join(homeDir, ".cargo");
// rustc runs in the fork so that rustup selects its rust-toolchain.toml.
const rustcVerbose = capture("rustc", ["-vV"], { cwd: source.source });
const rustCommitHash = rustcVerbose.match(/^commit-hash:\s*([0-9a-f]{40})$/m)?.[1] ?? null;
const rustSysroot = capture("rustc", ["--print", "sysroot"], { cwd: source.source });
mkdirSync(workRoot, { recursive: true });
const determinism = checked(() =>
  determinismEnvironment({
    sourceCommitTime,
    sourceDir: [source.source, realpathSync(source.source)],
    cargoHome,
    homeDir,
    cargoTargetDir,
    rustSource: {
      path: join(rustSysroot, "lib", "rustlib", "src", "rust"),
      commitHash: rustCommitHash,
    },
    appleLinker: target.endsWith("apple-darwin"),
  }),
);

const buildEnv = {
  ...uvEnv,
  ...determinism,
  CARGO_TARGET_DIR: cargoTargetDir,
  HALV_HEADROOM_BUILD_MANIFEST: buildManifestPath,
  HALV_HEADROOM_COMMIT: lock.commit,
  HALV_HEADROOM_LICENSES_DIR: licensesDir,
  HALV_HEADROOM_METADATA_DIR: metadataDir,
  HALV_HEADROOM_REPOSITORY: lock.repository,
  HALV_HEADROOM_SOURCE_DIR: source.source,
  HALV_HEADROOM_TIKTOKEN_DIR: tiktokenDir,
  PYINSTALLER_CONFIG_DIR: pyinstallerConfigDir,
  UV_PROJECT_ENVIRONMENT: environmentDir,
};
// CARGO_ENCODED_RUSTFLAGS would replace the remap flags in RUSTFLAGS.
delete buildEnv.CARGO_ENCODED_RUSTFLAGS;
delete buildEnv.HEADROOM_BUILD_VERSION;
delete buildEnv.HEADROOM_VERSION;
if (target.endsWith("apple-darwin")) {
  buildEnv.MACOSX_DEPLOYMENT_TARGET = "11.0";
}

// 4. A new environment for each build. maturin comes from a hash-pinned file
// and builds the editable fork without build isolation, so no floating build
// backend enters the runtime. The sync removes maturin when it is done.
rmSync(environmentDir, { recursive: true, force: true });
run(uv, ["venv", environmentDir, "--python", interpreter, "--no-project"], { env: buildEnv });
run(
  uv,
  ["pip", "install", "--python", python, "--require-hashes", "--no-deps", "--only-binary", ":all:", "-r", BUILD_CONSTRAINTS_PATH],
  { env: buildEnv },
);
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
    interpreter,
    "--no-build-isolation-package",
    "headroom-ai",
    "--reinstall-package",
    "headroom-ai",
  ],
  { env: buildEnv },
);
// 5. PyInstaller and its dependencies at exact versions with hashes.
run(
  uv,
  ["pip", "install", "--python", python, "--require-hashes", "--no-deps", "--only-binary", ":all:", "-r", BUILD_REQUIREMENTS_PATH],
  { env: buildEnv },
);
run(uv, ["pip", "check", "--python", python], { env: buildEnv });

const probe = JSON.parse(capture(python, ["-I", helper, "probe"], { env: buildEnv }));
checked(() => assertManagedInterpreter(probe.python.realBasePrefix, managedPythonDir));
const maturinPin = checked(() => pinnedRequirementVersion(BUILD_CONSTRAINTS_PATH, "maturin"));
if (probe.maturin?.version !== maturinPin) {
  fail(`the fork wheel was built by maturin ${probe.maturin?.version}; expected ${maturinPin}`);
}

const toolchain = collectToolchain({
  uvVersionOutput,
  pythonProbe: probe,
  determinism: {
    ...determinism,
    MACOSX_DEPLOYMENT_TARGET: buildEnv.MACOSX_DEPLOYMENT_TARGET ?? null,
    managedPythonDir,
  },
  sourceDir: source.source,
  env: buildEnv,
});
const manifest = { ...buildManifest(lock, target), toolchain };
writeFileSync(buildManifestPath, stableJson(manifest));

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

// 6. Offline data and license texts for the spec to bundle.
console.log(`build-headroom-runtime: ${capture(python, ["-I", helper, "licenses", licensesDir], { env: buildEnv })}`);
console.log(`build-headroom-runtime: ${capture(python, ["-I", helper, "tiktoken", tiktokenDir], { env: buildEnv })}`);

rmSync(distDir, { recursive: true, force: true });
rmSync(pyinstallerWorkDir, { recursive: true, force: true });
rmSync(pyinstallerConfigDir, { recursive: true, force: true });
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

mkdirSync(outputDir, { recursive: true });
for (const entryName of readdirSync(outputDir)) {
  if (entryName !== "README.md") {
    rmSync(join(outputDir, entryName), { recursive: true, force: true });
  }
}
// A separate output keeps the same file set as the bundled resource folder.
const trackedReadme = join(RESOURCE_DIR, "README.md");
if (outputDir !== RESOURCE_DIR && existsSync(trackedReadme)) {
  copyFileSync(trackedReadme, join(outputDir, "README.md"));
}
// Tauri cannot bundle links whose PyInstaller build tree is removed below.
stageRuntimeTree(builtDir, outputDir);
writeFileSync(join(outputDir, "build-manifest.json"), stableJson(manifest));

if (process.argv.includes("--ephemeral")) {
  rmSync(workRoot, { recursive: true, force: true });
  console.log("build-headroom-runtime: removed the ephemeral build environment");
}

const validation = checked(() =>
  validateStagedRuntime({ resourceDir: outputDir, target, requireToolchain: true }),
);
// 7. The per-file manifest of the unsigned runtime. It ships in the runtime
// folder, and a copy with the release asset name goes to the manifest folder.
const filesManifest = createRuntimeFilesManifest(outputDir, { target, stage: "pre-sign" });
const filesManifestText = stableJson(filesManifest);
writeFileSync(join(outputDir, RUNTIME_FILES_MANIFEST), filesManifestText);
const exportedManifest = checked(() =>
  exportRuntimeManifest({
    manifestText: filesManifestText,
    runtimeDir: outputDir,
    exportDir: manifestDir,
    name: releaseManifestName(target),
  }),
);
console.log(
  `build-headroom-runtime: staged ${validation.supportFileCount} support files for ${target} ` +
    `in ${outputDir}; ${RUNTIME_FILES_MANIFEST} lists ${filesManifest.fileCount} files; ` +
    `release copy at ${exportedManifest}`,
);

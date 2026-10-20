import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  lstatSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
} from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export const LOCK_PATH = join(APP_ROOT, "scripts", "headroom-runtime.lock.json");
export const RESOURCE_DIR = join(
  APP_ROOT,
  "src-tauri",
  "resources",
  "headroom-runtime",
);

export const SUPPORTED_TARGETS = Object.freeze([
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
]);

const TARGET_BY_HOST = Object.freeze({
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-msvc",
});

function invariant(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function canonicalJson(value) {
  if (Array.isArray(value)) {
    return value.map(canonicalJson);
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, entry]) => [key, canonicalJson(entry)]),
    );
  }
  return value;
}

function sameJson(left, right) {
  return JSON.stringify(canonicalJson(left)) === JSON.stringify(canonicalJson(right));
}

export function loadRuntimeLock(path = LOCK_PATH) {
  const lock = JSON.parse(readFileSync(path, "utf8"));
  invariant(lock.schemaVersion === 1, "unsupported Headroom runtime lock schema");
  invariant(
    typeof lock.repository === "string" && /^[\w.-]+\/[\w.-]+$/.test(lock.repository),
    "invalid Headroom repository",
  );
  invariant(
    typeof lock.commit === "string" && /^[0-9a-f]{40}$/.test(lock.commit),
    "Headroom commit must be a full lowercase Git SHA",
  );
  invariant(
    typeof lock.runtimeVersion === "string" &&
      lock.runtimeVersion.includes(lock.commit.slice(0, 12)),
    "runtimeVersion must carry the pinned Headroom commit",
  );
  invariant(
    typeof lock.uvLockSha256 === "string" && /^[0-9a-f]{64}$/.test(lock.uvLockSha256),
    "invalid Headroom uv.lock digest",
  );
  for (const key of [
    "gitUrl",
    "packageVersion",
    "sourceVersion",
    "pythonVersion",
    "pyinstallerVersion",
    "uvVersion",
    "artifactFormat",
  ]) {
    invariant(typeof lock[key] === "string" && lock[key].length > 0, `missing ${key}`);
  }
  return Object.freeze(lock);
}

export function hostTarget(platform = process.platform, arch = process.arch) {
  return TARGET_BY_HOST[`${platform}-${arch}`] ?? null;
}

export function assertSupportedTarget(target) {
  invariant(SUPPORTED_TARGETS.includes(target), `unsupported Headroom target: ${target}`);
}

export function runtimeExecutableName(target) {
  assertSupportedTarget(target);
  return target.includes("windows") ? "headroom-runtime.exe" : "headroom-runtime";
}

export function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function copyDereferencedTree(sourcePath, destinationPath, activeDirectories = new Set()) {
  const source = realpathSync(sourcePath);
  const metadata = statSync(source);
  if (!metadata.isDirectory()) {
    cpSync(source, destinationPath, { preserveTimestamps: true });
    return;
  }

  invariant(!activeDirectories.has(source), `symlink cycle while staging ${sourcePath}`);
  const nestedDirectories = new Set(activeDirectories).add(source);
  mkdirSync(destinationPath, { recursive: true });
  for (const entryName of readdirSync(source)) {
    copyDereferencedTree(
      join(source, entryName),
      join(destinationPath, entryName),
      nestedDirectories,
    );
  }
}

function materializeExternalSymlinks(path, root) {
  const metadata = lstatSync(path);
  if (metadata.isSymbolicLink()) {
    const target = realpathSync(path);
    const targetFromRoot = relative(root, target);
    const targetIsInternal =
      !isAbsolute(targetFromRoot) &&
      targetFromRoot !== ".." &&
      !targetFromRoot.startsWith(`..${sep}`);
    if (!targetIsInternal) {
      rmSync(path, { recursive: true, force: true });
      copyDereferencedTree(target, path);
    }
    return;
  }
  if (metadata.isDirectory()) {
    for (const entryName of readdirSync(path)) {
      materializeExternalSymlinks(join(path, entryName), root);
    }
  }
}

export function stageRuntimeTree(sourceDir, destinationDir) {
  const source = realpathSync(sourceDir);
  const destinationParent = dirname(destinationDir);
  mkdirSync(destinationParent, { recursive: true });
  const temporaryDestination = mkdtempSync(join(destinationParent, ".headroom-runtime-stage-"));
  try {
    for (const entryName of readdirSync(source)) {
      cpSync(join(source, entryName), join(temporaryDestination, entryName), {
        recursive: true,
        preserveTimestamps: true,
        // Keep compact internal aliases; external build-environment links are materialized below.
        verbatimSymlinks: true,
      });
    }
    const temporaryRoot = realpathSync(temporaryDestination);
    materializeExternalSymlinks(temporaryRoot, temporaryRoot);

    mkdirSync(destinationDir, { recursive: true });
    for (const entryName of readdirSync(temporaryDestination)) {
      const destinationEntry = join(destinationDir, entryName);
      rmSync(destinationEntry, { recursive: true, force: true });
      renameSync(join(temporaryDestination, entryName), destinationEntry);
    }
  } finally {
    rmSync(temporaryDestination, { recursive: true, force: true });
  }
}

function capture(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    ...options,
  });
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || result.error?.message || "unknown error").trim();
    throw new Error(`${command} ${args.join(" ")} failed: ${detail}`);
  }
  return result.stdout.trim();
}

export function inspectHeadroomSource(sourceDir) {
  const source = resolve(sourceDir);
  invariant(existsSync(join(source, ".git")), `${source} is not a Git checkout`);
  const commit = capture("git", ["-C", source, "rev-parse", "HEAD"]);
  const status = capture("git", ["-C", source, "status", "--porcelain", "--untracked-files=no"]);
  const pyproject = readFileSync(join(source, "pyproject.toml"), "utf8");
  const packageVersion = pyproject.match(
    /^\[project\][\s\S]*?^version\s*=\s*"([^"]+)"/m,
  )?.[1];
  return {
    source,
    commit,
    dirty: status.length > 0,
    packageVersion,
    uvLockSha256: sha256File(join(source, "uv.lock")),
  };
}

export function assertHeadroomSource(sourceDir, lock = loadRuntimeLock()) {
  const source = inspectHeadroomSource(sourceDir);
  invariant(!source.dirty, "pinned Headroom checkout has tracked modifications");
  invariant(
    source.commit === lock.commit,
    `Headroom checkout is ${source.commit}; expected ${lock.commit}`,
  );
  invariant(
    source.packageVersion === lock.packageVersion,
    `Headroom package version is ${source.packageVersion}; expected ${lock.packageVersion}`,
  );
  invariant(
    source.uvLockSha256 === lock.uvLockSha256,
    `Headroom uv.lock digest is ${source.uvLockSha256}; expected ${lock.uvLockSha256}`,
  );
  return source;
}

export function buildManifest(lock, target) {
  assertSupportedTarget(target);
  return {
    schemaVersion: lock.schemaVersion,
    repository: lock.repository,
    commit: lock.commit,
    packageVersion: lock.packageVersion,
    sourceVersion: lock.sourceVersion,
    runtimeVersion: lock.runtimeVersion,
    uvLockSha256: lock.uvLockSha256,
    pythonVersion: lock.pythonVersion,
    pyinstallerVersion: lock.pyinstallerVersion,
    artifactFormat: lock.artifactFormat,
    headroomExtras: ["proxy", "code"],
    target,
  };
}

function countFiles(path) {
  let count = 0;
  for (const entry of readdirSync(path, { withFileTypes: true })) {
    const entryPath = join(path, entry.name);
    count += entry.isDirectory() ? countFiles(entryPath) : 1;
  }
  return count;
}

export function validateStagedRuntime({
  resourceDir = RESOURCE_DIR,
  target = hostTarget(),
  execute = true,
  minimumSupportFiles = 10,
} = {}) {
  invariant(target, `unsupported build host ${process.platform}-${process.arch}`);
  assertSupportedTarget(target);
  const lock = loadRuntimeLock();
  const expected = buildManifest(lock, target);
  const manifestPath = join(resourceDir, "build-manifest.json");
  invariant(existsSync(manifestPath), `missing staged build manifest: ${manifestPath}`);
  const actual = JSON.parse(readFileSync(manifestPath, "utf8"));
  invariant(
    sameJson(actual, expected),
    "staged Headroom build metadata does not match the immutable lock",
  );

  const executable = join(resourceDir, runtimeExecutableName(target));
  invariant(existsSync(executable) && statSync(executable).isFile(), `missing ${executable}`);
  const supportDir = join(resourceDir, "_internal");
  invariant(existsSync(supportDir) && statSync(supportDir).isDirectory(), "missing PyInstaller _internal directory");
  invariant(
    countFiles(supportDir) >= minimumSupportFiles,
    "PyInstaller onedir support files were not staged",
  );
  invariant(
    existsSync(join(supportDir, "headroom-runtime-build.json")),
    "frozen runtime is missing embedded build metadata",
  );

  if (execute) {
    const version = capture(executable, ["--version"]);
    invariant(
      version === `headroom-runtime ${lock.runtimeVersion}`,
      `staged runtime reported ${JSON.stringify(version)}`,
    );
    const reported = JSON.parse(capture(executable, ["--build-info"]));
    invariant(
      sameJson(reported, expected),
      "runtime-reported build metadata does not match its staged manifest",
    );
  }

  return { executable, manifest: actual, supportFileCount: countFiles(supportDir) };
}

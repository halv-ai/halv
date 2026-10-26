import { createHash } from "node:crypto";
import {
  closeSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdtempSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export const LOCK_PATH = join(APP_ROOT, "scripts", "headroom-runtime.lock.json");
export const BUILD_REQUIREMENTS_PATH = join(
  APP_ROOT,
  "scripts",
  "headroom-runtime",
  "build-requirements.txt",
);
export const BUILD_CONSTRAINTS_PATH = join(
  APP_ROOT,
  "scripts",
  "headroom-runtime",
  "build-constraints.txt",
);
export const RUNTIME_FILES_MANIFEST = "runtime-files.sha256.json";
export const RUNTIME_FILES_SIGNED_MANIFEST = "runtime-files.signed.sha256.json";
// The build and the sign script put a copy of each file manifest here, with
// the release asset name, so that CI can upload them.
export const RUNTIME_MANIFEST_EXPORT_DIR = join(
  APP_ROOT,
  "src-tauri",
  "target",
  "headroom-runtime-manifests",
);
export const BUILD_METADATA_FILES = Object.freeze([
  "build-manifest.json",
  "_internal/headroom-runtime-build.json",
]);
// The build-only Python distributions. They run PyInstaller and never ship.
export const BUILD_TOOL_DISTRIBUTIONS = Object.freeze([
  "altgraph",
  "macholib",
  "pefile",
  "pyinstaller",
  "pyinstaller-hooks-contrib",
  "pywin32-ctypes",
]);
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

// The release asset names: runtime-files-<target>.sha256.json before code
// signing and runtime-files-<target>.signed.sha256.json after it.
export function releaseManifestName(target, { signed = false } = {}) {
  assertSupportedTarget(target);
  return `runtime-files-${target}.${signed ? "signed." : ""}sha256.json`;
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
  // PyInstaller reads the editable source tree, so an untracked file can ship.
  // Ignored files stay out of this check; the build output itself is ignored.
  const status = capture("git", ["-C", source, "status", "--porcelain", "--untracked-files=all"]);
  const pyproject = readFileSync(join(source, "pyproject.toml"), "utf8");
  const packageVersion = pyproject.match(
    /^\[project\][\s\S]*?^version\s*=\s*"([^"]+)"/m,
  )?.[1];
  const changes = status.split("\n").filter((line) => line.trim().length > 0);
  return {
    source,
    commit,
    dirty: changes.length > 0,
    changes,
    packageVersion,
    uvLockSha256: sha256File(join(source, "uv.lock")),
  };
}

export function assertHeadroomSource(sourceDir, lock = loadRuntimeLock()) {
  const source = inspectHeadroomSource(sourceDir);
  invariant(
    !source.dirty,
    `pinned Headroom checkout has modified or untracked files: ${source.changes.slice(0, 10).join("; ")}`,
  );
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

export function withoutToolchain(metadata) {
  if (!metadata || typeof metadata !== "object" || Array.isArray(metadata)) {
    return metadata;
  }
  const { toolchain: _toolchain, ...rest } = metadata;
  return rest;
}

function assertToolchainBlock(metadata, label) {
  const toolchain = metadata?.toolchain;
  invariant(
    toolchain && typeof toolchain === "object" && !Array.isArray(toolchain),
    `${label} has no toolchain block`,
  );
  for (const key of REQUIRED_TOOLCHAIN_KEYS) {
    invariant(key in toolchain, `${label} toolchain block is missing ${key}`);
  }
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
  requireToolchain = false,
} = {}) {
  invariant(target, `unsupported build host ${process.platform}-${process.arch}`);
  assertSupportedTarget(target);
  const lock = loadRuntimeLock();
  const expected = buildManifest(lock, target);
  const manifestPath = join(resourceDir, "build-manifest.json");
  invariant(existsSync(manifestPath), `missing staged build manifest: ${manifestPath}`);
  const actual = JSON.parse(readFileSync(manifestPath, "utf8"));
  invariant(
    sameJson(withoutToolchain(actual), expected),
    "staged Headroom build metadata does not match the immutable lock",
  );
  if (requireToolchain) {
    assertToolchainBlock(actual, "staged Headroom build metadata");
  }

  const executable = join(resourceDir, runtimeExecutableName(target));
  invariant(existsSync(executable) && statSync(executable).isFile(), `missing ${executable}`);
  const supportDir = join(resourceDir, "_internal");
  invariant(existsSync(supportDir) && statSync(supportDir).isDirectory(), "missing PyInstaller _internal directory");
  invariant(
    countFiles(supportDir) >= minimumSupportFiles,
    "PyInstaller onedir support files were not staged",
  );
  const embeddedPath = join(supportDir, "headroom-runtime-build.json");
  invariant(existsSync(embeddedPath), "frozen runtime is missing embedded build metadata");
  const embedded = JSON.parse(readFileSync(embeddedPath, "utf8"));
  invariant(
    sameJson(embedded, actual),
    "embedded Headroom build metadata does not match the staged manifest",
  );

  if (execute) {
    const version = capture(executable, ["--version"]);
    invariant(
      version === `headroom-runtime ${lock.runtimeVersion}`,
      `staged runtime reported ${JSON.stringify(version)}`,
    );
    const reported = JSON.parse(capture(executable, ["--build-info"]));
    invariant(
      sameJson(reported, actual),
      "runtime-reported build metadata does not match its staged manifest",
    );
  }

  return { executable, manifest: actual, supportFileCount: countFiles(supportDir) };
}

// ---------------------------------------------------------------------------
// Build inputs: uv version, uv-managed Python, determinism environment.
// ---------------------------------------------------------------------------

export function pinnedRequirementVersion(path, name) {
  const pattern = new RegExp(`^${name.replace(/[-_.]/g, "[-_.]")}==([^\\s;\\\\]+)`, "im");
  const match = readFileSync(path, "utf8").match(pattern);
  invariant(match, `${path} does not pin ${name} with ==`);
  return match[1];
}

export function parseUvVersion(output) {
  const match = String(output).trim().match(/^uv\s+(\d+\.\d+\.\d+(?:[-+.][0-9A-Za-z.-]+)?)/);
  invariant(match, `could not read the uv version from ${JSON.stringify(String(output).trim())}`);
  return match[1];
}

export function assertUvVersion(output, lock = loadRuntimeLock()) {
  const version = parseUvVersion(output);
  invariant(
    version === lock.uvVersion,
    `the uv binary is version ${version}; the runtime lock requires uv ${lock.uvVersion} ` +
      "(install that version, or set UV to its path)",
  );
  return version;
}

function isInside(child, parent) {
  const fromParent = relative(parent, child);
  return (
    fromParent === "" ||
    (!isAbsolute(fromParent) && fromParent !== ".." && !fromParent.startsWith(`..${sep}`))
  );
}

// The release copy of a file manifest must stay out of the runtime folder,
// because the runtime file list must not change.
export function assertManifestDirOutsideRuntime(exportDir, runtimeDir) {
  const destinationDir = resolve(exportDir);
  invariant(
    !isInside(destinationDir, resolve(runtimeDir)),
    `the manifest folder ${destinationDir} must be outside the runtime folder ${resolve(runtimeDir)}`,
  );
  return destinationDir;
}

// Writes the release copy of a file manifest.
export function exportRuntimeManifest({ manifestText, runtimeDir, exportDir, name }) {
  const destinationDir = assertManifestDirOutsideRuntime(exportDir, runtimeDir);
  mkdirSync(destinationDir, { recursive: true });
  const destination = join(destinationDir, name);
  writeFileSync(destination, manifestText);
  return destination;
}

function realOrResolved(path) {
  try {
    return realpathSync(path);
  } catch {
    return resolve(path);
  }
}

export function assertManagedInterpreter(interpreterPrefix, managedDir) {
  invariant(managedDir, "uv did not report its managed Python directory");
  const prefix = realOrResolved(interpreterPrefix);
  const managed = realOrResolved(managedDir);
  invariant(
    isInside(prefix, managed),
    `build Python ${prefix} is not a uv-managed interpreter under ${managed}`,
  );
  return prefix;
}

// rustc applies the last matching --remap-path-prefix, so the list goes from
// the widest prefix (home) to the narrowest (the source and target trees).
// rustSource maps a local rust-src component back to the /rustc/<commit> path
// that a toolchain without rust-src writes.
//
// appleLinker adds -Wl,-S. The Apple linker calculates LC_UUID before rustc
// strips the output. Without -S, the linker first writes a debug map with the
// paths of the objects in the Cargo target folder, and the length of these
// paths changes the UUID. The fork strips all symbols (strip = "symbols"), so
// -S changes only LC_UUID and the ad-hoc signature hash of its page.
export const APPLE_LINKER_RUSTFLAGS = Object.freeze(["-C", "link-arg=-Wl,-S"]);

export function determinismEnvironment({
  sourceCommitTime,
  sourceDir,
  cargoHome,
  homeDir,
  cargoTargetDir,
  rustSource = null,
  appleLinker = false,
}) {
  invariant(
    Number.isInteger(sourceCommitTime) && sourceCommitTime > 0,
    "the Headroom commit time must be a positive integer",
  );
  const seen = new Set();
  const remaps = [
    [homeDir, "/home"],
    [cargoHome, "/cargo"],
    [sourceDir, "/headroom-source"],
    [cargoTargetDir, "/cargo-target"],
    ...(rustSource?.path && /^[0-9a-f]{40}$/.test(rustSource.commitHash ?? "")
      ? [[rustSource.path, `/rustc/${rustSource.commitHash}`]]
      : []),
  ]
    .flatMap(([from, to]) => (Array.isArray(from) ? from : [from]).map((path) => [path, to]))
    .filter(([from, to]) => {
      const key = `${from}=${to}`;
      if (typeof from !== "string" || from.length === 0 || seen.has(key)) {
        return false;
      }
      seen.add(key);
      return true;
    });
  for (const [from] of remaps) {
    invariant(
      !/\s/.test(from),
      `cannot pass a path with whitespace to --remap-path-prefix: ${JSON.stringify(from)}`,
    );
  }
  return {
    SOURCE_DATE_EPOCH: String(sourceCommitTime),
    PYTHONHASHSEED: "0",
    RUSTFLAGS: [
      ...remaps.map(([from, to]) => `--remap-path-prefix=${from}=${to}`),
      ...(appleLinker ? APPLE_LINKER_RUSTFLAGS : []),
    ].join(" "),
  };
}

// ---------------------------------------------------------------------------
// Toolchain block for headroom-runtime-build.json.
// ---------------------------------------------------------------------------

export const REQUIRED_TOOLCHAIN_KEYS = Object.freeze([
  "uv",
  "python",
  "pythonPackages",
  "maturin",
  "rustc",
  "cargo",
  "node",
  "os",
  "determinism",
  "ci",
]);

function optionalCapture(run, command, args, options) {
  try {
    const value = run(command, args, options);
    return typeof value === "string" && value.length > 0 ? value : null;
  } catch {
    return null;
  }
}

function defaultRun(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", ...options });
  if (result.status !== 0) {
    throw new Error((result.stderr || result.stdout || `${command} failed`).trim());
  }
  return `${result.stdout ?? ""}${result.stdout ? "" : result.stderr ?? ""}`.trim();
}

function parseKeyValueLines(text) {
  const values = {};
  for (const line of String(text ?? "").split("\n")) {
    const index = line.indexOf(":");
    if (index > 0) {
      values[line.slice(0, index).trim()] = line.slice(index + 1).trim();
    }
  }
  return values;
}

export function collectToolchain({
  uvVersionOutput,
  pythonProbe,
  determinism,
  sourceDir,
  env = process.env,
  platform = process.platform,
  arch = process.arch,
  nodeVersion = process.version,
  run = defaultRun,
} = {}) {
  invariant(pythonProbe && typeof pythonProbe === "object", "missing Python toolchain probe");
  const rustcText = optionalCapture(run, "rustc", ["-vV"], { cwd: sourceDir, env });
  const rustc = rustcText ? parseKeyValueLines(rustcText) : null;
  const ci = {};
  for (const key of ["ImageOS", "ImageVersion", "GITHUB_SHA", "GITHUB_REPOSITORY"]) {
    if (env[key]) {
      ci[key] = env[key];
    }
  }
  if (env.GITHUB_SERVER_URL && env.GITHUB_REPOSITORY && env.GITHUB_RUN_ID) {
    ci.runUrl = `${env.GITHUB_SERVER_URL}/${env.GITHUB_REPOSITORY}/actions/runs/${env.GITHUB_RUN_ID}`;
    if (env.GITHUB_RUN_ATTEMPT) {
      ci.runAttempt = env.GITHUB_RUN_ATTEMPT;
    }
  }
  const toolchain = {
    uv: { version: parseUvVersion(uvVersionOutput), versionOutput: String(uvVersionOutput).trim() },
    python: pythonProbe.python,
    pythonPackages: pythonProbe.packages,
    maturin: pythonProbe.maturin,
    rustc: rustc ? { version: rustc.release ?? null, verbose: rustcText } : null,
    cargo: optionalCapture(run, "cargo", ["-V"], { cwd: sourceDir, env }),
    node: nodeVersion,
    os: {
      platform,
      arch,
      release: os.release(),
      version: typeof os.version === "function" ? os.version() : null,
    },
    determinism,
    ci,
  };
  if (platform === "darwin") {
    toolchain.xcode = {
      sdkVersion: optionalCapture(run, "xcrun", ["--show-sdk-version"], { env }),
      sdkBuildVersion: optionalCapture(run, "xcrun", ["--show-sdk-build-version"], { env }),
      sdkPath: optionalCapture(run, "xcrun", ["--show-sdk-path"], { env }),
      xcodebuild: optionalCapture(run, "xcodebuild", ["-version"], { env }),
      clang: optionalCapture(run, "xcrun", ["clang", "--version"], { env })?.split("\n")[0] ?? null,
    };
  }
  return toolchain;
}

// ---------------------------------------------------------------------------
// Per-file runtime manifest (runtime-files.sha256.json).
// ---------------------------------------------------------------------------

function compareCodeUnits(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

// Sorted keys at every level, with code-unit order so the output does not
// depend on the locale of the build host.
export function stableJson(value) {
  const normalize = (entry) => {
    if (Array.isArray(entry)) {
      return entry.map(normalize);
    }
    if (entry && typeof entry === "object") {
      return Object.fromEntries(
        Object.keys(entry)
          .sort(compareCodeUnits)
          .map((key) => [key, normalize(entry[key])]),
      );
    }
    return entry;
  };
  return `${JSON.stringify(normalize(value), null, 2)}\n`;
}

export function sha256Text(text) {
  return createHash("sha256").update(text).digest("hex");
}

export function buildMetadataDigest(metadata) {
  return sha256Text(stableJson(withoutToolchain(metadata)));
}

const MACHO_THIN_MAGICS = new Set([0xfeedface, 0xfeedfacf, 0xcefaedfe, 0xcffaedfe]);

export function isMachOHeader(bytes) {
  if (!bytes || bytes.length < 8) {
    return false;
  }
  const magic = bytes.readUInt32BE(0);
  if (MACHO_THIN_MAGICS.has(magic)) {
    return true;
  }
  if (magic === 0xcafebabe || magic === 0xcafebabf) {
    // Java class files share 0xcafebabe; their next word is a version >= 45.
    const architectures = bytes.readUInt32BE(4);
    return architectures > 0 && architectures < 30;
  }
  return false;
}

function readHeader(path) {
  const descriptor = openSync(path, "r");
  try {
    const buffer = Buffer.alloc(8);
    const length = readSync(descriptor, buffer, 0, 8, 0);
    return buffer.subarray(0, length);
  } finally {
    closeSync(descriptor);
  }
}

function toPosix(path) {
  return path.split(sep).join("/");
}

function isRuntimeManifestPath(path) {
  return path === RUNTIME_FILES_MANIFEST || path === RUNTIME_FILES_SIGNED_MANIFEST;
}

// On macOS each Mach-O entry can also carry two hashes of a copy without the
// code signature (codesign --remove-signature):
//   unsignedSha256          - the copy as codesign writes it.
//   unsignedLinkeditSha256  - the copy with the __LINKEDIT vmsize set to zero.
// With these hashes, verify-runtime-manifest.mjs --macos-normalize can compare
// a signed runtime with a manifest of an unsigned one, and the reverse.
export const MACHO_NORMALIZATION = "codesign-remove-signature-v1";

export function unsignedMachOHashes(path, workDir) {
  const bytes = removeSignatureCopy(path, workDir, "manifest");
  return {
    unsignedLinkeditSha256: createHash("sha256").update(zeroLinkeditVmsize(bytes)).digest("hex"),
    unsignedSha256: createHash("sha256").update(bytes).digest("hex"),
  };
}

export function listRuntimeFiles(root, { normalizeMachO = false } = {}) {
  const base = resolve(root);
  invariant(existsSync(base) && statSync(base).isDirectory(), `runtime directory not found: ${base}`);
  invariant(!normalizeMachO || process.platform === "darwin", "Mach-O normalization needs macOS codesign");
  const entries = [];
  const workDir = normalizeMachO ? mkdtempSync(join(os.tmpdir(), "halv-runtime-manifest-")) : null;
  const walk = (directory) => {
    const names = readdirSync(directory).sort(compareCodeUnits);
    for (const name of names) {
      const absolute = join(directory, name);
      const path = toPosix(relative(base, absolute));
      const metadata = lstatSync(absolute);
      if (metadata.isDirectory()) {
        walk(absolute);
        continue;
      }
      if (isRuntimeManifestPath(path)) {
        continue;
      }
      if (metadata.isSymbolicLink()) {
        const link = readlinkSync(absolute);
        entries.push({
          link,
          macho: false,
          mode: metadata.mode.toString(8),
          path,
          sha256: sha256Text(link),
          size: Buffer.byteLength(link),
        });
        continue;
      }
      invariant(metadata.isFile(), `unsupported file type in runtime: ${path}`);
      const macho = isMachOHeader(readHeader(absolute));
      entries.push({
        macho,
        mode: metadata.mode.toString(8),
        path,
        sha256: sha256File(absolute),
        size: metadata.size,
        ...(macho && workDir ? unsignedMachOHashes(absolute, workDir) : {}),
      });
    }
  };
  try {
    walk(base);
  } finally {
    if (workDir) {
      rmSync(workDir, { recursive: true, force: true });
    }
  }
  return entries.sort((left, right) => compareCodeUnits(left.path, right.path));
}

export function createRuntimeFilesManifest(
  root,
  { target, stage = "pre-sign", extraFields, normalizeMachO = process.platform === "darwin" } = {},
) {
  assertSupportedTarget(target);
  const files = listRuntimeFiles(root, { normalizeMachO });
  const metadataPath = join(root, "build-manifest.json");
  const metadata = existsSync(metadataPath)
    ? JSON.parse(readFileSync(metadataPath, "utf8"))
    : null;
  return {
    buildMetadataSha256: metadata ? buildMetadataDigest(metadata) : null,
    commit: metadata?.commit ?? null,
    fileCount: files.length,
    files,
    kind: "halv-headroom-runtime-files",
    machoNormalization: normalizeMachO ? MACHO_NORMALIZATION : null,
    runtimeVersion: metadata?.runtimeVersion ?? null,
    schemaVersion: 1,
    stage,
    target,
    totalSize: files.reduce((total, entry) => total + entry.size, 0),
    ...extraFields,
  };
}

export function loadRuntimeFilesManifest(path) {
  const manifest = JSON.parse(readFileSync(path, "utf8"));
  invariant(
    manifest?.kind === "halv-headroom-runtime-files" && manifest.schemaVersion === 1,
    `${path} is not a Halv runtime file manifest`,
  );
  invariant(Array.isArray(manifest.files), `${path} has no file list`);
  return manifest;
}

// ---------------------------------------------------------------------------
// Comparison and macOS signature normalization.
// ---------------------------------------------------------------------------

export const COMPARISON_CATEGORIES = Object.freeze([
  ["exact", "exact match", true],
  ["signatureOnly", "Mach-O signature only (macOS normalize)", true],
  ["linkeditOnly", "Mach-O __LINKEDIT vmsize only (macOS normalize)", true],
  ["toolchainOnly", "build metadata toolchain block only", true],
  ["modeDiffers", "file mode differs", false],
  ["contentDiffers", "content differs", false],
  ["missing", "missing from runtime", false],
  ["unexpected", "not in expected list", false],
]);

function machOSlices(bytes) {
  const magic = bytes.readUInt32BE(0);
  if (magic === 0xcafebabe || magic === 0xcafebabf) {
    const count = bytes.readUInt32BE(4);
    const slices = [];
    for (let index = 0; index < count; index += 1) {
      if (magic === 0xcafebabe) {
        const base = 8 + index * 20;
        slices.push(bytes.readUInt32BE(base + 8));
      } else {
        const base = 8 + index * 32;
        slices.push(Number(bytes.readBigUInt64BE(base + 8)));
      }
    }
    return slices;
  }
  return [0];
}

// Sets the __LINKEDIT vmsize to zero in every slice. A removed signature
// leaves vmsize at the page count of the old signature, so this field alone
// differs between a re-signed copy and the original unsigned file.
export function zeroLinkeditVmsize(input) {
  const bytes = Buffer.from(input);
  for (const offset of machOSlices(bytes)) {
    if (offset + 28 > bytes.length) {
      continue;
    }
    const magic = bytes.readUInt32LE(offset);
    const is64 = magic === 0xfeedfacf;
    if (!is64 && magic !== 0xfeedface) {
      continue;
    }
    const commandCount = bytes.readUInt32LE(offset + 16);
    let cursor = offset + (is64 ? 32 : 28);
    for (let index = 0; index < commandCount && cursor + 8 <= bytes.length; index += 1) {
      const command = bytes.readUInt32LE(cursor);
      const size = bytes.readUInt32LE(cursor + 4);
      if (size < 8) {
        break;
      }
      const segmentName = bytes
        .subarray(cursor + 8, cursor + 24)
        .toString("latin1")
        .replace(/\0+$/, "");
      if (command === 0x19 && segmentName === "__LINKEDIT") {
        bytes.writeBigUInt64LE(0n, cursor + 32);
      } else if (command === 0x1 && segmentName === "__LINKEDIT") {
        bytes.writeUInt32LE(0, cursor + 28);
      }
      cursor += size;
    }
  }
  return bytes;
}

function removeSignatureCopy(path, workDir, label) {
  const copy = join(workDir, `${label}-${createHash("sha1").update(path).digest("hex")}`);
  cpSync(path, copy);
  const result = spawnSync("/usr/bin/codesign", ["--remove-signature", copy], { encoding: "utf8" });
  // An unsigned file makes codesign fail; its bytes are already normalized.
  if (result.status !== 0 && !/not signed/i.test(`${result.stderr}${result.stdout}`)) {
    throw new Error(`codesign --remove-signature failed for ${path}: ${result.stderr.trim()}`);
  }
  return readFileSync(copy);
}

function readJsonOrNull(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return null;
  }
}

export function compareRuntimeFiles({
  runtimeDir,
  expected,
  expectedDir = null,
  expectedBuildMetadataSha256 = null,
  macosNormalize = false,
}) {
  invariant(
    !macosNormalize ||
      expectedDir ||
      !expected.some((entry) => entry.macho && !entry.link) ||
      expected.some((entry) => entry.unsignedSha256),
    "--macos-normalize needs a reference runtime directory or a manifest with unsigned Mach-O hashes",
  );
  invariant(
    !macosNormalize || process.platform === "darwin",
    "--macos-normalize needs macOS codesign",
  );
  const actual = listRuntimeFiles(runtimeDir);
  const expectedByPath = new Map(expected.map((entry) => [entry.path, entry]));
  const actualByPath = new Map(actual.map((entry) => [entry.path, entry]));
  const results = Object.fromEntries(COMPARISON_CATEGORIES.map(([key]) => [key, []]));
  const workDir = macosNormalize ? mkdtempSync(join(os.tmpdir(), "halv-runtime-normalize-")) : null;
  try {
    for (const entry of expected) {
      if (!actualByPath.has(entry.path)) {
        results.missing.push({ path: entry.path });
      }
    }
    for (const entry of actual) {
      const reference = expectedByPath.get(entry.path);
      if (!reference) {
        results.unexpected.push({ path: entry.path });
        continue;
      }
      if (entry.sha256 === reference.sha256 && entry.size === reference.size) {
        if (entry.mode !== reference.mode) {
          results.modeDiffers.push({ path: entry.path, actual: entry.mode, expected: reference.mode });
        } else {
          results.exact.push({ path: entry.path });
        }
        continue;
      }
      if (BUILD_METADATA_FILES.includes(entry.path)) {
        const local = readJsonOrNull(join(runtimeDir, entry.path));
        const referenceDigest = expectedDir
          ? (() => {
              const remote = readJsonOrNull(join(expectedDir, entry.path));
              return remote ? buildMetadataDigest(remote) : null;
            })()
          : expectedBuildMetadataSha256;
        if (local && referenceDigest && buildMetadataDigest(local) === referenceDigest) {
          results.toolchainOnly.push({ path: entry.path });
          continue;
        }
      }
      if (macosNormalize && entry.macho && reference.macho && !entry.link && !reference.link) {
        const left = removeSignatureCopy(join(runtimeDir, entry.path), workDir, "actual");
        if (expectedDir) {
          const right = removeSignatureCopy(join(expectedDir, entry.path), workDir, "expected");
          if (left.equals(right)) {
            results.signatureOnly.push({ path: entry.path });
            continue;
          }
          if (zeroLinkeditVmsize(left).equals(zeroLinkeditVmsize(right))) {
            results.linkeditOnly.push({ path: entry.path });
            continue;
          }
        } else if (reference.unsignedSha256) {
          if (createHash("sha256").update(left).digest("hex") === reference.unsignedSha256) {
            results.signatureOnly.push({ path: entry.path });
            continue;
          }
          const normalized = createHash("sha256").update(zeroLinkeditVmsize(left)).digest("hex");
          if (normalized === reference.unsignedLinkeditSha256) {
            results.linkeditOnly.push({ path: entry.path });
            continue;
          }
        }
      }
      results.contentDiffers.push({
        path: entry.path,
        actual: entry.sha256,
        expected: reference.sha256,
        macho: entry.macho,
      });
    }
  } finally {
    if (workDir) {
      rmSync(workDir, { recursive: true, force: true });
    }
  }
  const failures = COMPARISON_CATEGORIES.filter(([, , ok]) => !ok).reduce(
    (total, [key]) => total + results[key].length,
    0,
  );
  return { results, passed: failures === 0, failures, fileCount: actual.length };
}

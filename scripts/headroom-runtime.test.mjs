import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  BUILD_CONSTRAINTS_PATH,
  BUILD_REQUIREMENTS_PATH,
  REQUIRED_TOOLCHAIN_KEYS,
  RUNTIME_FILES_MANIFEST,
  RUNTIME_FILES_SIGNED_MANIFEST,
  RUNTIME_MANIFEST_EXPORT_DIR,
  SUPPORTED_TARGETS,
  assertHeadroomSource,
  assertManagedInterpreter,
  assertUvVersion,
  buildManifest,
  collectToolchain,
  compareRuntimeFiles,
  createRuntimeFilesManifest,
  determinismEnvironment,
  exportRuntimeManifest,
  inspectHeadroomSource,
  isMachOHeader,
  listRuntimeFiles,
  loadRuntimeLock,
  pinnedRequirementVersion,
  releaseManifestName,
  runtimeExecutableName,
  stableJson,
  stageRuntimeTree,
  validateStagedRuntime,
  zeroLinkeditVmsize,
} from "./headroom-runtime-lib.mjs";
import { classifyMacRuntimeSignature } from "./dev-preflight.mjs";

const ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));

test("runtime lock couples the display version to a full fork commit", () => {
  const lock = loadRuntimeLock();
  assert.match(lock.commit, /^[0-9a-f]{40}$/);
  assert.equal(lock.runtimeVersion, `${lock.sourceVersion}+halv.g${lock.commit.slice(0, 12)}`);
  assert.equal(lock.artifactFormat, "pyinstaller-onedir-v1");
});

test("production manifest requires the Code-Aware Headroom extra", () => {
  const manifest = buildManifest(loadRuntimeLock(), "aarch64-apple-darwin");
  assert.deepEqual(manifest.headroomExtras, ["proxy", "code"]);
});

test("all release targets have deterministic executable names", () => {
  assert.deepEqual(SUPPORTED_TARGETS, [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
  ]);
  assert.equal(runtimeExecutableName(SUPPORTED_TARGETS[0]), "headroom-runtime");
  assert.equal(runtimeExecutableName(SUPPORTED_TARGETS[3]), "headroom-runtime.exe");
});

test("dev preflight classifies only hardened ad-hoc signatures for remediation", () => {
  assert.equal(
    classifyMacRuntimeSignature(`
CodeDirectory v=20500 size=109914 flags=0x10002(adhoc,runtime) hashes=3428+2 location=embedded
Signature=adhoc
TeamIdentifier=not set
Runtime Version=11.1.0
`),
    "hardened-adhoc",
  );
  assert.equal(
    classifyMacRuntimeSignature(`
CodeDirectory v=20400 size=109914 flags=0x2(adhoc) hashes=3428+2 location=embedded
Signature=adhoc
TeamIdentifier=not set
`),
    "adhoc",
  );
  assert.equal(
    classifyMacRuntimeSignature(`
CodeDirectory v=20500 size=109914 flags=0x10000(runtime) hashes=3428+7 location=embedded
Signature size=9055
Authority=Developer ID Application: Example Developer (ABCDE12345)
TeamIdentifier=ABCDE12345
Runtime Version=11.1.0
`),
    "identity",
  );
  assert.equal(
    classifyMacRuntimeSignature("headroom-runtime: code object is not signed at all"),
    "unsigned",
  );
  assert.equal(classifyMacRuntimeSignature("unexpected codesign output"), "unknown");
});

test("dev preflight prepares the staged runtime before checking the Vite port", () => {
  const source = readFileSync(join(ROOT, "scripts", "dev-preflight.mjs"), "utf8");
  const preparation = source.slice(
    source.indexOf("function prepareMacRuntimeForDevelopment()"),
    source.indexOf("function probePort()"),
  );
  const main = source.slice(source.indexOf("async function main()"));

  assert.ok(main.indexOf("prepareMacRuntimeForDevelopment()") < main.indexOf("probePort()"));
  assert.ok(
    preparation.indexOf("HALV_HEADROOM_BINARY") <
      preparation.indexOf("inspectMacRuntimeSignature(executable)"),
  );
  assert.match(preparation, /if \(!existsSync\(executable\)\)/);
  assert.match(preparation, /!== "hardened-adhoc"/);
  assert.match(preparation, /\["--force", "--sign", "-", executable\]/);
  assert.match(
    preparation,
    /\["--verify", "--strict", "--verbose=2", executable\]/,
  );
  assert.match(preparation, /runChecked\(executable, \["--version"\]\)/);
  assert.doesNotMatch(preparation, /--options/);
  assert.match(source, /resolve\(process\.argv\[1\]\) === SCRIPT_PATH/);
});

test("staged validation requires the complete onedir layout and exact metadata", () => {
  const root = mkdtempSync(join(tmpdir(), "halv-headroom-runtime-test-"));
  try {
    const target = "aarch64-apple-darwin";
    const lock = loadRuntimeLock();
    const internal = join(root, "_internal");
    mkdirSync(internal, { recursive: true });
    writeFileSync(join(root, runtimeExecutableName(target)), "fixture");
    writeFileSync(
      join(root, "build-manifest.json"),
      `${JSON.stringify(buildManifest(lock, target), null, 2)}\n`,
    );
    writeFileSync(
      join(internal, "headroom-runtime-build.json"),
      `${JSON.stringify(buildManifest(lock, target), null, 2)}\n`,
    );
    for (let index = 0; index < 9; index += 1) {
      writeFileSync(join(internal, `support-${index}`), "fixture");
    }

    const result = validateStagedRuntime({ resourceDir: root, target, execute: false });
    assert.equal(result.supportFileCount, 10);

    writeFileSync(join(root, "build-manifest.json"), "{}\n");
    assert.throws(
      () => validateStagedRuntime({ resourceDir: root, target, execute: false }),
      /does not match the immutable lock/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("Tauri packages the onedir tree and verifies it before release builds", () => {
  const config = JSON.parse(readFileSync(join(ROOT, "src-tauri", "tauri.conf.json"), "utf8"));
  assert.equal(
    config.bundle.resources["resources/headroom-runtime/"],
    "headroom-runtime/",
  );
  assert.deepEqual(config.bundle.externalBin, [
    "binaries/rtk",
    "binaries/crux",
    "binaries/halv-supervisor",
  ]);
  assert.match(config.build.beforeBuildCommand, /verify-headroom-runtime\.mjs/);
  assert.equal(config.bundle.macOS.hardenedRuntime, true);
  assert.ok(config.bundle.targets.includes("deb"));
  assert.ok(config.bundle.targets.includes("rpm"));
});

test("release workflow has one native builder for every supported runtime target", () => {
  const workflow = readFileSync(join(ROOT, ".github", "workflows", "build.yml"), "utf8");
  const lock = loadRuntimeLock();
  for (const target of SUPPORTED_TARGETS) {
    assert.match(workflow, new RegExp(target.replaceAll("-", "\\-")));
  }
  assert.match(workflow, /macos-15-intel/);
  assert.match(workflow, /macos-15/);
  assert.match(workflow, new RegExp(lock.commit));
  assert.match(workflow, /build-headroom-runtime\.mjs/);
  // The fork in the lock is public, so the checkout needs no token.
  assert.match(workflow, new RegExp(`repository: ${lock.repository}\\n\\s+ref: ${lock.commit}`));
  assert.doesNotMatch(workflow, /HEADROOM_REPO_TOKEN/);
  assert.match(workflow, /smoke-headroom-runtime\.py/);
  assert.match(workflow, /sign-headroom-runtime\.mjs/);
  assert.match(workflow, /verify-native-bundle\.mjs/);
  assert.match(workflow, /git config --global core\.autocrlf false/);
  assert.match(workflow, /git config --global core\.eol lf/);
  assert.ok(
    workflow.indexOf("git config --global core.autocrlf false") <
      workflow.indexOf("Checkout immutable Headroom fork"),
  );
  assert.ok(
    workflow.indexOf("git config --global core.eol lf") <
      workflow.indexOf("Checkout immutable Headroom fork"),
  );
});

test("release workflow caches only an exact verified unsigned runtime per target", () => {
  const workflow = readFileSync(join(ROOT, ".github", "workflows", "build.yml"), "utf8");
  const restoreIndex = workflow.indexOf("Restore frozen Headroom runtime");
  const checkoutIndex = workflow.indexOf("Checkout immutable Headroom fork");
  const buildIndex = workflow.indexOf("Build frozen Headroom runtime from the pinned fork");
  const smokeIndex = workflow.indexOf("Smoke-test frozen first-request Responses compression");
  const saveIndex = workflow.indexOf("Save verified unsigned Headroom runtime");
  const signIndex = workflow.indexOf("Import Apple certificate and sign nested Headroom code");
  const restoreBlock = workflow.slice(
    restoreIndex,
    workflow.indexOf("Preserve LF in immutable Headroom checkout"),
  );

  assert.match(
    workflow,
    /actions\/cache\/restore@55cc8345863c7cc4c66a329aec7e433d2d1c52a9/,
  );
  assert.match(
    workflow,
    /actions\/cache\/save@55cc8345863c7cc4c66a329aec7e433d2d1c52a9/,
  );
  assert.match(
    workflow,
    /if: steps\.headroom-runtime-cache\.outputs\.cache-hit != 'true'/,
  );
  assert.match(
    workflow,
    /key: \$\{\{ steps\.headroom-runtime-cache\.outputs\.cache-primary-key \}\}/,
  );
  assert.doesNotMatch(restoreBlock, /restore-keys:/);
  for (const cacheKeyInput of [
    "matrix.platform",
    "matrix.target",
    "scripts/headroom-runtime.lock.json",
    "scripts/build-headroom-runtime.mjs",
    "scripts/headroom-runtime-lib.mjs",
    "scripts/headroom-runtime/**/*",
  ]) {
    assert.ok(restoreBlock.includes(cacheKeyInput), `cache key is missing ${cacheKeyInput}`);
  }
  assert.ok(restoreIndex < checkoutIndex);
  assert.ok(checkoutIndex < buildIndex);
  assert.ok(buildIndex < smokeIndex);
  assert.ok(smokeIndex < saveIndex);
  assert.ok(saveIndex < signIndex);
});

test("runtime staging materializes links and validates after ephemeral cleanup", () => {
  const buildScript = readFileSync(join(ROOT, "scripts", "build-headroom-runtime.mjs"), "utf8");
  const cleanupIndex = buildScript.indexOf('process.argv.includes("--ephemeral")');
  const validationIndex = buildScript.indexOf(
    "validateStagedRuntime({ resourceDir: outputDir, target, requireToolchain: true })",
  );
  const manifestIndex = buildScript.indexOf("createRuntimeFilesManifest(outputDir");
  assert.match(buildScript, /stageRuntimeTree\(builtDir, outputDir\)/);
  assert.match(buildScript, /resolve\(argumentValue\("--output-dir"\) \?\? RESOURCE_DIR\)/);
  assert.ok(validationIndex < manifestIndex);
  assert.notEqual(cleanupIndex, -1);
  assert.notEqual(validationIndex, -1);
  assert.ok(cleanupIndex < validationIndex);
});

test("production builds install, freeze, and smoke-test Code-Aware", () => {
  const buildScript = readFileSync(join(ROOT, "scripts", "build-headroom-runtime.mjs"), "utf8");
  const spec = readFileSync(
    join(ROOT, "scripts", "headroom-runtime", "headroom-runtime.spec"),
    "utf8",
  );
  const smoke = readFileSync(join(ROOT, "scripts", "smoke-headroom-runtime.py"), "utf8");

  assert.match(buildScript, /"--extra",\s*"proxy",\s*"--extra",\s*"code"/);
  assert.match(buildScript, /from tree_sitter import Parser/);
  assert.match(buildScript, /from tree_sitter_language_pack import get_language/);
  assert.match(spec, /"tree_sitter"/);
  assert.match(spec, /"tree_sitter_language_pack"/);
  assert.match(spec, /"tree-sitter-language-pack"/);
  assert.match(smoke, /headroomExtras"\) != \["proxy", "code"\]/);
  assert.match(smoke, /HEADROOM_CODE_AWARE_ENABLED": "1"/);
  assert.match(smoke, /\/debug\/warmup/);
  assert.match(smoke, /code_aware\.get\("status"\) != "loaded"/);
});

test(
  "runtime staging preserves internal links and materializes external links",
  { skip: process.platform === "win32" },
  () => {
    const root = mkdtempSync(join(tmpdir(), "halv-headroom-copy-test-"));
    try {
      const source = join(root, "source");
      const internal = join(source, "_internal");
      const external = join(root, "external.so");
      const destination = join(root, "destination");
      mkdirSync(internal, { recursive: true });
      writeFileSync(join(internal, "libgfortran-real.so"), "native");
      writeFileSync(external, "external");
      symlinkSync("libgfortran-real.so", join(internal, "libgfortran-alias.so"));
      symlinkSync(external, join(internal, "libpython-external.dylib"));

      stageRuntimeTree(source, destination);

      const stagedInternalLink = join(destination, "_internal", "libgfortran-alias.so");
      const stagedExternalLink = join(destination, "_internal", "libpython-external.dylib");
      assert.equal(lstatSync(stagedInternalLink).isSymbolicLink(), true);
      assert.equal(lstatSync(stagedExternalLink).isSymbolicLink(), false);
      rmSync(source, { recursive: true, force: true });
      rmSync(external, { force: true });
      assert.equal(readFileSync(stagedInternalLink, "utf8"), "native");
      assert.equal(readFileSync(stagedExternalLink, "utf8"), "external");

      const danglingSource = join(root, "dangling-source");
      mkdirSync(danglingSource);
      symlinkSync("missing.so", join(danglingSource, "libgfortran-dangling.so"));
      const failedDestination = join(root, "failed-destination");
      assert.throws(() => stageRuntimeTree(danglingSource, failedDestination), /ENOENT/);
      assert.equal(existsSync(failedDestination), false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  },
);

// ---------------------------------------------------------------------------
// Rebuild and verification support.
// ---------------------------------------------------------------------------

function machOFixture({ linkeditVmsize = 0x4000n, trailer = "code" } = {}) {
  // A 64-bit little-endian Mach-O header with one LC_SEGMENT_64 (__LINKEDIT).
  const header = Buffer.alloc(32);
  header.writeUInt32LE(0xfeedfacf, 0);
  header.writeUInt32LE(0x0100000c, 4);
  header.writeUInt32LE(1, 16);
  header.writeUInt32LE(72, 20);
  const segment = Buffer.alloc(72);
  segment.writeUInt32LE(0x19, 0);
  segment.writeUInt32LE(72, 4);
  segment.write("__LINKEDIT", 8, "latin1");
  segment.writeBigUInt64LE(0x100000000n, 24);
  segment.writeBigUInt64LE(linkeditVmsize, 32);
  return Buffer.concat([header, segment, Buffer.from(trailer)]);
}

function writeRuntimeFixture(root, target = "aarch64-apple-darwin") {
  const lock = loadRuntimeLock();
  const internal = join(root, "_internal");
  mkdirSync(join(internal, "pkg"), { recursive: true });
  const metadata = {
    ...buildManifest(lock, target),
    toolchain: { uv: { version: lock.uvVersion }, ci: {} },
  };
  writeFileSync(join(root, "build-manifest.json"), stableJson(metadata));
  writeFileSync(join(internal, "headroom-runtime-build.json"), stableJson(metadata));
  writeFileSync(join(root, runtimeExecutableName(target)), machOFixture());
  chmodSync(join(root, runtimeExecutableName(target)), 0o755);
  writeFileSync(join(internal, "pkg", "data.txt"), "data\n");
  writeFileSync(join(internal, "Zeta.txt"), "upper\n");
  writeFileSync(join(internal, "alpha.txt"), "lower\n");
  return metadata;
}

// The fixture Mach-O header is not a real binary, so these manifests skip the
// codesign hashes. A separate macOS test uses a real binary.
function fixtureManifest(runtime) {
  return createRuntimeFilesManifest(runtime, { target: "aarch64-apple-darwin", normalizeMachO: false });
}

function runVerify(args) {
  return spawnSync(process.execPath, [join(ROOT, "scripts", "verify-runtime-manifest.mjs"), ...args], {
    encoding: "utf8",
  });
}

test("runtime file manifest is stable, sorted, and excludes the manifests", () => {
  const root = mkdtempSync(join(tmpdir(), "halv-runtime-manifest-"));
  try {
    const runtime = join(root, "runtime");
    writeRuntimeFixture(runtime);
    const first = stableJson(fixtureManifest(runtime));
    writeFileSync(join(runtime, RUNTIME_FILES_MANIFEST), first);
    writeFileSync(join(runtime, RUNTIME_FILES_SIGNED_MANIFEST), "{}\n");
    utimesSync(join(runtime, "_internal", "alpha.txt"), new Date(0), new Date(0));
    const second = stableJson(fixtureManifest(runtime));
    assert.equal(second, first);

    const manifest = JSON.parse(first);
    const paths = manifest.files.map((entry) => entry.path);
    assert.deepEqual(paths, [...paths].sort((left, right) => (left < right ? -1 : left > right ? 1 : 0)));
    assert.ok(!paths.includes(RUNTIME_FILES_MANIFEST));
    assert.ok(!paths.includes(RUNTIME_FILES_SIGNED_MANIFEST));
    assert.ok(paths.includes("_internal/pkg/data.txt"));
    assert.equal(manifest.fileCount, paths.length);
    assert.doesNotMatch(first, /"(mtime|timestamp|generatedAt)"/);
    for (const entry of manifest.files) {
      assert.deepEqual(Object.keys(entry), ["macho", "mode", "path", "sha256", "size"]);
    }
    const executable = manifest.files.find((entry) => entry.path === "headroom-runtime");
    assert.equal(executable.macho, true);
    if (process.platform !== "win32") {
      assert.equal(executable.mode, "100755");
    }
    assert.equal(manifest.files.find((entry) => entry.path === "_internal/alpha.txt").macho, false);
    assert.match(manifest.buildMetadataSha256, /^[0-9a-f]{64}$/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("release copies of the file manifests use the asset names and stay out of the runtime", () => {
  assert.equal(
    releaseManifestName("x86_64-pc-windows-msvc"),
    "runtime-files-x86_64-pc-windows-msvc.sha256.json",
  );
  assert.equal(
    releaseManifestName("aarch64-apple-darwin", { signed: true }),
    "runtime-files-aarch64-apple-darwin.signed.sha256.json",
  );
  assert.throws(() => releaseManifestName("riscv64gc-unknown-linux-gnu"), /unsupported Headroom target/);
  assert.ok(RUNTIME_MANIFEST_EXPORT_DIR.endsWith(join("src-tauri", "target", "headroom-runtime-manifests")));

  const root = mkdtempSync(join(tmpdir(), "halv-runtime-export-"));
  try {
    const runtime = join(root, "runtime");
    writeRuntimeFixture(runtime);
    const text = stableJson(fixtureManifest(runtime));
    const name = releaseManifestName("aarch64-apple-darwin");
    const exported = exportRuntimeManifest({
      manifestText: text,
      runtimeDir: runtime,
      exportDir: join(root, "manifests"),
      name,
    });
    assert.equal(exported, join(root, "manifests", name));
    assert.equal(readFileSync(exported, "utf8"), text);
    for (const inside of [runtime, join(runtime, "_internal", "manifests")]) {
      assert.throws(
        () => exportRuntimeManifest({ manifestText: text, runtimeDir: runtime, exportDir: inside, name }),
        /must be outside the runtime folder/,
      );
    }
    assert.equal(
      stableJson(fixtureManifest(runtime)),
      text,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }

  const buildScript = readFileSync(join(ROOT, "scripts", "build-headroom-runtime.mjs"), "utf8");
  assert.match(buildScript, /argumentValue\("--manifest-dir"\) \?\? RUNTIME_MANIFEST_EXPORT_DIR/);
  assert.match(buildScript, /name: releaseManifestName\(target\)/);
  const signScript = readFileSync(join(ROOT, "scripts", "sign-headroom-runtime.mjs"), "utf8");
  assert.match(signScript, /name: releaseManifestName\(target\)/);
  assert.match(signScript, /name: releaseManifestName\(target, \{ signed: true \}\)/);
  assert.match(signScript, /entry\.cdhash = codeDirectoryHash/);
});

test("Mach-O detection accepts thin and fat headers but not Java classes", () => {
  assert.equal(isMachOHeader(machOFixture()), true);
  const fat = Buffer.alloc(8);
  fat.writeUInt32BE(0xcafebabe, 0);
  fat.writeUInt32BE(2, 4);
  assert.equal(isMachOHeader(fat), true);
  const javaClass = Buffer.from([0xca, 0xfe, 0xba, 0xbe, 0x00, 0x00, 0x00, 0x34]);
  assert.equal(isMachOHeader(javaClass), false);
  assert.equal(isMachOHeader(Buffer.from("#!/bin/sh\n")), false);
});

test("__LINKEDIT normalization changes only the segment vmsize", () => {
  const small = machOFixture({ linkeditVmsize: 0x4000n });
  const large = machOFixture({ linkeditVmsize: 0x8000n });
  assert.notDeepEqual(small, large);
  assert.deepEqual(zeroLinkeditVmsize(small), zeroLinkeditVmsize(large));
  assert.notDeepEqual(
    zeroLinkeditVmsize(small),
    zeroLinkeditVmsize(machOFixture({ linkeditVmsize: 0x4000n, trailer: "edit" })),
  );
});

test("toolchain block records the build tools, determinism inputs, and CI facts", () => {
  const lock = loadRuntimeLock();
  const calls = [];
  const run = (command, args) => {
    calls.push([command, ...args].join(" "));
    if (command === "rustc") {
      return "rustc 1.95.0 (abc 2026-04-14)\nhost: aarch64-apple-darwin\nrelease: 1.95.0";
    }
    if (command === "cargo") {
      return "cargo 1.95.0 (abc 2026-04-14)";
    }
    if (command === "xcrun" && args[0] === "--show-sdk-version") {
      return "15.5";
    }
    return "fixture";
  };
  const probe = {
    python: { version: "3.12.7 (main, Oct 16 2024)", realBasePrefix: "/uv/python/cpython-3.12.7" },
    packages: { pyinstaller: lock.pyinstallerVersion, "pyinstaller-hooks-contrib": "2026.9" },
    maturin: { version: "1.15.0", wheelGenerator: "maturin (1.15.0)" },
  };
  const determinism = { SOURCE_DATE_EPOCH: "1", PYTHONHASHSEED: "0", RUSTFLAGS: "" };
  const env = {
    ImageOS: "macos15",
    ImageVersion: "20260907.0337.1",
    GITHUB_SHA: "a".repeat(40),
    GITHUB_REPOSITORY: "halv-ai/halv",
    GITHUB_SERVER_URL: "https://github.com",
    GITHUB_RUN_ID: "42",
    GITHUB_RUN_ATTEMPT: "2",
  };
  const toolchain = collectToolchain({
    uvVersionOutput: `uv ${lock.uvVersion} (ebf0f43d7 2026-07-07 aarch64-apple-darwin)`,
    pythonProbe: probe,
    determinism,
    sourceDir: "/source",
    env,
    platform: "darwin",
    arch: "arm64",
    nodeVersion: "v22.0.0",
    run,
  });
  for (const key of REQUIRED_TOOLCHAIN_KEYS) {
    assert.ok(key in toolchain, `toolchain block is missing ${key}`);
  }
  assert.equal(toolchain.uv.version, lock.uvVersion);
  assert.equal(toolchain.rustc.version, "1.95.0");
  assert.equal(toolchain.cargo, "cargo 1.95.0 (abc 2026-04-14)");
  assert.equal(toolchain.maturin.version, "1.15.0");
  assert.equal(toolchain.xcode.sdkVersion, "15.5");
  assert.deepEqual(toolchain.determinism, determinism);
  assert.equal(toolchain.ci.runUrl, "https://github.com/halv-ai/halv/actions/runs/42");
  assert.equal(toolchain.ci.ImageOS, "macos15");
  assert.ok(calls.includes("rustc -vV"));

  const local = collectToolchain({
    uvVersionOutput: `uv ${lock.uvVersion}`,
    pythonProbe: probe,
    determinism,
    env: {},
    platform: "linux",
    arch: "x64",
    run: () => {
      throw new Error("not installed");
    },
  });
  assert.deepEqual(local.ci, {});
  assert.equal(local.rustc, null);
  assert.equal("xcode" in local, false);
});

test("the build requires the uv version from the lock", () => {
  const lock = loadRuntimeLock();
  assert.equal(
    assertUvVersion(`uv ${lock.uvVersion} (ebf0f43d7 2026-07-07 aarch64-apple-darwin)`, lock),
    lock.uvVersion,
  );
  assert.throws(() => assertUvVersion("uv 0.13.5 (46776a95d 2026-10-21)", lock), /requires uv/);
  assert.throws(() => assertUvVersion("not uv", lock), /could not read the uv version/);
});

test("the build Python must live under the uv-managed Python directory", () => {
  const root = mkdtempSync(join(tmpdir(), "halv-managed-python-"));
  try {
    const managed = join(root, "uv", "python");
    const interpreter = join(managed, "cpython-3.12.7", "bin", "python3.12");
    mkdirSync(join(managed, "cpython-3.12.7", "bin"), { recursive: true });
    writeFileSync(interpreter, "");
    assert.ok(assertManagedInterpreter(interpreter, managed).endsWith("python3.12"));
    const outside = join(root, "anaconda3", "bin", "python3.12");
    mkdirSync(join(root, "anaconda3", "bin"), { recursive: true });
    writeFileSync(outside, "");
    assert.throws(() => assertManagedInterpreter(outside, managed), /not a uv-managed interpreter/);
    assert.throws(() => assertManagedInterpreter(join(root, "uv", "python-other"), managed));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("determinism environment pins the hash seed, commit time, and Rust path remaps", () => {
  const environment = determinismEnvironment({
    sourceCommitTime: 1787095425,
    sourceDir: ["/work/source", "/private/work/source"],
    cargoHome: "/home/runner/.cargo",
    homeDir: "/home/runner",
    cargoTargetDir: "/work/target",
    rustSource: {
      path: "/home/runner/.rustup/toolchains/1.95.0/lib/rustlib/src/rust",
      commitHash: "59807616e1fa2540724bfbac14d7976d7e4a3860",
    },
  });
  assert.equal(environment.SOURCE_DATE_EPOCH, "1787095425");
  assert.equal(environment.PYTHONHASHSEED, "0");
  const remaps = environment.RUSTFLAGS.split(" ");
  assert.deepEqual(remaps, [
    "--remap-path-prefix=/home/runner=/home",
    "--remap-path-prefix=/home/runner/.cargo=/cargo",
    "--remap-path-prefix=/work/source=/headroom-source",
    "--remap-path-prefix=/private/work/source=/headroom-source",
    "--remap-path-prefix=/work/target=/cargo-target",
    "--remap-path-prefix=/home/runner/.rustup/toolchains/1.95.0/lib/rustlib/src/rust=" +
      "/rustc/59807616e1fa2540724bfbac14d7976d7e4a3860",
  ]);
  // On macOS the linker writes no debug map, so the Cargo target path cannot
  // change LC_UUID.
  const apple = determinismEnvironment({
    sourceCommitTime: 1787095425,
    sourceDir: "/work/source",
    cargoHome: "/Users/runner/.cargo",
    homeDir: "/Users/runner",
    cargoTargetDir: "/work/target",
    appleLinker: true,
  });
  assert.deepEqual(apple.RUSTFLAGS.split(" ").slice(-2), ["-C", "link-arg=-Wl,-S"]);
  assert.doesNotMatch(environment.RUSTFLAGS, /link-arg/);
  const buildScript = readFileSync(join(ROOT, "scripts", "build-headroom-runtime.mjs"), "utf8");
  assert.match(buildScript, /appleLinker: target\.endsWith\("apple-darwin"\)/);
  assert.throws(
    () => determinismEnvironment({ sourceCommitTime: 1, sourceDir: "/a b", homeDir: "/h" }),
    /whitespace/,
  );
  assert.throws(() => determinismEnvironment({ sourceCommitTime: 0, sourceDir: "/a" }), /commit time/);
});

test("the dirty check fails on untracked files and ignores ignored files", () => {
  const root = mkdtempSync(join(tmpdir(), "halv-headroom-source-"));
  const git = (...args) => {
    const result = spawnSync("git", ["-C", root, ...args], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim();
  };
  try {
    git("init", "-q");
    git("config", "user.email", "test@example.invalid");
    git("config", "user.name", "Test");
    writeFileSync(join(root, "pyproject.toml"), '[project]\nversion = "0.34.0"\n');
    writeFileSync(join(root, "uv.lock"), "lock\n");
    writeFileSync(join(root, ".gitignore"), "*.so\n");
    git("add", ".");
    git("commit", "-q", "-m", "fixture");
    assert.equal(inspectHeadroomSource(root).dirty, false);

    writeFileSync(join(root, "_core.abi3.so"), "ignored build output");
    assert.equal(inspectHeadroomSource(root).dirty, false);

    mkdirSync(join(root, "headroom"), { recursive: true });
    writeFileSync(join(root, "headroom", "stray.py"), "print('ships by accident')\n");
    const dirty = inspectHeadroomSource(root);
    assert.equal(dirty.dirty, true);
    assert.deepEqual(dirty.changes, ["?? headroom/stray.py"]);
    const commit = git("rev-parse", "HEAD");
    const lock = {
      ...loadRuntimeLock(),
      commit,
      packageVersion: "0.34.0",
      uvLockSha256: dirty.uvLockSha256,
    };
    assert.throws(() => assertHeadroomSource(root, lock), /untracked files: \?\? headroom\/stray\.py/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("verify-runtime-manifest passes on an exact match and fails on a mismatch", () => {
  const root = mkdtempSync(join(tmpdir(), "halv-runtime-verify-"));
  try {
    const runtime = join(root, "runtime");
    writeRuntimeFixture(runtime);
    const manifestPath = join(root, "runtime-files-aarch64-apple-darwin.sha256.json");
    writeFileSync(
      manifestPath,
      stableJson(fixtureManifest(runtime)),
    );

    const pass = runVerify([runtime, "--manifest", manifestPath]);
    assert.equal(pass.status, 0, pass.stdout + pass.stderr);
    assert.match(pass.stdout, /exact match\s+\d+/);
    assert.match(pass.stdout, /RESULT: PASS/);

    writeFileSync(join(runtime, "_internal", "pkg", "data.txt"), "changed\n");
    writeFileSync(join(runtime, "_internal", "extra.txt"), "extra\n");
    rmSync(join(runtime, "_internal", "alpha.txt"));
    const fail = runVerify([runtime, "--manifest", manifestPath]);
    assert.equal(fail.status, 1, fail.stdout + fail.stderr);
    assert.match(fail.stdout, /content differs\s+1\s+FAIL/);
    assert.match(fail.stdout, /missing from runtime\s+1\s+FAIL/);
    assert.match(fail.stdout, /not in expected list\s+1\s+FAIL/);
    assert.match(fail.stdout, /_internal\/pkg\/data\.txt/);
    assert.match(fail.stdout, /RESULT: FAIL \(3 of/);

    const usage = runVerify([runtime]);
    assert.equal(usage.status, 2);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test(
  "verify-runtime-manifest --macos-normalize compares a re-signed Mach-O with a manifest",
  { skip: process.platform === "darwin" ? false : "needs macOS codesign" },
  () => {
    const root = mkdtempSync(join(tmpdir(), "halv-runtime-normalize-"));
    const codesign = (...args) => {
      const result = spawnSync("/usr/bin/codesign", args, { encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
    };
    try {
      const runtime = join(root, "runtime");
      mkdirSync(join(runtime, "_internal"), { recursive: true });
      const tool = join(runtime, "_internal", "tool");
      writeFileSync(tool, readFileSync("/usr/bin/true"));
      chmodSync(tool, 0o755);
      codesign("--remove-signature", tool);
      codesign("--force", "--sign", "-", tool);
      writeFileSync(join(runtime, "_internal", "data.txt"), "data\n");
      const manifest = createRuntimeFilesManifest(runtime, { target: "aarch64-apple-darwin" });
      assert.equal(manifest.machoNormalization, "codesign-remove-signature-v1");
      const toolEntry = manifest.files.find((entry) => entry.path === "_internal/tool");
      assert.equal(toolEntry.macho, true);
      assert.match(toolEntry.unsignedSha256, /^[0-9a-f]{64}$/);
      assert.match(toolEntry.unsignedLinkeditSha256, /^[0-9a-f]{64}$/);
      assert.equal("unsignedSha256" in manifest.files.find((entry) => entry.path === "_internal/data.txt"), false);
      assert.equal(
        stableJson(createRuntimeFilesManifest(runtime, { target: "aarch64-apple-darwin" })),
        stableJson(manifest),
      );
      const manifestPath = join(root, "runtime-files-aarch64-apple-darwin.sha256.json");
      writeFileSync(manifestPath, stableJson(manifest));

      // Another signature (hardened runtime flag) gives other bytes.
      codesign("--force", "--sign", "-", "--options", "runtime", tool);
      const exact = runVerify([runtime, "--manifest", manifestPath]);
      assert.equal(exact.status, 1, exact.stdout + exact.stderr);
      assert.match(exact.stdout, /content differs\s+1\s+FAIL/);
      const normalized = runVerify([runtime, "--manifest", manifestPath, "--macos-normalize"]);
      assert.equal(normalized.status, 0, normalized.stdout + normalized.stderr);
      assert.match(normalized.stdout, /Mach-O (signature only|__LINKEDIT vmsize only) \(macOS normalize\)\s+1\s+ok/);
      assert.match(normalized.stdout, /RESULT: PASS/);

      // Other code fails also after normalization.
      writeFileSync(tool, readFileSync("/usr/bin/false"));
      codesign("--force", "--sign", "-", tool);
      const changed = runVerify([runtime, "--manifest", manifestPath, "--macos-normalize"]);
      assert.equal(changed.status, 1, changed.stdout + changed.stderr);
      assert.match(changed.stdout, /content differs\s+1\s+FAIL/);

      // A manifest without the unsigned hashes cannot drive --macos-normalize.
      const plain = createRuntimeFilesManifest(runtime, {
        target: "aarch64-apple-darwin",
        normalizeMachO: false,
      });
      writeFileSync(manifestPath, stableJson(plain));
      const refused = runVerify([runtime, "--manifest", manifestPath, "--macos-normalize"]);
      assert.equal(refused.status, 2);
      assert.match(refused.stderr, /unsigned Mach-O hashes/);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  },
);

test("verification reports toolchain-only and mode differences apart", () => {
  const root = mkdtempSync(join(tmpdir(), "halv-runtime-compare-"));
  try {
    const left = join(root, "left");
    const right = join(root, "right");
    const metadata = writeRuntimeFixture(left);
    writeRuntimeFixture(right);
    const rebuilt = { ...metadata, toolchain: { uv: { version: "other-machine" }, ci: {} } };
    writeFileSync(join(right, "build-manifest.json"), stableJson(rebuilt));
    writeFileSync(join(right, "_internal", "headroom-runtime-build.json"), stableJson(rebuilt));

    const sameLock = compareRuntimeFiles({
      runtimeDir: right,
      expected: listRuntimeFiles(left),
      expectedDir: left,
    });
    assert.equal(sameLock.passed, true);
    assert.equal(sameLock.results.toolchainOnly.length, 2);

    const fromManifest = fixtureManifest(left);
    const viaDigest = compareRuntimeFiles({
      runtimeDir: right,
      expected: fromManifest.files,
      expectedBuildMetadataSha256: fromManifest.buildMetadataSha256,
    });
    assert.equal(viaDigest.passed, true);
    assert.equal(viaDigest.results.toolchainOnly.length, 2);

    writeFileSync(
      join(right, "build-manifest.json"),
      stableJson({ ...rebuilt, commit: "0".repeat(40) }),
    );
    const otherCommit = compareRuntimeFiles({
      runtimeDir: right,
      expected: listRuntimeFiles(left),
      expectedDir: left,
    });
    assert.equal(otherCommit.passed, false);
    assert.equal(otherCommit.results.contentDiffers[0].path, "build-manifest.json");

    if (process.platform !== "win32") {
      chmodSync(join(left, "_internal", "alpha.txt"), 0o600);
      const modes = compareRuntimeFiles({
        runtimeDir: left,
        expected: listRuntimeFiles(right),
        expectedDir: right,
      });
      assert.equal(modes.results.modeDiffers[0].path, "_internal/alpha.txt");
    }
    if (process.platform === "darwin") {
      assert.throws(
        () =>
          compareRuntimeFiles({
            runtimeDir: left,
            expected: listRuntimeFiles(right),
            macosNormalize: true,
          }),
        /reference runtime directory or a manifest with unsigned Mach-O hashes/,
      );
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("staged validation accepts a toolchain block and can require it", () => {
  const root = mkdtempSync(join(tmpdir(), "halv-headroom-toolchain-"));
  try {
    const target = "aarch64-apple-darwin";
    writeRuntimeFixture(root, target);
    for (let index = 0; index < 9; index += 1) {
      writeFileSync(join(root, "_internal", `support-${index}`), "fixture");
    }
    assert.throws(
      () => validateStagedRuntime({ resourceDir: root, target, execute: false, requireToolchain: true }),
      /toolchain block is missing/,
    );
    const lock = loadRuntimeLock();
    const toolchain = Object.fromEntries(REQUIRED_TOOLCHAIN_KEYS.map((key) => [key, null]));
    const metadata = { ...buildManifest(lock, target), toolchain };
    writeFileSync(join(root, "build-manifest.json"), stableJson(metadata));
    writeFileSync(join(root, "_internal", "headroom-runtime-build.json"), stableJson(metadata));
    validateStagedRuntime({ resourceDir: root, target, execute: false, requireToolchain: true });

    writeFileSync(
      join(root, "_internal", "headroom-runtime-build.json"),
      stableJson({ ...metadata, toolchain: { ...toolchain, node: "other" } }),
    );
    assert.throws(
      () => validateStagedRuntime({ resourceDir: root, target, execute: false }),
      /embedded Headroom build metadata does not match/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the build pins uv, managed Python, maturin, and PyInstaller tools with hashes", () => {
  const buildScript = readFileSync(join(ROOT, "scripts", "build-headroom-runtime.mjs"), "utf8");
  assert.match(buildScript, /assertUvVersion\(uvVersionOutput, lock\)/);
  assert.match(buildScript, /UV_PYTHON_PREFERENCE: "only-managed"/);
  assert.match(buildScript, /"python", "install", "--no-bin", "--no-registry", lock\.pythonVersion/);
  assert.match(buildScript, /assertManagedInterpreter\(interpreter, managedPythonDir\)/);
  assert.match(buildScript, /assertManagedInterpreter\(probe\.python\.realBasePrefix, managedPythonDir\)/);
  assert.match(buildScript, /"--no-build-isolation-package",\s*"headroom-ai"/);
  assert.match(buildScript, /"--require-hashes", "--no-deps", "--only-binary", ":all:", "-r", BUILD_CONSTRAINTS_PATH/);
  assert.match(buildScript, /"--require-hashes", "--no-deps", "--only-binary", ":all:", "-r", BUILD_REQUIREMENTS_PATH/);
  assert.doesNotMatch(buildScript, /`pyinstaller==\$\{/);
  assert.match(buildScript, /delete buildEnv\.CARGO_ENCODED_RUSTFLAGS/);
  // The PyInstaller binary cache stays in the work folder.
  assert.match(buildScript, /PYINSTALLER_CONFIG_DIR: pyinstallerConfigDir/);
  assert.match(buildScript, /join\(workRoot, "pyinstaller-config"\)/);

  const lock = loadRuntimeLock();
  assert.equal(pinnedRequirementVersion(BUILD_REQUIREMENTS_PATH, "pyinstaller"), lock.pyinstallerVersion);
  assert.equal(pinnedRequirementVersion(BUILD_CONSTRAINTS_PATH, "maturin"), "1.15.0");
  for (const path of [BUILD_REQUIREMENTS_PATH, BUILD_CONSTRAINTS_PATH]) {
    const requirements = readFileSync(path, "utf8")
      .split(/\n(?=[a-z])/i)
      .filter((block) => !block.startsWith("#"));
    assert.ok(requirements.length > 0);
    for (const block of requirements) {
      assert.match(block, /^[a-z0-9-]+==[^\s;]+/i, block);
      assert.match(block, /--hash=sha256:[0-9a-f]{64}/, block);
    }
  }
  for (const name of [
    "pyinstaller-hooks-contrib",
    "altgraph",
    "macholib",
    "pefile",
    "pywin32-ctypes",
    "setuptools",
  ]) {
    assert.ok(pinnedRequirementVersion(BUILD_REQUIREMENTS_PATH, name));
  }
});

test("the frozen entry point keeps offline data, telemetry defaults, and HF pins", () => {
  const entry = readFileSync(join(ROOT, "scripts", "headroom-runtime", "entry.py"), "utf8");
  const spec = readFileSync(join(ROOT, "scripts", "headroom-runtime", "headroom-runtime.spec"), "utf8");
  // The offline data and the HF pins replace parent values.
  assert.match(entry, /os\.environ\["TIKTOKEN_CACHE_DIR"\] = str\(_TIKTOKEN_CACHE\)/);
  assert.match(entry, /os\.environ\["LITELLM_LOCAL_MODEL_COST_MAP"\] = "True"/);
  assert.match(entry, /os\.environ\["HEADROOM_HF_PIN"\] = "on"/);
  assert.doesNotMatch(entry, /setdefault\("(TIKTOKEN_CACHE_DIR|LITELLM_LOCAL_MODEL_COST_MAP|HEADROOM_HF_PIN)"/);
  // The telemetry defaults stay.
  assert.match(entry, /os\.environ\.setdefault\("HEADROOM_BEACON", "off"\)/);
  assert.match(entry, /os\.environ\.setdefault\("DO_NOT_TRACK", "1"\)/);
  assert.match(entry, /os\.environ\.setdefault\("HEADROOM_UPDATE_CHECK", "off"\)/);
  // The malloc marker stops the macOS re-exec of upstream Headroom, which
  // exits with code 2 in a frozen runtime. The malloc variables are macOS only.
  assert.match(entry, /^os\.environ\.setdefault\("_HEADROOM_MALLOC_TUNED", "1"\)$/m);
  assert.match(
    entry,
    /^if sys\.platform == "darwin":\n {4}os\.environ\.setdefault\("MallocAggressiveMadvise", "1"\)\n {4}os\.environ\.setdefault\("MallocLargeCache", "0"\)$/m,
  );
  assert.ok(
    entry.indexOf('setdefault("_HEADROOM_MALLOC_TUNED"') < entry.indexOf("def _run_proxy("),
    "the malloc marker must be set before the proxy imports Headroom",
  );
  assert.match(entry, /--offline-self-check/);
  assert.match(spec, /\(str\(LICENSES_DIR\), "licenses\/python"\)/);
  assert.match(spec, /\(str\(TIKTOKEN_DIR\), "tiktoken-cache"\)/);
  assert.match(spec, /normalize_metadata_toc\(/);
});

function findPython() {
  for (const candidate of ["python3", "python"]) {
    const result = spawnSync(candidate, ["-c", "import sys; print(sys.version_info >= (3, 9))"], {
      encoding: "utf8",
    });
    if (result.status === 0 && result.stdout.trim() === "True") {
      return candidate;
    }
  }
  return null;
}

const PYTHON = findPython();

test(
  "metadata normalization removes build paths and install times from dist-info",
  { skip: PYTHON ? false : "no Python 3.9+ on PATH" },
  () => {
    const root = mkdtempSync(join(tmpdir(), "halv-metadata-normalize-"));
    try {
      const source = join(root, "checkout");
      const distInfo = join(root, "site", "headroom_ai-0.34.0.dist-info");
      mkdirSync(join(distInfo, "sboms"), { recursive: true });
      const directUrl = `{"url":"file://${source}","dir_info":{"editable":true}}`;
      const sbom = JSON.stringify({
        serialNumber: "urn:uuid:random",
        metadata: { timestamp: "2026-10-21T22:25:31Z", component: { "bom-ref": `path+file://${source}/crates` } },
      });
      writeFileSync(join(distInfo, "direct_url.json"), directUrl);
      writeFileSync(join(distInfo, "uv_cache.json"), '{"timestamp":1}');
      writeFileSync(join(distInfo, "sboms", "headroom-py.cyclonedx.json"), sbom);
      writeFileSync(join(distInfo, "METADATA"), "Name: headroom-ai\n");
      writeFileSync(
        join(distInfo, "RECORD"),
        [
          "../../../bin/headroom,sha256=abc,384",
          "headroom_ai.pth,sha256=pth,45",
          "headroom_ai-0.34.0.dist-info/METADATA,sha256=keep,18",
          "headroom_ai-0.34.0.dist-info/direct_url.json,sha256=old,91",
          "headroom_ai-0.34.0.dist-info/uv_cache.json,sha256=old,137",
          "headroom_ai-0.34.0.dist-info/sboms/headroom-py.cyclonedx.json,sha256=old,10",
          "headroom_ai-0.34.0.dist-info/RECORD,,",
          "",
        ].join("\n"),
      );
      // A wheel from PyPI: no direct_url.json, so its SBOM keeps its bytes.
      const wheelInfo = join(root, "site", "cryptography-50.0.0.dist-info");
      mkdirSync(join(wheelInfo, "sboms"), { recursive: true });
      const wheelSbom = '{"serialNumber": "urn:uuid:from-pypi", "metadata": {"timestamp": "x"}}';
      writeFileSync(join(wheelInfo, "sboms", "sbom.json"), wheelSbom);
      writeFileSync(join(wheelInfo, "RECORD"), "cryptography/__init__.py,sha256=abc,1\n");
      chmodSync(join(distInfo, "RECORD"), 0o755);
      const name = "headroom_ai-0.34.0.dist-info";
      const toc = ["direct_url.json", "uv_cache.json", "sboms/headroom-py.cyclonedx.json", "METADATA", "RECORD"]
        .map((file) => [`${name}/${file}`, join(distInfo, ...file.split("/")), "DATA"])
        .concat(
          ["sboms/sbom.json", "RECORD"].map((file) => [
            `cryptography-50.0.0.dist-info/${file}`,
            join(wheelInfo, ...file.split("/")),
            "DATA",
          ]),
        );
      const script = [
        "import json, sys",
        "from pathlib import Path",
        `sys.path.insert(0, ${JSON.stringify(join(ROOT, "scripts", "headroom-runtime"))})`,
        "from halv_runtime_build import normalize_metadata_toc",
        "toc = [tuple(e) for e in json.loads(sys.argv[1])]",
        "out = normalize_metadata_toc(toc, Path(sys.argv[2]), Path(sys.argv[3]), 'pedr0v/headroom', 'c' * 40, 1787095425)",
        "print(json.dumps({d: [Path(s).read_text(), s, oct(Path(s).stat().st_mode & 0o777)] for d, s, _ in out}))",
      ].join("\n");
      const run = () => {
        const result = spawnSync(
          PYTHON,
          ["-I", "-c", script, JSON.stringify(toc), join(root, "normalized"), source],
          { encoding: "utf8" },
        );
        assert.equal(result.status, 0, result.stderr);
        const parsed = JSON.parse(result.stdout);
        const sources = Object.fromEntries(Object.entries(parsed).map(([key, value]) => [key, value[1]]));
        const modes = Object.fromEntries(Object.entries(parsed).map(([key, value]) => [key, value[2]]));
        return [Object.fromEntries(Object.entries(parsed).map(([key, value]) => [key, value[0]])), sources, modes];
      };
      const [first, sources, modes] = run();
      assert.deepEqual(run()[0], first);
      assert.equal(sources["cryptography-50.0.0.dist-info/sboms/sbom.json"], join(wheelInfo, "sboms", "sbom.json"));
      assert.equal(sources["cryptography-50.0.0.dist-info/RECORD"], join(wheelInfo, "RECORD"));
      assert.equal(first["cryptography-50.0.0.dist-info/sboms/sbom.json"], wheelSbom);
      assert.notEqual(sources[`${name}/RECORD`], join(distInfo, "RECORD"));
      if (process.platform !== "win32") {
        assert.equal(modes[`${name}/RECORD`], "0o755");
      }
      assert.equal(`${name}/uv_cache.json` in first, false);
      assert.equal(first[`${name}/METADATA`], "Name: headroom-ai\n");
      assert.match(first[`${name}/direct_url.json`], /file:\/\/\/headroom-source/);
      assert.ok(!first[`${name}/direct_url.json`].includes(source));
      const normalizedSbom = JSON.parse(first[`${name}/sboms/headroom-py.cyclonedx.json`]);
      assert.equal(normalizedSbom.metadata.timestamp, "2026-08-18T23:23:45Z");
      assert.match(normalizedSbom.serialNumber, /^urn:uuid:[0-9a-f-]{36}$/);
      assert.ok(!JSON.stringify(normalizedSbom).includes(source));
      const record = first[`${name}/RECORD`];
      assert.doesNotMatch(record, /bin\/headroom/);
      assert.doesNotMatch(record, /headroom_ai\.pth/);
      assert.doesNotMatch(record, /uv_cache\.json/);
      assert.match(record, /METADATA,sha256=keep,18/);
      assert.match(record, /direct_url\.json,sha256=[A-Za-z0-9_-]{43},\d+/);
      assert.doesNotMatch(record, /sha256=old/);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  },
);

test(
  "sysconfig normalization replaces the uv Python folder with the standalone prefix",
  { skip: PYTHON ? false : "no Python 3.9+ on PATH" },
  () => {
    const root = mkdtempSync(join(tmpdir(), "halv-sysconfig-normalize-"));
    try {
      const prefix = join(root, "uv-python", "cpython-3.12.7-macos-aarch64-none");
      const minor = join(root, "uv-python", "cpython-3.12-macos-aarch64-none");
      mkdirSync(join(prefix, "lib"), { recursive: true });
      const module = join(prefix, "lib", "_sysconfigdata__darwin_darwin.py");
      writeFileSync(
        module,
        `build_time_vars = {'BINDIR': '${prefix}/bin', 'LIBDIR': '${minor}/lib', 'CC': 'clang'}\n`,
      );
      const other = join(prefix, "lib", "json.py");
      writeFileSync(other, `PATH = '${prefix}'\n`);
      const script = [
        "import json, sys",
        "from pathlib import Path",
        `sys.path.insert(0, ${JSON.stringify(join(ROOT, "scripts", "headroom-runtime"))})`,
        "from halv_runtime_build import normalize_sysconfig_toc",
        "toc = [('_sysconfigdata__darwin_darwin', sys.argv[1], 'PYMODULE'), ('json', sys.argv[2], 'PYMODULE')]",
        "out = normalize_sysconfig_toc(toc, Path(sys.argv[3]), [sys.argv[4], sys.argv[5]])",
        "print(json.dumps([[n, s, Path(s).read_text()] for n, s, _ in out]))",
      ].join("\n");
      const result = spawnSync(
        PYTHON,
        ["-I", "-c", script, module, other, join(root, "out"), prefix, minor],
        { encoding: "utf8" },
      );
      assert.equal(result.status, 0, result.stderr);
      const [sysconfig, untouched] = JSON.parse(result.stdout);
      assert.notEqual(sysconfig[1], module);
      assert.equal(
        sysconfig[2],
        "build_time_vars = {'BINDIR': '/install/bin', 'LIBDIR': '/install/lib', 'CC': 'clang'}\n",
      );
      assert.equal(untouched[1], other);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  },
);

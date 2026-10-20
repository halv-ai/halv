import assert from "node:assert/strict";
import {
  existsSync,
  lstatSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  SUPPORTED_TARGETS,
  buildManifest,
  loadRuntimeLock,
  runtimeExecutableName,
  stageRuntimeTree,
  validateStagedRuntime,
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
  assert.match(workflow, /HEADROOM_REPO_TOKEN/);
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
    workflow.indexOf("Require private Headroom checkout token"),
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
    "validateStagedRuntime({ resourceDir: RESOURCE_DIR, target })",
  );
  assert.match(buildScript, /stageRuntimeTree\(builtDir, RESOURCE_DIR\)/);
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

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  buildManifest,
  loadRuntimeLock,
} from "./headroom-runtime-lib.mjs";
import {
  isMainModule,
  runChecked,
  verifyBundleLayout,
  verifyThirdPartyLicenses,
} from "./verify-native-bundle.mjs";

const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

// The license files that tauri.conf.json installs, with their sources.
function licenseSources() {
  const licenses = join(APP_ROOT, "src-tauri", "resources", "licenses");
  return {
    LICENSE: join(APP_ROOT, "LICENSE"),
    NOTICE: join(APP_ROOT, "NOTICE"),
    "THIRD_PARTY_NOTICES.md": join(APP_ROOT, "THIRD_PARTY_NOTICES.md"),
    "rtk-0.42.4-LICENSE": join(licenses, "rtk-0.42.4-LICENSE"),
    "crux-LICENSE": join(licenses, "crux-LICENSE"),
    "orca-LICENSE": join(licenses, "orca-LICENSE"),
    "ponytail-LICENSE": join(licenses, "ponytail-LICENSE"),
    "lucide-LICENSE": join(licenses, "lucide-LICENSE"),
    "THIRD_PARTY_LICENSES.md": join(licenses, "THIRD_PARTY_LICENSES.md"),
  };
}

function createLayout(target = "x86_64-unknown-linux-gnu") {
  const cleanupRoot = mkdtempSync(join(tmpdir(), "halv-native-bundle-test-"));
  const root = target.includes("apple-darwin")
    ? join(cleanupRoot, "Halv.app")
    : cleanupRoot;
  const bin = target.includes("apple-darwin")
    ? join(root, "Contents", "MacOS")
    : join(root, "usr", "bin");
  const resources = target.includes("apple-darwin")
    ? join(root, "Contents", "Resources")
    : join(root, "usr", "lib", "halv");
  const runtime = join(resources, "headroom-runtime");
  const internal = join(runtime, "_internal");
  mkdirSync(bin, { recursive: true });
  mkdirSync(internal, { recursive: true });
  mkdirSync(join(resources, "licenses"), { recursive: true });
  const sidecars = ["halv", "rtk", "halv-supervisor"];
  if (target.includes("apple-darwin")) {
    sidecars.push("crux");
  }
  for (const name of sidecars) {
    const path = join(bin, name);
    writeFileSync(path, "fixture");
    chmodSync(path, 0o755);
  }
  for (const [name, source] of Object.entries(licenseSources())) {
    copyFileSync(source, join(resources, "licenses", name));
  }
  const manifest = buildManifest(loadRuntimeLock(), target);
  writeFileSync(join(runtime, "build-manifest.json"), `${JSON.stringify(manifest)}\n`);
  writeFileSync(join(runtime, "headroom-runtime"), "fixture");
  writeFileSync(
    join(internal, "headroom-runtime-build.json"),
    `${JSON.stringify(manifest)}\n`,
  );
  for (let index = 0; index < 9; index += 1) {
    writeFileSync(join(internal, `support-${index}`), "fixture");
  }
  return { bin, cleanupRoot, resources, root };
}

test("verifies the installed resolver-relative runtime layout", () => {
  const layout = createLayout();
  try {
    const result = verifyBundleLayout({
      root: layout.root,
      target: "x86_64-unknown-linux-gnu",
      execute: false,
    });
    assert.equal(result.rtk, join(layout.bin, "rtk"));
    assert.equal(result.supervisor, join(layout.bin, "halv-supervisor"));
    assert.equal(result.resourceDir, layout.resources);

    rmSync(join(layout.resources, "licenses"), { recursive: true, force: true });
    assert.throws(
      () =>
        verifyBundleLayout({
          root: layout.root,
          target: "x86_64-unknown-linux-gnu",
          execute: false,
        }),
      /installed RTK license is missing/,
    );
  } finally {
    rmSync(layout.cleanupRoot, { recursive: true, force: true });
  }
});

test("requires each license file and its source content", () => {
  const layout = createLayout();
  try {
    rmSync(join(layout.resources, "licenses", "NOTICE"), { force: true });
    assert.throws(
      () =>
        verifyBundleLayout({
          root: layout.root,
          target: "x86_64-unknown-linux-gnu",
          execute: false,
        }),
      /installed license file is missing: .*NOTICE/,
    );
    writeFileSync(join(layout.resources, "licenses", "NOTICE"), "changed");
    assert.throws(
      () =>
        verifyBundleLayout({
          root: layout.root,
          target: "x86_64-unknown-linux-gnu",
          execute: false,
        }),
      /installed license file does not match its source: .*NOTICE/,
    );
  } finally {
    rmSync(layout.cleanupRoot, { recursive: true, force: true });
  }
});

test("requires a third-party license list that names the key packages", () => {
  const layout = createLayout();
  const installed = join(layout.resources, "licenses", "THIRD_PARTY_LICENSES.md");
  const verify = () =>
    verifyBundleLayout({
      root: layout.root,
      target: "x86_64-unknown-linux-gnu",
      execute: false,
    });
  try {
    rmSync(installed, { force: true });
    assert.throws(verify, /installed third-party license list is missing/);
    writeFileSync(installed, "\n");
    assert.throws(verify, /installed third-party license list is empty/);
    writeFileSync(
      installed,
      "# Third-party licenses\n\n## Rust crates\n\n### reqwest 0.12.28\n\n" +
        "## npm packages\n\n### react 18.3.1\n\n### tauri 2.0.0\n",
    );
    assert.throws(verify, /does not name the tauri crate/);
    writeFileSync(
      installed,
      "# Third-party licenses\n\n## Rust crates\n\n### tauri 2.11.5\n\n" +
        "### reqwest 0.12.28\n\n## npm packages\n\n### react-dom 18.3.1\n",
    );
    assert.throws(verify, /does not name the react npm package/);
  } finally {
    rmSync(layout.cleanupRoot, { recursive: true, force: true });
  }
});

test("the committed third-party license list names the key packages", () => {
  const path = licenseSources()["THIRD_PARTY_LICENSES.md"];
  assert.equal(verifyThirdPartyLicenses(path), path);
});

test("runs when Node starts the verifier through a symlink", () => {
  const root = mkdtempSync(join(tmpdir(), "halv-native-bundle-link-"));
  try {
    const scripts = join(APP_ROOT, "scripts");
    const linkedScripts = join(root, "scripts");
    // A junction needs no special right on Windows. Other platforms ignore the type.
    symlinkSync(scripts, linkedScripts, "junction");
    const linkedVerifier = join(linkedScripts, "verify-native-bundle.mjs");
    const verifierUrl = pathToFileURL(
      realpathSync(join(scripts, "verify-native-bundle.mjs")),
    ).href;
    // The fallback for Node versions without import.meta.main.
    assert.equal(isMainModule(verifierUrl, undefined, linkedVerifier), true);
    assert.equal(isMainModule(verifierUrl, undefined, join(root, "other.mjs")), false);

    const result = spawnSync(process.execPath, [linkedVerifier, "--help"], {
      encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /Usage: node scripts\/verify-native-bundle\.mjs/u);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("bundles every license file that the verifier checks", () => {
  const config = JSON.parse(
    readFileSync(join(APP_ROOT, "src-tauri", "tauri.conf.json"), "utf8"),
  );
  const installed = Object.values(config.bundle.resources)
    .filter((target) => target.startsWith("licenses/"))
    .map((target) => target.slice("licenses/".length))
    .sort();
  assert.deepEqual(installed, Object.keys(licenseSources()).sort());
});

test("requires halv-supervisor beside the installed Halv executable", () => {
  const layout = createLayout();
  try {
    rmSync(join(layout.bin, "halv-supervisor"), { force: true });
    assert.throws(
      () =>
        verifyBundleLayout({
          root: layout.root,
          target: "x86_64-unknown-linux-gnu",
          execute: false,
        }),
      /halv-supervisor is not installed beside the Halv executable/,
    );
  } finally {
    rmSync(layout.cleanupRoot, { recursive: true, force: true });
  }
});

test("requires an executable halv-supervisor on Unix", () => {
  const layout = createLayout();
  try {
    chmodSync(join(layout.bin, "halv-supervisor"), 0o644);
    assert.throws(
      () =>
        verifyBundleLayout({
          root: layout.root,
          target: "x86_64-unknown-linux-gnu",
          execute: false,
        }),
      /installed halv-supervisor is not executable/,
    );
  } finally {
    rmSync(layout.cleanupRoot, { recursive: true, force: true });
  }
});

test("requires halv-supervisor in the macOS executable directory", () => {
  const layout = createLayout("aarch64-apple-darwin");
  try {
    const result = verifyBundleLayout({
      root: layout.root,
      target: "aarch64-apple-darwin",
      execute: false,
    });
    assert.equal(
      result.supervisor,
      join(layout.root, "Contents", "MacOS", "halv-supervisor"),
    );
    assert.equal(
      result.crux,
      join(layout.root, "Contents", "MacOS", "crux"),
    );
    chmodSync(result.supervisor, 0o644);
    assert.throws(
      () =>
        verifyBundleLayout({
          root: layout.root,
          target: "aarch64-apple-darwin",
          execute: false,
        }),
      /installed halv-supervisor is not executable/,
    );
  } finally {
    rmSync(layout.cleanupRoot, { recursive: true, force: true });
  }
});

test("rejects RTK when it is not beside the installed Halv executable", () => {
  const layout = createLayout();
  try {
    rmSync(join(layout.bin, "halv"), { force: true });
    assert.throws(
      () =>
        verifyBundleLayout({
          root: layout.root,
          target: "x86_64-unknown-linux-gnu",
          execute: false,
        }),
      /RTK is not installed beside the Halv executable/,
    );
  } finally {
    rmSync(layout.cleanupRoot, { recursive: true, force: true });
  }
});

test("runChecked reports status, signal, stdout, and stderr", () => {
  assert.throws(
    () =>
      runChecked(process.execPath, [
        "-e",
        "process.stdout.write('out'); process.stderr.write('err'); process.exit(23)",
      ]),
    (error) => {
      assert.match(error.message, /status: 23/);
      assert.match(error.message, /signal: none/);
      assert.match(error.message, /stdout:\nout/);
      assert.match(error.message, /stderr:\nerr/);
      return true;
    },
  );
});

test("runChecked reports a terminating signal", { skip: process.platform === "win32" }, () => {
  assert.throws(
    () =>
      runChecked(process.execPath, [
        "-e",
        "process.stderr.write('before signal'); process.kill(process.pid, 'SIGTERM')",
      ]),
    (error) => {
      assert.match(error.message, /status: null/);
      assert.match(error.message, /signal: SIGTERM/);
      assert.match(error.message, /stderr:\nbefore signal/);
      return true;
    },
  );
});

test("Linux release verification covers DEB, RPM, and AppImage bundles", () => {
  const verifier = readFileSync(
    join(APP_ROOT, "scripts", "verify-native-bundle.mjs"),
    "utf8",
  );
  assert.match(verifier, /kind: "deb"/);
  assert.match(verifier, /kind: "rpm"/);
  assert.match(verifier, /kind: "appimage"/);
  const rpmExtractor = verifier.slice(
    verifier.indexOf("function extractRpm"),
    verifier.indexOf("function extractAppImage"),
  );
  assert.match(rpmExtractor, /rpm2cpio/);
  assert.match(rpmExtractor, /cpio -idmu --quiet/);
  assert.doesNotMatch(rpmExtractor, /"pipefail"/);
});

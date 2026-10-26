#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  assertSupportedTarget,
  runtimeExecutableName,
  validateStagedRuntime,
} from "./headroom-runtime-lib.mjs";

const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const RTK_VERSION = "0.42.4";
const RTK_LICENSE = `rtk-${RTK_VERSION}-LICENSE`;
const EXPECTED_RTK_LICENSE = join(
  APP_ROOT,
  "src-tauri",
  "resources",
  "licenses",
  RTK_LICENSE,
);
// scripts/collect-third-party-licenses.mjs makes this file.
const THIRD_PARTY_LICENSES = "THIRD_PARTY_LICENSES.md";
// Key packages of the app. A third-party license list that does not name each
// one is not complete.
const REQUIRED_THIRD_PARTY_PACKAGES = [
  { section: "Rust crates", kind: "crate", name: "tauri" },
  { section: "Rust crates", kind: "crate", name: "reqwest" },
  { section: "npm packages", kind: "npm package", name: "react" },
];
// Each license file that the bundle installs in its licenses/ folder, with the
// source file that tauri.conf.json copies there.
const LICENSE_SOURCES = {
  LICENSE: join(APP_ROOT, "LICENSE"),
  NOTICE: join(APP_ROOT, "NOTICE"),
  "THIRD_PARTY_NOTICES.md": join(APP_ROOT, "THIRD_PARTY_NOTICES.md"),
  [RTK_LICENSE]: EXPECTED_RTK_LICENSE,
  ...Object.fromEntries(
    [
      "crux-LICENSE",
      "orca-LICENSE",
      "ponytail-LICENSE",
      "lucide-LICENSE",
      THIRD_PARTY_LICENSES,
    ].map((name) => [
      name,
      join(APP_ROOT, "src-tauri", "resources", "licenses", name),
    ]),
  ),
};

function invariant(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function walk(root, visitor) {
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const entryPath = join(root, entry.name);
    visitor(entryPath, entry);
    if (entry.isDirectory() && !entry.isSymbolicLink()) {
      walk(entryPath, visitor);
    }
  }
}

function findPaths(root, predicate) {
  const matches = [];
  walk(root, (path, entry) => {
    if (predicate(path, entry)) {
      matches.push(path);
    }
  });
  return matches;
}

function findFilesNamed(root, name) {
  const normalized = name.toLowerCase();
  return findPaths(
    root,
    (path, entry) => entry.isFile() && basename(path).toLowerCase() === normalized,
  );
}

function commandDisplay(command, args) {
  return [command, ...args]
    .map((argument) =>
      /\s|["']/u.test(argument) ? JSON.stringify(argument) : argument,
    )
    .join(" ");
}

function spawnDetail(result) {
  const details = [
    `status: ${result.status ?? "null"}`,
    `signal: ${result.signal ?? "none"}`,
  ];
  if (result.error) {
    details.push(`spawn error: ${result.error.message}`);
  }
  const stdout = `${result.stdout ?? ""}`.trim();
  const stderr = `${result.stderr ?? ""}`.trim();
  if (stdout) {
    details.push(`stdout:\n${stdout}`);
  }
  if (stderr) {
    details.push(`stderr:\n${stderr}`);
  }
  return details.join("\n");
}

export function runChecked(
  command,
  args,
  options = {},
  acceptedStatuses = [0],
) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    ...options,
  });
  if (!acceptedStatuses.includes(result.status)) {
    const cwd = options.cwd ? `\ncwd: ${resolve(options.cwd)}` : "";
    throw new Error(
      `${commandDisplay(command, args)} failed${cwd}\n${spawnDetail(result)}`,
    );
  }
  return `${result.stdout ?? ""}${result.stderr ?? ""}`.trim();
}

function runDiagnostic(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    ...options,
  });
  return `${commandDisplay(command, args)}\n${spawnDetail(result)}`;
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function linuxRtkDiagnostics(rtk, target) {
  const stats = statSync(rtk);
  const stagedRtk = join(APP_ROOT, "src-tauri", "binaries", `rtk-${target}`);
  const installedSha256 = sha256File(rtk);
  const details = [
    `path: ${rtk}`,
    `size: ${stats.size}`,
    `mode: 0${(stats.mode & 0o777).toString(8)}`,
    `sha256: ${installedSha256}`,
  ];
  if (existsSync(stagedRtk)) {
    const stagedSha256 = sha256File(stagedRtk);
    details.push(
      `staged path: ${stagedRtk}`,
      `staged sha256: ${stagedSha256}`,
      `matches staged bytes: ${installedSha256 === stagedSha256}`,
    );
  } else {
    details.push(`staged RTK is missing: ${stagedRtk}`);
  }
  details.push(
    runDiagnostic("file", [rtk]),
    runDiagnostic("ldd", [rtk]),
    runDiagnostic("readelf", ["-lW", rtk]),
  );
  return details.join("\n");
}

function mainExecutableName(target) {
  return target.includes("windows") ? "halv.exe" : "halv";
}

function rtkExecutableName(target) {
  return target.includes("windows") ? "rtk.exe" : "rtk";
}

function supervisorExecutableName(target) {
  return target.includes("windows")
    ? "halv-supervisor.exe"
    : "halv-supervisor";
}

function verifySupervisor(appExecutable, target) {
  const supervisor = join(
    dirname(appExecutable),
    supervisorExecutableName(target),
  );
  invariant(
    existsSync(supervisor) && statSync(supervisor).isFile(),
    `halv-supervisor is not installed beside the Halv executable: ${supervisor}`,
  );
  if (!target.includes("windows")) {
    invariant(
      (statSync(supervisor).mode & 0o111) !== 0,
      `installed halv-supervisor is not executable: ${supervisor}`,
    );
  }
  return supervisor;
}

function relativeTreePath(root, path) {
  return relative(root, path).split(sep).join("/");
}

function windowsExtractionTree(extractionRoot) {
  const lines = ["[dir] ./"];
  const appendEntries = (directory) => {
    const entries = readdirSync(directory, { withFileTypes: true }).sort(
      (left, right) => left.name.localeCompare(right.name),
    );
    for (const entry of entries) {
      const path = join(directory, entry.name);
      const treePath = relativeTreePath(extractionRoot, path);
      if (entry.isDirectory()) {
        lines.push(`[dir] ${treePath}/`);
        if (!entry.isSymbolicLink()) {
          appendEntries(path);
        }
      } else if (entry.isFile()) {
        lines.push(`[file] ${treePath}`);
      } else {
        lines.push(`[other] ${treePath}`);
      }
    }
  };
  appendEntries(extractionRoot);
  return lines.join("\n");
}

function windowsBundleComponents(extractionRoot, target) {
  const components = [
    {
      candidates: findFilesNamed(extractionRoot, mainExecutableName(target)),
      errorLabel: "installed Halv executable",
      summaryLabel: mainExecutableName(target),
    },
    {
      candidates: findFilesNamed(extractionRoot, rtkExecutableName(target)),
      errorLabel: "installed RTK executable",
      summaryLabel: rtkExecutableName(target),
    },
    {
      candidates: findFilesNamed(
        extractionRoot,
        supervisorExecutableName(target),
      ),
      errorLabel: "installed halv-supervisor executable",
      summaryLabel: supervisorExecutableName(target),
    },
    {
      candidates: findFilesNamed(extractionRoot, runtimeExecutableName(target)),
      errorLabel: "installed Headroom runtime",
      summaryLabel: runtimeExecutableName(target),
    },
  ];
  const invalidComponents = components.filter(
    ({ candidates }) => candidates.length !== 1,
  );
  if (invalidComponents.length > 0) {
    console.error(
      "verify-native-bundle: Windows component search failed.\n" +
        `LessMSI extraction root searched: ${extractionRoot}\n` +
        `Candidate counts: ${components
          .map(
            ({ candidates, summaryLabel }) =>
              `${summaryLabel}=${candidates.length}`,
          )
          .join(", ")}\n` +
        "Full recursive LessMSI extraction tree:\n" +
        windowsExtractionTree(extractionRoot),
    );
  }
  for (const { candidates, errorLabel } of components) {
    invariant(
      candidates.length === 1,
      `expected one ${errorLabel}, found ${candidates.length} under ` +
        `LessMSI extraction root ${extractionRoot}`,
    );
  }
  return {
    appExecutable: components[0].candidates[0],
    rtk: components[1].candidates[0],
    supervisor: components[2].candidates[0],
    runtimeDir: dirname(components[3].candidates[0]),
  };
}

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

// Gives the text of one "## <title>" part of a Markdown file, or "".
function markdownSection(text, title) {
  const start = text.indexOf(`\n## ${title}\n`);
  if (start === -1) {
    return "";
  }
  const end = text.indexOf("\n## ", start + 1);
  return text.slice(start, end === -1 ? text.length : end);
}

// Checks the installed third-party license list. It must not be empty, and it
// must name the key crates and npm packages of the app.
export function verifyThirdPartyLicenses(path) {
  invariant(
    existsSync(path) && statSync(path).isFile(),
    `installed third-party license list is missing: ${path}`,
  );
  const text = readFileSync(path, "utf8").replace(/\r\n/gu, "\n");
  invariant(text.trim().length > 0, `installed third-party license list is empty: ${path}`);
  for (const { section, kind, name } of REQUIRED_THIRD_PARTY_PACKAGES) {
    invariant(
      new RegExp(`^### ${escapeRegExp(name)} \\d`, "mu").test(markdownSection(text, section)),
      `installed third-party license list does not name the ${name} ${kind}: ${path}`,
    );
  }
  return path;
}

// Checks the licenses/ folder of the bundle. The RTK license must match the
// pinned release license. The third-party license list must name the key
// packages. Each other file must match its source file.
function verifyLicenses(resourceDir) {
  const license = join(resourceDir, "licenses", RTK_LICENSE);
  invariant(
    existsSync(license) && statSync(license).isFile(),
    `installed RTK license is missing: ${license}`,
  );
  invariant(
    readFileSync(license).equals(readFileSync(EXPECTED_RTK_LICENSE)),
    "installed RTK license does not match the pinned release license",
  );
  verifyThirdPartyLicenses(join(resourceDir, "licenses", THIRD_PARTY_LICENSES));
  for (const [name, source] of Object.entries(LICENSE_SOURCES)) {
    const installed = join(resourceDir, "licenses", name);
    invariant(
      existsSync(installed) && statSync(installed).isFile(),
      `installed license file is missing: ${installed}`,
    );
    invariant(
      readFileSync(installed).equals(readFileSync(source)),
      `installed license file does not match its source: ${installed}`,
    );
  }
  return license;
}

function verifyWindowsBundleLayout({ extractionRoot, target, execute }) {
  const { appExecutable, rtk, runtimeDir, supervisor } = windowsBundleComponents(
    extractionRoot,
    target,
  );
  const resourceDir = dirname(runtimeDir);
  const license = verifyLicenses(resourceDir);

  const runtime = validateStagedRuntime({
    resourceDir: runtimeDir,
    target,
    execute,
  });
  if (execute) {
    const version = runChecked(rtk, ["--version"], { cwd: dirname(rtk) });
    invariant(
      version.includes(RTK_VERSION),
      `installed RTK reported ${JSON.stringify(version)}; expected ${RTK_VERSION}`,
    );
  }

  return {
    appExecutable,
    headroomExecutable: runtime.executable,
    headroomSupportFiles: runtime.supportFileCount,
    license,
    resourceDir,
    rtk,
    supervisor,
  };
}

export function verifyBundleLayout({ root, target, execute = true }) {
  assertSupportedTarget(target);
  const extractionRoot = resolve(root);
  invariant(
    existsSync(extractionRoot) && statSync(extractionRoot).isDirectory(),
    `native bundle root does not exist: ${extractionRoot}`,
  );

  if (target.includes("windows")) {
    return verifyWindowsBundleLayout({ extractionRoot, target, execute });
  }

  const layoutRoot = extractionRoot;
  const runtimeManifests = findPaths(
    layoutRoot,
    (path, entry) =>
      entry.isFile() &&
      basename(path) === "build-manifest.json" &&
      basename(dirname(path)) === "headroom-runtime",
  );
  invariant(
    runtimeManifests.length === 1,
    `expected one installed Headroom runtime, found ${runtimeManifests.length}`,
  );
  const runtimeDir = dirname(runtimeManifests[0]);
  const resourceDir = dirname(runtimeDir);
  const license = verifyLicenses(resourceDir);

  const rtkCandidates = findFilesNamed(
    extractionRoot,
    rtkExecutableName(target),
  );
  invariant(
    rtkCandidates.length === 1,
    `expected one installed RTK executable, found ${rtkCandidates.length}`,
  );
  const rtk = rtkCandidates[0];
  const appExecutable = join(dirname(rtk), mainExecutableName(target));
  invariant(
    existsSync(appExecutable) && statSync(appExecutable).isFile(),
    `RTK is not installed beside the Halv executable: ${appExecutable}`,
  );
  const supervisor = verifySupervisor(appExecutable, target);
  let crux = null;
  if (target.includes("apple-darwin")) {
    const macOSDir = join(extractionRoot, "Contents", "MacOS");
    invariant(
      rtk === join(macOSDir, "rtk"),
      `RTK is not installed in the macOS executable directory: ${macOSDir}`,
    );
    crux = join(macOSDir, "crux");
    invariant(
      existsSync(crux) && statSync(crux).isFile(),
      `Crux is missing from the macOS executable directory: ${crux}`,
    );
    invariant(
      (statSync(crux).mode & 0o111) !== 0,
      `installed Crux is not executable: ${crux}`,
    );
    invariant(
      supervisor === join(macOSDir, "halv-supervisor"),
      `halv-supervisor is not installed in the macOS executable directory: ${macOSDir}`,
    );
  }

  const runtime = validateStagedRuntime({
    resourceDir: runtimeDir,
    target,
    execute,
  });
  if (execute) {
    let version;
    try {
      version = runChecked(rtk, ["--version"], { cwd: dirname(rtk) });
    } catch (error) {
      if (target.includes("linux")) {
        throw new Error(
          `${error.message}\nLinux RTK diagnostics:\n` +
            linuxRtkDiagnostics(rtk, target),
          { cause: error },
        );
      }
      throw error;
    }
    invariant(
      version.includes(RTK_VERSION),
      `installed RTK reported ${JSON.stringify(version)}; expected ${RTK_VERSION}`,
    );
  }

  return {
    appExecutable,
    crux,
    headroomExecutable: runtime.executable,
    headroomSupportFiles: runtime.supportFileCount,
    license,
    resourceDir,
    rtk,
    supervisor,
  };
}

function defaultBundleRoot(target) {
  const targetSpecific = join(
    APP_ROOT,
    "src-tauri",
    "target",
    target,
    "release",
    "bundle",
  );
  if (existsSync(targetSpecific)) {
    return targetSpecific;
  }
  return join(APP_ROOT, "src-tauri", "target", "release", "bundle");
}

function firstPath(root, predicate, label) {
  const matches = findPaths(root, predicate);
  invariant(matches.length > 0, `${label} was not found under ${root}`);
  return matches[0];
}

function extractDeb(bundleRoot, destination) {
  const deb = firstPath(
    bundleRoot,
    (path, entry) => entry.isFile() && path.toLowerCase().endsWith(".deb"),
    "Debian package",
  );
  runChecked("dpkg-deb", ["-x", deb, destination]);
  return destination;
}

function extractRpm(bundleRoot, destination) {
  const rpm = firstPath(
    bundleRoot,
    (path, entry) => entry.isFile() && path.toLowerCase().endsWith(".rpm"),
    "RPM package",
  );
  // Ubuntu 22.04 rpm2cpio can return 1 after emitting a complete archive.
  // cpio's status and the exact bundle-layout checks below remain authoritative.
  runChecked("bash", [
    "-c",
    'cd "$2" && rpm2cpio "$1" | cpio -idmu --quiet',
    "halv-rpm-extract",
    rpm,
    destination,
  ]);
  return destination;
}

function extractAppImage(bundleRoot, destination) {
  const appImage = firstPath(
    bundleRoot,
    (path, entry) => entry.isFile() && path.toLowerCase().endsWith(".appimage"),
    "AppImage",
  );
  chmodSync(appImage, lstatSync(appImage).mode | 0o111);
  runChecked(appImage, ["--appimage-extract"], { cwd: destination });
  const extracted = join(destination, "squashfs-root");
  invariant(existsSync(extracted), "AppImage extraction did not create squashfs-root");
  return extracted;
}

function extractMsi(bundleRoot, destination) {
  const msi = firstPath(
    bundleRoot,
    (path, entry) => entry.isFile() && path.toLowerCase().endsWith(".msi"),
    "Windows installer",
  );
  const absoluteDestination = resolve(destination);
  invariant(
    existsSync(absoluteDestination) && statSync(absoluteDestination).isDirectory(),
    `MSI extraction destination does not exist: ${absoluteDestination}`,
  );
  try {
    runChecked("lessmsi.exe", ["x", msi, `${absoluteDestination}${sep}`]);
  } catch (lessMsiError) {
    throw new Error(
      `LessMSI extraction failed for ${msi} into ${absoluteDestination}\n` +
        lessMsiError.message,
      { cause: lessMsiError },
    );
  }
  invariant(
    findPaths(absoluteDestination, (path, entry) => entry.isFile()).length > 0,
    `LessMSI produced no files under ${absoluteDestination} ` +
      "(check the `x` invocation / trailing separator)\n" +
      "Full recursive LessMSI extraction tree:\n" +
      windowsExtractionTree(absoluteDestination),
  );
  return absoluteDestination;
}

export function verifyNativeBundles({ target, bundleRoot = defaultBundleRoot(target) }) {
  assertSupportedTarget(target);
  const root = resolve(bundleRoot);
  invariant(existsSync(root), `bundle output does not exist: ${root}`);
  const summaries = [];

  if (target.includes("apple-darwin")) {
    const app = firstPath(
      root,
      (path, entry) => entry.isDirectory() && basename(path) === "Halv.app",
      "Halv.app",
    );
    summaries.push({ kind: "app", ...verifyBundleLayout({ root: app, target }) });
    return summaries;
  }

  const temporaryRoots = [];
  try {
    if (target.includes("linux")) {
      const debRoot = mkdtempSync(join(tmpdir(), "halv-deb-"));
      temporaryRoots.push(debRoot);
      summaries.push({
        kind: "deb",
        ...verifyBundleLayout({ root: extractDeb(root, debRoot), target }),
      });

      const rpmRoot = mkdtempSync(join(tmpdir(), "halv-rpm-"));
      temporaryRoots.push(rpmRoot);
      summaries.push({
        kind: "rpm",
        ...verifyBundleLayout({ root: extractRpm(root, rpmRoot), target }),
      });

      const appImageRoot = mkdtempSync(join(tmpdir(), "halv-appimage-"));
      temporaryRoots.push(appImageRoot);
      summaries.push({
        kind: "appimage",
        ...verifyBundleLayout({
          root: extractAppImage(root, appImageRoot),
          target,
        }),
      });
      return summaries;
    }

    if (target.includes("windows")) {
      const msiRoot = mkdtempSync(join(tmpdir(), "halv-lessmsi-"));
      temporaryRoots.push(msiRoot);
      summaries.push({
        kind: "msi",
        ...verifyBundleLayout({ root: extractMsi(root, msiRoot), target }),
      });
      return summaries;
    }
  } finally {
    for (const temporaryRoot of temporaryRoots) {
      rmSync(temporaryRoot, { recursive: true, force: true });
    }
  }

  throw new Error(`no native bundle verifier for ${target}`);
}

function argumentValue(argv, name) {
  const index = argv.indexOf(name);
  return index === -1 ? null : (argv[index + 1] ?? null);
}

async function main() {
  const argv = process.argv.slice(2);
  if (argv.includes("--help") || argv.includes("-h")) {
    console.log(
      "Usage: node scripts/verify-native-bundle.mjs " +
        "--target <Rust target triple> [--bundle-root <path>]",
    );
    return;
  }
  const target = argumentValue(argv, "--target");
  invariant(target, "--target <Rust target triple> is required");
  const bundleRoot = argumentValue(argv, "--bundle-root") ?? defaultBundleRoot(target);
  const summaries = verifyNativeBundles({ target, bundleRoot });
  for (const summary of summaries) {
    console.log(
      `verify-native-bundle: ${summary.kind} has halv-supervisor, ` +
        `RTK ${RTK_VERSION}, Headroom with ${summary.headroomSupportFiles} ` +
        "support files, and its license",
    );
  }
}

// Tells if Node started the module at moduleUrl as the main module. "main"
// is import.meta.main, which older Node versions do not give. Node gives
// import.meta.url as the real path of the file, so the fallback also compares
// the real path of the started script. A start through a symlink thus runs
// the checks.
export function isMainModule(moduleUrl, main, startedPath = process.argv[1]) {
  if (typeof main === "boolean") {
    return main;
  }
  if (!startedPath) {
    return false;
  }
  const started = [pathToFileURL(resolve(startedPath)).href];
  try {
    started.push(pathToFileURL(realpathSync(startedPath)).href);
  } catch {
    // The started path does not exist, so only the resolved path can match.
  }
  return started.includes(moduleUrl);
}

if (isMainModule(import.meta.url, import.meta.main)) {
  await main();
}

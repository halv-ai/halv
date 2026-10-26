#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve, sep } from "node:path";
import process from "node:process";
import {
  RESOURCE_DIR,
  RUNTIME_FILES_MANIFEST,
  RUNTIME_FILES_SIGNED_MANIFEST,
  RUNTIME_MANIFEST_EXPORT_DIR,
  assertManifestDirOutsideRuntime,
  compareRuntimeFiles,
  createRuntimeFilesManifest,
  exportRuntimeManifest,
  hostTarget,
  loadRuntimeFilesManifest,
  releaseManifestName,
  stableJson,
  validateStagedRuntime,
} from "./headroom-runtime-lib.mjs";

// The release copies of both file manifests go here (for CI upload).
const manifestDirIndex = process.argv.indexOf("--manifest-dir");
const manifestDir =
  manifestDirIndex === -1
    ? RUNTIME_MANIFEST_EXPORT_DIR
    : resolve(process.argv[manifestDirIndex + 1] ?? "");

function walk(path, insideFramework = false) {
  const files = [];
  const frameworks = [];
  for (const entry of readdirSync(path, { withFileTypes: true })) {
    const entryPath = join(path, entry.name);
    if (entry.isSymbolicLink()) {
      continue;
    }
    if (entry.isDirectory()) {
      const isFramework = entry.name.endsWith(".framework");
      if (isFramework) {
        frameworks.push(entryPath);
      }
      const nested = walk(entryPath, insideFramework || isFramework);
      files.push(...nested.files);
      frameworks.push(...nested.frameworks);
    } else if (entry.isFile() && !insideFramework) {
      files.push(entryPath);
    }
  }
  return { files, frameworks };
}

function pathDepth(path) {
  return path.split(sep).length;
}

function frameworkSigningTargets(frameworkPath) {
  const versionsPath = join(frameworkPath, "Versions");
  try {
    const versions = readdirSync(versionsPath, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && !entry.isSymbolicLink())
      .map((entry) => join(versionsPath, entry.name));
    return versions.length > 0 ? versions : [frameworkPath];
  } catch (error) {
    if (error.code === "ENOENT") {
      return [frameworkPath];
    }
    throw error;
  }
}

function capture(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error((result.stderr || result.stdout || `${command} failed`).trim());
  }
  return result.stdout.trim();
}

function run(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.status !== 0) {
    throw new Error(`${command} exited with ${result.status}`);
  }
}

function sign(path, identity) {
  if (lstatSync(path).isSymbolicLink()) {
    throw new Error(`refusing to codesign symlink ${path}`);
  }
  run("/usr/bin/codesign", [
    "--force",
    "--timestamp",
    "--options",
    "runtime",
    "--sign",
    identity,
    path,
  ]);
}

function codeDirectoryHash(path) {
  const result = spawnSync("/usr/bin/codesign", ["-dvvv", path], { encoding: "utf8" });
  const match = `${result.stderr ?? ""}${result.stdout ?? ""}`.match(/^CDHash=([0-9a-f]+)$/m);
  return match ? match[1] : null;
}

// The unsigned runtime must still match the manifest that its build wrote,
// for example after a CI cache restore.
function assertUnsignedManifest() {
  const manifestPath = join(RESOURCE_DIR, RUNTIME_FILES_MANIFEST);
  if (!existsSync(manifestPath)) {
    throw new Error(`missing ${RUNTIME_FILES_MANIFEST}; rebuild the runtime`);
  }
  const manifest = loadRuntimeFilesManifest(manifestPath);
  const comparison = compareRuntimeFiles({
    runtimeDir: RESOURCE_DIR,
    expected: manifest.files,
    expectedBuildMetadataSha256: manifest.buildMetadataSha256,
  });
  if (!comparison.passed || comparison.results.toolchainOnly.length > 0) {
    const changed = Object.entries(comparison.results)
      .filter(([key, entries]) => key !== "exact" && entries.length > 0)
      .map(([key, entries]) => `${key}: ${entries.slice(0, 5).map((entry) => entry.path).join(", ")}`);
    throw new Error(`staged runtime does not match ${RUNTIME_FILES_MANIFEST}: ${changed.join("; ")}`);
  }
}

// Post-sign hashes and, on macOS, the code directory hash of each Mach-O file.
// Both manifests also go to the manifest folder with their release asset
// names. The pre-sign copy comes from the runtime folder, so a runtime from
// the CI cache gives the same release asset as a new build.
function writeSignedManifest(signing) {
  const target = hostTarget();
  const manifest = createRuntimeFilesManifest(RESOURCE_DIR, {
    target,
    stage: "post-sign",
    extraFields: { signing },
  });
  if (process.platform === "darwin") {
    for (const entry of manifest.files) {
      if (entry.macho && !entry.link) {
        entry.cdhash = codeDirectoryHash(join(RESOURCE_DIR, ...entry.path.split("/")));
      }
    }
  }
  const signedText = stableJson(manifest);
  writeFileSync(join(RESOURCE_DIR, RUNTIME_FILES_SIGNED_MANIFEST), signedText);
  const exported = [
    exportRuntimeManifest({
      manifestText: readFileSync(join(RESOURCE_DIR, RUNTIME_FILES_MANIFEST), "utf8"),
      runtimeDir: RESOURCE_DIR,
      exportDir: manifestDir,
      name: releaseManifestName(target),
    }),
    exportRuntimeManifest({
      manifestText: signedText,
      runtimeDir: RESOURCE_DIR,
      exportDir: manifestDir,
      name: releaseManifestName(target, { signed: true }),
    }),
  ];
  console.log(
    `sign-headroom-runtime: wrote ${RUNTIME_FILES_SIGNED_MANIFEST} (${manifest.fileCount} files, signing ${signing}); ` +
      `release copies: ${exported.join(", ")}`,
  );
}

try {
  assertManifestDirOutsideRuntime(manifestDir, RESOURCE_DIR);
  validateStagedRuntime();
  assertUnsignedManifest();
  if (process.platform !== "darwin") {
    console.log("sign-headroom-runtime: not macOS; nothing to sign");
    writeSignedManifest("none");
    process.exit(0);
  }
  if (process.env.HALV_SIGN_HEADROOM !== "1") {
    console.log("sign-headroom-runtime: signed release mode is disabled; unsigned build");
    writeSignedManifest("none");
    process.exit(0);
  }
  const identity = process.env.APPLE_SIGNING_IDENTITY?.trim();
  if (!identity) {
    console.log("sign-headroom-runtime: APPLE_SIGNING_IDENTITY is absent; unsigned build");
    writeSignedManifest("none");
    process.exit(0);
  }

  const discovered = walk(RESOURCE_DIR);
  const codeFiles = discovered.files
    .filter((path) => capture("/usr/bin/file", ["-b", path]).includes("Mach-O"))
    .sort((left, right) => pathDepth(right) - pathDepth(left));
  const frameworkTargets = discovered.frameworks
    .flatMap(frameworkSigningTargets)
    .sort((left, right) => pathDepth(right) - pathDepth(left));
  if (codeFiles.length === 0 && frameworkTargets.length === 0) {
    throw new Error("no Mach-O files found in the staged Headroom runtime");
  }
  for (const path of codeFiles) {
    sign(path, identity);
    run("/usr/bin/codesign", ["--verify", "--strict", "--verbose=2", path]);
  }
  for (const path of frameworkTargets) {
    sign(path, identity);
    run("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", path]);
  }
  for (const path of discovered.frameworks) {
    run("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", path]);
  }
  console.log(
    `sign-headroom-runtime: signed and verified ${codeFiles.length} Mach-O files and ${discovered.frameworks.length} frameworks`,
  );
  writeSignedManifest("developer-id");
} catch (error) {
  console.error(`sign-headroom-runtime: ${error.message}`);
  process.exit(1);
}

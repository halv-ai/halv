#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { lstatSync, readdirSync } from "node:fs";
import { join, sep } from "node:path";
import process from "node:process";
import { RESOURCE_DIR, validateStagedRuntime } from "./headroom-runtime-lib.mjs";

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

try {
  validateStagedRuntime();
  if (process.platform !== "darwin") {
    console.log("sign-headroom-runtime: not macOS; nothing to sign");
    process.exit(0);
  }
  if (process.env.HALV_SIGN_HEADROOM !== "1") {
    console.log("sign-headroom-runtime: signed release mode is disabled; unsigned build");
    process.exit(0);
  }
  const identity = process.env.APPLE_SIGNING_IDENTITY?.trim();
  if (!identity) {
    console.log("sign-headroom-runtime: APPLE_SIGNING_IDENTITY is absent; unsigned build");
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
} catch (error) {
  console.error(`sign-headroom-runtime: ${error.message}`);
  process.exit(1);
}

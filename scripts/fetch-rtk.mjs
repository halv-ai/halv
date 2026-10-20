#!/usr/bin/env node
// Fetches a pinned RTK binary into src-tauri/binaries/ for Tauri externalBin.
// The release archive is accepted only after verification against the pinned
// release's checksums.txt entry.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const RTK_VERSION = "0.42.4";
const PINNED_ARCHIVE_SHA256 = {
  "rtk-aarch64-apple-darwin.tar.gz":
    "f223ca074a0215af002679bc1d34ca92b93e25b3e8ae16aace6e84c06e586802",
  "rtk-x86_64-apple-darwin.tar.gz":
    "84121316867613e61925c209607f033b2113bb0ce312c267a79d3e3e8f221e49",
  "rtk-aarch64-unknown-linux-gnu.tar.gz":
    "cc2b91c064eb670c097c184913c8fbcb1a943d53d7fe505375e96ba0c5b6459f",
  "rtk-x86_64-unknown-linux-musl.tar.gz":
    "34975116da11e09e502501daf758143e0b22ed3a42a10eb67fb693a6270d9e36",
  "rtk-x86_64-pc-windows-msvc.zip":
    "f0ec18963581657173bd6a51f5ba012b093823f844db749fec218581af30a568",
};

const TARGET_BY_PLATFORM = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "win32-x64": "x86_64-pc-windows-msvc",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "linux-x64": "x86_64-unknown-linux-gnu",
};

function fail(message) {
  console.error(`fetch-rtk: ${message}`);
  process.exit(1);
}

function resolveTarget(argv) {
  const flagIndex = argv.indexOf("--target");
  if (flagIndex !== -1) {
    const triple = argv[flagIndex + 1];
    if (!triple) {
      fail("--target requires a value, e.g. --target aarch64-apple-darwin");
    }
    return triple;
  }
  const key = `${process.platform}-${process.arch}`;
  const triple = TARGET_BY_PLATFORM[key];
  if (!triple) {
    fail(
      `unsupported platform ${key}; pass an explicit --target <triple> ` +
        `(known: ${Object.values(TARGET_BY_PLATFORM).join(", ")})`,
    );
  }
  return triple;
}

function releaseTarget(target) {
  // RTK publishes a static musl binary for Linux x64. Tauri still requires
  // the staged filename to carry the Rust build target (gnu on our CI host).
  return target === "x86_64-unknown-linux-gnu"
    ? "x86_64-unknown-linux-musl"
    : target;
}

async function download(url) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) {
    fail(`download failed (${response.status} ${response.statusText}): ${url}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

function findBinary(dir, name) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const entryPath = join(dir, entry.name);
    if (entry.isFile() && entry.name === name) {
      return entryPath;
    }
    if (entry.isDirectory()) {
      const found = findBinary(entryPath, name);
      if (found) {
        return found;
      }
    }
  }
  return null;
}

function expectedChecksum(checksums, archiveName) {
  const line = checksums
    .toString("utf8")
    .split(/\r?\n/)
    .find((candidate) => candidate.trim().endsWith(`  ${archiveName}`));
  const digest = line?.trim().split(/\s+/)[0] ?? "";
  if (!/^[0-9a-f]{64}$/.test(digest)) {
    fail(`checksums.txt has no valid SHA-256 entry for ${archiveName}.`);
  }
  return digest;
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function cachedBinaryMatches(outputPath, metadataPath) {
  if (!existsSync(outputPath) || !existsSync(metadataPath)) {
    return false;
  }
  try {
    const metadata = JSON.parse(readFileSync(metadataPath, "utf8"));
    return (
      metadata.version === RTK_VERSION &&
      /^[0-9a-f]{64}$/.test(metadata.binarySha256 ?? "") &&
      sha256(readFileSync(outputPath)) === metadata.binarySha256
    );
  } catch {
    return false;
  }
}

async function main() {
  const target = resolveTarget(process.argv.slice(2));
  const artifactTarget = releaseTarget(target);
  const isWindows = target.includes("-windows-");
  const archiveExt = isWindows ? "zip" : "tar.gz";
  const binaryName = isWindows ? "rtk.exe" : "rtk";
  const archiveName = `rtk-${artifactTarget}.${archiveExt}`;
  const baseUrl =
    `https://github.com/rtk-ai/rtk/releases/download/v${RTK_VERSION}`;

  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const binariesDir = join(repoRoot, "src-tauri", "binaries");
  const outputPath = join(
    binariesDir,
    `rtk-${target}${isWindows ? ".exe" : ""}`,
  );
  const metadataPath = `${outputPath}.metadata.json`;
  const legacyVersionMarker = outputPath + ".version";

  if (cachedBinaryMatches(outputPath, metadataPath)) {
    rmSync(legacyVersionMarker, { force: true });
    console.log(
      `fetch-rtk: ${outputPath} already contains verified ${RTK_VERSION}.`,
    );
    return;
  }

  console.log(`fetch-rtk: fetching RTK ${RTK_VERSION} for ${target}.`);
  const [archive, checksums] = await Promise.all([
    download(`${baseUrl}/${archiveName}`),
    download(`${baseUrl}/checksums.txt`),
  ]);
  const pinned = PINNED_ARCHIVE_SHA256[archiveName];
  if (!pinned) {
    fail(`no immutable SHA-256 pin is configured for ${archiveName}.`);
  }
  const published = expectedChecksum(checksums, archiveName);
  if (published !== pinned) {
    fail(
      `published checksum for ${archiveName} differs from the immutable pin.`,
    );
  }
  const actual = sha256(archive);
  if (actual !== pinned) {
    fail(
      `SHA-256 mismatch for ${archiveName}: expected ${pinned}, got ${actual}.`,
    );
  }
  console.log(`fetch-rtk: SHA-256 verified (${pinned}).`);

  mkdirSync(binariesDir, { recursive: true });
  const tempDir = join(binariesDir, `.rtk-tmp-${process.pid}`);
  mkdirSync(tempDir, { recursive: true });
  try {
    const archivePath = join(tempDir, archiveName);
    writeFileSync(archivePath, archive);
    const tarArgs = isWindows
      ? ["-xf", archivePath, "-C", tempDir]
      : ["-xzf", archivePath, "-C", tempDir];
    const extraction = spawnSync("tar", tarArgs, { stdio: "inherit" });
    if (extraction.status !== 0) {
      fail(`tar failed to extract ${archiveName}.`);
    }

    const extracted = findBinary(tempDir, binaryName);
    if (!extracted) {
      fail(`${binaryName} not found inside ${archiveName}.`);
    }
    copyFileSync(extracted, outputPath);
    if (!isWindows) {
      chmodSync(outputPath, 0o755);
    }
    writeFileSync(
      metadataPath,
      `${JSON.stringify({
        version: RTK_VERSION,
        binarySha256: sha256(readFileSync(outputPath)),
        archiveSha256: pinned,
      })}\n`,
    );
    rmSync(legacyVersionMarker, { force: true });
  } finally {
    rmSync(tempDir, { recursive: true, force: true });
  }

  console.log(`fetch-rtk: wrote ${outputPath}.`);
}

await main();

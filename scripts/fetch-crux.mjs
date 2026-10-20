#!/usr/bin/env node
// Fetches a pinned Crux binary into src-tauri/binaries/ for Tauri externalBin.
// Generated binaries and metadata are gitignored; do not commit them.

import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const CRUX_VERSION = "v0.12.0";
const PINNED_BINARY_SHA256 = {
  "crux-aarch64-apple-darwin":
    "714323766c6128d2b3825983c4d7460bd386e20d21ff5076171d625ce8916299",
  "crux-x86_64-apple-darwin":
    "19b9de47d165e90a916ac83d51cc46376d010d32fe82aa86870026de4b957b70",
  "crux-aarch64-unknown-linux-gnu":
    "c8c758dab02f9d6d0849597d03b5d7add8211bef23ce69145500c3cd5764f71d",
  "crux-x86_64-unknown-linux-gnu":
    "b4b8f02d63495a3403d8546f4ed22e10c2aa22b0db6de7b44202ddb866014171",
  "crux-x86_64-pc-windows-msvc.exe":
    "8574686c5434f97443e7d8f8db5edbd76656849d07ba5ee4592243b60ca7601e",
};

const TARGET_BY_PLATFORM = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "win32-x64": "x86_64-pc-windows-msvc",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "linux-x64": "x86_64-unknown-linux-gnu",
};

function fail(message) {
  console.error(`fetch-crux: ${message}`);
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

async function download(url) {
  let response;
  try {
    response = await fetch(url, { redirect: "follow" });
  } catch (error) {
    fail(`download failed (${error.message}): ${url}`);
  }
  if (!response.ok) {
    fail(`download failed (${response.status} ${response.statusText}): ${url}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function cachedBinaryMatches(outputPath, metadataPath, pinned) {
  if (!existsSync(outputPath) || !existsSync(metadataPath)) {
    return false;
  }
  try {
    const metadata = JSON.parse(readFileSync(metadataPath, "utf8"));
    return (
      metadata.version === CRUX_VERSION &&
      metadata.binarySha256 === pinned &&
      sha256(readFileSync(outputPath)) === pinned
    );
  } catch {
    return false;
  }
}

async function main() {
  const target = resolveTarget(process.argv.slice(2));
  const isWindows = target.includes("-windows-");
  const assetName = `crux-${target}${isWindows ? ".exe" : ""}`;
  const baseUrl =
    `https://github.com/pedr0v/crux/releases/download/${CRUX_VERSION}`;
  const pinned = PINNED_BINARY_SHA256[assetName];
  if (!pinned) {
    fail(`no immutable SHA-256 pin is configured for ${assetName}.`);
  }
  if (!/^[a-f0-9]{64}$/.test(pinned)) {
    fail(
      `replace the ${assetName} SHA-256 TODO after the ${CRUX_VERSION} asset is available.`,
    );
  }

  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const binariesDir = join(repoRoot, "src-tauri", "binaries");
  const outputPath = join(binariesDir, assetName);
  const metadataPath = `${outputPath}.metadata.json`;

  if (cachedBinaryMatches(outputPath, metadataPath, pinned)) {
    console.log(
      `fetch-crux: ${outputPath} already contains verified ${CRUX_VERSION}.`,
    );
    return;
  }

  console.log(`fetch-crux: fetching Crux ${CRUX_VERSION} for ${target}.`);
  const binary = await download(`${baseUrl}/${assetName}`);
  const actual = sha256(binary);
  if (actual !== pinned) {
    fail(
      `SHA-256 mismatch for ${assetName}: expected ${pinned}, got ${actual}.`,
    );
  }
  console.log(`fetch-crux: SHA-256 verified (${pinned}).`);

  mkdirSync(binariesDir, { recursive: true });
  writeFileSync(outputPath, binary);
  if (!isWindows) {
    chmodSync(outputPath, 0o755);
  }
  writeFileSync(
    metadataPath,
    `${JSON.stringify({
      version: CRUX_VERSION,
      binarySha256: actual,
    })}\n`,
  );

  console.log(`fetch-crux: wrote ${outputPath}.`);
}

await main();

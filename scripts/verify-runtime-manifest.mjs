#!/usr/bin/env node

// Compares a Headroom runtime directory file by file with what Halv ships.
//
//   node scripts/verify-runtime-manifest.mjs <runtime-dir> --manifest <runtime-files.sha256.json>
//   node scripts/verify-runtime-manifest.mjs <runtime-dir> --reference <other-runtime-dir> [--macos-normalize]
//
// --manifest compares with a release manifest (hashes only).
// --reference compares with a second runtime, for example the one inside an
// installed Halv.app.
// --macos-normalize removes the code signature from a copy of each Mach-O
// file that differs and compares again, so a signed runtime and an unsigned
// rebuild can be compared. With --reference it compares the two copies. With
// --manifest it compares the copy with the unsigned hashes in the manifest
// (unsignedSha256, unsignedLinkeditSha256), which macOS builds write. It
// reports files that differ only in the __LINKEDIT vmsize field apart from
// real differences.

import { resolve } from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";
import {
  COMPARISON_CATEGORIES,
  compareRuntimeFiles,
  listRuntimeFiles,
  loadRuntimeFilesManifest,
} from "./headroom-runtime-lib.mjs";

const USAGE =
  "usage: verify-runtime-manifest.mjs <runtime-dir> (--manifest <file> | --reference <dir>) " +
  "[--macos-normalize] [--details <count>]";

export function parseArguments(argv) {
  const options = { runtimeDir: null, manifest: null, reference: null, macosNormalize: false, details: 50 };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--manifest" || argument === "--reference" || argument === "--details") {
      const value = argv[index + 1];
      if (!value) {
        throw new Error(`${argument} requires a value\n${USAGE}`);
      }
      index += 1;
      if (argument === "--details") {
        options.details = Number(value);
      } else {
        options[argument.slice(2)] = value;
      }
    } else if (argument === "--macos-normalize") {
      options.macosNormalize = true;
    } else if (argument.startsWith("--")) {
      throw new Error(`unknown option ${argument}\n${USAGE}`);
    } else if (!options.runtimeDir) {
      options.runtimeDir = argument;
    } else {
      throw new Error(`unexpected argument ${argument}\n${USAGE}`);
    }
  }
  if (!options.runtimeDir || Boolean(options.manifest) === Boolean(options.reference)) {
    throw new Error(USAGE);
  }
  return options;
}

export function verifyRuntime(options) {
  const runtimeDir = resolve(options.runtimeDir);
  if (options.manifest) {
    const manifest = loadRuntimeFilesManifest(resolve(options.manifest));
    return {
      label: resolve(options.manifest),
      ...compareRuntimeFiles({
        runtimeDir,
        expected: manifest.files,
        expectedBuildMetadataSha256: manifest.buildMetadataSha256,
        macosNormalize: options.macosNormalize,
      }),
    };
  }
  const referenceDir = resolve(options.reference);
  return {
    label: referenceDir,
    ...compareRuntimeFiles({
      runtimeDir,
      expected: listRuntimeFiles(referenceDir),
      expectedDir: referenceDir,
      macosNormalize: options.macosNormalize,
    }),
  };
}

export function formatReport({ runtimeDir, label, comparison, details = 50 }) {
  const lines = [
    `verify-runtime-manifest: ${runtimeDir}`,
    `verify-runtime-manifest: expected ${label}`,
    "",
    `${"category".padEnd(52)}${"files".padStart(7)}  status`,
  ];
  for (const [key, title, ok] of COMPARISON_CATEGORIES) {
    const count = comparison.results[key].length;
    const status = count === 0 ? "-" : ok ? "ok" : "FAIL";
    lines.push(`${title.padEnd(52)}${String(count).padStart(7)}  ${status}`);
  }
  lines.push("");
  for (const [key, title, ok] of COMPARISON_CATEGORIES) {
    if (key === "exact") {
      continue;
    }
    const entries = comparison.results[key];
    if (entries.length === 0) {
      continue;
    }
    lines.push(`${ok ? "note" : "FAIL"}: ${title}:`);
    for (const entry of entries.slice(0, details)) {
      const extra = entry.expected ? ` (actual ${entry.actual}, expected ${entry.expected})` : "";
      lines.push(`  ${entry.path}${extra}`);
    }
    if (entries.length > details) {
      lines.push(`  ... ${entries.length - details} more`);
    }
  }
  lines.push(
    comparison.passed
      ? `RESULT: PASS (${comparison.fileCount} files)`
      : `RESULT: FAIL (${comparison.failures} of ${comparison.fileCount} files do not match)`,
  );
  return lines.join("\n");
}

function main() {
  let options;
  try {
    options = parseArguments(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exit(2);
  }
  try {
    const { label, ...comparison } = verifyRuntime(options);
    console.log(
      formatReport({
        runtimeDir: resolve(options.runtimeDir),
        label,
        comparison,
        details: options.details,
      }),
    );
    process.exit(comparison.passed ? 0 : 1);
  } catch (error) {
    console.error(`verify-runtime-manifest: ${error.message}`);
    process.exit(2);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main();
}

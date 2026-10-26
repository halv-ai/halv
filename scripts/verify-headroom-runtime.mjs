#!/usr/bin/env node

import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  RESOURCE_DIR,
  RUNTIME_FILES_MANIFEST,
  RUNTIME_FILES_SIGNED_MANIFEST,
  compareRuntimeFiles,
  loadRuntimeFilesManifest,
  validateStagedRuntime,
} from "./headroom-runtime-lib.mjs";

const resourceIndex = process.argv.indexOf("--resource-dir");
const resourceDir =
  resourceIndex === -1 ? RESOURCE_DIR : resolve(process.argv[resourceIndex + 1] ?? "");

// The newest file manifest describes the current bytes: the post-sign one
// after signing, else the one the build wrote.
function checkFileManifest(requireMatch) {
  const name = [RUNTIME_FILES_SIGNED_MANIFEST, RUNTIME_FILES_MANIFEST].find((candidate) =>
    existsSync(join(resourceDir, candidate)),
  );
  if (!name) {
    if (requireMatch) {
      throw new Error(`no ${RUNTIME_FILES_MANIFEST} in ${resourceDir}`);
    }
    return "no file manifest";
  }
  const manifest = loadRuntimeFilesManifest(join(resourceDir, name));
  const comparison = compareRuntimeFiles({
    runtimeDir: resourceDir,
    expected: manifest.files,
    expectedBuildMetadataSha256: manifest.buildMetadataSha256,
  });
  const differences = comparison.fileCount - comparison.results.exact.length + comparison.results.missing.length;
  if (differences > 0 && requireMatch) {
    throw new Error(
      `${differences} files do not match ${name}; run node scripts/verify-runtime-manifest.mjs for details`,
    );
  }
  return differences === 0
    ? `${comparison.fileCount} files match ${name}`
    : `${differences} files differ from ${name} (for example after a local ad-hoc signature)`;
}

try {
  const result = validateStagedRuntime({ resourceDir });
  const files = checkFileManifest(process.argv.includes("--files"));
  console.log(
    `verify-headroom-runtime: ${result.manifest.runtimeVersion} for ${result.manifest.target}; ` +
      `${result.supportFileCount} support files present; ${files}`,
  );
} catch (error) {
  console.error(`verify-headroom-runtime: ${error.message}`);
  process.exit(1);
}

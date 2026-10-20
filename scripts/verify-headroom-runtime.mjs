#!/usr/bin/env node

import { validateStagedRuntime } from "./headroom-runtime-lib.mjs";

try {
  const result = validateStagedRuntime();
  console.log(
    `verify-headroom-runtime: ${result.manifest.runtimeVersion} for ${result.manifest.target}; ` +
      `${result.supportFileCount} support files present`,
  );
} catch (error) {
  console.error(`verify-headroom-runtime: ${error.message}`);
  process.exit(1);
}

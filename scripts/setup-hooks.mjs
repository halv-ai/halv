#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const result = spawnSync("git", ["config", "core.hooksPath", ".githooks"], {
  cwd: repoRoot,
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"],
});

if (result.error) {
  console.error(`Could not configure Git hooks: ${result.error.message}`);
  process.exitCode = 1;
} else if (result.status !== 0) {
  const detail = result.stderr.trim() || `git exited with status ${result.status}`;
  console.error(`Could not configure Git hooks: ${detail}`);
  process.exitCode = 1;
} else {
  console.log("Git hooks enabled: core.hooksPath=.githooks");
  console.log("Future pushes will run the fast Halv preflight gate.");
}

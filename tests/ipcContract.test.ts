import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { commandNames } from "../src/ipc.ts";
import { fixtureCommandHandlers } from "../src/harness/mock-ipc.ts";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sourceRoot = path.join(root, "src");

async function sourceFiles(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = await Promise.all(
    entries.map(async (entry) => {
      const resolved = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        return resolved === path.join(sourceRoot, "harness")
          ? []
          : sourceFiles(resolved);
      }
      return /\.tsx?$/.test(entry.name) && resolved !== path.join(sourceRoot, "ipc.ts")
        ? [resolved]
        : [];
    }),
  );
  return files.flat();
}

test("production source does not import raw Tauri invoke", async () => {
  const offenders: string[] = [];
  for (const file of await sourceFiles(sourceRoot)) {
    const source = await readFile(file, "utf8");
    if (
      /import\s*\{[^}]*\binvoke\b[^}]*\}\s*from\s*["']@tauri-apps\/api(?:\/[^"']+)?["']/.test(
        source,
      )
    ) {
      offenders.push(path.relative(root, file));
    }
  }
  assert.deepEqual(offenders, []);
});

test("fixture transport handles every contract command", () => {
  assert.deepEqual(
    Object.keys(fixtureCommandHandlers).sort(),
    [...commandNames].sort(),
  );
});

test("Rust registers every contract command", async () => {
  const source = await readFile(path.join(root, "src-tauri/src/lib.rs"), "utf8");
  const handler = source.match(/generate_handler!\s*\[([\s\S]*?)\]\s*\)/);
  assert.ok(handler, "src-tauri/src/lib.rs must contain generate_handler![...]");
  const registered = new Set(
    handler[1]
      .split(",")
      .map((entry) => entry.replace(/\/\/.*$/gm, "").trim())
      .filter(Boolean)
      .map((entry) => entry.split("::").at(-1)),
  );
  const missing = commandNames.filter((name) => !registered.has(name));
  assert.deepEqual(missing, []);
});

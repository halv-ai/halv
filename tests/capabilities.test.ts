import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { commandNames } from "../src/ipc.ts";

// Tauri rejects an app command in a webview when no capability of that webview
// grants it. These tests compare the commands that each local webview invokes
// with the grants in src-tauri/capabilities.

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sourceRoot = path.join(root, "src");
const trayRoot = path.join(sourceRoot, "tray");

// createIpc in src/ipc.ts maps "tray_get_snapshot" to ipc.tray.getSnapshot.
const commandByCall = new Map<string, string>(
  commandNames.map((name) => {
    const separator = name.indexOf("_");
    const method = name
      .slice(separator + 1)
      .replace(/_([a-z])/g, (_, letter: string) => letter.toUpperCase());
    return [`${name.slice(0, separator)}.${method}`, name];
  }),
);

async function sourceFiles(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = await Promise.all(
    entries.map(async (entry) => {
      const resolved = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        return resolved === path.join(sourceRoot, "harness") ? [] : sourceFiles(resolved);
      }
      return /\.tsx?$/.test(entry.name) ? [resolved] : [];
    }),
  );
  return files.flat();
}

async function invokedCommands(files: string[]): Promise<Set<string>> {
  const invoked = new Set<string>();
  for (const file of files) {
    const source = await readFile(file, "utf8");
    for (const [, group, method] of source.matchAll(/\bipc\.(\w+)\.(\w+)\s*(?:<[^>]*>)?\(/g)) {
      if (group === "events") continue;
      const command = commandByCall.get(`${group}.${method}`);
      assert.ok(command, `${path.relative(root, file)} calls unknown ipc.${group}.${method}`);
      invoked.add(command);
    }
  }
  return invoked;
}

async function grantedCommands(capability: string): Promise<Set<string>> {
  const file = path.join(root, "src-tauri/capabilities", capability);
  const { permissions } = JSON.parse(await readFile(file, "utf8")) as { permissions: string[] };
  return new Set(
    permissions
      .filter((permission) => !permission.includes(":"))
      .map((permission) => permission.replace(/^allow-/, "").replaceAll("-", "_")),
  );
}

test("the tray capability grants exactly the commands that the tray panel invokes", async () => {
  const trayFiles = await sourceFiles(trayRoot);
  for (const file of trayFiles) {
    const source = (await readFile(file, "utf8")).replace(/^import .*$/gm, "");
    const references = source.match(/\bipc\b/g) ?? [];
    const calls = source.match(/\bipc\.\w+\.\w+\s*(?:<[^>]*>)?\(/g) ?? [];
    assert.equal(references.length, calls.length, `${path.relative(root, file)} uses ipc other than ipc.<group>.<method>()`);
  }
  const invoked = await invokedCommands(trayFiles);
  assert.deepEqual([...invoked].sort(), ["tray_action", "tray_get_snapshot", "tray_resize"]);
  assert.deepEqual([...(await grantedCommands("tray.json"))].sort(), [...invoked].sort());
});

test("the main capability grants every contract command except the tray panel commands", async () => {
  const panel = await invokedCommands(await sourceFiles(trayRoot));
  const main = await grantedCommands("default.json");
  assert.deepEqual(
    commandNames.filter((name) => !panel.has(name) && !main.has(name)),
    [],
  );
  assert.deepEqual([...panel].filter((name) => main.has(name)), []);
});

test("the main window does not invoke the tray panel commands", async () => {
  const panel = await invokedCommands(await sourceFiles(trayRoot));
  const mainFiles = (await sourceFiles(sourceRoot)).filter((file) => !file.startsWith(trayRoot + path.sep));
  const invoked = await invokedCommands(mainFiles);
  assert.deepEqual([...invoked].filter((name) => panel.has(name)), []);
});

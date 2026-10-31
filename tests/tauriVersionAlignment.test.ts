import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// `tauri build` refuses to run when an `@tauri-apps/*` npm package and its
// Rust crate differ in major.minor ("Found version mismatched Tauri
// packages"). Dependabot bumps npm and cargo in separate PRs, so this guard
// makes `Test` fail when only one side moved.

const root = new URL("../", import.meta.url);

function npmVersions(): Map<string, string> {
  const lock = readFileSync(new URL("pnpm-lock.yaml", root), "utf8");
  const importer = lock.split(/^importers:\n/m)[1]?.split(/^packages:\n/m)[0];
  assert.ok(importer, "pnpm-lock.yaml has no importers section");
  const versions = new Map<string, string>();
  const entry =
    /^ {6}'(@tauri-apps\/[a-z0-9-]+)':\n {8}specifier: [^\n]+\n {8}version: ([0-9][^\s(]*)/gm;
  for (const [, name, version] of importer.matchAll(entry)) {
    versions.set(name, version);
  }
  return versions;
}

function crateVersions(): Map<string, string[]> {
  const lock = readFileSync(new URL("src-tauri/Cargo.lock", root), "utf8");
  const versions = new Map<string, string[]>();
  for (const [, name, version] of lock.matchAll(
    /^name = "([^"]+)"\nversion = "([^"]+)"/gm,
  )) {
    versions.set(name, [...(versions.get(name) ?? []), version]);
  }
  return versions;
}

// Same mapping tauri-cli uses: `@tauri-apps/api` pairs with `tauri`,
// `@tauri-apps/plugin-<x>` pairs with `tauri-plugin-<x>`. The CLI and its
// platform binaries have no runtime crate.
function crateFor(npmName: string): string | undefined {
  const short = npmName.slice("@tauri-apps/".length);
  if (short === "api") return "tauri";
  if (short.startsWith("plugin-")) return `tauri-${short}`;
  return undefined;
}

const majorMinor = (version: string) => version.split(".").slice(0, 2).join(".");

test("every @tauri-apps npm package matches its crate's major.minor", () => {
  const npm = npmVersions();
  const crates = crateVersions();
  const pairs = [...npm].filter(([name]) => crateFor(name));
  assert.ok(npm.has("@tauri-apps/api"), "expected @tauri-apps/api in pnpm-lock.yaml");
  assert.ok(pairs.length >= 2, "expected @tauri-apps plugins in pnpm-lock.yaml");

  const mismatches: string[] = [];
  for (const [name, version] of pairs) {
    const crate = crateFor(name)!;
    const crateVersionsFound = crates.get(crate);
    if (!crateVersionsFound) {
      mismatches.push(`${name} ${version}: crate ${crate} missing from Cargo.lock`);
      continue;
    }
    if (!crateVersionsFound.some((v) => majorMinor(v) === majorMinor(version))) {
      mismatches.push(
        `${name} ${version} vs ${crate} ${crateVersionsFound.join(", ")}`,
      );
    }
  }
  assert.deepEqual(mismatches, [], "align the npm package and the crate in the same PR");
});

#!/usr/bin/env node

// Collects the license texts of the third-party Rust crates and npm packages
// that the Halv app contains. The script writes
// src-tauri/resources/licenses/THIRD_PARTY_LICENSES.md. tauri.conf.json copies
// that file into the licenses/ folder of each bundle.
//
// Scope:
// - The Rust crates are the crates of the Halv app (src-tauri) and of the
//   halv-supervisor sidecar (src-tauri/supervisor).
// - The npm packages are the packages that the frontend contains.
// - The installers also contain the RTK and Crux binaries, which
//   bundle.externalBin in tauri.conf.json lists. Each binary links its own
//   Rust crates. This file does not list those crates, and the file header
//   says so. THIRD_PARTY_NOTICES.md names RTK and Crux.
//
// Method:
// - Rust: for each Cargo workspace that builds shipped code
//   (CARGO_WORKSPACES), the script runs
//   `cargo tree --locked -e normal,build --target all`. That command resolves
//   features like a release build, follows only normal and build edges, and
//   includes each platform target. Thus the file holds the crates of each
//   installer, but no crate that only a dev-dependency or a test needs.
//   `cargo metadata --locked` gives the data of each package. The script
//   stops when cargo tree names a crate that the normal and build edges of
//   the cargo metadata graph do not reach.
// - npm: the script runs `pnpm list --prod --json --depth Infinity`. Vite
//   bundles these packages into dist/. The script also adds the build
//   packages whose code the build can copy into dist/
//   (BUILD_INJECTED_NPM_PACKAGES).
// - For each package, the script copies each license and notice file from the
//   package folder. It also copies the license files of vendored code in
//   subfolders, to NESTED_LICENSE_DEPTH folders deep. EXTRA_LICENSE_TEXTS adds
//   license texts that the scan cannot find, for example the license of a
//   static library that a crate links.
// - If a package has no license file of its own, the script uses the standard
//   SPDX text from scripts/license-texts/ for the license expression. For an
//   OR expression, it prefers a text without a copyright line template. It
//   writes the package authors into a template copyright line.
// - A package that has no license text must have an entry with a reason in
//   scripts/third-party-license-exceptions.json. Otherwise the script fails.
// - Each license text appears one time. Each package entry names its texts.
//
// Usage:
//   node scripts/collect-third-party-licenses.mjs          Write the file.
//   node scripts/collect-third-party-licenses.mjs --check  Exit 1 if the
//                                                          committed file is
//                                                          not current.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  readFileSync,
  readdirSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

export const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const OUTPUT_PATH = join(
  APP_ROOT,
  "src-tauri",
  "resources",
  "licenses",
  "THIRD_PARTY_LICENSES.md",
);
export const EXCEPTIONS_PATH = join(
  APP_ROOT,
  "scripts",
  "third-party-license-exceptions.json",
);
export const LICENSE_TEXTS_DIR = join(APP_ROOT, "scripts", "license-texts");

// The Cargo workspaces that build code that the installers contain. The
// supervisor sidecar has its own lock file, so the script reads it too. The
// app builds crates/halv-remote as a path dependency with the src-tauri lock
// file, so the src-tauri graph already holds the halv-remote crates. The own
// lock file of crates/halv-remote does not change what the installers contain.
export const CARGO_WORKSPACES = [
  { manifest: "src-tauri/Cargo.toml" },
  { manifest: "src-tauri/supervisor/Cargo.toml" },
];

// Build packages whose code the build can copy into dist/. A "parent" is the
// devDependency that depends on the package.
export const BUILD_INJECTED_NPM_PACKAGES = {
  esbuild: {
    parent: "vite",
    note: "Vite minifies the app JavaScript with esbuild, which can add esbuild helper functions.",
  },
  rollup: {
    parent: "vite",
    note: "Vite bundles the app JavaScript with Rollup, which can add Rollup helper code.",
  },
  tailwindcss: { note: "The build copies the Tailwind base styles into the app CSS." },
  vite: { note: "The build copies the Vite module preload code into the app JavaScript." },
};

const PUBLISHED_WITHOUT_LICENSE =
  "The published package has no license file. The license text comes from its source repository.";
const OBJC2_TEXTS = [
  { vendored: "packages/objc2/LICENSE-MIT.txt", role: "license" },
  { vendored: "packages/objc2/LICENSE.md", role: "license" },
];
const WEBVIEW2_RS_TEXT = { vendored: "packages/webview2-rs/LICENSE", role: "license" };

// License texts that the folder scan does not find. Each key is
// "<ecosystem>:<package name>". "versions", when present, names the only
// versions that the texts are correct for. The "note" goes into the package
// entry. A "file" path is relative to the package folder. A "vendored" path is
// relative to scripts/license-texts/, and scripts/license-texts/README.md
// gives its source. A text with the role "license" is the license of the
// package itself. A text with the role "component" is the license of code
// that the package contains or links. The script fails when a text file does
// not exist or when no package matches an entry.
export const EXTRA_LICENSE_TEXTS = {
  "cargo:alloc-stdlib": {
    note: PUBLISHED_WITHOUT_LICENSE,
    texts: [{ vendored: "packages/rust-alloc-no-stdlib/LICENSE", role: "license" }],
  },
  "cargo:block2": { note: PUBLISHED_WITHOUT_LICENSE, texts: OBJC2_TEXTS },
  "cargo:dlopen2": {
    note: PUBLISHED_WITHOUT_LICENSE,
    texts: [{ vendored: "packages/dlopen2/LICENSE", role: "license" }],
  },
  "cargo:dlopen2_derive": {
    note: PUBLISHED_WITHOUT_LICENSE,
    texts: [{ vendored: "packages/dlopen2/LICENSE", role: "license" }],
  },
  "cargo:libdbus-sys": {
    note:
      "The \"vendored\" feature of keyring compiles the libdbus C library from " +
      "vendor/dbus into the Linux app.",
    texts: [{ file: "vendor/dbus/COPYING", role: "component" }],
  },
  "cargo:objc2": { note: PUBLISHED_WITHOUT_LICENSE, texts: OBJC2_TEXTS },
  "cargo:objc2-encode": { note: PUBLISHED_WITHOUT_LICENSE, texts: OBJC2_TEXTS },
  "cargo:objc2-foundation": { note: PUBLISHED_WITHOUT_LICENSE, texts: OBJC2_TEXTS },
  "cargo:regex-syntax": {
    note: "The crate contains Unicode data tables.",
    texts: [{ file: "src/unicode_tables/LICENSE-UNICODE", role: "component" }],
  },
  "cargo:ring": {
    note: "The crate contains code from BoringSSL, once_cell, and fiat-crypto.",
    texts: [
      { file: "src/polyfill/once_cell/LICENSE-APACHE", role: "component" },
      { file: "src/polyfill/once_cell/LICENSE-MIT", role: "component" },
      { file: "third_party/fiat/LICENSE", role: "component" },
    ],
  },
  "cargo:tracing-core": {
    note: "The crate contains a copy of the spin crate.",
    texts: [{ file: "src/spin/LICENSE", role: "component" }],
  },
  "cargo:webview2-com": { note: PUBLISHED_WITHOUT_LICENSE, texts: [WEBVIEW2_RS_TEXT] },
  "cargo:webview2-com-macros": { note: PUBLISHED_WITHOUT_LICENSE, texts: [WEBVIEW2_RS_TEXT] },
  "cargo:webview2-com-sys": {
    versions: ["0.38.2"],
    note:
      `${PUBLISHED_WITHOUT_LICENSE} On Windows with MSVC, the crate links ` +
      "WebView2LoaderStatic.lib from the Microsoft WebView2 SDK 1.0.3650.58 " +
      "into the app.",
    texts: [
      WEBVIEW2_RS_TEXT,
      { vendored: "packages/webview2-sdk/LICENSE.txt", role: "component" },
      { vendored: "packages/webview2-sdk/NOTICE.txt", role: "component" },
    ],
  },
};

// The copyright line of each standard text that has a template for the
// copyright holder. The script writes the package authors into that line.
export const COPYRIGHT_TEMPLATE_LINES = {
  "0BSD": "Copyright (C) YEAR by AUTHOR EMAIL",
  "BSD-2-Clause": "Copyright (c) <year> <owner>",
  "BSD-3-Clause": "Copyright (c) <year> <owner>.",
  ISC: "Copyright <year> <owner>",
  MIT: "Copyright (c) <year> <copyright holders>",
};
const COPYRIGHT_TEMPLATE_PATTERN = /<year>|<owner>|<copyright holders?>/iu;

// The folder scan looks for license files of vendored code to this many
// folders below the package folder.
export const NESTED_LICENSE_DEPTH = 4;
const LICENSE_FILE_PATTERN =
  /^(licen[cs]e|copying|copyright|notice|unlicense|third[-_ ]?party)/iu;
const LICENSE_DIRECTORY_PATTERN = /^licen[cs]es$/iu;
// A file with such a name gives notices or SPDX metadata, not the license of
// the package. For example, security-framework has a THIRD_PARTY file with the
// license of the Apple documentation that the crate adapts.
const NOTICE_FILE_PATTERN = /^(copyright|notice|third[-_ ]?party)|\.spdx$/iu;
// A file with such an extension holds code or data, not a license text. For
// example, objc2-foundation has src/copying.rs.
const CODE_FILE_PATTERN =
  /\.(c|cc|cjs|cpp|css|cts|go|h|hpp|java|js|json|jsx|map|mjs|mts|py|rs|sh|svg|toml|ts|tsx|wasm|ya?ml)$/iu;
// The folder scan does not go into such folders. They hold tests, examples,
// and other code that the installers do not contain.
const SKIPPED_DIRECTORY_PATTERN =
  /^\.|^(bench|benches|example|examples|fuzz|node_modules|target|test|test-data|testdata|tests)$/iu;
const SPDX_OPERATORS = new Set(["AND", "OR", "WITH"]);
const ECOSYSTEMS = ["cargo", "npm"];
const SECTION_TITLES = { cargo: "Rust crates", npm: "npm packages" };

// Compares two strings by code point, so the order does not depend on locale.
function compareText(left, right) {
  if (left < right) {
    return -1;
  }
  return left > right ? 1 : 0;
}

function compareVersions(left, right) {
  const leftParts = left.split(/[.+-]/u);
  const rightParts = right.split(/[.+-]/u);
  for (let index = 0; index < Math.max(leftParts.length, rightParts.length); index += 1) {
    const a = leftParts[index] ?? "";
    const b = rightParts[index] ?? "";
    if (/^\d+$/u.test(a) && /^\d+$/u.test(b)) {
      const difference = Number(a) - Number(b);
      if (difference !== 0) {
        return difference;
      }
    } else if (a !== b) {
      return compareText(a, b);
    }
  }
  return 0;
}

function isFile(path) {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

// Removes a byte order mark, line ending differences, and trailing spaces, so
// that equal texts get equal hashes.
export function normalizeText(text) {
  return text
    .replace(/^﻿/u, "")
    .replace(/\r\n?/gu, "\n")
    .split("\n")
    .map((line) => line.replace(/[ \t]+$/u, ""))
    .join("\n")
    .replace(/^\n+/u, "")
    .replace(/\n+$/u, "");
}

function textId(text) {
  return `t-${createHash("sha256").update(text).digest("hex").slice(0, 12)}`;
}

// Parses an SPDX license expression into a tree. Cargo also accepts "/" as OR.
export function parseSpdxExpression(expression) {
  const tokens = expression
    .replace(/\//gu, " OR ")
    .replace(/([()])/gu, " $1 ")
    .trim()
    .split(/\s+/u)
    .filter(Boolean);
  let index = 0;
  const fail = () => {
    throw new Error(`invalid SPDX license expression: ${JSON.stringify(expression)}`);
  };
  const peekOperator = () => tokens[index]?.toUpperCase();
  const parseAtom = () => {
    const token = tokens[index];
    index += 1;
    if (token === "(") {
      const inner = parseOr();
      if (tokens[index] !== ")") {
        fail();
      }
      index += 1;
      return inner;
    }
    if (!token || token === ")" || SPDX_OPERATORS.has(token.toUpperCase())) {
      fail();
    }
    return { id: token };
  };
  const parseWith = () => {
    const license = parseAtom();
    if (peekOperator() !== "WITH") {
      return license;
    }
    index += 1;
    const exception = tokens[index];
    index += 1;
    if (!exception || exception === "(" || exception === ")") {
      fail();
    }
    return { op: "WITH", license, exception };
  };
  const parseAnd = () => {
    const terms = [parseWith()];
    while (peekOperator() === "AND") {
      index += 1;
      terms.push(parseWith());
    }
    return terms.length === 1 ? terms[0] : { op: "AND", terms };
  };
  function parseOr() {
    const terms = [parseAnd()];
    while (peekOperator() === "OR") {
      index += 1;
      terms.push(parseAnd());
    }
    return terms.length === 1 ? terms[0] : { op: "OR", terms };
  }
  if (tokens.length === 0) {
    fail();
  }
  const tree = parseOr();
  if (index !== tokens.length) {
    fail();
  }
  return tree;
}

// Returns the license identifiers that satisfy the expression with the
// available standard texts, or null. AND needs each term. For OR, the first
// alternative in the written order whose texts have no copyright line
// template ("templated") wins. If each alternative has such a text, the first
// alternative with texts wins.
export function selectStandardLicenses(tree, available, templated = new Set()) {
  if (tree.id) {
    return available.has(tree.id) ? [tree.id] : null;
  }
  if (tree.op === "WITH") {
    const license = selectStandardLicenses(tree.license, available, templated);
    return license && available.has(tree.exception)
      ? [...license, tree.exception]
      : null;
  }
  if (tree.op === "AND") {
    const selected = [];
    for (const term of tree.terms) {
      const ids = selectStandardLicenses(term, available, templated);
      if (!ids) {
        return null;
      }
      selected.push(...ids);
    }
    return [...new Set(selected)];
  }
  const choices = tree.terms
    .map((term) => selectStandardLicenses(term, available, templated))
    .filter(Boolean);
  return choices.find((ids) => !ids.some((id) => templated.has(id))) ?? choices[0] ?? null;
}

// Reads scripts/license-texts/<SPDX id>.txt into a map from id to text. Each
// text with a copyright line template must have its line in
// COPYRIGHT_TEMPLATE_LINES, so that the script can fill it in.
export function loadLicenseTexts(directory = LICENSE_TEXTS_DIR) {
  const texts = new Map();
  for (const name of readdirSync(directory).sort(compareText)) {
    if (!name.endsWith(".txt") || !isFile(join(directory, name))) {
      continue;
    }
    const id = name.slice(0, -".txt".length);
    const text = normalizeText(readFileSync(join(directory, name), "utf8"));
    if (!text) {
      throw new Error(`standard license text is empty: ${join(directory, name)}`);
    }
    const template = COPYRIGHT_TEMPLATE_LINES[id];
    if (template && !text.split("\n").includes(template)) {
      throw new Error(`standard ${id} text has no line "${template}": ${join(directory, name)}`);
    }
    if (!template && COPYRIGHT_TEMPLATE_PATTERN.test(text)) {
      throw new Error(
        `standard ${id} text has a copyright line template. Add the line to ` +
          "COPYRIGHT_TEMPLATE_LINES.",
      );
    }
    texts.set(id, text);
  }
  return texts;
}

function isLicenseFileName(name) {
  return LICENSE_FILE_PATTERN.test(name) && !CODE_FILE_PATTERN.test(name);
}

// Finds the license and notice files of a package folder. Each result has a
// role:
// - "license": a license file in the package folder or in its LICENSES
//   folder, or the license file that the package metadata declares.
// - "notice": a NOTICE, COPYRIGHT, THIRD_PARTY, or SPDX metadata file in the
//   package folder.
// - "component": a license file in a subfolder, to NESTED_LICENSE_DEPTH
//   folders deep. Such a file usually gives the license of vendored code.
export function findLicenseFiles(directory, declaredFiles = []) {
  const found = new Map();
  const add = (path, role) => {
    if (!found.has(path)) {
      found.set(path, { name: relative(directory, path).split(sep).join("/"), role });
    }
  };
  const scan = (folder, depth, inLicenseFolder) => {
    const entries = readdirSync(folder, { withFileTypes: true }).sort((left, right) =>
      compareText(left.name, right.name),
    );
    for (const entry of entries) {
      const path = join(folder, entry.name);
      if (entry.isDirectory()) {
        if (LICENSE_DIRECTORY_PATTERN.test(entry.name) && depth < NESTED_LICENSE_DEPTH) {
          scan(path, depth + 1, true);
        } else if (!SKIPPED_DIRECTORY_PATTERN.test(entry.name) && depth < NESTED_LICENSE_DEPTH) {
          scan(path, depth + 1, false);
        }
        continue;
      }
      const licenseFile = inLicenseFolder
        ? !CODE_FILE_PATTERN.test(entry.name)
        : isLicenseFileName(entry.name);
      if (!licenseFile || !isFile(path)) {
        continue;
      }
      if (depth === 0) {
        add(path, NOTICE_FILE_PATTERN.test(entry.name) ? "notice" : "license");
      } else if (depth === 1 && inLicenseFolder) {
        add(path, "license");
      } else {
        add(path, "component");
      }
    }
  };
  scan(directory, 0, false);
  for (const declared of declaredFiles) {
    const path = resolve(directory, declared);
    if (isFile(path)) {
      found.set(path, { name: relative(directory, path).split(sep).join("/"), role: "license" });
    }
  }
  return [...found.entries()]
    .map(([path, { name, role }]) => ({
      name,
      role,
      text: normalizeText(readFileSync(path, "utf8")),
    }))
    .filter(({ text }) => text.length > 0)
    .sort((left, right) => compareText(left.name, right.name));
}

// Removes the e-mail part from "Name <e-mail>" author strings.
function authorName(author) {
  const name = typeof author === "string" ? author : author?.name;
  return `${name ?? ""}`.replace(/\s*<[^>]*>/gu, "").replace(/\s*\([^)]*\)\s*$/u, "").trim();
}

// Parses the output of `cargo tree --prefix none --format {p}` into a set of
// "<name>@<version>" keys. Each line is "<name> v<version>", optionally
// followed by the source, "(proc-macro)", or "(*)" for a repeated package.
export function parseCargoTree(output) {
  const keys = new Set();
  for (const line of output.split(/\r?\n/u)) {
    if (!line.trim()) {
      continue;
    }
    // A Windows path in the source can hold parentheses, for example
    // "C:\Program Files (x86)\src". The suffix pattern thus takes each
    // character up to the last ")".
    const match = /^([A-Za-z0-9_-]+) v(\d\S*)(?: \(.*\))?$/u.exec(line.trim());
    if (!match) {
      throw new Error(`unexpected cargo tree line: ${JSON.stringify(line)}`);
    }
    keys.add(`${match[1]}@${match[2]}`);
  }
  if (keys.size === 0) {
    throw new Error("cargo tree printed no packages");
  }
  return keys;
}

// Lists each non-local package that cargo tree names. Each such package must
// be one that a workspace member reaches through normal or build dependency
// edges in the cargo metadata resolve graph.
export function collectCargoPackages(metadata, treeOutput) {
  if (!metadata?.resolve?.nodes) {
    throw new Error("cargo metadata output has no resolve graph");
  }
  const treeKeys = parseCargoTree(treeOutput);
  const packages = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
  const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const reached = new Set();
  const pending = [...metadata.workspace_members];
  while (pending.length > 0) {
    const id = pending.pop();
    if (reached.has(id)) {
      continue;
    }
    reached.add(id);
    const node = nodes.get(id);
    if (!node) {
      throw new Error(`cargo metadata resolve graph has no node for ${id}`);
    }
    for (const dependency of node.deps) {
      // A dep_kinds kind is null for a normal edge, "build", or "dev".
      if (dependency.dep_kinds.some(({ kind }) => kind !== "dev")) {
        pending.push(dependency.pkg);
      }
    }
  }
  const reachedPackages = [...reached].map((id) => packages.get(id)).filter(Boolean);
  const reachedKeys = new Set(reachedPackages.map(({ name, version }) => `${name}@${version}`));
  for (const key of [...treeKeys].sort(compareText)) {
    if (!reachedKeys.has(key)) {
      throw new Error(
        `cargo tree names ${key}, but the normal and build edges of the cargo ` +
          "metadata graph do not reach it",
      );
    }
  }
  return reachedPackages
    .filter((pkg) => pkg.source !== null && treeKeys.has(`${pkg.name}@${pkg.version}`))
    .map((pkg) => ({
      ecosystem: "cargo",
      name: pkg.name,
      version: pkg.version,
      license: pkg.license ? pkg.license.replace(/\s*\/\s*/gu, " OR ") : null,
      licenseFile: pkg.license_file ?? null,
      directory: dirname(pkg.manifest_path),
      url:
        pkg.repository ||
        pkg.homepage ||
        `https://crates.io/crates/${pkg.name}/${pkg.version}`,
      authors: (pkg.authors ?? []).map(authorName).filter(Boolean),
      note: null,
    }));
}

export function npmRepositoryUrl(manifest) {
  const repository =
    typeof manifest.repository === "string" ? manifest.repository : manifest.repository?.url;
  if (!repository) {
    return manifest.homepage || `https://www.npmjs.com/package/${manifest.name}/v/${manifest.version}`;
  }
  let url = repository.trim();
  if (/^[\w.-]+\/[\w.-]+$/u.test(url)) {
    url = `https://github.com/${url}`;
  }
  return url
    .replace(/^github:/u, "https://github.com/")
    .replace(/^gitlab:/u, "https://gitlab.com/")
    .replace(/^bitbucket:/u, "https://bitbucket.org/")
    .replace(/^git\+/u, "")
    .replace(/^git:\/\//u, "https://")
    .replace(/^ssh:\/\/git@/u, "https://")
    .replace(/^git@([^:/]+):/u, "https://$1/")
    .replace(/\.git(#.*)?$/u, "$1");
}

// Gives the license expression and the declared license file of an npm
// package manifest.
function npmLicense(manifest) {
  let license = manifest.license;
  if (license && typeof license === "object") {
    license = license.type;
  }
  if (!license && Array.isArray(manifest.licenses)) {
    const types = manifest.licenses
      .map((entry) => (typeof entry === "string" ? entry : entry?.type))
      .filter(Boolean);
    license = types.length > 1 ? `(${types.join(" OR ")})` : types[0];
  }
  if (typeof license !== "string" || !license.trim()) {
    return { license: null, licenseFile: null };
  }
  const seeFile = /^SEE LICEN[CS]E IN (.+)$/iu.exec(license.trim());
  return seeFile
    ? { license: license.trim(), licenseFile: seeFile[1].trim() }
    : { license: license.trim(), licenseFile: null };
}

function npmPackage(directory, note) {
  const manifestPath = join(directory, "package.json");
  if (!isFile(manifestPath)) {
    throw new Error(`npm package has no package.json: ${directory}. Run pnpm install.`);
  }
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  const { license, licenseFile } = npmLicense(manifest);
  const authors = [manifest.author, ...(manifest.contributors ?? [])]
    .map(authorName)
    .filter(Boolean);
  return {
    ecosystem: "npm",
    name: manifest.name,
    version: manifest.version,
    license,
    licenseFile,
    directory,
    url: npmRepositoryUrl(manifest),
    authors,
    note,
  };
}

// Lists each package in the `pnpm list --prod --json --depth Infinity` tree,
// plus each build injected package from `pnpm list --dev --json --depth 1`.
export function collectNpmPackages(
  pnpmProduction,
  pnpmDevelopment,
  buildInjected = BUILD_INJECTED_NPM_PACKAGES,
) {
  const directories = new Map();
  const visit = (dependencies) => {
    for (const [name, info] of Object.entries(dependencies ?? {})) {
      if (!info?.version || /^(link|file|workspace):/u.test(info.version)) {
        continue;
      }
      const key = `${name}@${info.version}`;
      if (!directories.has(key)) {
        if (!info.path) {
          throw new Error(`pnpm list gave no path for ${key}. Run pnpm install.`);
        }
        directories.set(key, info.path);
      }
      // pnpm prints a repeated subtree one time and marks the others as
      // "deduped", so the script visits each subtree that it prints.
      visit(info.dependencies);
      visit(info.optionalDependencies);
    }
  };
  for (const importer of pnpmProduction) {
    visit(importer.dependencies);
    visit(importer.optionalDependencies);
  }
  const packages = [...directories.values()].map((directory) => npmPackage(directory, null));
  for (const [name, { parent, note }] of Object.entries(buildInjected)) {
    const info = pnpmDevelopment
      .map((importer) =>
        parent
          ? importer.devDependencies?.[parent]?.dependencies?.[name]
          : importer.devDependencies?.[name],
      )
      .find(Boolean);
    if (!info?.path) {
      const label = parent ? `${parent} > ${name}` : name;
      throw new Error(`pnpm list --dev gave no path for ${label}. Run pnpm install.`);
    }
    packages.push(npmPackage(info.path, note));
  }
  return packages;
}

// Reads the texts of the EXTRA_LICENSE_TEXTS entry of a package.
function readExtraTexts(pkg, extra, licenseTextsDir) {
  return extra.texts.map(({ file, vendored, role }) => {
    const path = file ? resolve(pkg.directory, file) : resolve(licenseTextsDir, vendored);
    if (!isFile(path)) {
      throw new Error(
        `${packageKey(pkg)}: the extra license text ${file ?? vendored} does not exist: ${path}`,
      );
    }
    const text = normalizeText(readFileSync(path, "utf8"));
    if (!text) {
      throw new Error(`${packageKey(pkg)}: the extra license text is empty: ${path}`);
    }
    return {
      label: file ? `\`${file}\`` : `copy of the upstream text: \`${vendored}\``,
      text,
      role,
    };
  });
}

// Gives a standard text. For a text with a copyright line template, it writes
// the package authors into that line.
function standardText(id, text, authors) {
  const template = COPYRIGHT_TEMPLATE_LINES[id];
  if (!template) {
    return { label: `standard SPDX text: ${id}`, text, role: "license", standard: id };
  }
  const holders = `Copyright (c) ${authors.join(", ")}`;
  return {
    label: `standard SPDX text: ${id}, copyright line from the package authors`,
    text: text
      .split("\n")
      .map((line) => (line === template ? holders : line))
      .join("\n"),
    role: "license",
    standard: id,
    holders: true,
  };
}

// Finds the license texts of one package. The package license comes from its
// own license files, from an EXTRA_LICENSE_TEXTS text with the role "license",
// or else from the standard SPDX texts for its license expression. Notice,
// SPDX metadata, and component texts do not give the package license, but the
// entry also names them. Result kinds:
// - "files": the package has a license text of its own.
// - "standard": the entry uses standard texts.
// - "no-holder": a standard text needs a copyright holder, but the package
//   metadata names no authors.
// - "missing": no license text.
export function resolvePackageTexts(
  pkg,
  licenseTexts,
  extra = null,
  licenseTextsDir = LICENSE_TEXTS_DIR,
) {
  const extras = extra ? readExtraTexts(pkg, extra, licenseTextsDir) : [];
  const extraLabels = new Set(extras.map(({ label }) => label));
  const found = findLicenseFiles(pkg.directory, pkg.licenseFile ? [pkg.licenseFile] : [])
    .map(({ name, role, text }) => ({ label: `\`${name}\``, text, role }))
    .filter(({ label }) => !extraLabels.has(label));
  const texts = [...found, ...extras];
  if (texts.some(({ role }) => role === "license")) {
    return { kind: "files", texts };
  }
  if (pkg.license) {
    let ids = null;
    try {
      ids = selectStandardLicenses(
        parseSpdxExpression(pkg.license),
        new Set(licenseTexts.keys()),
        new Set(Object.keys(COPYRIGHT_TEMPLATE_LINES)),
      );
    } catch {
      ids = null;
    }
    if (ids) {
      const templated = ids.filter((id) => COPYRIGHT_TEMPLATE_LINES[id]);
      if (templated.length > 0 && pkg.authors.length === 0) {
        return { kind: "no-holder", texts, templated };
      }
      return {
        kind: "standard",
        texts: [
          ...texts,
          ...ids.map((id) => standardText(id, licenseTexts.get(id), pkg.authors)),
        ],
      };
    }
  }
  return { kind: "missing", texts };
}

function packageKey({ ecosystem, name, version }) {
  return `${ecosystem}:${name}@${version}`;
}

function validateExceptions(exceptions) {
  const errors = [];
  const byKey = new Map();
  if (!Array.isArray(exceptions)) {
    return { errors: ["the exceptions file needs an \"exceptions\" array"], byKey };
  }
  for (const [index, exception] of exceptions.entries()) {
    const { ecosystem, name, version, reason } = exception ?? {};
    if (
      !ECOSYSTEMS.includes(ecosystem) ||
      typeof name !== "string" ||
      !name ||
      typeof version !== "string" ||
      !version ||
      typeof reason !== "string" ||
      !reason.trim()
    ) {
      errors.push(
        `exception ${index} needs "ecosystem" (cargo or npm), "name", "version", and a "reason"`,
      );
      continue;
    }
    byKey.set(packageKey({ ecosystem, name, version }), reason.trim());
  }
  return { errors, byKey };
}

// Checks the shape of the EXTRA_LICENSE_TEXTS entries.
function validateExtraTexts(extraTexts) {
  const errors = [];
  for (const [key, extra] of Object.entries(extraTexts)) {
    const [ecosystem, ...nameParts] = key.split(":");
    const valid =
      ECOSYSTEMS.includes(ecosystem) &&
      nameParts.join(":").length > 0 &&
      typeof extra?.note === "string" &&
      extra.note.trim().length > 0 &&
      (extra.versions === undefined ||
        (Array.isArray(extra.versions) &&
          extra.versions.length > 0 &&
          extra.versions.every((version) => typeof version === "string" && version))) &&
      Array.isArray(extra.texts) &&
      extra.texts.length > 0 &&
      extra.texts.every(
        (text) =>
          ["license", "component"].includes(text?.role) &&
          (typeof text.file === "string") !== (typeof text.vendored === "string") &&
          (text.file ?? text.vendored).length > 0,
      );
    if (!valid) {
      errors.push(
        `extra license texts ${key} need a "<ecosystem>:<name>" key, a "note", optional ` +
          '"versions", and "texts" with a "file" or "vendored" path and a role',
      );
    }
  }
  return errors;
}

function fenceFor(text) {
  const longest = Math.max(0, ...(text.match(/`+/gu) ?? []).map((run) => run.length));
  return "`".repeat(Math.max(3, longest + 1));
}

function renderEntry(entry) {
  const lines = [`### ${entry.name} ${entry.version}`, ""];
  lines.push(`- License: ${entry.license ?? "not declared"}`);
  lines.push(`- Source: ${entry.url}`);
  for (const note of entry.notes) {
    lines.push(`- Note: ${note}`);
  }
  // Two files of a package can hold the same text. The entry names that text
  // one time, with each file label.
  const labelsById = new Map();
  for (const { label, text } of entry.resolution.texts) {
    const id = textId(text);
    labelsById.set(id, [...(labelsById.get(id) ?? []), label]);
  }
  const references = [...labelsById].map(
    ([id, labels]) => `[${id}](#${id}) (${labels.join(", ")})`,
  );
  if (entry.resolution.kind === "missing" && references.length === 0) {
    lines.push(`- License texts: none. Reason: ${entry.exception}`);
  } else if (entry.resolution.kind === "missing") {
    lines.push(
      `- License texts: ${references.join(", ")}. None of them is the license of the ` +
        `package. Reason: ${entry.exception}`,
    );
  } else {
    lines.push(`- License texts: ${references.join(", ")}`);
  }
  if (entry.resolution.kind === "standard" && entry.authors.length > 0) {
    lines.push(`- Authors: ${entry.authors.join(", ")}`);
  }
  lines.push("");
  return lines;
}

// Gives each different license text of the entries one time, by text id.
function collectTexts(entries) {
  const texts = new Map();
  for (const entry of entries) {
    for (const { text, standard, holders } of entry.resolution.texts) {
      const id = textId(text);
      if (!texts.has(id)) {
        texts.set(id, { text, standard: standard ?? null, holders: holders === true });
      }
    }
  }
  return texts;
}

export function renderLicenseFile(entries, counts) {
  const texts = collectTexts(entries);
  const lines = [
    "# Third-party licenses",
    "",
    "<!-- scripts/collect-third-party-licenses.mjs makes this file. Do not edit it. -->",
    "",
    "This file lists each third-party Rust crate and npm package that the Halv",
    "app contains. The Rust crates are the crates of the Halv app (`src-tauri`)",
    "and of its `halv-supervisor` sidecar (`src-tauri/supervisor`). The npm",
    "packages are the packages that the app frontend contains. The file can",
    "list packages that one installer does not contain, for example the crates",
    "of other platforms. Each entry gives the package name, version, SPDX",
    "license expression, and source URL. Each entry also names its license",
    "texts. Each license text appears one time, in the \"License texts\" part.",
    "",
    "The installers also contain the RTK and Crux binaries. Each binary",
    "contains its own Rust crates. This file does not list those crates.",
    "`THIRD_PARTY_NOTICES.md` in the same folder names RTK and Crux. It also",
    "tells where to find their crates.",
    "",
    "Most entries use the license files of the package. Some entries also name",
    "the license files of code that the package contains or links. An entry",
    "with a \"standard SPDX text\" comes from a package that has no license",
    "text file. For that entry, this file uses the standard text of the license",
    "that the package declares. In a standard MIT, ISC, BSD, or 0BSD text, the",
    "copyright line names the package authors from the package metadata.",
    "",
    "The Headroom runtime keeps the license files of its Python packages in",
    "its `licenses/python` folder. `THIRD_PARTY_NOTICES.md` lists the other",
    "components.",
    "",
    "To make this file again, run `pnpm licenses:collect`.",
    "",
    `- Rust crates: ${counts.cargo}`,
    `- npm packages: ${counts.npm}`,
    `- Entries with a standard SPDX text: ${counts.standard}`,
    `- Entries without license text (exceptions): ${counts.exceptions}`,
    `- License texts: ${counts.texts}`,
    "",
  ];
  for (const ecosystem of ECOSYSTEMS) {
    lines.push(`## ${SECTION_TITLES[ecosystem]}`, "");
    for (const entry of entries.filter((item) => item.ecosystem === ecosystem)) {
      lines.push(...renderEntry(entry));
    }
  }
  lines.push("## License texts", "");
  for (const id of [...texts.keys()].sort(compareText)) {
    const { text, standard, holders } = texts.get(id);
    const fence = fenceFor(text);
    lines.push(`### ${id}`, "");
    if (standard && holders) {
      lines.push(
        `Standard SPDX text: ${standard}. The copyright line names the package authors.`,
        "",
      );
    } else if (standard) {
      lines.push(`Standard SPDX text: ${standard}`, "");
    }
    lines.push(`${fence}text`, text, fence, "");
  }
  return `${lines.join("\n").replace(/\n+$/u, "")}\n`;
}

// Builds the license file from the command outputs. It returns the Markdown,
// the counts, and each error that must stop the build.
export function buildLicenseReport({
  cargo,
  pnpmProduction,
  pnpmDevelopment,
  exceptions,
  licenseTexts,
  buildInjected = BUILD_INJECTED_NPM_PACKAGES,
  extraTexts = EXTRA_LICENSE_TEXTS,
  licenseTextsDir = LICENSE_TEXTS_DIR,
}) {
  const extraErrors = validateExtraTexts(extraTexts);
  if (extraErrors.length > 0) {
    throw new Error(extraErrors.join("\n"));
  }
  const unique = new Map();
  const packages = [
    ...cargo.flatMap(({ metadata, tree }) => collectCargoPackages(metadata, tree)),
    ...collectNpmPackages(pnpmProduction, pnpmDevelopment, buildInjected),
  ];
  for (const pkg of packages) {
    if (!unique.has(packageKey(pkg))) {
      unique.set(packageKey(pkg), pkg);
    }
  }
  const { errors, byKey } = validateExceptions(exceptions);
  const usedExtras = new Set();
  const entries = [...unique.values()]
    .sort(
      (left, right) =>
        compareText(left.ecosystem, right.ecosystem) ||
        compareText(left.name, right.name) ||
        compareVersions(left.version, right.version) ||
        compareText(left.version, right.version),
    )
    .map((pkg) => {
      const extraKey = `${pkg.ecosystem}:${pkg.name}`;
      let extra = Object.hasOwn(extraTexts, extraKey) ? extraTexts[extraKey] : null;
      if (extra) {
        usedExtras.add(extraKey);
        if (extra.versions && !extra.versions.includes(pkg.version)) {
          errors.push(
            `${packageKey(pkg)} is not in the versions of EXTRA_LICENSE_TEXTS ` +
              `"${extraKey}" (${extra.versions.join(", ")}). Check the license texts ` +
              "of this version and update the entry.",
          );
          extra = null;
        }
      }
      return {
        ...pkg,
        notes: [pkg.note, extra?.note].filter(Boolean),
        resolution: resolvePackageTexts(pkg, licenseTexts, extra, licenseTextsDir),
      };
    });
  for (const key of Object.keys(extraTexts)) {
    if (!usedExtras.has(key)) {
      errors.push(`EXTRA_LICENSE_TEXTS "${key}" matches no package. Remove it.`);
    }
  }
  for (const entry of entries.filter(({ resolution }) => resolution.kind === "no-holder")) {
    errors.push(
      `${packageKey(entry)} has no license file, and the standard ` +
        `${entry.resolution.templated.join(", ")} text needs a copyright holder, but the ` +
        "package metadata names no authors. Copy the upstream license text into " +
        "scripts/license-texts/packages/ and add it to EXTRA_LICENSE_TEXTS.",
    );
  }
  const missingKeys = new Set();
  for (const entry of entries.filter(({ resolution }) => resolution.kind === "missing")) {
    const key = packageKey(entry);
    missingKeys.add(key);
    if (byKey.has(key)) {
      entry.exception = byKey.get(key);
    } else {
      errors.push(
        `${key} has no license file and no license expression with a standard text ` +
          `(license: ${entry.license ?? "not declared"}). Add the text to ` +
          "scripts/license-texts/ or add an exception with a reason.",
      );
    }
  }
  for (const key of byKey.keys()) {
    if (!missingKeys.has(key)) {
      errors.push(
        `exception ${key} does not match a package without license text. Remove it.`,
      );
    }
  }
  const counts = {
    cargo: entries.filter(({ ecosystem }) => ecosystem === "cargo").length,
    npm: entries.filter(({ ecosystem }) => ecosystem === "npm").length,
    standard: entries.filter(({ resolution }) => resolution.kind === "standard").length,
    exceptions: entries.filter(({ exception }) => exception).length,
    texts: collectTexts(entries).size,
  };
  return { markdown: renderLicenseFile(entries, counts), counts, errors };
}

function runCommand(command, args, cwd) {
  const result = spawnSync(command, args, {
    cwd,
    encoding: "utf8",
    maxBuffer: 512 * 1024 * 1024,
    shell: process.platform === "win32" && command === "pnpm",
  });
  if (result.error) {
    throw new Error(`could not run ${command}: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(
      `${[command, ...args].join(" ")} failed with status ${result.status}\n` +
        `${result.stderr ?? ""}`.trim(),
    );
  }
  return result.stdout;
}

// Runs cargo metadata, cargo tree, and pnpm list. Run `cargo fetch` for each
// Cargo workspace and `pnpm install` first if the sources are not on disk.
export function loadInputs({ appRoot = APP_ROOT, run = runCommand } = {}) {
  const cargo = CARGO_WORKSPACES.map(({ manifest }) => {
    const manifestArgs = ["--locked", "--manifest-path", join(appRoot, manifest)];
    return {
      metadata: JSON.parse(
        run("cargo", ["metadata", "--format-version", "1", ...manifestArgs], appRoot),
      ),
      tree: run(
        "cargo",
        [
          "tree",
          ...manifestArgs,
          "--edges",
          "normal,build",
          "--target",
          "all",
          "--prefix",
          "none",
          "--format",
          "{p}",
        ],
        appRoot,
      ),
    };
  });
  const pnpmProduction = JSON.parse(
    run("pnpm", ["list", "--prod", "--json", "--depth", "Infinity"], appRoot),
  );
  const pnpmDevelopment = JSON.parse(
    run("pnpm", ["list", "--dev", "--json", "--depth", "1"], appRoot),
  );
  return { cargo, pnpmProduction, pnpmDevelopment };
}

function firstDifferentLine(left, right) {
  const leftLines = left.split("\n");
  const rightLines = right.split("\n");
  for (let index = 0; index < Math.max(leftLines.length, rightLines.length); index += 1) {
    if (leftLines[index] !== rightLines[index]) {
      return index + 1;
    }
  }
  return 0;
}

function summary(counts) {
  return (
    `${counts.cargo} Rust crates, ${counts.npm} npm packages, ` +
    `${counts.standard} standard SPDX texts, ${counts.exceptions} exceptions, ` +
    `${counts.texts} license texts`
  );
}

// Runs the collector. It returns the process exit code.
export function runCollector({
  argv = [],
  appRoot = APP_ROOT,
  outputPath = OUTPUT_PATH,
  exceptionsPath = EXCEPTIONS_PATH,
  licenseTextsDir = LICENSE_TEXTS_DIR,
  inputs = () => loadInputs({ appRoot }),
  buildInjected = BUILD_INJECTED_NPM_PACKAGES,
  extraTexts = EXTRA_LICENSE_TEXTS,
  log = console.log,
  error = console.error,
} = {}) {
  if (argv.includes("--help") || argv.includes("-h")) {
    log("Usage: node scripts/collect-third-party-licenses.mjs [--check]");
    return 0;
  }
  const unknown = argv.filter((argument) => argument !== "--check");
  if (unknown.length > 0) {
    error(`collect-third-party-licenses: unknown argument: ${unknown.join(" ")}`);
    return 2;
  }
  const check = argv.includes("--check");
  const outputLabel = relative(appRoot, outputPath).split(sep).join("/");
  let report;
  try {
    const exceptionsFile = JSON.parse(readFileSync(exceptionsPath, "utf8"));
    report = buildLicenseReport({
      ...inputs(),
      exceptions: exceptionsFile.exceptions,
      licenseTexts: loadLicenseTexts(licenseTextsDir),
      buildInjected,
      extraTexts,
      licenseTextsDir,
    });
  } catch (caught) {
    error(`collect-third-party-licenses: ${caught.message}`);
    return 1;
  }
  if (report.errors.length > 0) {
    for (const message of report.errors) {
      error(`collect-third-party-licenses: ${message}`);
    }
    return 1;
  }
  if (check) {
    const committed = existsSync(outputPath)
      ? readFileSync(outputPath, "utf8").replace(/\r\n/gu, "\n")
      : null;
    if (committed !== report.markdown) {
      const where =
        committed === null
          ? "the file does not exist"
          : `first difference at line ${firstDifferentLine(committed, report.markdown)}`;
      error(
        `collect-third-party-licenses: ${outputLabel} is not current (${where}). ` +
          "Run pnpm licenses:collect and commit the result.",
      );
      return 1;
    }
    log(`collect-third-party-licenses: ${outputLabel} is current: ${summary(report.counts)}`);
    return 0;
  }
  writeFileSync(outputPath, report.markdown);
  log(`collect-third-party-licenses: wrote ${outputLabel}: ${summary(report.counts)}`);
  return 0;
}

// Tells if Node started the module at moduleUrl as the main module. "main"
// is import.meta.main, which older Node versions do not give. Node gives
// import.meta.url as the real path of the file, so the fallback also compares
// the real path of the started script. A start through a symlink thus runs
// the collector.
export function isMainModule(moduleUrl, main, startedPath = process.argv[1]) {
  if (typeof main === "boolean") {
    return main;
  }
  if (!startedPath) {
    return false;
  }
  const started = [pathToFileURL(resolve(startedPath)).href];
  try {
    started.push(pathToFileURL(realpathSync(startedPath)).href);
  } catch {
    // The started path does not exist, so only the resolved path can match.
  }
  return started.includes(moduleUrl);
}

if (isMainModule(import.meta.url, import.meta.main)) {
  process.exitCode = runCollector({ argv: process.argv.slice(2) });
}

#!/usr/bin/env node

import { readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, relative, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const ROOT_DIR = resolve(SCRIPT_DIR, "..");
const SRC_DIR = resolve(ROOT_DIR, "src");
const OUTPUT_PATH = resolve(SRC_DIR, "i18n/sources.json");
const CHECK_ONLY = process.argv.slice(2).includes("--check");
const CALL_PATTERN =
  /(?<![\w$])(?:t|translate)\s*\(\s*|(?<![\w$])useT\s*\(\s*\)\s*\(\s*/g;

async function walkSourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) {
      if (path === resolve(SRC_DIR, "harness")) continue;
      files.push(...(await walkSourceFiles(path)));
      continue;
    }
    if (entry.isFile() && /\.tsx?$/.test(entry.name)) files.push(path);
  }

  return files;
}

function lineNumberAt(source, index) {
  return source.slice(0, index).split("\n").length;
}

function skipTrivia(source, startIndex) {
  let index = startIndex;
  while (index < source.length) {
    if (/\s/.test(source[index])) {
      index += 1;
      continue;
    }
    if (source.startsWith("//", index)) {
      const lineEnd = source.indexOf("\n", index + 2);
      return lineEnd === -1 ? source.length : skipTrivia(source, lineEnd + 1);
    }
    if (source.startsWith("/*", index)) {
      const commentEnd = source.indexOf("*/", index + 2);
      return commentEnd === -1
        ? source.length
        : skipTrivia(source, commentEnd + 2);
    }
    break;
  }
  return index;
}

function decodeEscape(source, index) {
  const escape = source[index];
  const simpleEscapes = {
    b: "\b",
    f: "\f",
    n: "\n",
    r: "\r",
    t: "\t",
    v: "\v",
    0: "\0",
  };
  if (escape in simpleEscapes) {
    return { value: simpleEscapes[escape], nextIndex: index + 1 };
  }
  if (escape === "\n") return { value: "", nextIndex: index + 1 };
  if (escape === "\r" && source[index + 1] === "\n") {
    return { value: "", nextIndex: index + 2 };
  }
  if (escape === "x" && /^[\da-f]{2}$/i.test(source.slice(index + 1, index + 3))) {
    return {
      value: String.fromCodePoint(Number.parseInt(source.slice(index + 1, index + 3), 16)),
      nextIndex: index + 3,
    };
  }
  if (escape === "u") {
    const codePointMatch = source.slice(index + 1).match(/^\{([\da-f]+)\}/i);
    if (codePointMatch) {
      return {
        value: String.fromCodePoint(Number.parseInt(codePointMatch[1], 16)),
        nextIndex: index + 1 + codePointMatch[0].length,
      };
    }
    const codeUnit = source.slice(index + 1, index + 5);
    if (/^[\da-f]{4}$/i.test(codeUnit)) {
      return {
        value: String.fromCharCode(Number.parseInt(codeUnit, 16)),
        nextIndex: index + 5,
      };
    }
  }
  return { value: escape, nextIndex: index + 1 };
}

function readLiteral(source, startIndex) {
  const delimiter = source[startIndex];
  if (delimiter !== '"' && delimiter !== "'" && delimiter !== "`") return null;

  let value = "";
  let index = startIndex + 1;
  while (index < source.length) {
    const character = source[index];
    if (character === delimiter) return { value, interpolated: false };
    if (
      delimiter === "`" &&
      character === "$" &&
      source[index + 1] === "{"
    ) {
      return { value: "", interpolated: true };
    }
    if (character === "\\") {
      const decoded = decodeEscape(source, index + 1);
      value += decoded.value;
      index = decoded.nextIndex;
      continue;
    }
    value += character;
    index += 1;
  }
  return null;
}

async function extractSources() {
  const sources = new Set();
  const files = await walkSourceFiles(SRC_DIR);

  for (const file of files) {
    const contents = await readFile(file, "utf8");
    for (const match of contents.matchAll(CALL_PATTERN)) {
      const argumentIndex = skipTrivia(
        contents,
        match.index + match[0].length,
      );
      const literal = readLiteral(contents, argumentIndex);
      if (!literal) continue;
      if (literal.interpolated) {
        const displayPath = relative(ROOT_DIR, file);
        const line = lineNumberAt(contents, match.index);
        console.warn(
          `i18n-extract: ${displayPath}:${line}: skipped template literal with interpolation`,
        );
        continue;
      }
      sources.add(literal.value);
    }
  }

  return `${JSON.stringify([...sources].sort(), null, 2)}\n`;
}

const generated = await extractSources();
if (CHECK_ONLY) {
  const current = await readFile(OUTPUT_PATH, "utf8").catch(() => "");
  if (current !== generated) {
    console.error(
      "i18n-extract: src/i18n/sources.json is out of date; run node scripts/i18n-extract.mjs",
    );
    process.exitCode = 1;
  }
} else {
  await writeFile(OUTPUT_PATH, generated);
}

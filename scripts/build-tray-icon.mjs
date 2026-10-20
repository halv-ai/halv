#!/usr/bin/env node

// This script renders the tray SVG as a 32 px template PNG.
// Chromium provides consistent SVG rasterization.

import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const iconSize = 32;

async function main() {
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const sourcePath = join(repoRoot, "src-tauri", "tray-icon.svg");
  const outputPath = join(
    repoRoot,
    "src-tauri",
    "icons",
    "tray",
    "32x32.png",
  );
  const svg = (await readFile(sourcePath, "utf8")).replace(
    /^<\?xml[^>]*>\s*/u,
    "",
  );
  const browser = await chromium.launch({ args: ["--single-process"] });

  try {
    const page = await browser.newPage({
      viewport: { width: iconSize, height: iconSize },
      deviceScaleFactor: 1,
    });
    await page.setContent(`
      <!doctype html>
      <style>
        html, body {
          width: ${iconSize}px;
          height: ${iconSize}px;
          margin: 0;
          overflow: hidden;
          background: transparent;
        }
        svg { display: block; }
      </style>
      ${svg}
    `);
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({
      path: outputPath,
      omitBackground: true,
      clip: { x: 0, y: 0, width: iconSize, height: iconSize },
    });
  } finally {
    await browser.close();
  }

  console.log(`build-tray-icon: wrote ${outputPath}.`);
}

main().catch((error) => {
  console.error(`build-tray-icon: ${error.message}`);
  process.exitCode = 1;
});

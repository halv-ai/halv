import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { chromium } from "@playwright/test";
import { createServer, type ViteDevServer } from "vite";
import { MARKETING_EXPORTS } from "../src/harness/marketing-exports.ts";
import {
  VISUAL_STATE_NAMES,
  type HarnessStateName,
} from "../src/harness/states.ts";
import { openVisualState } from "../tests/visual/capture.ts";

const BASE_URL = "http://127.0.0.1:1420";
const SHOTS_DIRECTORY = resolve(process.cwd(), "shots");
type ExportSettings = {
  state: HarnessStateName;
  viewport: { width: number; height: number };
  deviceScaleFactor: number;
  colorScheme: "dark" | "light";
};
const DEFAULT_EXPORT_SETTINGS = {
  viewport: { width: 1280, height: 800 },
  deviceScaleFactor: 2,
  colorScheme: "dark",
} as const;
const EXPORTS: readonly ExportSettings[] = [
  ...VISUAL_STATE_NAMES.map((state) => ({
    state,
    ...DEFAULT_EXPORT_SETTINGS,
  })),
  // marketing-chat-dark/light stay excluded because their bootstrap is stale.
  ...MARKETING_EXPORTS,
];

let server: ViteDevServer | null = null;
let browser: Awaited<ReturnType<typeof chromium.launch>> | null = null;

try {
  server = await startServerIfNeeded();
  await mkdir(SHOTS_DIRECTORY, { recursive: true });
  browser = await chromium.launch();

  for (const exportSettings of EXPORTS) {
    const { state } = exportSettings;
    const context = await browser.newContext({
      viewport: exportSettings.viewport,
      deviceScaleFactor: exportSettings.deviceScaleFactor,
      reducedMotion: "reduce",
      colorScheme: exportSettings.colorScheme,
      locale: "en-US",
      timezoneId: "UTC",
      baseURL: BASE_URL,
    });
    try {
      const page = await context.newPage();
      await openVisualState(page, state);
      const outputPath = resolve(SHOTS_DIRECTORY, `${state}.png`);
      await page.screenshot({
        path: outputPath,
        animations: "disabled",
        caret: "hide",
        scale: "device",
      });
      console.log(`Exported shots/${state}.png`);
    } finally {
      await context.close();
    }
  }
} finally {
  await browser?.close();
  await server?.close();
}

async function startServerIfNeeded() {
  if (await serverIsReady()) {
    return null;
  }
  const vite = await createServer();
  await vite.listen();
  return vite;
}

async function serverIsReady() {
  try {
    const response = await fetch(`${BASE_URL}/ux-audit.html`, {
      signal: AbortSignal.timeout(1_000),
    });
    return response.ok;
  } catch {
    return false;
  }
}

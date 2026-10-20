import { defineConfig } from "@playwright/test";

const port = Number(process.env.HALV_VISUAL_PORT ?? 1420);
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "./tests/visual",
  testMatch: "**/*.visual.spec.ts",
  outputDir: "test-results/playwright",
  fullyParallel: false,
  workers: 1,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
  reporter: "line",
  snapshotPathTemplate: "{testDir}/__screenshots__/{arg}{ext}",
  expect: {
    toHaveScreenshot: {
      animations: "disabled",
      caret: "hide",
      scale: "device",
    },
  },
  use: {
    baseURL,
    viewport: { width: 1280, height: 800 },
    deviceScaleFactor: 2,
    contextOptions: {
      reducedMotion: "reduce",
    },
    colorScheme: "dark",
    locale: "en-US",
    timezoneId: "UTC",
    trace: "retain-on-failure",
    video: "off",
  },
  webServer: {
    command: `pnpm dev --port ${port} --strictPort`,
    url: `${baseURL}/ux-audit.html`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});

import { defineConfig } from "@playwright/test";
import baseConfig from "./playwright.config";

export default defineConfig({
  ...baseConfig,
  testDir: "./tests/marketing",
  testMatch: "marketing-shots.spec.ts",
  outputDir: "test-results/playwright-marketing",
  // The shipped app runs in a WebKit view. WebKit resolves ui-monospace
  // to SF Mono, so marketing captures must use the same engine.
  use: { ...baseConfig.use, browserName: "webkit" },
});

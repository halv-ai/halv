import { expect, test } from "@playwright/test";
import { VISUAL_STATE_NAMES } from "../../src/harness/states";
import { openVisualState } from "./capture";

for (const state of VISUAL_STATE_NAMES) {
  test(state, async ({ page }) => {
    await openVisualState(page, state);
    await expect(page).toHaveScreenshot(`${state}.png`);
  });
}

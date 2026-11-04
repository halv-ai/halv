import { expect, test } from "@playwright/test";
import { VISUAL_STATE_NAMES } from "../../src/harness/states";
import { openVisualState } from "./capture";

// With the same layout, Chromium sometimes paints the usage bars of the
// open Claude popover one device pixel apart: about 700 pixels on the
// fills of the 5h and 7d bars. This state allows a little more than that.
const MAX_DIFF_PIXELS: Partial<Record<(typeof VISUAL_STATE_NAMES)[number], number>> = {
  "status-bar-orca": 800,
};

for (const state of VISUAL_STATE_NAMES) {
  test(state, async ({ page }) => {
    await openVisualState(page, state);
    await expect(page).toHaveScreenshot(`${state}.png`, {
      maxDiffPixels: MAX_DIFF_PIXELS[state] ?? 0,
    });
  });
}

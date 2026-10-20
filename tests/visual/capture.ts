import type { Page } from "@playwright/test";
import {
  FIXED_VISUAL_NOW,
  type HarnessStateName,
} from "../../src/harness/states.ts";

const DISABLE_MOTION_CSS = `
  *,
  *::before,
  *::after {
    animation: none !important;
    caret-color: transparent !important;
    scroll-behavior: auto !important;
    transition: none !important;
  }
`;

// `query` adds harness parameters, for example "update=ready&theme=light".
export async function openVisualState(
  page: Page,
  state: HarnessStateName,
  query = "",
) {
  await page.clock.setFixedTime(new Date(FIXED_VISUAL_NOW));
  await page.goto(`ux-audit.html?state=${state}${query ? `&${query}` : ""}`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(
    (expectedState) => {
      const root = document.documentElement;
      return (
        root.dataset.visualReady === expectedState ||
        root.dataset.visualError !== undefined
      );
    },
    state,
  );

  const harnessError = await page
    .locator("html")
    .getAttribute("data-visual-error");
  if (harnessError) {
    throw new Error(harnessError);
  }

  await page.addStyleTag({ content: DISABLE_MOTION_CSS });
  await page.evaluate(async () => {
    await document.fonts.ready;
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
  });
}

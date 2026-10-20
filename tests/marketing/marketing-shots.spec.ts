import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { MARKETING_STATE_NAMES } from "../../src/harness/states";
import { openVisualState } from "../visual/capture";

const SHOTS_DIR = fileURLToPath(new URL("./shots/", import.meta.url));

test.beforeAll(async () => {
  await mkdir(SHOTS_DIR, { recursive: true });
});

for (const state of MARKETING_STATE_NAMES) {
  test(state, async ({ page }) => {
    await openVisualState(page, state);

    const bodyFontFamily = await page.evaluate(
      () => getComputedStyle(document.body).fontFamily,
    );
    expect(bodyFontFamily).toMatch(/^ui-sans-serif/);

    await expect(page.locator("body")).not.toContainText("API USAGE");
    const chatState = state.startsWith("marketing-chat");
    if (chatState) {
      await expect(
        page.locator("aside").getByText("halv-web", { exact: true }),
      ).toBeVisible();
      await expect(page.getByText("PLAN WINDOW", { exact: true })).toBeVisible();

      const responseOptions = page.getByRole("dialog", {
        name: "Response options",
      });
      await expect(responseOptions).toBeVisible();
      const modelSelect = responseOptions.getByLabel("Model");
      await expect(modelSelect).toHaveValue("sonnet");
      await expect(modelSelect.locator("option:checked")).toHaveText(
        "Sonnet 5 · via Claude Code",
      );

      const savingsMeter = page.getByLabel("Session savings meter");
      await expect(savingsMeter).toBeVisible();
      await expect(savingsMeter).toContainText("2.4K in");
      await expect(savingsMeter).toContainText("~3,830 tokens saved");

      const savingsPanel = page.locator("#live-savings-details");
      const panelBox = await savingsPanel.boundingBox();
      expect(panelBox).not.toBeNull();
      expect(panelBox!.y + panelBox!.height).toBeLessThanOrEqual(800);
    } else if (state.startsWith("marketing-workspace")) {
      // The 0.8.0 hero: the folder group, the strip and the measured meter.
      await expect(page.locator("[data-workspace-tab-strip]")).toContainText(
        "Ship checkout v2",
      );
      await expect(page.getByLabel("Session savings meter")).toContainText(
        "$6.42 saved in this conversation",
      );
      const sidebar = page.getByLabel("Conversation navigation");
      await expect(sidebar).toContainText("Stripe retry policy");
      await expect(sidebar).toContainText("Pricing page copy");
    } else {
      const sessionsDialog = page.getByRole("dialog", {
        name: "Agent sessions",
      });
      await expect(sessionsDialog).toBeVisible();
      await expect(
        sessionsDialog.getByRole("switch", { name: /Pro mode/ }),
      ).toHaveAttribute("aria-checked", "true");
      await expect(sessionsDialog).toContainText("Opens in the built-in terminal");
      await expect(
        sessionsDialog.locator(".agent-session-status-dot-running").first(),
      ).toBeVisible();

      const savingsMeter = sessionsDialog.getByLabel("Session savings meter");
      await expect(savingsMeter).toBeVisible();
      await expect(savingsMeter).toContainText("4.9K in");
      await expect(savingsMeter).toContainText("~8,920 tokens saved");

      const terminal = sessionsDialog.getByLabel("Claude embedded terminal");
      await expect(terminal).toBeVisible();
      await expect(terminal.locator(".xterm-rows")).toContainText(
        "Review the release checklist",
      );
      await expect(terminal.locator(".xterm-rows")).toContainText(
        "Read(package.json, RELEASING.md)",
      );
      await expect(sessionsDialog).not.toContainText(
        "Embedded terminal unavailable",
      );
    }

    const expectedDark = state.endsWith("-dark");
    expect(
      await page.evaluate(() =>
        document.documentElement.classList.contains("dark"),
      ),
    ).toBe(expectedDark);

    const screenshot = await page.screenshot({
      path: `${SHOTS_DIR}${state}.png`,
      fullPage: true,
    });
    expect(screenshot.readUInt32BE(16)).toBe(2_560);
    expect(screenshot.readUInt32BE(20)).toBe(1_600);
  });
}

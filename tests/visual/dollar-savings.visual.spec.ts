import { expect, test } from "@playwright/test";
import { openVisualState } from "./capture";

// The dollar metric of ADR 0015 on each savings surface, in both themes.
for (const theme of ["dark", "light"] as const) {
  test(`the status bar and the drawer lead with dollars (${theme})`, async ({ page }) => {
    await openVisualState(page, "chat-active", `theme=${theme}`);
    const bar = page.getByRole("region", { name: "Status bar", exact: true });
    const savings = bar.getByRole("button", { name: /saved/ });
    await expect(savings).toContainText("$12.3 saved");
    await expect(savings).toHaveAttribute(
      "title",
      "Saved in the last 7 days at API prices. Click for details.",
    );
    await expect(savings).toHaveScreenshot(`dollar-status-bar-${theme}.png`);

    await savings.click();
    const drawer = page.locator("#savings-drawer");
    await expect(drawer).toContainText("saved at API prices");
    await expect(drawer).toContainText("1.9M tokens kept out of context");
    await expect(drawer).toContainText("By source");
    await expect(drawer).toContainText("Routing savings not estimated yet");
    await expect(drawer).toContainText("$27.6 all-time");
    await expect(drawer).toContainText("since Aug 6, 2026");
    await expect(drawer).toContainText("Before Aug 6, 2026: 1.2M tokens saved");
    await expect(drawer).toHaveScreenshot(`dollar-drawer-${theme}.png`);

    // The activity grid switches between dollars and tokens.
    const unit = drawer.getByRole("group", { name: "Unit" });
    await expect(unit.getByRole("button", { name: "Dollars" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(drawer.getByText(/in the last year$/)).toContainText("$");
    await unit.getByRole("button", { name: "Tokens" }).click();
    await expect(drawer.getByText(/in the last year$/)).not.toContainText("$");
  });

  test(`the Halv Engine dashboard leads with dollars and explains the method (${theme})`, async ({ page }) => {
    await openVisualState(page, "savings-dashboard", `theme=${theme}`);
    const method = page.locator("#savings-method");
    await expect(method).toBeVisible();
    const dashboard = method.locator("..");
    await expect(dashboard).toContainText("$27.6");
    await expect(dashboard).toContainText("2.8M tokens kept out of context");
    await expect(method).toContainText("Prices as of Oct 15, 2026.");
    await expect(dashboard).toHaveScreenshot(`dollar-dashboard-${theme}.png`);
  });
}

test("the dashboard labels the provider cache discount as not counted", async ({ page }) => {
  await openVisualState(page, "savings-dashboard");
  await page.getByText("Advanced diagnostics", { exact: true }).click();
  await expect(page.getByText("provider cache discount, not counted")).toBeVisible();
});

test("the drawer link opens How Halv estimates savings", async ({ page }) => {
  await openVisualState(page, "chat-active");
  await page
    .getByRole("region", { name: "Status bar", exact: true })
    .getByRole("button", { name: /saved/ })
    .click();
  await page
    .locator("#savings-drawer")
    .getByRole("button", { name: "How Halv estimates savings" })
    .click();
  await expect(page.locator("#savings-drawer")).toBeHidden();
  await expect(page.locator("#savings-method")).toBeVisible();
});

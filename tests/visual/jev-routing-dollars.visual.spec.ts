import { expect, test, type Page } from "@playwright/test";
import { openVisualState } from "./capture";

// ADR 0016: `routing=estimated` gives the harness settled JEV routing
// chains. A total with routing shows "≈" and adds the low estimate; the
// details show the range.
async function openRoutedConversation(page: Page, theme: "dark" | "light") {
  await openVisualState(
    page,
    "marketing-pro-dark",
    ["taskRoutes=workflow", "routing=estimated", theme === "light" ? "theme=light" : ""]
      .filter(Boolean)
      .join("&"),
  );
  await page.getByRole("button", { name: "Close agent sessions" }).click();
  await page
    .getByRole("button", { name: /Agent conversation Release readiness audit/ })
    .first()
    .click();
  const dismissHint = page.getByRole("button", { name: "Dismiss Pro mode hint" });
  if (await dismissHint.isVisible()) await dismissHint.click();
  const surface = page.locator(".terminal-diff-surface-agent").first();
  await expect(surface.locator(".terminal-pane")).toHaveCount(1, {
    timeout: 15_000,
  });
  return surface;
}

for (const theme of ["dark", "light"] as const) {
  test(`the status bar and the drawer add the routing estimate (${theme})`, async ({
    page,
  }) => {
    await openVisualState(page, "chat-active", `theme=${theme}&routing=estimated`);
    const bar = page.getByRole("region", { name: "Status bar", exact: true });
    const savings = bar.getByRole("button", { name: /saved/ });
    await expect(savings).toContainText("≈ $13.2 saved");
    await expect(savings).toHaveScreenshot(`routing-status-bar-${theme}.png`);

    await savings.click();
    const drawer = page.locator("#savings-drawer");
    await expect(drawer).toContainText("≈ $13.2");
    await expect(drawer).toContainText("Context reduction$12.3");
    await expect(drawer).toContainText("≈ $0.84 (range $0.84 to $2.65)");
    await expect(drawer).toContainText("≈ $30 all-time");
    await expect(drawer).not.toContainText("Routing savings not estimated yet");
    await expect(drawer).toHaveScreenshot(`routing-drawer-${theme}.png`);
  });

  test(`the task rail and the session meter show routing dollars (${theme})`, async ({
    page,
  }) => {
    const surface = await openRoutedConversation(page, theme);
    await surface.getByRole("button", { name: "Open task routing" }).click();
    const board = surface.getByRole("complementary", { name: "Task routing" });
    await expect(board.locator("h4")).toContainText("\u2248\u00a0$0.18 saved by routing");
    await expect(board).toContainText("Range $0.18 to $1.42");
    await expect(board.getByText("Cost $0.43 on claude-sonnet-5-5")).toBeVisible();
    await expect(board.getByText("Cost $1.12 on claude-opus-5-5")).toBeVisible();
    await expect(board).toHaveScreenshot(`routing-task-rail-${theme}.png`);

    const meter = surface.getByRole("region", { name: "Session savings meter" });
    await expect(meter.locator("strong")).toContainText(
      /^≈ \$[0-9.]+ saved in this conversation$/,
    );
    await meter.getByText("Details", { exact: true }).click();
    await expect(meter).toContainText(
      "Routing: ≈ $0.18 (range $0.18 to $1.42)",
    );
    await expect(meter).toHaveScreenshot(`routing-meter-${theme}.png`);
  });
}

test("the savings method names the bench calibration", async ({ page }) => {
  await openVisualState(page, "savings-dashboard", "routing=estimated");
  const method = page.locator("#savings-method");
  await expect(method).toContainText("from Halv benchmark tasks that ran on both models");
  await expect(method).toContainText("uses 4 times the tokens");
  await expect(method).toContainText("Routing calibration 2026-10-15.1.");
});

test("without settled chains every routing surface says not estimated", async ({
  page,
}) => {
  await openVisualState(page, "chat-active");
  const bar = page.getByRole("region", { name: "Status bar", exact: true });
  const savings = bar.getByRole("button", { name: /saved/ });
  await expect(savings).not.toContainText("≈");
  await savings.click();
  await expect(page.locator("#savings-drawer")).toContainText(
    "Routing savings not estimated yet",
  );
});

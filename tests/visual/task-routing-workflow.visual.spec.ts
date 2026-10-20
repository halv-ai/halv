import { expect, test, type Page } from "@playwright/test";
import { openVisualState } from "./capture";

// ADR 0014: `taskRoutes=workflow` adds one routed Workflow batch to the
// task routing rail of the harness.
async function openWorkflowRoutes(page: Page, query = "") {
  await openVisualState(
    page,
    "marketing-pro-dark",
    ["taskRoutes=workflow", query].filter(Boolean).join("&"),
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
  await surface.getByRole("button", { name: "Open task routing" }).click();
  return surface.getByRole("complementary", { name: "Task routing" });
}

for (const theme of ["dark", "light"] as const) {
  test(`task routing groups a workflow batch with its receipts (${theme})`, async ({
    page,
  }) => {
    const board = await openWorkflowRoutes(
      page,
      theme === "light" ? "theme=light" : "",
    );
    const batch = board.locator(".task-board-batch");
    await expect(batch).toHaveCount(1);
    await expect(batch.locator(".task-board-batch-title")).toHaveText(
      "Workflow · fix-findings · 11 agents",
    );
    await expect(batch.locator(".task-board-row")).toHaveCount(6);
    await expect(batch.getByText("scout · read ×4", { exact: true })).toBeVisible();
    await expect(batch.getByText("Inherits claude-opus-5-5", { exact: true })).toBeVisible();
    await expect(batch.getByText("Not applied", { exact: true })).toHaveCount(1);
    await expect(batch.getByText("1.9M tokens measured", { exact: true }).first()).toBeVisible();
    await expect(batch.getByText(/escalated from Fix one parser finding/)).toBeVisible();
    // Legacy routes stay outside the batch.
    await expect(board.locator(".task-board-list > .task-board-row").first()).toBeVisible();
    await expect(board).toHaveScreenshot(`task-routing-workflow-${theme}.png`);
  });
}

test("JEV settings switch workflow routing and ultracode for new panes", async ({
  page,
}, testInfo) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark");
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await dialog.getByRole("button", { name: /^JEV/ }).click();

  const routing = dialog.getByRole("button", { name: /^Route workflow agents/ });
  const ultracode = dialog.getByRole("button", { name: /^Start Claude panes in ultracode/ });
  await expect(routing).toHaveAttribute("aria-pressed", "true");
  await expect(ultracode).toHaveAttribute("aria-pressed", "false");

  await routing.click();
  await expect(dialog.getByRole("status")).toContainText("Workflow routing disabled.");
  await expect(routing).toHaveAttribute("aria-pressed", "false");
  await ultracode.click();
  await expect(dialog.getByRole("status")).toContainText(
    "New Claude panes start in ultracode.",
  );
  const saved = await page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    return ipc.preferences.get();
  });
  expect(saved.workflowRouting).toBe(false);
  expect(saved.claudeUltracode).toBe(true);

  await dialog.getByRole("button", { name: /^Route tasks to cheaper agents/ }).click();
  await expect(routing).toBeDisabled();
  await expect(ultracode).toBeDisabled();
  await dialog.screenshot({
    animations: "disabled",
    path: testInfo.outputPath("settings-jev-workflow.png"),
  });
});

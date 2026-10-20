import { expect, test, type Locator, type Page } from "@playwright/test";
import { taskRouteDismissalsStorageKey } from "../../src/taskBoard.ts";
import { openVisualState } from "./capture";

async function openTerminalDiffFixture(page: Page) {
  await openVisualState(page, "marketing-pro-dark");
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
  await expect(surface.locator(".terminal-pane")).toHaveAttribute(
    "data-terminal-scene-complete", "true",
  );
  // Opening the completed conversation acknowledges its yellow indicator.
  await expect(
    surface.locator(
      ".terminal-status-label .agent-session-status-dot-waiting_input",
    ),
  ).toBeHidden({ timeout: 5_000 });
  return surface;
}

async function expectRepositoryGroups(surface: Locator) {
  await expect(
    surface.getByRole("button", { name: /web 2 changed files/ }),
  ).toBeVisible();
  await expect(
    surface.getByRole("button", { name: /design-system 2 changed files/ }),
  ).toBeVisible();
}

test("diff rail collapsed", async ({ page }) => {
  const surface = await openTerminalDiffFixture(page);

  await expect(surface.getByRole("button", { name: "Open diff rail" })).toBeVisible();
  await expect(surface).toHaveScreenshot("diff-rail-collapsed.png");
});

test("task routing starts compact and can be opened and closed", async ({ page }) => {
  const surface = await openTerminalDiffFixture(page);
  const openButton = surface.getByRole("button", { name: "Open task routing" });
  await expect(openButton).toBeVisible();
  await expect(openButton).toHaveAttribute("aria-expanded", "false");
  await expect(openButton).toContainText(
    "1 Active · 2 Completed · 1 Failed · 1 Escalated",
  );
  await expect(openButton.locator("strong")).toHaveText("1");

  const surfaceBox = await surface.boundingBox();
  const terminalBox = await surface.locator(".terminal-panel-agent").boundingBox();
  const compactBox = await surface.getByRole("complementary", { name: "Task routing" }).boundingBox();
  expect(surfaceBox).not.toBeNull();
  expect(terminalBox).not.toBeNull();
  expect(compactBox).not.toBeNull();
  expect(compactBox!.width).toBeLessThanOrEqual(44);
  expect(terminalBox!.width / surfaceBox!.width).toBeGreaterThan(0.85);

  await openButton.click();
  const closeButton = surface.getByRole("button", { name: "Collapse task routing" });
  await expect(closeButton).toBeVisible();
  await expect(closeButton).toHaveAttribute("aria-expanded", "true");
  await expect(surface.getByText("Completed", { exact: true }).first()).toBeVisible();
  const dismissCompleted = surface.getByRole("button", {
    name: "Dismiss completed task Rename the settings header",
  });
  await expect(dismissCompleted).toBeVisible();
  await expect(surface).toHaveScreenshot("task-routing-open.png");

  await dismissCompleted.click();
  await expect(dismissCompleted).toBeHidden();
  await closeButton.click();
  await expect(openButton).toBeVisible();
});

test("task routing rail can dismiss when every visible task is finished", async ({
  page,
}) => {
  await page.goto("ux-audit.html?state=marketing-pro-dark", {
    waitUntil: "domcontentloaded",
  });
  await page.evaluate(
    ({ key, value }) => window.localStorage.setItem(key, value),
    {
      key: taskRouteDismissalsStorageKey("audit-marketing"),
      value: JSON.stringify({
        version: 1,
        routes: [
          {
            id: "route-3",
            updatedAt: "2026-09-21T10:09:12.000Z",
          },
        ],
      }),
    },
  );
  const surface = await openTerminalDiffFixture(page);
  const openButton = surface.getByRole("button", { name: "Open task routing" });
  await expect(openButton.locator("strong")).toHaveText("0");
  await openButton.click();

  const dismissAll = surface.getByRole("button", {
    name: "Close all",
  });
  await expect(dismissAll).toBeVisible();
  await dismissAll.click();
  await expect(
    surface.getByRole("complementary", { name: "Task routing" }),
  ).toBeHidden();
});

test("task routing closes finished tasks while running tasks stay first", async ({ page }) => {
  const surface = await openTerminalDiffFixture(page);
  await surface.getByRole("button", { name: "Open task routing" }).click();
  const board = surface.getByRole("complementary", { name: "Task routing" });
  await expect(board.locator(".task-board-row").first().getByText("Running", { exact: true })).toBeVisible();
  await expect(board.getByText(/Suggested route:/).first()).toBeVisible();
  await board.getByRole("button", { name: "Close all", exact: true }).click();
  await expect(board.locator(".task-board-row")).toHaveCount(1);
  await expect(board.getByText("Running", { exact: true })).toBeVisible();
  await expect(board.getByRole("button", { name: "Close all", exact: true })).toBeHidden();
  await expect(board.getByRole("button", { name: /Dismiss .* task/ })).toHaveCount(0);
});

test("diff rail expanded in unified mode", async ({ page }) => {
  const surface = await openTerminalDiffFixture(page);
  await surface.getByRole("button", { name: "Open diff rail" }).click();
  await expect(
    surface.getByText("src/release.ts", { exact: true }).first(),
  ).toBeVisible();
  await expectRepositoryGroups(surface);
  await expect(surface.getByRole("button", { name: "Unified" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  await expect(surface).toHaveScreenshot("diff-rail-unified.png");
});

test("diff rail expanded in split mode", async ({ page }) => {
  const surface = await openTerminalDiffFixture(page);
  await surface.getByRole("button", { name: "Open diff rail" }).click();
  await expect(
    surface.getByText("src/release.ts", { exact: true }).first(),
  ).toBeVisible();
  await expectRepositoryGroups(surface);
  await surface.getByRole("button", { name: "Split" }).click();
  await expect(surface.getByRole("button", { name: "Split" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  await expect(surface).toHaveScreenshot("diff-rail-split.png");
});

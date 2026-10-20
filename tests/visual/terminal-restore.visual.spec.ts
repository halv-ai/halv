import { expect, test, type Page } from "@playwright/test";

// Regression for the terminal-restore saga: the terminal opens at send in
// Pro mode, a split saves as a split, and a reload (the harness stand-in
// for an app relaunch) restores the split side by side with every pane
// auto-resumed. ?slow=1 gives session creation real-world latency so the
// historical race conditions stay covered.
const HARNESS_URL = "/ux-audit.html?scenario=normal&ghostty=off&slow=1";

async function waitForHarnessApp(page: Page) {
  await page
    .locator(".group button", { hasText: "Audit agent refactor" })
    .first()
    .waitFor({ timeout: 30_000 });
}

async function openFixtureConversation(page: Page) {
  await waitForHarnessApp(page);
  await page
    .locator(".group button", { hasText: "Audit agent refactor" })
    .first()
    .click();
}

test("pro terminals restore their split and resume after a relaunch", async ({
  page,
}) => {
  await page.goto(HARNESS_URL);
  await waitForHarnessApp(page);
  await page.evaluate(() => {
    for (const key of Object.keys(window.localStorage)) {
      if (key.includes("terminal")) window.localStorage.removeItem(key);
    }
  });
  await page.reload();

  await openFixtureConversation(page);
  // A real OFF→ON Pro click converts the open chat conversation.
  const pro = page.getByRole("switch").first();
  if ((await pro.getAttribute("aria-checked")) === "true") {
    await pro.click();
  }
  await pro.click();

  // The terminal opens at send — before any assistant response.
  await page
    .getByPlaceholder(/describe a task|message/i)
    .first()
    .fill("regression: terminal at send");
  await page.getByRole("button", { name: "Send" }).first().click();
  await expect(page.locator(".terminal-pane")).toHaveCount(1, {
    timeout: 15_000,
  });

  // The message rides along as the CLI's prompt argument at spawn; typing
  // it into the booting TUI used to lose it.
  const spawnPrompts = await page.evaluate(
    () =>
      (
        window as typeof window & {
          __harnessInitialPrompts?: Array<{ prompt: string }>;
        }
      ).__harnessInitialPrompts ?? [],
  );
  expect(spawnPrompts.map((item) => item.prompt)).toContain(
    "regression: terminal at send",
  );

  // Split — one tab holding one split with two panes, never loose tabs.
  await page.locator('summary[aria-label="Split terminal right"]').click();
  await page
    .locator(".terminal-split-menu-popover button", { hasText: "Duplicate" })
    .first()
    .click();
  await expect(page.locator(".terminal-pane")).toHaveCount(2, {
    timeout: 15_000,
  });
  await expect(page.locator(".terminal-split")).toHaveCount(1);
  // One tab hides the strip; panes live inside the single tab.
  await expect(page.locator(".terminal-tab")).toHaveCount(0);
  await page.waitForFunction(() => {
    return Object.keys(window.localStorage).some((key) => {
      if (!key.startsWith("ux-audit.terminal-restore:")) return false;
      const raw = window.localStorage.getItem(key);
      if (!raw) return false;
      try {
        const saved = JSON.parse(raw) as { panes?: unknown[] };
        return saved.panes?.length === 2;
      } catch {
        return false;
      }
    });
  });

  // Relaunch: the split comes back side by side and both panes resume
  // (no "Saved terminal" overlays stay behind).
  await page.reload();
  await openFixtureConversation(page);
  await expect(page.locator(".terminal-split")).toHaveCount(1, {
    timeout: 20_000,
  });
  await expect(page.locator(".terminal-pane")).toHaveCount(2);
  await expect(page.locator(".terminal-tab")).toHaveCount(0);
  await expect(page.locator(".terminal-exit-card")).toHaveCount(0, {
    timeout: 20_000,
  });
});

test("a launch failure preserves the saved conversation for retry", async ({ page }) => {
  await page.goto(HARNESS_URL);
  await openFixtureConversation(page);
  const pro = page.getByRole("switch").first();
  if ((await pro.getAttribute("aria-checked")) === "true") await pro.click();
  await pro.click();
  await page.getByPlaceholder(/describe a task|message/i).first().fill("restore failure check");
  await page.getByRole("button", { name: "Send" }).first().click();
  await expect(page.locator(".terminal-pane")).toHaveCount(1);
  await page.waitForFunction(() => Object.keys(localStorage).some(key => {
    if (!key.startsWith("ux-audit.terminal-restore:")) return false;
    const panel = JSON.parse(localStorage.getItem(key) ?? "null");
    return panel?.panes?.some((pane: { cliSessionId?: string }) => pane.cliSessionId);
  }));

  await page.goto(`${HARNESS_URL}&failAgentLaunch=1`);
  await openFixtureConversation(page);
  await page.getByRole("button", { name: "Focus terminal for Audit agent refactor", exact: true }).click();
  await expect(page.getByText("Account initialization connection failed", { exact: true }).first()).toBeVisible();
  const failedCalls = await page.evaluate(() => (window as unknown as { __harnessTerminalCreateRequests: string[] }).__harnessTerminalCreateRequests);
  expect(failedCalls).toHaveLength(1);
  expect(failedCalls[0]).toBeTruthy();
  await expect(page.locator(".terminal-pane")).toHaveCount(1);

  await page.evaluate(() => {
    const url = new URL(window.location.href);
    url.searchParams.delete("failAgentLaunch");
    window.history.replaceState(null, "", url);
  });
  await page.locator(".terminal-exit-card").getByRole("button", { name: "Resume", exact: true }).click();
  await expect(page.locator(".terminal-exit-card")).toHaveCount(0);
  const allCalls = await page.evaluate(() => (window as unknown as { __harnessTerminalCreateRequests: string[] }).__harnessTerminalCreateRequests);
  expect(allCalls).toEqual([failedCalls[0], failedCalls[0]]);
});

import { expect, test, type Page } from "@playwright/test";
import { AGENT_STATUS_SESSION_IDS as IDS } from "../../src/harness/agent-status.ts";
import { openVisualState } from "./capture";

// The restart states reuse the agent status panes: the Codex pane rests
// after a finished turn, and the Claude pane works.
type HarnessWindow = Window & {
  __harnessTerminalCreateRequests?: Array<string | null>;
};

function pane(page: Page, sessionId: string) {
  return page.locator(`.terminal-pane[data-terminal-session-id="${sessionId}"]`);
}

function chip(page: Page, sessionId: string) {
  return pane(page, sessionId).getByRole("region", { name: "Account switched", exact: true });
}

function swapNotice(page: Page) {
  return page.getByRole("region", { name: "Claude usage limit", exact: true });
}

async function waitForClaudeUsage(page: Page) {
  await expect(page.locator('[data-status-provider="claude"]')).toContainText("% used");
}

for (const theme of ["dark", "light"] as const) {
  test(`the swap notice offers the account with the most room in ${theme} mode`, async ({ page }) => {
    await openVisualState(page, `account-swap-${theme}`);
    const notice = swapNotice(page);
    await expect(notice).toContainText("Claude usage is at 92%");
    await expect(notice).toContainText("dev@acme.example · resets in 41m");
    // research@ is at 92 % too, so personal@ (64 % at most) has the most room.
    await expect(notice).toContainText(
      "Switch to auditor.personal@example.com? It has used 64% of its limits.",
    );
    await expect(notice.getByRole("button", { name: "Switch account", exact: true })).toBeVisible();

    const box = (await notice.boundingBox())!;
    const bar = (await page.getByRole("region", { name: "Status bar", exact: true }).boundingBox())!;
    expect(box.x).toBe(12);
    expect(box.y + box.height).toBeLessThanOrEqual(bar.y - 12);
    await expect(notice).toHaveScreenshot(`account-swap-notice-${theme}.png`);
  });

  test(`stale panes ask to restart on the new account in ${theme} mode`, async ({ page }) => {
    await openVisualState(page, `account-restart-${theme}`);
    const codex = chip(page, IDS.done);
    await expect(codex).toContainText("Codex is still signed in as ops@acme.example");
    await expect(codex).toContainText(
      "Restart this session to use lab@acme.example. It resumes the same conversation.",
    );
    await expect(codex.getByRole("button", { name: "Keep old account", exact: true })).toBeVisible();
    await expect(codex.getByRole("button", { name: "Restart", exact: true })).toBeVisible();
    // The pane that waits for an approval runs the active account.
    await expect(chip(page, IDS.approval)).toHaveCount(0);

    // The Claude pane works, so its restart waits for the end of the turn.
    const claude = chip(page, IDS.working);
    await expect(claude).toContainText("Claude is still signed in as auditor.personal@example.com");
    await claude.getByRole("button", { name: "Restart", exact: true }).click();
    await expect(claude).toContainText("Restarts on dev@acme.example when the current turn ends.");
    await expect(claude.getByRole("button", { name: "Restart", exact: true })).toHaveCount(0);
    await expect(pane(page, IDS.working)).toBeVisible();

    await expect(codex).toHaveScreenshot(`account-restart-chip-${theme}.png`);
    await expect(claude).toHaveScreenshot(`account-restart-chip-queued-${theme}.png`);
  });

  test(`the status bar prompt restarts every stale pane in ${theme} mode`, async ({ page }) => {
    await openVisualState(page, `account-restart-${theme}`);
    await expect(chip(page, IDS.done)).toBeVisible();
    await page.locator('[data-status-provider="codex"]').click();
    const popover = page.getByRole("dialog", { name: "Codex usage and accounts" });
    const prompt = popover.locator('[data-account-restart-prompt="codex"]');
    await expect(prompt).toContainText("1 Codex session is still on the old account.");
    await expect(prompt).toContainText("Each session restarts when its current turn ends.");
    await expect(popover).toHaveScreenshot(`account-restart-prompt-${theme}.png`);

    await prompt.getByRole("button", { name: "Restart session", exact: true }).click();
    await expect(prompt).toHaveCount(0);
    await expect(pane(page, IDS.done)).toHaveCount(0);
  });
}

test("a resting pane restarts in place and resumes its CLI session", async ({ page }) => {
  await openVisualState(page, "account-restart-dark");
  const codex = chip(page, IDS.done);
  await codex.getByRole("button", { name: "Restart", exact: true }).click();

  // A new session takes the slot of the old pane, with the same CLI session.
  await expect(pane(page, IDS.done)).toHaveCount(0);
  await expect
    .poll(() =>
      page.evaluate(() => (window as HarnessWindow).__harnessTerminalCreateRequests ?? []),
    )
    .toContain("codex-refresh-api-service");
  await expect(page.locator(".terminal-pane")).toHaveCount(3);
  // Only the working Claude pane still runs an old account.
  await expect(page.getByRole("region", { name: "Account switched", exact: true })).toHaveCount(1);
});

test("Keep old account leaves the pane alone until the next switch", async ({ page }) => {
  await openVisualState(page, "account-restart-dark");
  const codex = chip(page, IDS.done);
  await codex.getByRole("button", { name: "Keep old account", exact: true }).click();
  await expect(codex).toHaveCount(0);
  await expect(pane(page, IDS.done)).toBeVisible();
  // A focus refresh asks Rust again; the kept pane does not come back.
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(chip(page, IDS.done)).toHaveCount(0);
});

test("one click switches through the normal path, and the notice stays away", async ({ page }) => {
  await openVisualState(page, "account-swap-dark");
  const notice = swapNotice(page);
  await notice.getByRole("button", { name: "Switch account", exact: true }).click();
  await expect(notice).toBeHidden();
  const pill = page.locator('[data-status-provider="claude"]');
  await expect(pill).toHaveAttribute("title", "Claude · auditor.personal@example.com");
  await expect(pill).toContainText("8% used");
  await expect(notice).toBeHidden();
});

test("Later hides the notice, and the setting keeps it off", async ({ page }) => {
  await openVisualState(page, "account-swap-dark");
  const notice = swapNotice(page);
  await notice.getByRole("button", { name: "Later", exact: true }).click();
  await expect(notice).toBeHidden();

  await page.addInitScript(() => {
    window.localStorage.setItem("halv.account-swap-notice.v1", "off");
  });
  await openVisualState(page, "account-swap-dark");
  await waitForClaudeUsage(page);
  await expect(page.locator('[data-status-provider="claude"]')).toContainText("92% used");
  await expect(swapNotice(page)).toHaveCount(0);
});

// Automatic rotation of Claude accounts (ADR 0017). The setting is off by
// default; these tests turn it on in the localStorage of the viewer.
const ROTATION_KEY = "halv.account-auto-rotation.v1";

async function turnOnRotation(
  page: Page,
  settings: { startPercent: number; latestPercent: number } = {
    startPercent: 92,
    latestPercent: 99,
  },
) {
  await page.addInitScript(
    ({ key, value }) => window.localStorage.setItem(key, value),
    { key: ROTATION_KEY, value: JSON.stringify({ enabled: true, ...settings }) },
  );
}

function rotationCard(page: Page) {
  return page.getByRole("region", { name: "Automatic account switch", exact: true });
}

for (const theme of ["dark", "light"] as const) {
  test(`automatic rotation switches Claude and says so in ${theme} mode`, async ({ page }) => {
    await turnOnRotation(page);
    await openVisualState(page, `account-swap-${theme}`);
    const card = rotationCard(page);
    await expect(card).toContainText("Switched Claude to auditor.personal@example.com");
    await expect(card).toContainText("dev@acme.example reached 92% of its limit.");
    await expect(card.getByRole("button", { name: "Dismiss", exact: true })).toBeVisible();
    const pill = page.locator('[data-status-provider="claude"]');
    await expect(pill).toHaveAttribute("title", "Claude · auditor.personal@example.com");
    await expect(pill).toContainText("8% used");
    // The rotation replaces the assisted card.
    await expect(swapNotice(page)).toHaveCount(0);
    // The card never takes the focus.
    expect(await card.evaluate((element) => element.contains(document.activeElement))).toBe(false);

    const box = (await card.boundingBox())!;
    const bar = (await page.getByRole("region", { name: "Status bar", exact: true }).boundingBox())!;
    expect(box.x).toBe(12);
    expect(box.y + box.height).toBeLessThanOrEqual(bar.y - 12);
    await expect(card).toHaveScreenshot(`account-auto-rotation-${theme}.png`);
  });

  test(`the rotation settings show two percents in ${theme} mode`, async ({ page }) => {
    await page.goto(`/ux-audit.html?state=marketing-app-${theme}`);
    await page.getByLabel("Workspace and application menu", { exact: true }).click();
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
    await dialog.getByRole("button", { name: /^AI connections/ }).click();
    const group = dialog.locator("[data-account-switch-settings]");
    const toggle = group.getByRole("checkbox", { name: /^Rotate Claude accounts automatically/ });
    await expect(toggle).not.toBeChecked();
    await expect(group.getByRole("spinbutton")).toHaveCount(0);

    await toggle.check();
    const start = group.getByRole("spinbutton", { name: "Start at", exact: true });
    const latest = group.getByRole("spinbutton", { name: "Switch by", exact: true });
    await expect(start).toHaveValue("92");
    await expect(latest).toHaveValue("99");
    await expect(group).toContainText(
      "Between 92% and 99% of a limit, Halv switches to the Claude account with the most room. It waits for a pause between turns until 99%.",
    );
    // The screenshot shows the resting controls, without hover or focus.
    await toggle.blur();
    await page.mouse.move(0, 0);
    await expect(group).toHaveScreenshot(`account-rotation-settings-${theme}.png`);
  });
}

test("the rotation settings clamp bad input and persist", async ({ page }) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark");
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await dialog.getByRole("button", { name: /^AI connections/ }).click();
  const group = dialog.locator("[data-account-switch-settings]");
  await group.getByRole("checkbox", { name: /^Rotate Claude accounts automatically/ }).check();
  const start = group.getByRole("spinbutton", { name: "Start at", exact: true });
  const latest = group.getByRole("spinbutton", { name: "Switch by", exact: true });

  await start.fill("30");
  await start.press("Enter");
  await expect(start).toHaveValue("50");
  await latest.fill("140");
  await latest.blur();
  await expect(latest).toHaveValue("100");
  // An empty field keeps the stored value.
  await latest.fill("");
  await latest.press("Enter");
  await expect(latest).toHaveValue("100");
  // The latest percent never goes below the start percent.
  await start.fill("97");
  await start.press("Enter");
  await latest.fill("90");
  await latest.press("Enter");
  await expect(latest).toHaveValue("97");
  expect(
    JSON.parse((await page.evaluate((key) => window.localStorage.getItem(key), ROTATION_KEY))!),
  ).toEqual({ enabled: true, startPercent: 97, latestPercent: 97 });

  await group.getByRole("checkbox", { name: /^Rotate Claude accounts automatically/ }).uncheck();
  await expect(group.getByRole("spinbutton")).toHaveCount(0);
  expect(
    JSON.parse((await page.evaluate((key) => window.localStorage.getItem(key), ROTATION_KEY))!),
  ).toEqual({ enabled: false, startPercent: 97, latestPercent: 97 });
});

test("below the start percent, nothing rotates and the assisted card stays away", async ({ page }) => {
  await turnOnRotation(page, { startPercent: 95, latestPercent: 99 });
  await openVisualState(page, "account-swap-dark");
  await waitForClaudeUsage(page);
  const pill = page.locator('[data-status-provider="claude"]');
  await expect(pill).toContainText("92% used");
  // Give a rotation or a card the time that a switch takes in Orca mode.
  await page.waitForTimeout(1_000);
  await expect(pill).toHaveAttribute("title", "Claude · dev@acme.example");
  await expect(rotationCard(page)).toHaveCount(0);
  await expect(swapNotice(page)).toHaveCount(0);
});

test("without a target below the start percent, the assisted card is the fallback", async ({ page }) => {
  // personal@ is at 64 % and research@ at 92 %: none is below 50 %.
  await turnOnRotation(page, { startPercent: 50, latestPercent: 99 });
  await openVisualState(page, "account-swap-dark");
  const notice = swapNotice(page);
  await expect(notice).toContainText("Claude usage is at 92%");
  await expect(notice).toContainText(
    "Switch to auditor.personal@example.com? It has used 64% of its limits.",
  );
  await expect(rotationCard(page)).toHaveCount(0);
  await expect(page.locator('[data-status-provider="claude"]')).toHaveAttribute(
    "title",
    "Claude · dev@acme.example",
  );
});

test("the rotation waits for the Claude turns to end", async ({ page }) => {
  await turnOnRotation(page);
  await openVisualState(page, "account-rotation-busy-dark");
  await waitForClaudeUsage(page);
  const pill = page.locator('[data-status-provider="claude"]');
  await expect(pill).toContainText("92% used");
  // One Claude pane works, and one waits for an approval.
  await page.waitForTimeout(1_000);
  await expect(rotationCard(page)).toHaveCount(0);
  await expect(pill).toHaveAttribute("title", "Claude · dev@acme.example");

  const rest = (sessionId: string, turnSeq: number) =>
    page.evaluate(
      async ({ entry }) => {
        // @ts-expect-error Served by the harness dev server.
        const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
        await emitHarnessEvent("agent-status", entry);
      },
      {
        entry: {
          sessionId,
          status: "done",
          updatedAt: Date.parse("2026-08-13T15:00:00.000Z"),
          unread: true,
          waitSeq: 0,
          turnSeq,
          source: "presence",
        },
      },
    );
  await rest(IDS.working, 3);
  await page.waitForTimeout(500);
  // The approval pane is still busy.
  await expect(rotationCard(page)).toHaveCount(0);
  await rest(IDS.approval, 4);
  await expect(rotationCard(page)).toContainText(
    "Switched Claude to auditor.personal@example.com",
  );
  await expect(pill).toHaveAttribute("title", "Claude · auditor.personal@example.com");
});

test("at the latest percent, the rotation does not wait for busy panes", async ({ page }) => {
  await turnOnRotation(page, { startPercent: 90, latestPercent: 92 });
  await openVisualState(page, "account-rotation-busy-dark");
  await expect(rotationCard(page)).toContainText(
    "Switched Claude to auditor.personal@example.com",
  );
  // Escape closes the card when the focus is in it.
  await rotationCard(page).getByRole("button", { name: "Dismiss", exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(rotationCard(page)).toHaveCount(0);
});

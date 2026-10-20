import { expect, test, type Page } from "@playwright/test";
import { AGENT_STATUS_SESSION_IDS as IDS } from "../../src/harness/agent-status.ts";
import { openVisualState } from "./capture";

type HarnessWindow = Window & {
  __harnessTerminalNotifications?: Array<{
    sessionId: string;
    title: string;
    body: string;
  }>;
};

function pane(page: Page, sessionId: string) {
  return page.locator(`.terminal-pane[data-terminal-session-id="${sessionId}"]`);
}

function conversationRow(page: Page, title: string) {
  return page.locator(".group button", { hasText: title }).first();
}

function terminalRow(page: Page, title: string) {
  return page.locator("button", { hasText: title }).filter({
    has: page.locator('[data-agent-glyph], [role="status"]'),
  }).first();
}

async function emit(page: Page, event: string, payload: unknown) {
  await page.evaluate(
    async ({ event, payload }) => {
      // @ts-expect-error Served by the harness dev server.
      const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
      await emitHarnessEvent(event, payload);
    },
    { event, payload },
  );
}

async function notificationsFor(page: Page, sessionId: string) {
  return page.evaluate(
    (id) =>
      ((window as HarnessWindow).__harnessTerminalNotifications ?? []).filter(
        (notification) => notification.sessionId === id,
      ),
    sessionId,
  );
}

for (const theme of ["dark", "light"] as const) {
  test(`each agent state has its glyph in the ${theme} theme`, async ({ page }) => {
    await openVisualState(page, `agent-status-${theme}`);

    await expect(pane(page, IDS.approval).locator('[data-agent-glyph="attention"]')).toBeVisible();
    await expect(pane(page, IDS.working).locator('[data-agent-glyph="working"]')).toBeVisible();
    await expect(pane(page, IDS.done).locator('[data-agent-glyph="done"]')).toBeVisible();
    await expect(page.locator('.terminal-status-label[data-agent-status="needsApproval"]'))
      .toHaveText("needs approval");

    await expect(
      conversationRow(page, "Authentication middleware refactor").locator('[data-agent-glyph="attention"]'),
    ).toBeVisible();
    await expect(
      conversationRow(page, "Audit agent refactor").locator('[data-agent-glyph="attention"]'),
    ).toHaveAttribute("aria-label", "Needs input");
    await expect(
      conversationRow(page, "Release checklist").locator('[data-agent-glyph="done"]'),
    ).toBeVisible();
    await expect(terminalRow(page, "Grok 1").locator('[data-agent-glyph="failed"]')).toBeVisible();

    // The question is orange; running work keeps the coral accent.
    const colors = await page.evaluate(() => {
      const question = document.querySelector('[data-agent-glyph="attention"]')!;
      const accent = getComputedStyle(document.documentElement)
        .getPropertyValue("--accent")
        .trim();
      return { question: getComputedStyle(question).color, accent };
    });
    expect(colors.question).toBe(
      theme === "dark" ? "rgb(249, 115, 22)" : "rgb(234, 88, 12)",
    );
    expect(colors.question).not.toBe(`rgb(${colors.accent.replace(/ /g, ", ")})`);

    await expect(page).toHaveScreenshot(`agent-status-${theme}.png`);
  });
}

test("reading a pane clears its check but never its question", async ({ page }) => {
  await openVisualState(page, "agent-status-dark");
  await pane(page, IDS.done).locator(".terminal-pane-header").click();
  await expect(pane(page, IDS.done).locator("[data-agent-glyph]")).toHaveCount(0);

  await pane(page, IDS.approval).locator(".terminal-pane-header").click();
  await expect(pane(page, IDS.approval).locator('[data-agent-glyph="attention"]')).toBeVisible();
  await expect(
    conversationRow(page, "Authentication middleware refactor").locator('[data-agent-glyph="attention"]'),
  ).toBeVisible();
});

test("a new wait sends one banner and its click focuses the pane", async ({ page }) => {
  await openVisualState(page, "agent-status-dark");
  await pane(page, IDS.approval).locator(".terminal-pane-header").click();
  await emit(page, "agent-status", {
    sessionId: IDS.working,
    status: "needsInput",
    reason: "question",
    updatedAt: Date.parse("2026-08-13T15:00:00.000Z"),
    unread: false,
    waitSeq: 1,
    turnSeq: 2,
    source: "presence",
    prompt: { kind: "question", text: "Which store should I use?" },
  });
  await expect(pane(page, IDS.working).locator('[data-agent-glyph="attention"]')).toBeVisible();
  await expect.poll(() => notificationsFor(page, IDS.working)).toEqual([
    {
      sessionId: IDS.working,
      title: "Claude has a question",
      body: "Authentication middleware refactor · Which store should I use?",
    },
  ]);

  // The pane the user looks at gets no banner for its own wait.
  expect(await page.evaluate(() => document.hasFocus())).toBe(true);
  const before = (await notificationsFor(page, IDS.approval)).length;
  await emit(page, "agent-status", {
    sessionId: IDS.approval,
    status: "needsApproval",
    reason: "permission",
    updatedAt: Date.parse("2026-08-13T15:00:00.000Z"),
    unread: false,
    waitSeq: 3,
    turnSeq: 3,
    source: "presence",
  });
  await page.waitForTimeout(1_500);
  expect(await notificationsFor(page, IDS.approval)).toHaveLength(before);

  await emit(page, "terminal-notification-clicked", { sessionId: IDS.working });
  await expect(pane(page, IDS.working)).toHaveClass(/terminal-pane-active/);
  await expect(pane(page, IDS.working).locator(".xterm-helper-textarea")).toBeFocused();
});

test("a banner carries the prompt detail of the wait", async ({ page }) => {
  await openVisualState(page, "agent-status-dark");
  // The user looks at the approval pane, so the other panes earn banners.
  await pane(page, IDS.approval).locator(".terminal-pane-header").click();
  const card = page.locator("[data-harness-notification-card]");
  const showCardFor = (sessionId: string) =>
    page.evaluate((id) => {
      (window as Window & { __harnessNotificationCardFor?: string })
        .__harnessNotificationCardFor = id;
    }, sessionId);
  await showCardFor(IDS.working);

  // Claude: the presence file starts the wait, and the hook then adds the
  // command to the same wait. One banner carries the command.
  const wait = {
    sessionId: IDS.working,
    status: "needsApproval",
    reason: "permission",
    updatedAt: Date.parse("2026-08-13T15:00:00.000Z"),
    unread: false,
    waitSeq: 1,
    turnSeq: 2,
    source: "presence",
  };
  await emit(page, "agent-status", wait);
  await emit(page, "agent-status", {
    ...wait,
    prompt: { kind: "approval", toolName: "Bash", command: "pnpm test --filter auth" },
  });
  await expect.poll(() => notificationsFor(page, IDS.working)).toEqual([
    {
      sessionId: IDS.working,
      title: "Claude needs your approval",
      body: "Authentication middleware refactor · Claude wants to run: pnpm test --filter auth",
    },
  ]);
  await expect(card).toContainText("Claude wants to run: pnpm test --filter auth");
  await expect(card).toHaveScreenshot("agent-status-notification-claude.png");

  // Codex: a terminal notification starts the wait with the file to edit.
  await showCardFor(IDS.done);
  await emit(page, "agent-status", {
    sessionId: IDS.done,
    status: "needsApproval",
    reason: "permission",
    updatedAt: Date.parse("2026-08-13T15:01:00.000Z"),
    unread: false,
    waitSeq: 1,
    turnSeq: 5,
    source: "terminal",
    prompt: { kind: "approval", toolName: "apply_patch", filePath: "src/middleware/session.ts" },
  });
  await expect(pane(page, IDS.done).locator('[data-agent-glyph="attention"]')).toBeVisible();
  // The pane also had an unread finish at load, so check the last banner.
  await expect.poll(async () => (await notificationsFor(page, IDS.done)).at(-1)).toEqual({
    sessionId: IDS.done,
    title: "Codex needs your approval",
    body: "Authentication middleware refactor · Codex wants to edit src/middleware/session.ts",
  });
  await expect(card).toContainText("Codex wants to edit src/middleware/session.ts");
  await expect(card).toHaveScreenshot("agent-status-notification-codex.png");
});

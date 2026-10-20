import { expect, test, type Page } from "@playwright/test";

async function openCli(page: Page) {
  await page.goto("/ux-audit.html?state=marketing-app-dark&ghostty=off");
  const conversation = page.locator(".group button", { hasText: "Authentication middleware refactor" }).first();
  const pane = page.locator('.terminal-pane[data-terminal-scene-complete="true"]').first();
  await expect(pane).toBeVisible();
  const id = (await pane.getAttribute("data-terminal-session-id"))!;
  await pane.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type("Check CLI activity");
  await page.keyboard.press("Enter");
  await expect(conversation.locator(".conversation-spinner")).toBeVisible();
  return { id, conversation, pane };
}

async function output(page: Page, sessionId: string, sequence: number, replay = false) {
  await page.evaluate(async ({ sessionId, sequence, replay }) => {
    // @ts-expect-error Served by the harness dev server.
    const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
    await emitHarnessEvent("terminal_event", {
      sessionId, sequence, replay, kind: "data", data: "answer output\r\n".repeat(100),
    });
  }, { sessionId, sequence, replay });
}

async function leaveConversation(page: Page) {
  await page.getByRole("button", { name: "New chat", exact: true }).click();
  await expect(page.locator(".terminal-pane:visible")).toHaveCount(0);
}

test("reading a finished CLI clears its check without restarting work", async ({ page }) => {
  const { id, conversation, pane } = await openCli(page);
  await page.clock.install();
  await output(page, id, 1);
  await page.clock.runFor(1_500);
  // A turn that ends out of sight stays unread.
  await leaveConversation(page);
  await page.clock.runFor(8_000);
  await expect(conversation.locator('[data-agent-glyph="done"]')).toBeVisible();

  await conversation.click();
  await expect(conversation.locator("[role=status]")).toHaveCount(0);
  await output(page, id, 2); // Full redraw on focus or resize.
  await page.clock.runFor(10_000);
  await expect(conversation.locator("[role=status]")).toHaveCount(0);
  await expect(pane.locator("[data-agent-glyph]")).toHaveCount(0);

  await leaveConversation(page);
  await conversation.click();
  await expect(pane).toBeVisible();
  await output(page, id, 3, true); // Reconnect replay is not a new turn.
  await output(page, id, 4);
  await expect(conversation.locator("[role=status]")).toHaveCount(0);

  await pane.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type("continue");
  await page.keyboard.press("Enter");
  await expect(conversation.locator(".conversation-spinner")).toBeVisible();
});

test("a CLI turn that ends in the watched pane is already read", async ({ page }) => {
  const { id, conversation, pane } = await openCli(page);
  await page.clock.install();
  await output(page, id, 1);
  await page.clock.runFor(1_500);
  await page.clock.runFor(8_000);
  await expect(conversation.locator(".conversation-spinner")).toHaveCount(0);
  await expect(conversation.locator("[role=status]")).toHaveCount(0);
  await expect(pane.locator("[data-agent-glyph]")).toHaveCount(0);
  await expect(page.locator('.terminal-status-label[data-agent-status="done"]')).toBeVisible();
});

test("response pauses and background navigation preserve one running turn", async ({ page }) => {
  const { id, conversation } = await openCli(page);
  await page.clock.install();
  await output(page, id, 1);
  await page.clock.runFor(1_500);
  await leaveConversation(page);
  for (let sequence = 2; sequence <= 4; sequence++) {
    await page.clock.runFor(4_000);
    await expect(conversation.locator(".conversation-spinner")).toBeVisible();
    await expect(conversation.locator('[data-agent-glyph="done"]')).toHaveCount(0);
    await output(page, id, sequence);
    await page.clock.runFor(1_500);
  }
  await page.clock.runFor(8_000);
  await expect(conversation.locator(".conversation-spinner")).toHaveCount(0);
  await expect(conversation.locator('[data-agent-glyph="done"]')).toBeVisible();
  await conversation.click();
  await expect(conversation.locator("[role=status]")).toHaveCount(0);
});

test("semantic CLI completion survives a long pause and reading does not restart it", async ({ page }) => {
  const { id, conversation } = await openCli(page);
  await page.clock.install();
  await page.evaluate(async (sessionId) => {
    const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
    await emitHarnessEvent("agent-session-event", { sessionId,
      event: { type: "embeddedTurnStarted", turnId: "long-turn" } });
  }, id);
  await output(page, id, 1);
  await page.clock.runFor(30_000);
  await expect(conversation.locator(".conversation-spinner")).toBeVisible();
  await expect(conversation.locator('[data-agent-glyph="done"]')).toHaveCount(0);
  await leaveConversation(page);
  await page.evaluate(async (sessionId) => {
    const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
    await emitHarnessEvent("agent-session-event", { sessionId,
      event: { type: "embeddedTurnFinished", turnId: "long-turn", outcome: "completed" } });
  }, id);
  await expect(conversation.locator(".conversation-spinner")).toHaveCount(0);
  await expect(conversation.locator('[data-agent-glyph="done"]')).toBeVisible();
  await conversation.click();
  await output(page, id, 2);
  await page.clock.runFor(10_000);
  await expect(conversation.locator("[role=status]")).toHaveCount(0);
});

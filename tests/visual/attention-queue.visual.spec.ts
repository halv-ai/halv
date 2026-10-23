import { expect, test, type Locator, type Page } from "@playwright/test";
import { AGENT_STATUS_SESSION_IDS as IDS } from "../../src/harness/agent-status.ts";
import { FIXED_VISUAL_NOW } from "../../src/harness/states.ts";
import { openVisualState } from "./capture";

// The attention queue. The agent-status state holds six panes: two wait
// for the user, three have a result that the user did not view, and one
// works. Oldest first, the queue is: failed (10m), finished (5m), question
// (2m), done (1m), approval (45s).

const NOW = Date.parse(FIXED_VISUAL_NOW);

const OLDEST_FIRST = [
  `pane:${IDS.failed}`,
  `pane:${IDS.finished}`,
  `pane:${IDS.question}`,
  `pane:${IDS.done}`,
  `pane:${IDS.approval}`,
];

function pane(page: Page, sessionId: string) {
  return page.locator(`.terminal-pane[data-terminal-session-id="${sessionId}"]`);
}

function dock(page: Page) {
  return page.locator('[data-attention-queue="dock"]');
}

function chip(page: Page) {
  return page.locator('[data-attention-queue="chip"]');
}

function popover(page: Page) {
  return page.locator("[data-attention-queue-popover]");
}

function dockRow(page: Page, key: string) {
  return dock(page).locator(`[data-attention-row="${key}"]`);
}

async function keysIn(list: Locator) {
  return list
    .locator("[data-attention-row]")
    .evaluateAll((rows) => rows.map((row) => row.getAttribute("data-attention-row")));
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

function agentStatus(sessionId: string, overrides: Record<string, unknown> = {}) {
  return {
    sessionId,
    status: "idle",
    updatedAt: NOW - 1000,
    unread: false,
    waitSeq: 0,
    turnSeq: 5,
    source: "presence",
    ...overrides,
  };
}

// The pane that the user looks at is not in the queue. A pane that works is
// never in it, so focusing that pane shows the whole queue.
async function lookAtWorkingPane(page: Page) {
  await pane(page, IDS.working).locator(".terminal-pane-header").click();
  await expect(pane(page, IDS.working)).toHaveClass(/terminal-pane-active/);
}

async function openQueue(page: Page, theme: "dark" | "light" = "dark") {
  await openVisualState(page, `agent-status-${theme}`);
  await lookAtWorkingPane(page);
  await expect(dock(page)).toBeVisible();
}

for (const theme of ["dark", "light"] as const) {
  test(`the dock lists waits and unread results oldest first in the ${theme} theme`, async ({
    page,
  }) => {
    await openQueue(page, theme);
    await expect(dock(page).locator("[data-attention-title]")).toHaveText("5 in queue");

    // Three rows and a "+2 more" button. The head row has the accent bar.
    expect(await keysIn(dock(page))).toEqual(OLDEST_FIRST.slice(0, 3));
    await expect(dock(page).locator("[data-attention-more]")).toHaveText("+2 more");
    await expect(dock(page).locator("[data-attention-accent]")).toHaveCount(1);
    await expect(dockRow(page, OLDEST_FIRST[0]).locator("[data-attention-accent]")).toBeVisible();

    // Colour is the urgency: failed red, finished green, a question orange.
    await expect(dockRow(page, `pane:${IDS.failed}`).locator('[data-agent-glyph="failed"]')).toBeVisible();
    await expect(dockRow(page, `pane:${IDS.finished}`).locator('[data-agent-glyph="done"]')).toBeVisible();
    await expect(dockRow(page, `pane:${IDS.question}`).locator('[data-agent-glyph="attention"]')).toBeVisible();

    // Each row names the conversation, or the terminal without one, and its age.
    await expect(dockRow(page, `pane:${IDS.failed}`)).toContainText("Grok 1");
    await expect(dockRow(page, `pane:${IDS.failed}`).locator("[data-attention-age]")).toHaveText("10m");
    await expect(dockRow(page, `pane:${IDS.finished}`)).toContainText("Release checklist");
    await expect(dockRow(page, `pane:${IDS.question}`)).toContainText("Audit agent refactor");
  });
}

test("a waiting pane in view is not in the queue", async ({ page }) => {
  await openVisualState(page, "agent-status-dark");
  // The first pane of the open conversation asks for approval and has the
  // focus, so four items show: three rows and "+1 more".
  await expect(dock(page)).toBeVisible();
  await expect(dock(page).locator("[data-attention-title]")).toHaveText("4 in queue");
  await expect(dock(page).locator("[data-attention-more]")).toHaveText("+1 more");
  // The user looks at another pane. The approval returns at its place.
  await lookAtWorkingPane(page);
  await expect(dock(page).locator("[data-attention-title]")).toHaveText("5 in queue");
  await expect(dock(page).locator("[data-attention-more]")).toHaveText("+2 more");
});

test("the +N more button opens a popover with every row", async ({ page }) => {
  await openQueue(page);
  const more = dock(page).locator("[data-attention-more]");
  await more.click();
  await expect(popover(page)).toBeVisible();
  await expect(popover(page)).toContainText("5 in queue");
  await expect(popover(page)).toContainText("Oldest first");
  expect(await keysIn(popover(page))).toEqual(OLDEST_FIRST);
  // The popover adds the question or the command of the wait.
  await expect(popover(page).locator(`[data-attention-row="pane:${IDS.question}"]`)).toContainText(
    "Which session store should the middleware use?",
  );
  await expect(popover(page).locator(`[data-attention-row="pane:${IDS.approval}"]`)).toContainText(
    "Claude wants to run: pnpm test --filter auth",
  );
  // Three rows are finished, so the footer offers "Mark all as read".
  await expect(popover(page).locator("[data-attention-mark-all]")).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(popover(page)).toHaveCount(0);
  await expect(more).toBeFocused();
});

test("a click outside closes the popover", async ({ page }) => {
  await openQueue(page);
  await dock(page).locator("[data-attention-more]").click();
  await expect(popover(page)).toBeVisible();
  await page.locator(".terminal-pane-header").first().click();
  await expect(popover(page)).toHaveCount(0);
});

test("a click on a finished row reads it and opens its pane", async ({ page }) => {
  await openQueue(page);
  await dockRow(page, `pane:${IDS.finished}`).getByRole("button").first().click();
  await expect(pane(page, IDS.finished)).toHaveClass(/terminal-pane-active/);
  await expect(dock(page).locator(`[data-attention-row="pane:${IDS.finished}"]`)).toHaveCount(0);
  // The open pane of another row is unaffected.
  expect(await keysIn(dock(page))).toContain(`pane:${IDS.failed}`);
});

test("a waiting row hides while its pane is in view and returns when the user leaves", async ({
  page,
}) => {
  await openQueue(page);
  await dockRow(page, `pane:${IDS.question}`).getByRole("button").first().click();
  await expect(pane(page, IDS.question)).toHaveClass(/terminal-pane-active/);
  await expect(dock(page).locator(`[data-attention-row="pane:${IDS.question}"]`)).toHaveCount(0);

  // The question stays unanswered. Back in the first conversation it
  // returns, still ahead of the rows that entered after it.
  await page.locator(".group button", { hasText: "Authentication middleware refactor" }).first().click();
  await expect(dock(page).locator(`[data-attention-row="pane:${IDS.question}"]`)).toBeVisible();
  const keys = (await keysIn(dock(page))).filter((key): key is string => key !== null);
  const done = keys.indexOf(`pane:${IDS.done}`);
  if (done >= 0) expect(keys.indexOf(`pane:${IDS.question}`)).toBeLessThan(done);
});

test("an answered wait leaves for good and a new wait enters at the back", async ({ page }) => {
  await openQueue(page);
  await emit(page, "agent-status", agentStatus(IDS.approval, { status: "working", turnSeq: 4 }));
  await expect(dock(page).locator(`[data-attention-row="pane:${IDS.approval}"]`)).toHaveCount(0);
  expect(await keysIn(dock(page))).toEqual(OLDEST_FIRST.slice(0, 3));

  await emit(
    page,
    "agent-status",
    agentStatus(IDS.approval, {
      status: "needsApproval",
      reason: "permission",
      updatedAt: NOW,
      waitSeq: 3,
      turnSeq: 4,
    }),
  );
  await dock(page).locator("[data-attention-more]").click();
  const keys = await keysIn(popover(page));
  expect(keys.at(-1)).toBe(`pane:${IDS.approval}`);
  expect(keys).toHaveLength(5);
});

test("Mark as read clears a finished row and keeps the waits", async ({ page }) => {
  await openQueue(page);
  const row = dockRow(page, `pane:${IDS.finished}`);
  await row.hover();
  await row.locator("[data-attention-mark-read]").click();
  await expect(row).toHaveCount(0);
  await expect(dock(page).locator("[data-attention-title]")).toHaveText("4 in queue");
  // No jump: the first conversation stays open and the wait rows have no mark button.
  await expect(page.locator(".terminal-pane-active .terminal-pane-header")).toBeVisible();
  await expect(dockRow(page, `pane:${IDS.question}`).locator("[data-attention-mark-read]")).toHaveCount(0);
});

test("Mark all as read clears every finished row and keeps the waits", async ({ page }) => {
  await openQueue(page);
  await dock(page).locator("[data-attention-more]").click();
  await popover(page).locator("[data-attention-mark-all]").click();
  await expect(popover(page)).toHaveCount(0);
  expect(await keysIn(dock(page))).toEqual([`pane:${IDS.question}`, `pane:${IDS.approval}`]);
  await expect(dock(page).locator("[data-attention-more]")).toHaveCount(0);
});

test("an empty queue renders nothing", async ({ page }) => {
  await openQueue(page);
  for (const sessionId of Object.values(IDS)) {
    await emit(page, "agent-status", agentStatus(sessionId));
  }
  await expect(page.locator("[data-attention-queue]")).toHaveCount(0);
  await expect(page.locator("[data-attention-queue-popover]")).toHaveCount(0);
});

test("one waiting item shows one row", async ({ page }) => {
  await openQueue(page);
  for (const [name, sessionId] of Object.entries(IDS)) {
    if (name === "question") continue;
    await emit(page, "agent-status", agentStatus(sessionId, { status: "working" }));
  }
  await expect(dock(page).locator("[data-attention-title]")).toHaveText("1 in queue");
  expect(await keysIn(dock(page))).toEqual([`pane:${IDS.question}`]);
  await expect(dock(page).locator("[data-attention-more]")).toHaveCount(0);
});

test("a short window shows one row and the more button", async ({ page }) => {
  await openVisualState(page, "agent-status-dark");
  await lookAtWorkingPane(page);
  await page.setViewportSize({ width: 1280, height: 650 });
  await expect(dock(page).locator("[data-attention-row]")).toHaveCount(1);
  await expect(dock(page).locator("[data-attention-more]")).toHaveText("+4 more");
});

test("a collapsed sidebar moves the queue to a chip at the left of the status bar", async ({
  page,
}) => {
  await openQueue(page);
  await page.getByRole("button", { name: "Collapse sidebar" }).click();
  await expect(dock(page)).toHaveCount(0);
  await expect(chip(page)).toBeVisible();
  // A wide bar shows the full label; a narrow one shows the count only.
  await expect(chip(page)).toHaveText("5 in queue", { useInnerText: true });
  const first = page.locator('[role="region"][aria-label="Status bar"] > *').first();
  await expect(first).toHaveAttribute("data-attention-queue", "chip");

  await chip(page).click();
  expect(await keysIn(popover(page))).toEqual(OLDEST_FIRST);
  // A row in the popover opens its item and closes the popover.
  await popover(page).locator(`[data-attention-row="pane:${IDS.finished}"]`).getByRole("button").first().click();
  await expect(popover(page)).toHaveCount(0);
  await expect(pane(page, IDS.finished)).toHaveClass(/terminal-pane-active/);

  await page.getByRole("button", { name: "Expand sidebar" }).click();
  await expect(chip(page)).toHaveCount(0);
  await expect(dock(page)).toBeVisible();
});

test("the next chord opens the oldest item, then the next one", async ({ page }) => {
  await openQueue(page);
  const chord = "ControlOrMeta+Shift+U";
  await page.keyboard.press(chord);
  // The failed result is read and leaves. The finished one is now first.
  await expect(dock(page).locator(`[data-attention-row="pane:${IDS.failed}"]`)).toHaveCount(0);
  expect((await keysIn(dock(page)))[0]).toBe(`pane:${IDS.finished}`);

  await page.keyboard.press(chord);
  await expect(dock(page).locator(`[data-attention-row="pane:${IDS.finished}"]`)).toHaveCount(0);
  await expect(pane(page, IDS.finished)).toHaveClass(/terminal-pane-active/);
});

test("the next chord never reaches the terminal, also with an empty queue", async ({ page }) => {
  await openQueue(page);
  for (const sessionId of Object.values(IDS)) {
    await emit(page, "agent-status", agentStatus(sessionId));
  }
  await expect(page.locator("[data-attention-queue]")).toHaveCount(0);
  await page.evaluate(() => {
    const target = window as Window & { __chordKeys?: string[] };
    target.__chordKeys = [];
    document.addEventListener("keydown", (event) => target.__chordKeys!.push(event.key), true);
  });
  await page.keyboard.press("ControlOrMeta+Shift+U");
  // The modifier keys arrive. The letter key does not.
  const keys = await page.evaluate(
    () => (window as Window & { __chordKeys?: string[] }).__chordKeys,
  );
  expect(keys).not.toContain("u");
  expect(keys).not.toContain("U");
});

// Baselines come from the final stage. These tests have "screenshot" in
// their names so that a run without baselines can leave them out.
for (const theme of ["dark", "light"] as const) {
  test(`attention queue dock ${theme} screenshot`, async ({ page }) => {
    await openQueue(page, theme);
    await expect(dock(page)).toHaveScreenshot(`attention-queue-dock-${theme}.png`);
  });

  test(`attention queue popover ${theme} screenshot`, async ({ page }) => {
    await openQueue(page, theme);
    await dock(page).locator("[data-attention-more]").click();
    await expect(popover(page)).toBeVisible();
    await expect(popover(page)).toHaveScreenshot(`attention-queue-popover-${theme}.png`);
  });
}

test("attention queue chip screenshot", async ({ page }) => {
  await openQueue(page);
  await page.getByRole("button", { name: "Collapse sidebar" }).click();
  await expect(chip(page)).toBeVisible();
  await expect(page.locator('[role="region"][aria-label="Status bar"]')).toHaveScreenshot(
    "attention-queue-chip-dark.png",
  );
});

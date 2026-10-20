import { expect, test, type Page } from "@playwright/test";

// Exit to shell (ADR 0007). A clean agent exit (`/exit`, code 0) drops the
// pane back to a plain shell in the same folder: same pane, new session, the
// last agent screen above the prompt. Any other ended agent pane keeps its
// exit card, which offers "Open shell here". The harness helper ends a
// terminal the way the backend does.
const HARNESS_URL = "/ux-audit.html?scenario=normal&ghostty=off";
// Claude Code names its terminal with an OSC title. That title belongs to
// the agent and must not name the shell.
const AGENT_TITLE = "✳ Fix login bug";
const LAST_AGENT_SCREEN = `\x1b]0;${AGENT_TITLE}\x07Release checklist verified.\r\n> /exit\r\n`;

type HarnessWindow = typeof window & {
  __harnessTerminalWrites?: Array<{ sessionId: string; data: string }>;
  __harnessTerminalCreateRequests?: Array<string | null>;
};

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

async function openAgentPane(page: Page, prompt: string) {
  await page.goto(HARNESS_URL);
  await waitForHarnessApp(page);
  await page.evaluate(() => {
    for (const key of Object.keys(window.localStorage)) {
      if (key.includes("terminal")) window.localStorage.removeItem(key);
    }
  });
  await page.reload();
  await openFixtureConversation(page);
  const pro = page.getByRole("switch").first();
  if ((await pro.getAttribute("aria-checked")) === "true") await pro.click();
  await pro.click();
  await page.getByPlaceholder(/describe a task|message/i).first().fill(prompt);
  await page.getByRole("button", { name: "Send" }).first().click();
  const pane = page.locator(".terminal-pane");
  await expect(pane).toHaveCount(1, { timeout: 15_000 });
  await expect(pane.locator(".xterm-rows")).toContainText("$");
  const agentId = await pane.getAttribute("data-terminal-session-id");
  expect(agentId).toBeTruthy();
  return { pane, agentId: agentId! };
}

async function exitTerminal(
  page: Page,
  sessionId: string,
  exit: {
    exitCode?: number;
    signal?: string;
    lastScreen?: string;
    continueInShell?: boolean;
  },
) {
  return page.evaluate(
    async ({ sessionId, exit }) => {
      // @ts-expect-error Served by the harness dev server.
      const { exitHarnessTerminal } = await import("/src/harness/mock-ipc.ts");
      const shell = await exitHarnessTerminal(sessionId, exit);
      return (shell?.id as string | undefined) ?? null;
    },
    { sessionId, exit },
  );
}

async function listedTerminals(page: Page) {
  return page.evaluate(async () => {
    // @ts-expect-error Served by the harness dev server.
    const { ipc } = await import("/src/ipc.ts");
    const rows = (await ipc.terminal.list()) as Array<{
      id: string;
      agentKind?: string | null;
      continuedFrom?: string | null;
      status: string;
    }>;
    return rows.map(({ id, agentKind, continuedFrom, status }) => ({
      id,
      agentKind: agentKind ?? null,
      continuedFrom: continuedFrom ?? null,
      status,
    }));
  });
}

test("a clean agent exit drops the pane back to a shell", async ({ page }) => {
  const { pane, agentId } = await openAgentPane(page, "exit to shell check");
  // The agent terminal names the conversation row, so no standalone
  // "Terminals" section shows.
  const sidebarTerminals = page.locator("section", {
    has: page.locator("span", { hasText: /^Terminals$/ }),
  });
  await expect(sidebarTerminals).toHaveCount(0);

  const shellId = await exitTerminal(page, agentId, {
    exitCode: 0,
    lastScreen: LAST_AGENT_SCREEN,
  });
  expect(shellId).toMatch(/^terminal-/);

  // The same pane now shows the shell. No exit card flashes.
  await expect(pane).toHaveCount(1);
  await expect(pane).toHaveAttribute("data-terminal-session-id", shellId!);
  await expect(page.locator(".terminal-exit-card")).toHaveCount(0);
  // The whole last agent screen stays above the marker and the prompt.
  const rows = pane.locator(".xterm-rows");
  await expect(rows).toContainText(
    /Release checklist verified\.\s*> \/exit\s*\[(Claude Code|Codex|Kimi) exited 0 · shell\]\s*\$/,
  );
  // The replayed agent screen does not rename the shell.
  await expect(pane.locator(".terminal-pane-title")).not.toContainText(
    "Fix login bug",
  );
  await expect(pane.locator(".terminal-access-label")).toHaveText("Full access");
  await expect(pane).toHaveScreenshot("terminal-exit-shell.png", {
    mask: [pane.locator(".terminal-pane-title")],
  });

  // Typed input goes to the shell, not to the ended agent.
  await page.evaluate(() => {
    (window as HarnessWindow).__harnessTerminalWrites = [];
  });
  await pane.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type("pwd");
  await page.keyboard.press("Enter");
  await expect
    .poll(() =>
      page.evaluate(() =>
        ((window as HarnessWindow).__harnessTerminalWrites ?? [])
          .map((write) => write.sessionId)
          .filter((sessionId, index, all) => all.indexOf(sessionId) === index),
      ),
    )
    .toEqual([shellId]);

  // The sidebar keeps one row for the conversation: no working spinner and
  // no extra "Terminals" row for the shell.
  const conversation = page
    .locator(".group button", { hasText: "Audit agent refactor" })
    .first();
  await expect(conversation.locator(".conversation-spinner")).toHaveCount(0);
  await expect(conversation).toContainText("Audit agent refactor");
  await expect(sidebarTerminals).toHaveCount(0);

  // Closing the running shell pane asks first. The close also forgets the
  // ended agent terminal.
  await pane.getByRole("button", { name: /^Close / }).click();
  const dialog = page.getByRole("dialog");
  const confirm = dialog.getByRole("button", { name: "Confirm", exact: true });
  await expect(confirm).toBeVisible();
  await expect(dialog).not.toContainText("Fix login bug");
  await confirm.click();
  await expect(page.locator(".terminal-pane")).toHaveCount(0);
  await expect
    .poll(async () => (await listedTerminals(page)).map((row) => row.id))
    .not.toContain(agentId);
  await expect
    .poll(() =>
      page.evaluate(async () => {
        // @ts-expect-error Served by the harness dev server.
        const { ipc } = await import("/src/ipc.ts");
        const sessions = (await ipc.agent.sessionList()) as Array<{ id: string }>;
        return sessions.map((session) => session.id);
      }),
    )
    .not.toContain(agentId);
});

test("a failed agent keeps its exit card and can open a shell here", async ({
  page,
}) => {
  const { pane, agentId } = await openAgentPane(page, "failed exit check");

  expect(
    await exitTerminal(page, agentId, {
      exitCode: 1,
      lastScreen: "Error: the session could not be resumed.\r\n",
    }),
  ).toBeNull();
  const card = page.locator(".terminal-exit-card");
  await expect(card).toBeVisible();
  const openShell = card.getByRole("button", {
    name: "Open shell here",
    exact: true,
  });
  await expect(openShell).toBeVisible();
  await expect(card.getByRole("button", { name: "Close pane" })).toBeVisible();
  await expect(pane).toHaveScreenshot("terminal-exit-open-shell-card.png", {
    mask: [pane.locator(".terminal-pane-title")],
  });

  await openShell.click();
  await expect(card).toHaveCount(0);
  await expect(pane).toHaveCount(1);
  await expect(pane).not.toHaveAttribute("data-terminal-session-id", agentId);
  const shellId = await pane.getAttribute("data-terminal-session-id");
  await expect(pane.locator(".xterm-rows")).toContainText(
    /\[(Claude Code|Codex|Kimi) exited 1 · shell\]/,
  );
  const listed = await listedTerminals(page);
  expect(listed.find((row) => row.id === shellId)).toMatchObject({
    agentKind: null,
    continuedFrom: agentId,
    status: "running",
  });
});

test("a clean exit without a shell shows its card after the grace delay", async ({
  page,
}) => {
  const { pane, agentId } = await openAgentPane(page, "grace delay check");

  // The continuation shell cannot start, for example in a removed folder.
  expect(
    await exitTerminal(page, agentId, { exitCode: 0, continueInShell: false }),
  ).toBeNull();
  const overlay = pane.locator('.terminal-exit-overlay[data-grace="true"]');
  await expect(overlay).toHaveCount(1);
  await expect(pane.locator('.terminal-stage[data-grace="true"]')).toHaveCount(1);
  // The card and the dimming wait, so a quick continuation never flashes.
  expect(
    await overlay.evaluate((element) => {
      const style = window.getComputedStyle(element);
      return [style.animationName, style.animationDelay];
    }),
  ).toEqual(["terminal-exit-grace-reveal", "0.9s"]);

  const card = overlay.locator(".terminal-exit-card");
  await expect(card).toBeVisible();
  await card.getByRole("button", { name: "Open shell here", exact: true }).click();
  await expect(page.locator(".terminal-exit-card")).toHaveCount(0);
  await expect(pane).not.toHaveAttribute("data-terminal-session-id", agentId);
  await expect(pane.locator(".xterm-rows")).toContainText(
    /\[(Claude Code|Codex|Kimi) exited 0 · shell\]/,
  );
});

test("a supervisor failure offers Retry, not a shell", async ({ page }) => {
  const { pane, agentId } = await openAgentPane(page, "supervisor failure check");

  // The agent can still run in the supervisor, so a shell must not take
  // over its pane.
  await page.evaluate(
    async ({ sessionId }) => {
      // @ts-expect-error Served by the harness dev server.
      const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
      await emitHarnessEvent("terminal_event", {
        sessionId,
        sequence: 1_000_000,
        kind: "error",
        message: "Supervisor is outdated (0.7.0). Restart it to use Halv 0.7.1.",
      });
    },
    { sessionId: agentId },
  );
  const card = pane.locator(".terminal-exit-card");
  await expect(card).toBeVisible();
  await expect(card.getByRole("button", { name: "Retry connection" })).toBeVisible();
  await expect(
    card.getByRole("button", { name: "Open shell here", exact: true }),
  ).toHaveCount(0);
});

test("a relaunch after an exit to shell restores the shell, not the agent", async ({
  page,
}) => {
  const { pane, agentId } = await openAgentPane(page, "relaunch check");
  const shellId = await exitTerminal(page, agentId, { exitCode: 0 });
  await expect(pane).toHaveAttribute("data-terminal-session-id", shellId!);

  // The pane saves as a plain shell: no agent kind and no CLI session.
  await page.waitForFunction((shellId) => {
    return Object.keys(window.localStorage).some((key) => {
      if (!key.startsWith("ux-audit.terminal-restore:")) return false;
      const saved = JSON.parse(window.localStorage.getItem(key) ?? "null") as {
        panes?: Array<{
          sessionId: string;
          agentKind: string | null;
          cliSessionId: string | null;
        }>;
      } | null;
      return (
        saved?.panes?.length === 1 &&
        saved.panes[0].sessionId === shellId &&
        saved.panes[0].agentKind === null &&
        saved.panes[0].cliSessionId === null
      );
    });
  }, shellId);

  await page.reload();
  await openFixtureConversation(page);
  const restored = page.locator(".terminal-pane");
  await expect(restored).toHaveCount(1, { timeout: 20_000 });
  await expect(restored.locator(".terminal-access-label")).toHaveText(
    "Full access",
  );
  await expect(page.locator(".terminal-exit-card")).toHaveCount(0);
  // No agent starts again over the shell.
  expect(
    await page.evaluate(
      () => (window as HarnessWindow).__harnessTerminalCreateRequests ?? [],
    ),
  ).toEqual([]);
});

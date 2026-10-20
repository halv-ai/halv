import { expect, test, type Page } from "@playwright/test";

// A plain shell at any time (ADR 0007). In the Pro agent surface, the split
// "Terminal" choice, the header "+" button, and the /shell command open a
// login shell beside the active pane, in the folder of that pane. A plain
// shell never passes through the trial gate.
const HARNESS_URL = "/ux-audit.html?scenario=normal&ghostty=off";

type HarnessWindow = typeof window & {
  __harnessTerminalWrites?: Array<{ sessionId: string; data: string }>;
  __analyticsEvents?: string[];
};

type ListedTerminal = {
  id: string;
  cwd: string;
  conversationId: string | null;
  agentKind: string | null;
};

async function waitForHarnessApp(page: Page) {
  await page
    .locator(".group button", { hasText: "Audit agent refactor" })
    .first()
    .waitFor({ timeout: 30_000 });
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
  await waitForHarnessApp(page);
  await page
    .locator(".group button", { hasText: "Audit agent refactor" })
    .first()
    .click();
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
  return { agentPane: pane, agentId: agentId! };
}

async function listedTerminals(page: Page) {
  return page.evaluate(async () => {
    // @ts-expect-error Served by the harness dev server.
    const { ipc } = await import("/src/ipc.ts");
    const rows = (await ipc.terminal.list()) as Array<{
      id: string;
      cwd: string;
      conversationId?: string | null;
      agentKind?: string | null;
    }>;
    return rows.map((row) => ({
      id: row.id,
      cwd: row.cwd,
      conversationId: row.conversationId ?? null,
      agentKind: row.agentKind ?? null,
    }));
  });
}

// Waits for the second pane: a plain shell beside the agent, in the same
// folder and conversation.
async function expectShellBeside(page: Page, agentId: string) {
  await expect(page.locator(".terminal-pane")).toHaveCount(2, {
    timeout: 15_000,
  });
  await expect(page.locator(".terminal-split")).toHaveCount(1);
  const shellPane = page.locator(".terminal-pane", {
    has: page.locator(".terminal-access-label"),
  });
  await expect(shellPane).toHaveCount(1);
  await expect(shellPane.locator(".terminal-access-label")).toHaveText(
    "Full access",
  );
  const shellId = await shellPane.getAttribute("data-terminal-session-id");
  expect(shellId).not.toBe(agentId);
  const rows: ListedTerminal[] = await listedTerminals(page);
  const agent = rows.find((row) => row.id === agentId);
  const shell = rows.find((row) => row.id === shellId);
  expect(agent?.agentKind).toBeTruthy();
  expect(shell).toMatchObject({
    agentKind: null,
    cwd: agent?.cwd,
    conversationId: agent?.conversationId,
  });
  return { shellPane, shellId: shellId! };
}

test("the split Terminal choice opens a shell beside an agent pane", async ({
  page,
}) => {
  const { agentId } = await openAgentPane(page, "split shell check");

  await page.locator('summary[aria-label="Split terminal right"]').click();
  const terminalChoice = page
    .locator(".terminal-split-menu-popover button", {
      hasText: "opens a clean shell",
    })
    .first();
  await expect(terminalChoice).toBeEnabled();
  await terminalChoice.click();

  await expectShellBeside(page, agentId);
  // The agent keeps running in its own pane.
  await expect(page.locator(".terminal-exit-card")).toHaveCount(0);
});

test("the header + button opens a shell beside the active pane", async ({
  page,
}) => {
  const { agentId } = await openAgentPane(page, "header shell check");

  const newShell = page.getByRole("button", { name: "New shell", exact: true });
  await expect(newShell).toBeEnabled();
  await expect(newShell).toHaveAttribute(
    "title",
    "Open a shell beside the active terminal pane.",
  );
  await newShell.click();

  await expectShellBeside(page, agentId);
  await expect(page.locator(".terminal-diff-surface-agent").first()).toHaveScreenshot(
    "terminal-open-shell-split.png",
    { mask: [page.locator(".terminal-pane-title")] },
  );
});

test("the /shell command opens a shell beside the active pane", async ({
  page,
}) => {
  const { agentId } = await openAgentPane(page, "palette shell check");

  // Keys inside the terminal stay with the terminal, so leave it first.
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press("ControlOrMeta+k");
  const search = page.getByRole("combobox", { name: "Search commands" }).or(
    page.getByRole("textbox", { name: "Search commands" }),
  );
  await expect(search).toBeVisible();
  await search.fill("shell");
  const option = page.locator('[id="halv-command-option-workspace.new_shell"]');
  await expect(option).toContainText("Open a shell beside the active terminal pane.");
  await expect(option).toBeEnabled();
  await option.click();

  await expectShellBeside(page, agentId);
});

test("a plain shell command skips the trial gate", async ({ page }) => {
  // The Pro panel asks the trial gate about each submitted agent prompt. An
  // anonymous user after onboarding gets the trial prompt, and the line is
  // dropped. A signed-in user passes, and the gate records an active day.
  // A plain shell command must never reach the gate.
  const { agentId } = await openAgentPane(page, "trial gate check");
  const agentPane = page.locator(
    `.terminal-pane[data-terminal-session-id="${agentId}"]`,
  );
  await page.getByRole("button", { name: "New shell", exact: true }).click();
  const { shellPane, shellId } = await expectShellBeside(page, agentId);

  await page.evaluate(async () => {
    // @ts-expect-error Served by the harness dev server.
    const { ipc } = await import("/src/ipc.ts");
    const record = ipc.analytics.record;
    const global = window as HarnessWindow;
    ipc.analytics.record = async (args: { event: string }) => {
      (global.__analyticsEvents ??= []).push(args.event);
      return record(args);
    };
    global.__analyticsEvents = [];
    global.__harnessTerminalWrites = [];
  });
  const gateCalls = () =>
    page.evaluate(
      () =>
        ((window as HarnessWindow).__analyticsEvents ?? []).filter(
          (event) => event === "active_day",
        ).length,
    );
  const writesTo = (sessionId: string) =>
    page.evaluate(
      (sessionId) =>
        ((window as HarnessWindow).__harnessTerminalWrites ?? [])
          .filter((write) => write.sessionId === sessionId)
          .map((write) => write.data)
          .join(""),
      sessionId,
    );

  await shellPane.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type("ls");
  await page.keyboard.press("Enter");
  await expect.poll(() => writesTo(shellId)).toBe("ls\r");
  expect(await gateCalls()).toBe(0);

  // Control: the same keys in the agent pane pass through the gate.
  await agentPane.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type("ls");
  await page.keyboard.press("Enter");
  await expect.poll(() => writesTo(agentId)).toBe("ls\r");
  await expect.poll(gateCalls).toBe(1);
  await expect(page.locator('[aria-labelledby="trial-prompt-title"]')).toHaveCount(0);
});

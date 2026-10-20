import { expect, test, type Page } from "@playwright/test";
import { openVisualState } from "./capture";
import {
  activeTab,
  skipFixtureAdoption,
  strip,
  stripTab,
  tabKeys,
  terminalWrites,
} from "./workspaceTabsProbe";

// Shell tabs, Cmd+T and the Cmd+W chain (ADR 0010, stage S5b) behind the
// shellTabs flag: a shell in the workspace root, splits that stay in their
// tab, a reload, Cmd+W pane then tab, `exit`, the hide path, Run actions,
// the terminal rows of the Chats area, titles and rename, and the strip in
// both themes.

const SHELL_QUERY = "flags=projectTree,workspaceTabs,shellTabs&sidebar=projects&ghostty=off";
const PRIMARY_ROOT = "/Users/auditor/Projects/halv";
const ENVIRONMENT_CONFIG_PATH = `${PRIMARY_ROOT}/.codex/environments/environment.toml`;

type HarnessWindow = typeof window & {
  __harnessTerminalOpenRequests?: Array<Record<string, unknown>>;
  __harnessHideMainWindowCalls?: number;
  __harnessShellBusy?: Record<string, string>;
  __harnessTerminalWrites?: Array<{ sessionId: string; data: string }>;
};

type ListedTerminal = { id: string; status: string; shellTabId?: string | null };

async function openShellTabs(
  page: Page,
  options: { theme?: "dark" | "light"; query?: string; flags?: string } = {},
) {
  const query = options.flags
    ? SHELL_QUERY.replace(/^flags=[^&]+/, `flags=${options.flags}`)
    : SHELL_QUERY;
  // The fixture terminals stay out of the strip (ADR 0020 adoption).
  await skipFixtureAdoption(page);
  if (options.query?.includes("environments=1")) {
    // Halv selects an environment only from a stored user choice (#34).
    await page.addInitScript((configPath) => {
      window.localStorage.setItem("halv.localEnvironment.v2.1", configPath);
    }, ENVIRONMENT_CONFIG_PATH);
  }
  await openVisualState(
    page,
    options.theme === "light" ? "workspace-tabs-light" : "workspace-tabs-dark",
    options.query ? `${query}&${options.query}` : query,
  );
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
}

/** The title of the active tab. A shell tab has a ">_" badge before it. */
function activeTitle(page: Page) {
  return activeTab(page).locator(".workspace-tab-title");
}

function activeBody(page: Page) {
  return page.locator('.conversation-mode-surface[data-active="true"]');
}

function activePanes(page: Page) {
  return activeBody(page).locator(".terminal-pane[data-terminal-session-id]");
}

async function invoke(page: Page, command: string, args: Record<string, unknown> = {}) {
  return page.evaluate(
    ({ command, args }) =>
      (
        window as typeof window & {
          __TAURI_INTERNALS__: { invoke: (name: string, args: unknown) => Promise<unknown> };
        }
      ).__TAURI_INTERNALS__.invoke(command, args),
    { command, args },
  );
}

async function listed(page: Page) {
  return (await invoke(page, "terminal_list")) as ListedTerminal[];
}

async function exitShell(page: Page, sessionId: string, exitCode: number) {
  await page.evaluate(
    async ({ sessionId, exitCode }) => {
      // @ts-expect-error Served by the harness dev server.
      const { exitHarnessTerminal } = await import("/src/harness/mock-ipc.ts");
      await exitHarnessTerminal(sessionId, { exitCode });
    },
    { sessionId, exitCode },
  );
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

function focusedSessionId(page: Page) {
  return page.evaluate(
    () =>
      document.activeElement
        ?.closest(".terminal-pane")
        ?.getAttribute("data-terminal-session-id") ?? null,
  );
}

/** Cmd+T, then the id of the new shell tab and of its first shell. */
async function newShellTab(page: Page) {
  const before = (await tabKeys(page)).length;
  await page.keyboard.press("Meta+t");
  await expect(strip(page).locator(".workspace-tab")).toHaveCount(before + 1);
  const key = (await activeTab(page).getAttribute("data-workspace-tab-key"))!;
  expect(key).toMatch(/^shell:[0-9a-f-]{36}$/);
  await expect(activePanes(page)).toHaveCount(1);
  const sessionId = (await activePanes(page).first().getAttribute("data-terminal-session-id"))!;
  // The new shell attaches and takes the keys.
  await expect.poll(() => focusedSessionId(page)).toBe(sessionId);
  return { key, tabId: key.slice("shell:".length), sessionId };
}

async function paneIds(page: Page) {
  return activePanes(page).evaluateAll((nodes) =>
    nodes.map((node) => node.getAttribute("data-terminal-session-id")!),
  );
}

async function splitActivePane(page: Page) {
  const count = await activePanes(page).count();
  await activePanes(page).last().locator(".xterm-helper-textarea").focus();
  await page.keyboard.press("Meta+d");
  await expect(activePanes(page)).toHaveCount(count + 1);
}

test("Cmd+T opens a shell tab after the visible tab, with its shell in the workspace root", async ({ page }) => {
  await openShellTabs(page);
  await expect(activeTitle(page)).toHaveText("Audit agent refactor");
  const keysBefore = await tabKeys(page);
  const { key, tabId, sessionId } = await newShellTab(page);
  expect(await tabKeys(page)).toEqual([...keysBefore, key]);
  await expect(activeTitle(page)).toHaveText("Terminal 1");
  await expect(activeBody(page)).toHaveAttribute("data-workspace-tab-body", key);
  // The shell belongs to the tab and runs in the root of the workspace.
  const request = await page.evaluate(
    () => (window as HarnessWindow).__harnessTerminalOpenRequests?.at(-1) ?? null,
  );
  expect(request).toMatchObject({
    shellTabId: tabId,
    workspaceId: "primary-1",
    sessionId,
    cwd: PRIMARY_ROOT,
  });
  expect(request?.conversationId ?? null).toBeNull();
  expect((await listed(page)).find((row) => row.id === sessionId)?.shellTabId).toBe(tabId);
  // The shell takes the keys. No chat composer shows over a shell tab.
  await expect.poll(() => focusedSessionId(page)).toBe(sessionId);
  await expect(page.getByRole("textbox", { name: "Message" })).toHaveCount(0);
  // One node per session: the shell shows in one panel only.
  await expect(page.locator(`[data-terminal-session-id="${sessionId}"]`)).toHaveCount(1);

  // "+" > New Terminal opens the next one after the visible tab.
  await stripTab(page, "Audit agent refactor").click();
  await strip(page).getByRole("button", { name: "New tab" }).click();
  await page.getByRole("menuitem", { name: /New Terminal/ }).click();
  await expect(activeTitle(page)).toHaveText("Terminal 2");
  const keys = await tabKeys(page);
  expect(keys[0]).toBe("conversation:audit-agent-refactor");
  expect(keys[2]).toBe(key);
});

test("a split stays in its shell tab, and each shell tab keeps its own panes", async ({ page }) => {
  await openShellTabs(page);
  const first = await newShellTab(page);
  await splitActivePane(page);
  const firstPanes = await paneIds(page);
  expect(firstPanes).toHaveLength(2);
  const second = await newShellTab(page);
  expect(await paneIds(page)).toEqual([second.sessionId]);
  // Both panes of the first tab carry its owner. Nothing leaks into the
  // second tab or into a conversation tab.
  const rows = await listed(page);
  for (const id of firstPanes) {
    expect(rows.find((row) => row.id === id)?.shellTabId).toBe(first.tabId);
  }
  await stripTab(page, "Terminal 1").click();
  expect((await paneIds(page)).sort()).toEqual([...firstPanes].sort());
  await stripTab(page, "Audit agent refactor").click();
  for (const id of firstPanes) {
    await expect(activeBody(page).locator(`[data-terminal-session-id="${id}"]`)).toHaveCount(0);
  }
});

test("two shell tabs keep their own panes across a reload", async ({ page }) => {
  await openShellTabs(page);
  const first = await newShellTab(page);
  await splitActivePane(page);
  const second = await newShellTab(page);
  const keys = await tabKeys(page);
  // The panels save their panes before the reload.
  const savedPanes = (tabId: string) =>
    page.evaluate((scope) => {
      const raw = window.localStorage.getItem(`ux-audit.terminal-restore:${scope}`);
      return raw ? (JSON.parse(raw) as { panes: unknown[] }).panes.length : 0;
    }, `halv.terminal-layout.v1:shell:${tabId}`);
  await expect.poll(() => savedPanes(first.tabId)).toBe(2);
  await expect.poll(() => savedPanes(second.tabId)).toBe(1);

  await page.reload();
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
  expect(await tabKeys(page)).toEqual(keys);
  await expect(activeTitle(page)).toHaveText("Terminal 2");
  await expect(activePanes(page)).toHaveCount(1);
  await stripTab(page, "Terminal 1").click();
  await expect(activePanes(page)).toHaveCount(2);
  // The shells came back in their own tabs: the scope names the owner.
  await expect
    .poll(async () => {
      const rows = await listed(page);
      return rows.filter((row) => row.status === "running" && row.shellTabId).map((row) => row.shellTabId).sort();
    })
    .toEqual([first.tabId, first.tabId, second.tabId].sort());
});

test("Cmd+W closes the focused pane, then the tab, and shows the most recent tab", async ({ page }) => {
  await openShellTabs(page);
  await newShellTab(page);
  await splitActivePane(page);
  const [, focused] = await paneIds(page);
  await activeBody(page).locator(`[data-terminal-session-id="${focused}"] .xterm-helper-textarea`).focus();
  await page.evaluate(() => {
    (window as HarnessWindow).__harnessTerminalWrites = [];
  });

  // An idle shell closes without a prompt.
  await page.keyboard.press("Meta+w");
  await expect(activePanes(page)).toHaveCount(1);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await paneIds(page)).not.toContain(focused);
  await expect(activeTitle(page)).toHaveText("Terminal 1");

  // A busy shell asks once for the tab.
  const [last] = await paneIds(page);
  await page.evaluate((id) => {
    (window as HarnessWindow).__harnessShellBusy = { [id]: "busy" };
  }, last);
  await activePanes(page).first().locator(".xterm-helper-textarea").focus();
  await page.keyboard.press("Meta+w");
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Close Terminal 1?");
  await expect(dialog).toContainText("A command is running.");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(activeTitle(page)).toHaveText("Terminal 1");
  await page.evaluate(() => {
    (window as HarnessWindow).__harnessShellBusy = {};
  });

  // The only pane closes the tab. The most recent tab shows.
  await activePanes(page).first().locator(".xterm-helper-textarea").focus();
  await page.keyboard.press("Meta+w");
  await expect(stripTab(page, "Terminal 1")).toHaveCount(0);
  await expect(activeTitle(page)).toHaveText("Audit agent refactor");
  // Cmd+W never became PTY input, and the window stayed.
  expect(await terminalWrites(page)).toBe("");
  expect(await page.evaluate(() => (window as HarnessWindow).__harnessHideMainWindowCalls ?? 0)).toBe(0);
  expect((await listed(page)).some((row) => row.id === last && row.status === "running")).toBe(false);
});

test("a shell that exits with 0 closes its tab; a failed shell offers Restart", async ({ page }) => {
  await openShellTabs(page);
  const failing = await newShellTab(page);
  await exitShell(page, failing.sessionId, 2);
  const card = activeBody(page).locator(".terminal-exit-card");
  await expect(card).toContainText("Process exited (2)");
  await expect(stripTab(page, "Terminal 1")).toHaveCount(1);
  await card.getByRole("button", { name: "Restart" }).click();
  await expect(activeBody(page).locator(".terminal-exit-card")).toHaveCount(0);
  const [restarted] = await paneIds(page);
  expect(restarted).not.toBe(failing.sessionId);
  expect((await listed(page)).find((row) => row.id === restarted)?.shellTabId).toBe(failing.tabId);

  // `exit` in the only shell closes the pane, then the tab.
  await exitShell(page, restarted, 0);
  await expect(stripTab(page, "Terminal 1")).toHaveCount(0);
  await expect(activeTitle(page)).toHaveText("Audit agent refactor");
});

test("with nothing to close, Cmd+W hides the window", async ({ page }) => {
  // Without the shellTabs flag, Cmd+W keeps its old meaning.
  await openShellTabs(page, { flags: "projectTree,workspaceTabs" });
  await page.keyboard.press("Meta+w");
  await expect
    .poll(() => page.evaluate(() => (window as HarnessWindow).__harnessHideMainWindowCalls ?? 0))
    .toBe(1);
  await expect(activeTitle(page)).toHaveText("Audit agent refactor");
  // No shell tab without the flag: Cmd+T does nothing.
  const keys = await tabKeys(page);
  await page.keyboard.press("Meta+t");
  expect(await tabKeys(page)).toEqual(keys);

  // With shell tabs, an empty strip hides the window too.
  await openShellTabs(page);
  await activeTab(page).locator(".workspace-tab-close").click();
  // In Pro mode the empty strip shows the launcher (ADR 0020).
  await expect(page.locator("[data-workspace-launcher]")).toBeVisible();
  await page.keyboard.press("Meta+w");
  await expect
    .poll(() => page.evaluate(() => (window as HarnessWindow).__harnessHideMainWindowCalls ?? 0))
    .toBe(1);
});

test("without an action the strip has no Run button, and the menu opens the settings", async ({ page }) => {
  await openShellTabs(page);
  await expect(page.locator("[data-workspace-run]")).toHaveCount(0);
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Set up local environment…", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Local environments" })).toBeVisible();
});

test("a Run action opens a shell tab named after it and types its command", async ({ page }) => {
  await openShellTabs(page, { query: "environments=1" });
  const run = page.locator("[data-workspace-run]");
  await expect(run.getByRole("button", { name: "Run Dev server" })).toBeVisible();
  // The button sits at the right end of the tab strip, not in the header
  // controls (ADR 0020).
  await expect(strip(page).locator("[data-workspace-run]")).toHaveCount(1);
  const runBox = (await run.boundingBox())!;
  const plusBox = (await strip(page).getByRole("button", { name: "New tab" }).boundingBox())!;
  expect(runBox.x).toBeGreaterThan(plusBox.x);
  // The header menu keeps the actions.
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Run local environment action Dev server" }),
  ).toBeVisible();
  // A second click closes the menu.
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  const tabsBefore = (await tabKeys(page)).length;
  await run.getByRole("button", { name: "Run Dev server" }).click();
  // The first run asks for an approval of the file. No tab opens before it.
  const approval = page.getByRole("dialog");
  await expect(approval).toContainText("pnpm dev");
  expect(await tabKeys(page)).toHaveLength(tabsBefore);
  await approval.getByRole("button", { name: "Approve and run" }).click();
  await expect(activeTitle(page)).toHaveText("Dev server");
  await expect(activePanes(page)).toHaveCount(1);
  const sessionId = (await activePanes(page).first().getAttribute("data-terminal-session-id"))!;
  await expect.poll(() => terminalWrites(page, sessionId)).toBe("pnpm dev\r");

  await run.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Unit tests" }).click();
  await expect(activeTitle(page)).toHaveText("Unit tests");
  const testsId = (await activePanes(page).first().getAttribute("data-terminal-session-id"))!;
  await expect.poll(() => terminalWrites(page, testsId)).toBe("pnpm test:unit\r");
});

test("an owned shell never shows in the Chats area, and an unowned one becomes a tab", async ({ page }) => {
  await openShellTabs(page);
  const owned = await newShellTab(page);
  await invoke(page, "terminal_rename", { sessionId: owned.sessionId, title: "Owned shell probe" });
  // A shell without an owner in the project root becomes a tab of the
  // primary strip by itself (ADR 0020).
  const unowned = (await invoke(page, "terminal_open", {
    projectId: 1,
    workspaceId: "primary-1",
    conversationId: null,
    cols: 100,
    rows: 24,
    executionProfile: "full_access",
  })) as { id: string };
  await invoke(page, "terminal_rename", { sessionId: unowned.id, title: "Unowned shell probe" });
  // A terminal event refreshes the terminal list of the app.
  await expect
    .poll(async () => {
      await emit(page, "terminal_event", { sessionId: "refresh", sequence: 1, kind: "exit", exitCode: 0 });
      return (await listed(page)).find((row) => row.id === unowned.id)?.shellTabId ?? null;
    })
    .not.toBeNull();
  await expect(strip(page).locator(".workspace-tab")).toContainText(["Unowned shell probe"]);
  const sidebar = page.locator("aside").first();
  await expect(sidebar).not.toContainText("Unowned shell probe");
  await expect(sidebar).not.toContainText("Owned shell probe");
  // The bottom panel filter (P3) never takes an owned shell either.
  expect((await listed(page)).find((row) => row.id === owned.sessionId)?.shellTabId).toBe(owned.tabId);
});

test("a shell tab shows the live title of its pane, and a rename wins", async ({ page }) => {
  await openShellTabs(page);
  const shell = await newShellTab(page);
  await emit(page, "terminal_event", {
    sessionId: shell.sessionId,
    sequence: 50,
    kind: "data",
    data: "\u001b]0;vim notes.md\u0007",
  });
  await expect(activeTitle(page)).toHaveText("vim notes.md");

  await activeTab(page).dblclick();
  const input = strip(page).locator('[data-tab-rename-input="true"]');
  await expect(input).toBeFocused();
  // The tab chords skip the rename field.
  await input.press("Meta+w");
  await expect(input).toBeVisible();
  await input.fill("dev server");
  await input.press("Enter");
  await expect(activeTitle(page)).toHaveText("dev server");
  await page.reload();
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
  await expect(stripTab(page, "dev server")).toHaveCount(1);

  // An empty title restores the default title.
  await stripTab(page, "dev server").dblclick();
  await strip(page).locator('[data-tab-rename-input="true"]').fill("");
  await strip(page).locator('[data-tab-rename-input="true"]').press("Enter");
  await expect(stripTab(page, "Terminal 1")).toHaveCount(1);
});

for (const theme of ["dark", "light"] as const) {
  test(`shell tabs in the strip, ${theme} theme`, async ({ page }) => {
    await openShellTabs(page, { theme, query: "environments=1" });
    await newShellTab(page);
    await activeTab(page).dblclick();
    await strip(page).locator('[data-tab-rename-input="true"]').fill("dev server");
    await strip(page).locator('[data-tab-rename-input="true"]').press("Enter");
    await newShellTab(page);
    await stripTab(page, "Audit agent refactor").click();
    const header = page.locator("[data-app-header]");
    await expect(header).toHaveScreenshot(`shell-tabs-strip-${theme}.png`);
    await strip(page).getByRole("button", { name: "New tab" }).click();
    await expect(page.getByRole("menu", { name: "New tab" })).toBeVisible();
    const box = (await header.boundingBox())!;
    await expect(page).toHaveScreenshot(`shell-tabs-menu-${theme}.png`, {
      clip: { x: box.x, y: box.y, width: 1000, height: 340 },
    });
  });
}

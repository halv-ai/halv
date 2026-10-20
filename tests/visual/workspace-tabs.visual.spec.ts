import { expect, test, type Page } from "@playwright/test";
import {
  activeTab,
  openFromSearch,
  openWorkspaceTabs,
  strip,
  stripTab,
  tabBody,
  tabKeys,
  tabTitles,
  terminalWrites,
  treeRow,
} from "./workspaceTabsProbe";

// The workspace strip core (ADR 0010, stage S4a) behind the workspaceTabs
// flag: order and one active tab, kept bodies, the tab chords, T14, the
// "+" menu, the draft tab, the close procedure, a refresh storm, reload,
// and the narrow window.

const PARKED_SCOPE = "halv.terminal-layout.v1:conversation:audit-parked";
const PRIMARY_ROOT = "/Users/auditor/Projects/halv";
const LOGIN_ROOT = "/Users/auditor/Library/Application Support/Halv/worktrees/a1b2/halv";

/** A saved panel for the parked conversation: one Claude pane that can
 *  resume. The seed opens its tab, and the tab stays dormant. */
async function plantParkedPanel(page: Page) {
  await page.addInitScript(
    ({ scope, root }) => {
      const layout = {
        version: 1,
        activeTabId: "parked-tab",
        activeLeafId: "pane-parked",
        tabs: [{ tabId: "parked-tab", root: { type: "leaf", leafId: "pane-parked" } }],
      };
      const now = new Date().toISOString();
      window.localStorage.setItem(scope, JSON.stringify(layout));
      window.localStorage.setItem(
        `ux-audit.terminal-restore:${scope}`,
        JSON.stringify({
          layout,
          updatedAt: now,
          panes: [
            {
              paneId: "pane-parked",
              sessionId: "parked-session",
              projectId: 1,
              conversationId: "audit-parked",
              projectName: "Halv Desktop",
              cwd: root,
              executionProfile: "full_access",
              agentKind: "claude",
              laneKind: "claudeSubscription",
              credentialProfileId: null,
              customTitle: null,
              cliSessionId: "cli-parked",
              cols: 100,
              rows: 30,
              scrollback: "$ parked\r\n",
              updatedAt: now,
            },
          ],
        }),
      );
    },
    { scope: PARKED_SCOPE, root: PRIMARY_ROOT },
  );
}

async function createRequests(page: Page) {
  return page.evaluate(
    () =>
      (window as typeof window & { __harnessTerminalCreateRequests?: Array<string | null> })
        .__harnessTerminalCreateRequests ?? [],
  );
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

async function openPlusAgent(page: Page, agent: string) {
  await strip(page).getByRole("button", { name: "New tab" }).click();
  await page.getByRole("menuitem", { name: agent, exact: true }).click();
}

function activePane(page: Page) {
  return page.locator('.conversation-mode-surface[data-active="true"] .terminal-pane[data-terminal-session-id]');
}

test("the strip lists the open tabs of the workspace in order, with one active tab", async ({ page }) => {
  await plantParkedPanel(page);
  await openWorkspaceTabs(page);
  // The seed opened the live and the saved agent conversations once.
  expect(await tabKeys(page)).toEqual(["conversation:audit-parked", "conversation:audit-agent-refactor"]);
  await expect(strip(page).locator('.workspace-tab[aria-selected="true"]')).toHaveCount(1);
  await expect(activeTab(page)).toHaveText("Audit agent refactor");
  await expect(strip(page).getByRole("tablist")).toHaveAttribute("aria-label", "Tabs in main");
  // The in-panel terminal strip never shows.
  await expect(page.locator(".terminal-tab")).toHaveCount(0);
});

test("a tab click switches the body and keeps the live background panel mounted", async ({ page }) => {
  await openWorkspaceTabs(page);
  const pane = activePane(page);
  await expect(pane).toHaveCount(1);
  const sessionId = (await pane.getAttribute("data-terminal-session-id"))!;
  await pane.evaluate((node) => {
    (node as HTMLElement & { __mountMark?: boolean }).__mountMark = true;
  });
  await openPlusAgent(page, "Claude Code");
  await expect(activeTab(page)).not.toHaveText("Audit agent refactor");
  // The live panel stays mounted but hidden and inert.
  const hidden = tabBody(page, "audit-agent-refactor");
  await expect(hidden).toHaveAttribute("data-active", "false");
  await expect(hidden).toHaveAttribute("inert", "");
  await stripTab(page, "Audit agent refactor").click();
  await expect(hidden).toHaveAttribute("data-active", "true");
  const same = page.locator(`.terminal-pane[data-terminal-session-id="${sessionId}"]`);
  await expect(same).toHaveCount(1);
  expect(
    await same.evaluate((node) => (node as HTMLElement & { __mountMark?: boolean }).__mountMark === true),
  ).toBe(true);
});

test("Cmd+2 and Shift+Cmd+] inside xterm switch tabs; typed text reaches only the new tab (T14)", async ({ page }) => {
  await openWorkspaceTabs(page);
  await openPlusAgent(page, "Claude Code");
  const secondPane = activePane(page);
  await expect(secondPane).toHaveCount(1);
  const second = (await secondPane.getAttribute("data-terminal-session-id"))!;
  await stripTab(page, "Audit agent refactor").click();
  const first = (await activePane(page).getAttribute("data-terminal-session-id"))!;
  expect(first).not.toBe(second);
  await activePane(page).locator(".xterm-helper-textarea").focus();
  await page.evaluate(() => {
    (window as typeof window & { __harnessTerminalWrites?: unknown[] }).__harnessTerminalWrites = [];
  });

  await page.keyboard.press("Meta+2");
  await expect(activeTab(page)).not.toHaveText("Audit agent refactor");
  await expect(activePane(page)).toHaveAttribute("data-terminal-session-id", second);
  await expect.poll(() =>
    page.evaluate(() => document.activeElement?.closest(".terminal-pane")?.getAttribute("data-terminal-session-id")),
  ).toBe(second);
  await page.keyboard.type("hi");
  await expect.poll(() => terminalWrites(page, second)).toContain("hi");
  expect(await terminalWrites(page, first)).toBe("");

  await page.keyboard.press("Meta+Shift+BracketRight");
  await expect(activeTab(page)).toHaveText("Audit agent refactor");
  await page.keyboard.press("Meta+Shift+BracketLeft");
  await expect(activePane(page)).toHaveAttribute("data-terminal-session-id", second);
  // The chords never became PTY input.
  expect(await terminalWrites(page)).toBe("hi");
});

test('"+" > Claude opens a tab and starts the agent in the workspace root', async ({ page }) => {
  await openWorkspaceTabs(page);
  await treeRow(page, "conversation:audit-proxy").click();
  await expect(activeTab(page)).toHaveText(/Proxy restart/);
  const before = (await createRequests(page)).length;
  await openPlusAgent(page, "Claude Code");
  await expect(strip(page).getByRole("tablist")).toHaveAttribute("aria-label", /halv\/login-flow/);
  expect(await strip(page).locator(".workspace-tab").count()).toBe(2);
  await expect(activePane(page)).toHaveCount(1);
  expect((await createRequests(page)).length).toBe(before + 1);
  // The pane title bar is the one header of a tab body (S7). Its tooltip
  // names the folder of the pane.
  await expect(
    page.locator('.conversation-mode-surface[data-active="true"] .terminal-pane-title'),
  ).toHaveAttribute("title", new RegExp(` · ${LOGIN_ROOT}$`));
  await expect.poll(() =>
    page.evaluate(() => document.activeElement?.classList.contains("xterm-helper-textarea") ?? false),
  ).toBe(true);
});

test("the first typed character turns the draft tab into a conversation tab in place", async ({ page }) => {
  await openWorkspaceTabs(page);
  await page.getByRole("button", { name: "New chat" }).click();
  const before = await tabKeys(page);
  expect(before.at(-1)).toBe("draft");
  await page.getByRole("textbox", { name: "Message" }).pressSequentially("Plan", { delay: 5 });
  await expect(strip(page).locator('[data-draft="true"]')).toHaveCount(0);
  const after = await tabKeys(page);
  expect(after.length).toBe(before.length);
  expect(after.slice(0, -1)).toEqual(before.slice(0, -1));
  expect(after.at(-1)).toMatch(/^conversation:/);
  await expect(activeTab(page)).toHaveText("Plan");
});

test("a close with a working agent asks once; an idle agent closes without a prompt; the MRU tab shows", async ({ page }) => {
  await plantParkedPanel(page);
  await openWorkspaceTabs(page);
  await openPlusAgent(page, "Claude Code");
  const order = await tabTitles(page);
  expect(order).toHaveLength(3);
  // Activation order: the new tab, then the parked tab, then the audit tab.
  await stripTab(page, "Parked login spike").click();
  await stripTab(page, "Audit agent refactor").click();
  // The audit agent rests (done): no prompt. Its successor is the most
  // recent tab (Parked), not its neighbor.
  await activeTab(page).locator(".workspace-tab-close").click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(stripTab(page, "Audit agent refactor")).toHaveCount(0);
  await expect(activeTab(page)).toHaveText("Parked login spike");
  const terminals = (await invoke(page, "terminal_list")) as Array<{ id: string; status: string }>;
  expect(terminals.find((terminal) => terminal.id === "tree-claude-primary")?.status).not.toBe("running");

  // The docs agent works: one prompt for the whole tab.
  await treeRow(page, "conversation:audit-docs-sweep").click();
  await expect(activeTab(page)).toHaveText("Docs refresh sweep");
  await activeTab(page).locator(".workspace-tab-close").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toHaveCount(1);
  await expect(dialog).toContainText("Close Docs refresh sweep?");
  await expect(dialog).toContainText("1 agent is working.");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(activeTab(page)).toHaveText("Docs refresh sweep");
  await activeTab(page).locator(".workspace-tab-close").click();
  await page.getByRole("dialog").getByRole("button", { name: "Close tab", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  // The strip is empty now: in Pro mode the launcher shows (ADR 0020).
  await expect(page.locator("[data-workspace-launcher]")).toBeVisible();
  await expect(strip(page).locator('[data-draft="true"]')).toHaveCount(0);
  const after = (await invoke(page, "terminal_list")) as Array<{ id: string; status: string }>;
  expect(after.some((terminal) => terminal.id === "tree-codex-docs" && terminal.status === "running")).toBe(false);
});

test("a storm of session refreshes never adds, drops or repeats a tab", async ({ page }) => {
  const warnings: string[] = [];
  page.on("console", (message) => {
    if (/same key|duplicate key/i.test(message.text())) warnings.push(message.text());
  });
  await openWorkspaceTabs(page);
  await openPlusAgent(page, "Claude Code");
  await treeRow(page, "conversation:audit-docs-sweep").click();
  await treeRow(page, "workspace:primary-1").click();
  const keys = await tabKeys(page);
  const conversations = ["audit-agent-refactor", "audit-proxy", "audit-docs-sweep", "audit-release"];
  const created: string[] = [];
  for (let round = 0; round < 50; round += 1) {
    const conversationId = conversations[round % conversations.length];
    const session = (await invoke(page, "agent_terminal_create", {
      agentKind: "codex",
      lane: { kind: "codexChatGpt" },
      cwd: PRIMARY_ROOT,
      projectId: 1,
      conversationId,
      conversationTitle: "storm",
      mode: "terminal_embedded",
      cols: 100,
      rows: 24,
    })) as { id: string };
    created.push(session.id);
    if (round % 3 === 2) {
      const gone = created.shift()!;
      await invoke(page, "agent_session_kill", { sessionId: gone });
    }
    await emit(page, "terminal_event", { sessionId: session.id, sequence: 1, kind: "exit", exitCode: 0 });
  }
  await page.waitForTimeout(400);
  expect(await tabKeys(page)).toEqual(keys);
  expect(new Set(await tabKeys(page)).size).toBe(keys.length);
  expect(warnings).toEqual([]);
  await expect(page.locator(".terminal-tab")).toHaveCount(0);
});

test("a reload brings back the same tabs and active tab; a dormant tab starts nothing until a click", async ({ page }) => {
  await plantParkedPanel(page);
  await openWorkspaceTabs(page);
  await page.getByRole("button", { name: "New chat" }).click();
  // The tree lists open tabs only: the search opens the chat conversation.
  await openFromSearch(page, "release verification", "Release checklist");
  await treeRow(page, "workspace:primary-1").click();
  await expect(activeTab(page)).toHaveText("Audit agent refactor");
  const keys = await tabKeys(page);

  await page.reload();
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
  expect(await tabKeys(page)).toEqual(keys);
  await expect(activeTab(page)).toHaveText("Audit agent refactor");
  await page.waitForTimeout(300);
  // The parked tab has no live session and is not visible: no resume yet.
  expect(await createRequests(page)).not.toContain("cli-parked");
  await expect(tabBody(page, "audit-parked")).toHaveCount(0);

  await stripTab(page, "Parked login spike").click();
  await expect(tabBody(page, "audit-parked")).toHaveAttribute("data-active", "true");
  await expect.poll(() => createRequests(page)).toContain("cli-parked");
  // The other strip keeps its own active tab.
  await treeRow(page, "workspace:primary-2").click();
  await expect(activeTab(page)).toHaveText("Release checklist");
});

for (const theme of ["dark", "light"] as const) {
  test(`the strip in the ${theme} theme`, async ({ page }) => {
    await plantParkedPanel(page);
    await openWorkspaceTabs(page, theme);
    await openPlusAgent(page, "Claude Code");
    await stripTab(page, "Audit agent refactor").click();
    await expect(strip(page)).toHaveScreenshot(`workspace-strip-${theme}.png`);
    await strip(page).getByRole("button", { name: "New tab" }).click();
    await expect(page.getByRole("menu", { name: "New tab" })).toBeVisible();
    const header = (await page.locator("[data-app-header]").boundingBox())!;
    await expect(page).toHaveScreenshot(`workspace-strip-menu-${theme}.png`, {
      clip: { x: header.x, y: header.y, width: 1000, height: 340 },
    });
  });
}

test("the strip fits a 375 px window without a horizontal page scroll", async ({ page }) => {
  await plantParkedPanel(page);
  await openWorkspaceTabs(page);
  await openPlusAgent(page, "Claude Code");
  // A narrow window collapses the sidebar.
  await page.setViewportSize({ width: 375, height: 760 });
  await expect(strip(page).locator(".workspace-tab")).toHaveCount(3);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  // The active tab scrolls into view inside the strip.
  const box = await activeTab(page).boundingBox();
  expect(box).not.toBeNull();
  expect(box!.x + box!.width).toBeLessThanOrEqual(375);
  await expect(page).toHaveScreenshot("workspace-strip-narrow.png");
});

test("without the flag the header keeps the conversation title and shows no strip", async ({ page }) => {
  await page.goto("ux-audit.html?state=workspace-tabs-dark&flags=projectTree&sidebar=projects&ghostty=off");
  await page.waitForFunction(() => document.documentElement.dataset.visualReady === "workspace-tabs-dark");
  await expect(page.locator("[data-workspace-tab-strip]")).toHaveCount(0);
  await expect(page.locator("main[data-workspace-tabs]")).toHaveCount(0);
});

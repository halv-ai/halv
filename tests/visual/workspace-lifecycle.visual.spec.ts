import { expect, test, type Page } from "@playwright/test";
import {
  activeTab,
  focusedBodyKey,
  openWorkspaceTabs,
  strip,
  stripTab,
  tabBody,
  tabKeys,
  treeRow,
} from "./workspaceTabsProbe";

// The lifecycle hooks and the tree wiring of the workspace strip (ADR 0010,
// stage S4b): tree rows are the open tabs, a workspace row switches the
// strip, a handoff moves the tab, archive and delete close the tab and stop
// its processes, and the Agent sessions panel points to the tab.

const PARKED_SCOPE = "halv.terminal-layout.v1:conversation:audit-parked";

async function plantParkedPanel(page: Page) {
  await page.addInitScript((scope) => {
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
            cwd: "/Users/auditor/Projects/halv",
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
  }, PARKED_SCOPE);
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

async function runCommand(page: Page, query: string, id: string) {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press("ControlOrMeta+k");
  const search = page
    .getByRole("combobox", { name: "Search commands" })
    .or(page.getByRole("textbox", { name: "Search commands" }));
  await search.fill(query);
  await page.locator(`[id="halv-command-option-${id}"]`).click();
}

function tree(page: Page) {
  return page.getByRole("tree", { name: "Projects", exact: true });
}

async function rowKeys(page: Page, workspaceId: string) {
  return page
    .locator(`[role="treeitem"][data-tree-key="workspace:${workspaceId}"] [role="treeitem"][data-tree-key^="conversation:"]`)
    .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-tree-key")));
}

for (const theme of ["dark", "light"] as const) {
  test(`tree rows are the open tabs; two or more fold into the agents pill (${theme})`, async ({ page }) => {
    await plantParkedPanel(page);
    await openWorkspaceTabs(page, theme);
    // The primary strip has two tabs: they fold.
    const pill = page.locator('[role="treeitem"][data-tree-key="agents:primary-1"]');
    await expect(pill).toHaveAttribute("aria-expanded", "false");
    await expect(pill).toContainText("2 agents");
    expect(await rowKeys(page, "primary-1")).toEqual([]);
    await pill.locator(":scope > .project-tree-row").click();
    await expect(pill).toHaveAttribute("aria-expanded", "true");
    // The rows follow the strip order.
    expect(await rowKeys(page, "primary-1")).toEqual(
      (await tabKeys(page)).filter((key) => key?.startsWith("conversation:")),
    );
    // One tab: no pill. A closed, quiet conversation has no row.
    await expect(page.locator('[data-tree-key="agents:audit-wt-docs"]')).toHaveCount(0);
    expect(await rowKeys(page, "audit-wt-docs")).toEqual(["conversation:audit-docs-sweep"]);
    expect(await rowKeys(page, "primary-2")).toEqual([]);
    // Without shell tabs, an agent terminal without a conversation waits in
    // the Chats area (ADR 0020).
    await expect(tree(page).getByText("Codex 2")).toHaveCount(0);
    await expect(
      page
        .getByRole("tree", { name: "Chats", exact: true })
        .locator('[data-tree-key="terminal:tree-codex-loose"]'),
    ).toBeVisible();
    await expect(page.getByRole("complementary", { name: "Conversation navigation" })).toHaveScreenshot(
      `workspace-tree-tabs-${theme}.png`,
    );
    // The open pill survives a reload.
    await page.reload();
    await page.locator('main[data-workspace-tabs="ready"]').waitFor();
    await expect(page.locator('[role="treeitem"][data-tree-key="agents:primary-1"]')).toHaveAttribute(
      "aria-expanded",
      "true",
    );
  });
}

test("a live conversation without a tab keeps a dimmed row; a click opens its tab", async ({ page }) => {
  await openWorkspaceTabs(page);
  // The docs agent works. Its tab closes, but an automation starts another
  // agent for the same conversation.
  await treeRow(page, "conversation:audit-docs-sweep").click();
  await activeTab(page).locator(".workspace-tab-close").click();
  await page.getByRole("dialog").getByRole("button", { name: "Close tab", exact: true }).click();
  await expect(stripTab(page, "Docs refresh sweep")).toHaveCount(0);
  await expect(treeRow(page, "conversation:audit-docs-sweep")).toHaveCount(0);
  await invoke(page, "agent_terminal_create", {
    agentKind: "codex",
    lane: { kind: "codexChatGpt" },
    cwd: "/Users/auditor/Library/Application Support/Halv/worktrees/c3d4/halv",
    projectId: 1,
    conversationId: "audit-docs-sweep",
    conversationTitle: "Docs refresh sweep",
    mode: "terminal_embedded",
    cols: 100,
    rows: 24,
  });
  // A window focus refreshes the session lists.
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.getByRole("button", { name: "New tab", exact: true }).focus();
  await page.keyboard.press("Escape");
  const dimmed = page.locator('[role="treeitem"][data-tree-key="conversation:audit-docs-sweep"]');
  await expect
    .poll(async () => {
      await page.evaluate(async () => {
        // @ts-expect-error Served by the harness dev server.
        const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
        await emitHarnessEvent("terminal_event", { sessionId: "refresh", sequence: 1, kind: "exit", exitCode: 0 });
      });
      return dimmed.count();
    })
    .toBe(1);
  await expect(dimmed).toHaveClass(/project-tree-untabbed/);
  await expect(stripTab(page, "Docs refresh sweep")).toHaveCount(0);
  await dimmed.locator(":scope > .project-tree-row").click();
  await expect(activeTab(page)).toHaveText("Docs refresh sweep");
  await expect(dimmed).not.toHaveClass(/project-tree-untabbed/);
});

test("a live conversation that never had a tab gets one by itself, and the view stays", async ({ page }) => {
  await openWorkspaceTabs(page);
  await expect(activeTab(page)).toHaveText("Audit agent refactor");
  // An automation starts an agent for a conversation of another project.
  await invoke(page, "agent_terminal_create", {
    agentKind: "codex",
    lane: { kind: "codexChatGpt" },
    cwd: "/Users/auditor/Projects/design-system",
    projectId: 2,
    conversationId: "audit-release",
    conversationTitle: "Release checklist",
    mode: "terminal_embedded",
    cols: 100,
    rows: 24,
  });
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  // Adoption (ADR 0020) opens its tab in its home workspace, in the
  // background: the visible workspace and tab stay.
  const row = page.locator('[role="treeitem"][data-tree-key="conversation:audit-release"]');
  await expect
    .poll(async () => {
      await page.evaluate(async () => {
        // @ts-expect-error Served by the harness dev server.
        const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
        await emitHarnessEvent("terminal_event", { sessionId: "refresh", sequence: 1, kind: "exit", exitCode: 0 });
      });
      return row.count();
    })
    .toBe(1);
  await expect(row).not.toHaveClass(/project-tree-untabbed/);
  await expect(activeTab(page)).toHaveText("Audit agent refactor");
  await expect(page.locator('[role="treeitem"][data-tree-key="workspace:primary-1"]')).toHaveAttribute(
    "aria-current",
    "page",
  );
  await treeRow(page, "workspace:primary-2").click();
  await expect(stripTab(page, "Release checklist")).toHaveCount(1);
});

test("a workspace row switches the strip; an agent row of another workspace opens its tab and pane", async ({ page }) => {
  await openWorkspaceTabs(page);
  await treeRow(page, "workspace:audit-wt-login").click();
  await expect(strip(page).getByRole("tablist")).toHaveAttribute("aria-label", "Tabs in halv/login-flow");
  await expect(activeTab(page)).toHaveText(/Proxy restart/);
  await expect(tabBody(page, "audit-proxy")).toHaveAttribute("data-active", "true");
  await treeRow(page, "conversation:audit-docs-sweep").click();
  await expect(strip(page).getByRole("tablist")).toHaveAttribute("aria-label", "Tabs in halv/docs-refresh");
  await expect.poll(() => focusedBodyKey(page)).toBe("conversation:audit-docs-sweep");
  await expect(page.locator('[role="treeitem"][data-tree-key="workspace:audit-wt-docs"]')).toHaveAttribute(
    "aria-current",
    "page",
  );
});

test("a handoff moves the tab and its tree row to the target workspace", async ({ page }) => {
  await openWorkspaceTabs(page);
  await treeRow(page, "conversation:audit-proxy").click();
  await expect(activeTab(page)).toHaveText(/Proxy restart/);
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Hand off to Local" }).click();
  await expect(strip(page).getByRole("tablist")).toHaveAttribute("aria-label", "Tabs in main");
  await expect(activeTab(page)).toHaveText(/Proxy restart/);
  // The tab goes to the end of the target strip.
  expect((await tabKeys(page)).at(-1)).toBe("conversation:audit-proxy");
  // The login worktree has no tab left. The tree row moved with the tab.
  expect(await rowKeys(page, "audit-wt-login")).toEqual([]);
  const pill = page.locator('[role="treeitem"][data-tree-key="agents:primary-1"]');
  await pill.locator(":scope > .project-tree-row").click();
  expect(await rowKeys(page, "primary-1")).toContain("conversation:audit-proxy");
});

test("archive closes the tab and shows the most recent tab", async ({ page }) => {
  await plantParkedPanel(page);
  await openWorkspaceTabs(page);
  await page.getByRole("button", { name: "New tab" }).click();
  await page.getByRole("menuitem", { name: "Claude Code", exact: true }).click();
  await stripTab(page, "Parked login spike").click();
  await stripTab(page, "Audit agent refactor").click();
  await runCommand(page, "archive", "conversation.archive");
  await expect(stripTab(page, "Audit agent refactor")).toHaveCount(0);
  await expect(activeTab(page)).toHaveText("Parked login spike");
  const terminals = (await invoke(page, "terminal_list")) as Array<{ id: string; status: string }>;
  expect(terminals.find((item) => item.id === "tree-claude-primary")?.status).not.toBe("running");
  // The archived conversation opens loose: no strip, its title shows.
  await page.getByRole("button", { name: /Archived \(1\)/ }).click();
  await page.getByTitle("Audit agent refactor - archived conversation").click();
  await expect(strip(page)).toHaveCount(0);
  await expect(page.locator("[data-app-header]").getByText("Audit agent refactor")).toBeVisible();
  // Restore from the loose view moves the view into the strip.
  await page.getByRole("button", { name: "Actions for Audit agent refactor" }).last().click();
  await page.getByRole("menuitem", { name: "Restore conversation" }).click();
  await expect(activeTab(page).locator(".workspace-tab-title")).toHaveText("Audit agent refactor");
  await expect(page.locator('[role="treeitem"][data-tree-key="agents:primary-1"]')).toContainText("3 agents");
});

test("delete stops the sessions of the conversation before the row goes", async ({ page }) => {
  await openWorkspaceTabs(page);
  await treeRow(page, "conversation:audit-docs-sweep").click();
  await expect(activeTab(page)).toHaveText("Docs refresh sweep");
  await runCommand(page, "delete", "conversation.delete");
  await page.getByRole("dialog").getByRole("button", { name: "Confirm" }).click();
  await expect(stripTab(page, "Docs refresh sweep")).toHaveCount(0);
  const terminals = (await invoke(page, "terminal_list")) as Array<{ id: string; status: string }>;
  expect(terminals.some((item) => item.id === "tree-codex-docs" && item.status === "running")).toBe(false);
  // The strip of the workspace is empty: in Pro mode the launcher shows.
  await expect(page.locator("[data-workspace-launcher]")).toBeVisible();
  await expect(strip(page).locator('[data-draft="true"]')).toHaveCount(0);
});

test("the Agent sessions panel points a conversation agent to its tab", async ({ page }) => {
  await openWorkspaceTabs(page);
  await page.getByRole("button", { name: "New tab" }).click();
  await page.getByRole("menuitem", { name: "Claude Code", exact: true }).click();
  const pane = page.locator('.conversation-mode-surface[data-active="true"] .terminal-pane[data-terminal-session-id]');
  await expect(pane).toHaveCount(1);
  const sessionId = (await pane.getAttribute("data-terminal-session-id"))!;
  const title = await activeTab(page).innerText();
  await stripTab(page, "Audit agent refactor").click();

  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Agent sessions" }).click();
  await page.locator(".agent-session-list-select").filter({ hasText: /Claude/ }).first().click();
  const openInTab = page.getByRole("button", { name: "Open in tab" });
  await expect(openInTab).toBeVisible();
  // One node per session: the panel shows no second terminal.
  await expect(page.locator(`.terminal-pane[data-terminal-session-id="${sessionId}"]`)).toHaveCount(1);
  await openInTab.click();
  await expect(activeTab(page)).toHaveText(title);
  await expect.poll(() =>
    page.evaluate(() => document.activeElement?.closest(".terminal-pane")?.getAttribute("data-terminal-session-id")),
  ).toBe(sessionId);
});

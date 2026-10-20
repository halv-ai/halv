import assert from "node:assert/strict";
import test from "node:test";

// Restore, relaunch and downgrade checks against the shipped panel modules.
// They show that a tab needs no change to the panel layout format, that a
// relaunch with a live supervisor rebinds panes without a new tab, and where
// the owner field (S5a) is needed.

// @ts-expect-error Node's test runner needs the source extension.
import {
  collectTerminalLeafIds,
  deserializeTerminalPanelLayout,
  reconcileTerminalPanelLayout,
  serializeTerminalPanelLayout,
  splitTerminalLeaf,
  terminalLeaf,
  type TerminalPanelLayoutSnapshot,
} from "../src/terminalLayout.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  restoredTerminalSession,
  restoreTerminalPanel,
  type TerminalPaneState,
  type TerminalPanelState,
} from "../src/terminalRestore.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { filterRestoredPanesForAutoResume, reconcileListedTerminalPanes } from "../src/terminalPaneReconcile.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { closedTerminalRestorePayload, closeTerminalPanelSessions } from "../src/terminalClose.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { ownerKeyOfSession, panelAcceptsSession, shellTabIdFromScope, type SessionOwnerFact, type TabKey } from "../src/workspaceTabs.ts";
import type { TerminalSession } from "../src/types.ts";

const UPDATED_AT = "2026-10-14T18:00:00Z";
const SHELL_ID = "00000000-0000-4000-8000-000000000009";
const OTHER_SHELL_ID = "00000000-0000-4000-8000-000000000007";

function pane(paneId: string, sessionId: string, extra: Partial<TerminalPaneState> = {}): TerminalPaneState {
  return {
    paneId,
    sessionId,
    projectId: 1,
    conversationId: "c1",
    projectName: "app",
    cwd: "/u/halv/app",
    executionProfile: "full_access",
    agentKind: null,
    laneKind: null,
    credentialProfileId: null,
    customTitle: null,
    cliSessionId: null,
    cols: 100,
    rows: 24,
    scrollback: "saved output",
    updatedAt: UPDATED_AT,
    ...extra,
  };
}

/** A listed session. `shellTabId` is the S5a owner field. */
type LiveRow = TerminalSession & { shellTabId?: string | null };

function live(id: string, extra: Partial<LiveRow> = {}): LiveRow {
  return {
    id,
    projectId: 1,
    conversationId: "c1",
    projectName: "app",
    cwd: "/u/halv/app",
    shell: "/bin/zsh",
    pid: 4242,
    executionProfile: "full_access",
    agentKind: null,
    status: "running",
    cols: 100,
    rows: 24,
    startedAt: UPDATED_AT,
    ...extra,
  };
}

const owner = (row: LiveRow): SessionOwnerFact => ({
  id: row.id,
  conversationId: row.conversationId ?? null,
  shellTabId: row.shellTabId ?? null,
  agentKind: row.agentKind ?? null,
});

/** A conversation tab's panel as TerminalPanel writes it today: one split tree. */
function savedConversationPanel(): TerminalPanelState {
  const root = splitTerminalLeaf(terminalLeaf("pane-a"), "pane-a", "pane-b", "vertical");
  const layout: TerminalPanelLayoutSnapshot = {
    version: 1,
    activeTabId: "pane-a",
    activeLeafId: "pane-b",
    tabs: [{ tabId: "pane-a", root }],
  };
  return {
    layout,
    panes: [
      pane("pane-a", "sA", { agentKind: "claude", laneKind: "claudeSubscription", cliSessionId: "cli-A" }),
      pane("pane-b", "sB"),
    ],
    updatedAt: UPDATED_AT,
  };
}

let paneCounter = 0;
const createPaneId = () => `pane-new-${paneCounter++}`;

/** The refresh pipeline of TerminalPanel with the owner filter, built from
 *  the shipped functions: listed rows of the owner, then the detached rows
 *  of saved panes that no listed row holds. */
function refreshPanel(
  scope: TabKey,
  restored: ReturnType<typeof restoreTerminalPanel>,
  listed: LiveRow[],
  agentEntries: Array<{ session: { id: string; cliSessionId: string | null } }>,
) {
  const owned: TerminalSession[] = listed.filter((row) => panelAcceptsSession({ kind: "tab", key: scope }, owner(row)));
  const listedIds = new Set(owned.map((row) => row.id));
  const detached = Object.values(restored.panes)
    .filter((saved) => !listedIds.has(saved.sessionId))
    .map((saved) => restoredTerminalSession(saved));
  const rows = [...owned, ...detached];
  const reconciled = reconcileListedTerminalPanes(rows, restored.paneSessionIds, restored.panes, agentEntries, new Set(), createPaneId);
  const validPaneIds = reconciled.rows.flatMap((row) => {
    const paneId = Object.entries(reconciled.paneSessionIds).find(([, sessionId]) => sessionId === row.id)?.[0];
    return paneId ? [paneId] : [];
  });
  const layout = reconcileTerminalPanelLayout(restored.layout, validPaneIds, null, null, reconciled.appendablePaneIds);
  return { rows, paneSessionIds: reconciled.paneSessionIds, layout };
}

test("downgrade: a tab's panel is today's single-tree format; the shipped parser and restore read it unchanged", () => {
  const saved = savedConversationPanel();
  const roundTrip = deserializeTerminalPanelLayout(serializeTerminalPanelLayout(saved.layout as TerminalPanelLayoutSnapshot));
  assert.deepEqual(roundTrip, saved.layout, "no new field, no marker, no second tab");
  const restored = restoreTerminalPanel(saved, "c1");
  assert.deepEqual(restored.layout, saved.layout, "an older build restores the exact split");
  assert.deepEqual(restored.sessions.map((session) => session.id), ["sA", "sB"]);
});

test("the shipped merge rule keeps every panel at one tree, so panes never become tabs", () => {
  const legacy: TerminalPanelLayoutSnapshot = {
    version: 1,
    activeTabId: "t2",
    activeLeafId: "x",
    tabs: [
      { tabId: "t1", root: terminalLeaf("w") },
      { tabId: "t2", root: terminalLeaf("x") },
    ],
  };
  const merged = reconcileTerminalPanelLayout(legacy, ["w", "x", "y"], null, null, new Set(["y"]));
  assert.equal(merged.tabs.length, 1);
  assert.deepEqual(collectTerminalLeafIds(merged.tabs[0].root), ["w", "x", "y"]);
});

test("relaunch with a live supervisor: adopted sessions rebind, the split survives, an unsaved owned session splits in, other owners stay out", () => {
  const restored = restoreTerminalPanel(savedConversationPanel(), "c1");
  const listed = [
    live("sA", { agentKind: "claude" }),
    live("sB"),
    live("sC"),
    live("sD", { conversationId: null, shellTabId: SHELL_ID }),
    live("sE", { conversationId: null }),
    live("sF", { conversationId: "c2", agentKind: "codex" }),
  ];
  const agentEntries = [{ session: { id: "sA", cliSessionId: "cli-A" } }];
  const refreshed = refreshPanel("conversation:c1", restored, listed, agentEntries);
  assert.deepEqual(refreshed.rows.map((row) => row.id), ["sA", "sB", "sC"]);
  assert.equal(refreshed.paneSessionIds["pane-a"], "sA");
  assert.equal(refreshed.paneSessionIds["pane-b"], "sB");
  assert.equal(refreshed.layout.tabs.length, 1, "no new in-panel tab and no new workspace tab");
  const leaves = collectTerminalLeafIds(refreshed.layout.tabs[0].root);
  assert.deepEqual(leaves.slice(0, 2), ["pane-a", "pane-b"], "the saved split keeps its order");
  assert.equal(leaves.length, 3, "the unsaved owned session became one split");
  for (const row of listed) {
    assert.equal(panelAcceptsSession({ kind: "tab", key: "conversation:c1" }, owner(row)), row.conversationId === "c1");
  }
  const { droppedPaneIds, resumablePanes } = filterRestoredPanesForAutoResume(
    Object.values(restored.panes),
    listed,
    [{ session: { cliSessionId: "cli-A", status: "running" } }],
  );
  assert.deepEqual(droppedPaneIds.sort(), ["pane-a", "pane-b"], "auto-resume skips live panes");
  assert.deepEqual(resumablePanes, []);
});

test("relaunch without a supervisor: panes stay as leaves, agents auto-resume, a shell tab's shell gets its owner from the scope", () => {
  const restored = restoreTerminalPanel(savedConversationPanel(), "c1");
  const refreshed = refreshPanel("conversation:c1", restored, [], []);
  assert.equal(refreshed.layout.tabs.length, 1);
  assert.deepEqual(collectTerminalLeafIds(refreshed.layout.tabs[0].root), ["pane-a", "pane-b"], "restore keeps leaves of dead panes");
  const { resumablePanes } = filterRestoredPanesForAutoResume(Object.values(restored.panes), [], []);
  assert.deepEqual(resumablePanes.map((p) => p.paneId).sort(), ["pane-a", "pane-b"]);
  const scope = `shell:${SHELL_ID}` as TabKey;
  const recreated: SessionOwnerFact = { id: "sNew", shellTabId: shellTabIdFromScope(scope) };
  assert.equal(ownerKeyOfSession(recreated), scope);
  assert.ok(panelAcceptsSession({ kind: "tab", key: scope }, recreated));
});

test("a shell tab panel takes exactly its own shells; the global panel keeps the rest", () => {
  const listed = [
    live("sD", { conversationId: null, shellTabId: SHELL_ID }),
    live("sG", { conversationId: null, shellTabId: OTHER_SHELL_ID }),
    live("sE", { conversationId: null }),
    live("sA", { agentKind: "claude" }),
  ];
  const own = (key: TabKey) =>
    listed.filter((row) => panelAcceptsSession({ kind: "tab", key }, owner(row))).map((row) => row.id);
  assert.deepEqual(own(`shell:${SHELL_ID}`), ["sD"]);
  assert.deepEqual(own(`shell:${OTHER_SHELL_ID}`), ["sG"]);
  const global = listed
    .filter((row) => panelAcceptsSession({ kind: "unowned", scope: "panel:global" }, owner(row)))
    .map((row) => row.id);
  assert.deepEqual(global, ["sE"]);
  const refreshed = refreshPanel(`shell:${SHELL_ID}`, restoreTerminalPanel(null, null), listed, []);
  assert.equal(refreshed.layout.tabs.length, 1);
  assert.equal(collectTerminalLeafIds(refreshed.layout.tabs[0].root).length, 1);
});

test("Cmd+W: a pane of a split closes alone; only the explicit close of the last pane leaves no pane", () => {
  const layout = savedConversationPanel().layout as TerminalPanelLayoutSnapshot;
  const first = closeTerminalPanelSessions(layout, new Set(["pane-b"]));
  assert.deepEqual(first.tabs.map((tab) => collectTerminalLeafIds(tab.root)), [["pane-a"]]);
  const last = closeTerminalPanelSessions(first, new Set(["pane-a"]));
  assert.deepEqual(last.tabs, [], "the caller then runs the explicit close of the tab");
  // Emptiness from a refresh is not a close: the panel shows its empty
  // state, and no tab intent runs.
  assert.deepEqual(reconcileTerminalPanelLayout(layout, []).tabs, []);
});

test("park: a close keeps agent panes with a CLI session id and no scrollback; an empty write drops the scope", () => {
  const restored = restoreTerminalPanel(savedConversationPanel(), "c1");
  const snapshot = {
    scope: "conversation:c1",
    layout: restored.layout,
    sessions: restored.sessions,
    paneSessionIds: restored.paneSessionIds,
    agentEntries: [],
    laneOptions: [],
    restoredPanes: restored.panes,
    resumeFallbackPanes: {},
  };
  const parked = closedTerminalRestorePayload({ ...snapshot, excludedPaneIds: new Set(["pane-b"]) });
  assert.deepEqual(parked.panes.map((p) => [p.paneId, p.agentKind, p.cliSessionId]), [["pane-a", "claude", "cli-A"]]);
  assert.ok(parked.panes.every((p) => !("scrollback" in p)), "the park payload carries no scrollback");
  assert.deepEqual(collectTerminalLeafIds(parked.layout.tabs[0].root), ["pane-a"], "the layout is pruned to the kept panes");
  const cleared = closedTerminalRestorePayload({ ...snapshot, excludedPaneIds: new Set(Object.keys(restored.panes)) });
  assert.deepEqual(cleared.panes, [], "terminal_state.rs removes a scope whose pane list is empty");
  assert.deepEqual(cleared.layout.tabs, []);
});

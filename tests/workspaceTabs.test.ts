import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  activateTab,
  applyBatchToDisk,
  bulkCloseConfirm,
  cascadeConversationDelete,
  cascadeWorkspaceDelete,
  closeSuccessor,
  closeTab,
  conversationHome,
  conversationTabKey,
  currentConversationOf,
  cycleTab,
  defaultShellTitle,
  EMPTY_TABS_STATE,
  garbageTabKeys,
  homeWorkspaceId,
  isActivationBatch,
  mergeActivationBatches,
  moveTab,
  openConversationIntent,
  openTab,
  ownerKeyOfSession,
  panelAcceptsSession,
  panelScopeForTab,
  paneCloseNeedsConfirm,
  parseTabKey,
  plusMenuItems,
  rebase,
  renameShellTab,
  revealTarget,
  seedTabs,
  selectTabByNumber,
  selectWorkspaceNavigation,
  shellTabIdFromScope,
  shellTabKey,
  startComposingNavigation,
  stripView,
  tabCloseNeedsConfirm,
  tabCloseReasons,
  tabFacts,
  tabState,
  tabKeyForDigit,
  visibleTabKey,
  workspaceGlyph,
  type ClosePaneFact,
  type ConversationFact,
  type DiskState,
  type IntentResult,
  type SessionOwnerFact,
  type StripView,
  type TabFacts,
  type TabKey,
  type TabNavigation,
  type TabsOp,
  type TabsState,
  type WorkspaceFact,
} from "../src/workspaceTabs.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { terminalPanelStorageKey } from "../src/conversationWorkspace.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { shortAge } from "../src/shortAge.ts";
import type { AgentPaneView } from "../src/agentPaneStatus.ts";
import type { ConversationSummary } from "../src/types.ts";

// ---------------------------------------------------------------- fixtures

const ws = (id: string, projectId: number, kind: WorkspaceFact["kind"] = "worktree"): WorkspaceFact => ({
  id,
  projectId,
  kind,
});

function conv(
  id: string,
  projectId: number | null,
  workspaceId: string | null,
  extra: Partial<ConversationFact> = {},
): ConversationFact {
  return {
    id,
    projectId,
    workspaceId,
    mode: "agent",
    archived: false,
    createdAt: 1,
    updatedAt: 1,
    ...extra,
  };
}

function makeFacts(conversations: ConversationFact[], workspaces: WorkspaceFact[], complete = true): TabFacts {
  return {
    complete,
    conversations: new Map(conversations.map((c) => [c.id, c])),
    workspaces: new Map(workspaces.map((w) => [w.id, w])),
  };
}

const baseWorkspaces = [ws("primary-1", 1, "primary"), ws("wt-a", 1), ws("primary-2", 2, "primary")];
const ck = conversationTabKey;

function uuid(index: number) {
  return `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`;
}
const sk = (index: number) => shellTabKey(uuid(index));

function run(state: TabsState, ...steps: Array<(s: TabsState) => IntentResult>) {
  let current = state;
  const ops: TabsOp[] = [];
  for (const step of steps) {
    const result = step(current);
    assert.equal(result.rejected, undefined, `unexpected rejection: ${result.rejected}`);
    current = result.state;
    ops.push(...result.ops);
  }
  return { state: current, ops };
}

/** The shipped filter of a panel without a conversation. It has no owner
 *  field, so it accepts every conversation-less shell. */
function shippedUnboundPanelAccepts(session: SessionOwnerFact) {
  return !session.conversationId && !session.agentKind;
}

// ---------------------------------------------------------------- identity

test("tab key = owner key = today's panel scope; a shell scope names its owner", () => {
  const key = ck("c-1");
  assert.equal(panelScopeForTab(key), "conversation:c-1");
  const storage = terminalPanelStorageKey({ conversationId: "c-1", workspaceId: "wt-a", projectId: 1 });
  assert.equal(storage, `halv.terminal-layout.v1:${panelScopeForTab(key)}`, "saved layouts are reused as is");
  assert.deepEqual(parseTabKey("shell:s-1"), { kind: "shell", id: "s-1", key: "shell:s-1" });
  assert.equal(parseTabKey("conversation:"), null);
  assert.equal(parseTabKey("workspace:primary-1"), null, "legacy scopes are not tab keys");
  assert.equal(shellTabIdFromScope("shell:abc"), "abc");
  assert.equal(shellTabIdFromScope("conversation:abc"), null);
  assert.equal(shellTabIdFromScope("panel:global"), null);
});

test("facts come from the App lists and stay incomplete until both lists load", () => {
  const summary = {
    id: "c1",
    mode: "chat",
    createdAt: "2026-10-15T10:00:00Z",
    updatedAt: "2026-10-15T11:00:00Z",
    archivedAt: null,
    projectId: 1,
    workspaceId: null,
  } as ConversationSummary;
  assert.equal(tabFacts({ conversations: [summary], workspaces: null }).complete, false);
  const facts = tabFacts({ conversations: [summary], workspaces: [ws("primary-1", 1, "primary")] });
  assert.equal(facts.complete, true);
  assert.deepEqual(facts.conversations.get("c1"), {
    id: "c1",
    projectId: 1,
    workspaceId: null,
    mode: "chat",
    archived: false,
    createdAt: Date.parse("2026-10-15T10:00:00Z"),
    updatedAt: Date.parse("2026-10-15T11:00:00Z"),
  });
  assert.equal(conversationHome(facts.conversations.get("c1")!, facts.workspaces), "primary-1");
});

// ---------------------------------------------------------------- open and membership

test("open is explicit and idempotent: a second open only activates", () => {
  const facts = makeFacts([conv("c1", 1, "primary-1"), conv("c2", 1, "primary-1")], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: ck("c1"), now: 10 }),
    (s) => openTab(s, facts, { key: ck("c2"), now: 11 }),
    (s) => openTab(s, facts, { key: ck("c1"), now: 12 }),
  );
  const view = stripView(state, facts, "primary-1");
  assert.deepEqual(view.keys, ["conversation:c1", "conversation:c2"]);
  assert.equal(view.activeKey, "conversation:c1");
  assert.ok(
    openTab(state, facts, { key: ck("c1"), now: 13 }).ops.every((op) => op.type !== "insertTab"),
    "a second open never inserts",
  );
});

test("open accepts chat and agent conversations; it rejects archived, unknown, project-less and homeless owners", () => {
  const facts = makeFacts(
    [
      conv("chat", 1, "primary-1", { mode: "chat" }),
      conv("arch", 1, "primary-1", { archived: true }),
      conv("loose", null, null),
      conv("orphan", 3, null),
    ],
    baseWorkspaces,
  );
  assert.equal(openTab(EMPTY_TABS_STATE, facts, { key: ck("chat"), now: 1 }).rejected, undefined);
  for (const id of ["arch", "loose", "missing", "orphan"]) {
    const result = openTab(EMPTY_TABS_STATE, facts, { key: ck(id), now: 1 });
    assert.ok(result.rejected, id);
    assert.equal(result.ops.length, 0);
  }
  assert.ok(openTab(EMPTY_TABS_STATE, facts, { key: sk(1), shellWorkspaceId: "gone", now: 1 }).rejected);
  assert.ok(
    openTab(EMPTY_TABS_STATE, facts, { key: "shell:not-a-uuid", shellWorkspaceId: "primary-1", now: 1 }).rejected,
    "Rust accepts only shell:<uuid>",
  );
});

test("a stale workspace binding falls back to the primary strip, as in the project tree", () => {
  const facts = makeFacts([conv("c1", 1, "wt-gone")], baseWorkspaces);
  const { state } = run(EMPTY_TABS_STATE, (s) => openTab(s, facts, { key: ck("c1"), now: 1 }));
  assert.deepEqual(stripView(state, facts, "primary-1").keys, ["conversation:c1"]);
});

test("a Pro-mode toggle never changes a strip (the mode is the body, not the membership)", () => {
  const c1 = conv("c1", 1, "primary-1");
  const chat = conv("chat", 1, "primary-1", { mode: "chat" });
  const before = makeFacts([c1, chat], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, before, { key: ck("c1"), now: 1 }),
    (s) => openTab(s, before, { key: ck("chat"), now: 2 }),
  );
  const after = makeFacts([{ ...c1, mode: "chat" }, { ...chat, mode: "agent" }], baseWorkspaces);
  assert.deepEqual(stripView(state, after, "primary-1"), stripView(state, before, "primary-1"));
});

test("a handoff moves the conversation tab; it is never in two strips", () => {
  const c1 = conv("c1", 1, "primary-1");
  let facts = makeFacts([c1, conv("c2", 1, "primary-1")], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: ck("c1"), now: 1 }),
    (s) => openTab(s, facts, { key: ck("c2"), now: 2 }),
  );
  facts = makeFacts([{ ...c1, workspaceId: "wt-a" }, conv("c2", 1, "primary-1")], baseWorkspaces);
  assert.deepEqual(stripView(state, facts, "primary-1").keys, ["conversation:c2"]);
  assert.deepEqual(stripView(state, facts, "wt-a").keys, ["conversation:c1"]);
  assert.equal(stripView(state, facts, "wt-a").activeKey, "conversation:c1", "the MRU fallback picks the moved tab");
});

test("a legacy conversation without a workspace shows in the primary strip", () => {
  const facts = makeFacts([conv("old", 2, null)], baseWorkspaces);
  const { state } = run(EMPTY_TABS_STATE, (s) => openTab(s, facts, { key: ck("old"), now: 1 }));
  assert.deepEqual(stripView(state, facts, "primary-2").keys, ["conversation:old"]);
});

test("incomplete facts (app start): every view is empty; nothing is written or deleted", () => {
  const complete = makeFacts([conv("c1", 1, "primary-1")], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, complete, { key: ck("c1"), now: 1 }),
    (s) => openTab(s, complete, { key: sk(1), shellWorkspaceId: "primary-1", now: 2 }),
  );
  const loading = { ...complete, complete: false };
  for (const workspace of baseWorkspaces) assert.deepEqual(stripView(state, loading, workspace.id).keys, []);
  assert.deepEqual(garbageTabKeys(state, loading), [], "no GC on incomplete facts");
  assert.ok(openTab(state, loading, { key: ck("c1"), now: 3 }).rejected, "no intent writes while loading");
  assert.equal(Object.keys(state.rows).length, 2);
  assert.equal(state.activeByWorkspace["primary-1"], sk(1));
});

// ---------------------------------------------------------------- strip operations

test("close of the active tab: MRU first, then the right neighbor, then the left one", () => {
  const facts = makeFacts([conv("a", 1, "primary-1"), conv("b", 1, "primary-1"), conv("c", 1, "primary-1")], baseWorkspaces);
  const [a, b, c] = ["a", "b", "c"].map(ck);
  let { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: a, now: 1 }),
    (s) => openTab(s, facts, { key: b, now: 2 }),
    (s) => openTab(s, facts, { key: c, now: 3 }),
    (s) => activateTab(s, facts, a, 4),
    (s) => activateTab(s, facts, b, 5),
  );
  const closed = closeTab(state, facts, b);
  assert.equal(stripView(closed.state, facts, "primary-1").activeKey, a, "MRU wins over the visual neighbor");
  assert.ok(closed.ops.some((op) => op.type === "setActive" && op.key === a));
  const noHistory: TabsState = {
    ...state,
    rows: Object.fromEntries(
      Object.entries(state.rows).map(([k, r]) => [k, { ...r, activatedAt: k === b ? 9 : null }]),
    ),
  };
  assert.equal(stripView(closeTab(noHistory, facts, b).state, facts, "primary-1").activeKey, c, "right neighbor");
  state = {
    ...noHistory,
    activeByWorkspace: { "primary-1": c },
    rows: { ...noHistory.rows, [c]: { ...noHistory.rows[c], activatedAt: 9 }, [b]: { ...noHistory.rows[b], activatedAt: null } },
  };
  assert.equal(stripView(closeTab(state, facts, c).state, facts, "primary-1").activeKey, b, "left neighbor at the end");
});

test("closing the last tab leaves the workspace with no active tab", () => {
  const facts = makeFacts([conv("a", 1, "primary-1")], baseWorkspaces);
  const { state } = run(EMPTY_TABS_STATE, (s) => openTab(s, facts, { key: ck("a"), now: 1 }));
  const closed = closeTab(state, facts, ck("a"));
  assert.deepEqual(closed.ops, [
    { type: "deleteTab", key: "conversation:a" },
    { type: "setActive", workspaceId: "primary-1", key: null },
  ]);
  assert.equal(stripView(closed.state, facts, "primary-1").activeKey, null);
});

test("closing an inactive tab keeps the active tab and writes one op", () => {
  const facts = makeFacts([conv("a", 1, "primary-1"), conv("b", 1, "primary-1")], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: ck("a"), now: 1 }),
    (s) => openTab(s, facts, { key: ck("b"), now: 2 }),
  );
  const closed = closeTab(state, facts, ck("a"));
  assert.equal(stripView(closed.state, facts, "primary-1").activeKey, "conversation:b");
  assert.deepEqual(closed.ops, [{ type: "deleteTab", key: "conversation:a" }]);
});

test("the close successor rule on a strip view (Orca pickNextActiveTab)", () => {
  const view = (activated: Array<number | null>): StripView => {
    const keys = activated.map((_, index) => ck(`t${index}`));
    return {
      workspaceId: "primary-1",
      keys,
      rows: keys.map((key, index) => ({
        key,
        conversationId: `t${index}`,
        shellWorkspaceId: null,
        customTitle: null,
        defaultTitle: null,
        position: index,
        createdAt: 1,
        activatedAt: activated[index],
      })),
      activeKey: keys[1],
    };
  };
  assert.equal(closeSuccessor(view([5, 9, 7, null]), ck("t1")), ck("t2"), "most recent survivor");
  assert.equal(closeSuccessor(view([null, 9, null, null]), ck("t1")), ck("t2"), "right neighbor");
  assert.equal(closeSuccessor(view([null, null, 9]), ck("t2")), ck("t1"), "left neighbor at the end");
  assert.equal(closeSuccessor(view([3]), ck("t0")), null, "an empty strip");
});

test("open after an anchor, move, cycle, and select by number (Cmd+T, drag, Cmd+Shift+], Cmd+3)", () => {
  const facts = makeFacts([conv("a", 1, "primary-1"), conv("b", 1, "primary-1")], baseWorkspaces);
  const [a, b] = [ck("a"), ck("b")];
  const s1 = sk(1);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: a, now: 1 }),
    (s) => openTab(s, facts, { key: b, now: 2 }),
    (s) => openTab(s, facts, { key: s1, shellWorkspaceId: "primary-1", afterKey: a, now: 3 }),
  );
  assert.deepEqual(stripView(state, facts, "primary-1").keys, [a, s1, b]);
  assert.equal(state.rows[s1].defaultTitle, "Terminal 1");
  const moved = moveTab(state, facts, b, 0).state;
  assert.deepEqual(stripView(moved, facts, "primary-1").keys, [b, a, s1]);
  assert.equal(stripView(cycleTab(moved, facts, "primary-1", 1, 4).state, facts, "primary-1").activeKey, b, "s1 + 1 wraps to b");
  assert.equal(stripView(selectTabByNumber(moved, facts, "primary-1", 3, 5).state, facts, "primary-1").activeKey, s1);
  assert.equal(stripView(selectTabByNumber(moved, facts, "primary-1", 9, 5).state, facts, "primary-1").activeKey, s1, "Cmd+9 = last");
  assert.deepEqual(selectTabByNumber(moved, facts, "primary-1", 8, 6).ops, [], "a missing number is a no-op");
});

test("Cmd+1 to Cmd+8 select tab N, Cmd+9 selects the last tab", () => {
  const keys = "abcdefghijk".split("").map(ck);
  assert.equal(tabKeyForDigit(keys, 1), ck("a"));
  assert.equal(tabKeyForDigit(keys, 8), ck("h"));
  assert.equal(tabKeyForDigit(keys, 9), ck("k"));
  assert.equal(tabKeyForDigit(keys.slice(0, 3), 9), ck("c"));
  assert.equal(tabKeyForDigit(keys.slice(0, 3), 5), null);
  assert.equal(tabKeyForDigit(keys, 0), null);
  assert.equal(tabKeyForDigit([], 9), null);
});

test("cycling wraps in both directions and a one-tab strip does not write", () => {
  const facts = makeFacts([conv("a", 1, "primary-1"), conv("b", 1, "primary-1"), conv("c", 1, "primary-1")], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: ck("a"), now: 1 }),
    (s) => openTab(s, facts, { key: ck("b"), now: 2 }),
    (s) => openTab(s, facts, { key: ck("c"), now: 3 }),
  );
  assert.equal(stripView(cycleTab(state, facts, "primary-1", 1, 4).state, facts, "primary-1").activeKey, ck("a"));
  const first = activateTab(state, facts, ck("a"), 5).state;
  assert.equal(stripView(cycleTab(first, facts, "primary-1", -1, 6).state, facts, "primary-1").activeKey, ck("c"));
  const single = run(EMPTY_TABS_STATE, (s) => openTab(s, facts, { key: ck("a"), now: 1 })).state;
  assert.deepEqual(cycleTab(single, facts, "primary-1", 1, 2).ops, []);
});

test("activation is per workspace: each strip keeps its own active tab", () => {
  const facts = makeFacts(
    [conv("a", 1, "primary-1"), conv("b", 1, "primary-1"), conv("c", 1, "wt-a"), conv("d", 1, "wt-a")],
    baseWorkspaces,
  );
  const { state } = run(
    EMPTY_TABS_STATE,
    ...["a", "b", "c", "d"].map((id, index) => (s: TabsState) => openTab(s, facts, { key: ck(id), now: index, activate: false })),
    (s) => activateTab(s, facts, ck("b"), 10),
    (s) => activateTab(s, facts, ck("c"), 11),
  );
  assert.equal(stripView(state, facts, "primary-1").activeKey, ck("b"));
  assert.equal(stripView(state, facts, "wt-a").activeKey, ck("c"));
});

test("a drag keeps every tab once and the active tab unchanged", () => {
  const facts = makeFacts([conv("a", 1, "primary-1"), conv("b", 1, "primary-1"), conv("c", 1, "primary-1")], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    ...["a", "b", "c"].map((id, index) => (s: TabsState) => openTab(s, facts, { key: ck(id), now: index })),
  );
  const moved = moveTab(state, facts, ck("c"), 0).state;
  assert.deepEqual(stripView(moved, facts, "primary-1").keys, [ck("c"), ck("a"), ck("b")]);
  const back = moveTab(moved, facts, ck("c"), 99).state;
  assert.deepEqual(stripView(back, facts, "primary-1").keys, [ck("a"), ck("b"), ck("c")]);
  assert.equal(stripView(back, facts, "primary-1").activeKey, ck("c"));
});

test("only shell tabs keep their own title; default titles count up per strip", () => {
  const facts = makeFacts([conv("a", 1, "primary-1")], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: ck("a"), now: 1 }),
    (s) => openTab(s, facts, { key: sk(1), shellWorkspaceId: "primary-1", now: 2 }),
    (s) => openTab(s, facts, { key: sk(2), shellWorkspaceId: "primary-1", title: "  dev server ", now: 3 }),
    (s) => openTab(s, facts, { key: sk(3), shellWorkspaceId: "wt-a", now: 4 }),
  );
  assert.ok(renameShellTab(state, ck("a"), "x").rejected, "a conversation tab renames the conversation instead");
  assert.equal(renameShellTab(state, sk(1), "  build ").state.rows[sk(1)].customTitle, "build");
  assert.equal(renameShellTab(state, sk(1), "   ").state.rows[sk(1)].customTitle, null);
  assert.equal(state.rows[sk(2)].customTitle, "dev server");
  assert.equal(state.rows[sk(2)].defaultTitle, "Terminal 2");
  assert.equal(state.rows[sk(3)].defaultTitle, "Terminal 1");
  const afterClose = closeTab(state, facts, sk(1)).state;
  assert.equal(defaultShellTitle(afterClose, "primary-1"), "Terminal 3", "a closed number is not reused");
});

test("activation batches merge into one batch with the same result", () => {
  const facts = makeFacts([conv("a", 1, "primary-1"), conv("b", 1, "primary-1")], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: ck("a"), now: 1 }),
    (s) => openTab(s, facts, { key: ck("b"), now: 2 }),
  );
  const first = activateTab(state, facts, ck("a"), 3);
  const second = activateTab(first.state, facts, ck("b"), 4);
  assert.ok(isActivationBatch(first.ops));
  assert.ok(!isActivationBatch(closeTab(state, facts, ck("a")).ops));
  const merged = mergeActivationBatches(first.ops, second.ops);
  assert.equal(merged.length, 3, "two touches and one pointer");
  assert.deepEqual(
    applyBatchToDisk(state, merged, facts).disk,
    applyBatchToDisk(applyBatchToDisk(state, first.ops, facts).disk, second.ops, facts).disk,
  );
});

// ---------------------------------------------------------------- ownership

test("session ownership is a partition over tab panels and the global panel; the shipped unbound filter is not", () => {
  const sessions: SessionOwnerFact[] = [
    { id: "agent", conversationId: "c1", shellTabId: null, agentKind: "claude" },
    { id: "split-shell", conversationId: "c1", shellTabId: null, agentKind: null },
    { id: "tab-shell", conversationId: null, shellTabId: uuid(1), agentKind: null },
    { id: "login", conversationId: null, shellTabId: null, agentKind: null },
    { id: "corrupt", conversationId: "c1", shellTabId: uuid(1), agentKind: null },
  ];
  const tabScopes = [ck("c1"), sk(1)].map((key) => ({ kind: "tab", key }) as const);
  const global = { kind: "unowned", scope: "panel:global" } as const;
  for (const session of sessions) {
    const holders = [...tabScopes, global].filter((scope) => panelAcceptsSession(scope, session));
    assert.ok(holders.length <= 1, `${session.id} shows in ${holders.length} surfaces`);
  }
  assert.equal(ownerKeyOfSession(sessions[4]), "conversation:c1", "a conversation wins on corrupt data");
  assert.ok(panelAcceptsSession(global, sessions[3]), "an unowned login terminal stays reachable");
  assert.ok(!panelAcceptsSession(global, { id: "x", agentKind: "codex" }), "an unsaved unowned agent stays out");
  assert.ok(panelAcceptsSession(global, { id: "x", agentKind: "codex" }, (id) => id === "x"), "a saved one stays in");
  assert.ok(panelAcceptsSession({ kind: "loose-conversation", conversationId: "c1" }, sessions[1]));
  assert.ok(shippedUnboundPanelAccepts(sessions[2]), "the shipped filter grabs a shell-tab shell");
});

// ---------------------------------------------------------------- close prompts, "+" menu, reveal, glyphs

test("close prompts: idle agents are parked without a prompt; work in progress asks once", () => {
  const agent = (status: ClosePaneFact["status"]): ClosePaneFact => ({ kind: "agent", running: true, status, shellBusy: null });
  const shell = (busy: boolean | null): ClosePaneFact => ({ kind: "shell", running: true, status: null, shellBusy: busy });
  assert.equal(tabCloseNeedsConfirm([agent("idle")]), false);
  assert.equal(tabCloseNeedsConfirm([agent("done")]), false);
  assert.equal(tabCloseNeedsConfirm([agent("working")]), true);
  assert.equal(tabCloseNeedsConfirm([agent("needsApproval")]), true);
  assert.equal(tabCloseNeedsConfirm([agent("needsInput")]), true);
  assert.equal(tabCloseNeedsConfirm([agent(null)]), true, "no status yet: ask");
  assert.equal(tabCloseNeedsConfirm([{ ...agent("working"), running: false }]), false, "a stopped pane never asks");
  assert.equal(tabCloseNeedsConfirm([shell(false)]), false);
  assert.equal(tabCloseNeedsConfirm([shell(true)]), true);
  assert.equal(tabCloseNeedsConfirm([shell(null)]), true, "an unknown probe asks");
  assert.deepEqual(tabCloseReasons([agent("working"), agent("needsInput"), agent("idle"), shell(true)]), {
    busyAgents: 2,
    busyShells: 1,
  });
  assert.equal(paneCloseNeedsConfirm(agent("idle"), false), true, "a split pane close loses the agent");
  assert.equal(paneCloseNeedsConfirm(agent("idle"), true), false, "the last pane follows the tab rule");
  assert.equal(paneCloseNeedsConfirm(shell(false), false), false);
  assert.deepEqual(bulkCloseConfirm([[agent("idle")], [agent("working")], [shell(true)]]), { confirm: true, guardedTabs: 2 });
});

test("'+' menu: terminal first, default agent first with Option+Cmd+T on macOS only, settings last", () => {
  const agents = [
    { kind: "codex", ready: true },
    { kind: "claude", ready: true },
    { kind: "kimi", ready: false },
  ];
  const ids = (items: ReturnType<typeof plusMenuItems>) =>
    items.map((item) => (item.type === "item" ? item.id : item.type === "agent" ? `agent:${item.agentKind}` : "-"));
  const mac = plusMenuItems({ platform: "mac", agents, defaultAgent: "claude", shellTabs: true, reopen: { shown: true, available: false } });
  assert.deepEqual(ids(mac), ["new-terminal", "-", "agent:claude", "agent:codex", "-", "text-conversation", "reopen", "-", "agent-settings"]);
  // "Text conversation" has no chord: Cmd+N follows the Pro toggle (ADR 0020).
  assert.ok(mac.some((item) => item.type === "item" && item.id === "text-conversation" && item.shortcut === null));
  assert.deepEqual(mac[0], { type: "item", id: "new-terminal", shortcut: "⌘T", disabled: false });
  assert.deepEqual(mac[2], { type: "agent", agentKind: "claude", shortcut: "⌥⌘T" });
  assert.equal(mac[3].type === "agent" && mac[3].shortcut, null);
  assert.ok(mac.some((item) => item.type === "item" && item.id === "reopen" && item.disabled));
  const linux = plusMenuItems({ platform: "linux", agents, defaultAgent: "claude", shellTabs: true, reopen: { shown: false, available: false } });
  assert.deepEqual(linux[0], { type: "item", id: "new-terminal", shortcut: "Ctrl+Shift+T", disabled: false });
  assert.deepEqual(linux[2], { type: "agent", agentKind: "claude", shortcut: null });
  const shortcuts = [...linux, ...plusMenuItems({ platform: "windows", agents, defaultAgent: "claude", shellTabs: true, reopen: { shown: true, available: true } })]
    .flatMap((item) => (item.type === "separator" ? [] : [item.shortcut]));
  for (const chord of ["Ctrl+T", "Ctrl+W", "Ctrl+R"]) assert.ok(!shortcuts.includes(chord), `${chord} stays with the CLIs`);
  const early = plusMenuItems({ platform: "mac", agents: [], defaultAgent: null, shellTabs: false, reopen: { shown: false, available: false } });
  assert.deepEqual(ids(early), ["no-agents", "-", "text-conversation", "-", "agent-settings"], "before S5b and S7");
  assert.ok(early.some((item) => item.type === "item" && item.id === "no-agents" && item.disabled));
});

test("reveal: the tab of the session, else open the conversation, else background; reveal never writes", () => {
  const facts = makeFacts([conv("a", 1, "wt-a"), conv("closed", 1, "primary-1"), conv("arch", 1, "primary-1", { archived: true })], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: ck("a"), now: 1 }),
    (s) => openTab(s, facts, { key: sk(9), shellWorkspaceId: "primary-1", now: 2 }),
  );
  const frozen = JSON.stringify(state);
  assert.deepEqual(revealTarget(state, facts, { id: "x1", conversationId: "a" }), {
    kind: "tab",
    workspaceId: "wt-a",
    key: "conversation:a",
    sessionId: "x1",
  });
  assert.deepEqual(revealTarget(state, facts, { id: "x2", shellTabId: uuid(9) }), {
    kind: "tab",
    workspaceId: "primary-1",
    key: sk(9),
    sessionId: "x2",
  });
  assert.deepEqual(revealTarget(state, facts, { id: "x3", conversationId: "closed" }), { kind: "open-conversation", conversationId: "closed" });
  assert.deepEqual(revealTarget(state, facts, { id: "x4", conversationId: "arch" }), { kind: "background", sessionId: "x4" });
  assert.deepEqual(revealTarget(state, facts, { id: "x5" }), { kind: "background", sessionId: "x5" });
  assert.equal(JSON.stringify(state), frozen);
});

test("glyph ladder: needs input > failed > working > stopped > done > not reporting > idle; tabs and tree rows share it", () => {
  const view = (status: AgentPaneView["status"], unread = false, updatedAt = 0): AgentPaneView => ({
    status,
    reason: null,
    unread,
    waitSeq: 0,
    turnSeq: 0,
    prompt: null,
    source: "hook",
    updatedAt,
  });
  const glyph = (paneViews: AgentPaneView[], extra: { chatRunning?: boolean; chatUnviewed?: boolean } = {}) =>
    tabState({ paneViews, terminalStatus: null, chatRunning: extra.chatRunning ?? false, chatUnviewed: extra.chatUnviewed ?? false }).glyph;
  const unread = (paneViews: AgentPaneView[]) =>
    tabState({ paneViews, terminalStatus: null, chatRunning: false, chatUnviewed: false }).unread;
  assert.equal(glyph([view("working"), view("needsInput")]), "attention");
  assert.equal(glyph([view("working"), view("failed", true)]), "failed");
  assert.equal(glyph([view("working"), view("failed", false)]), "failed", "a failure stays after a view");
  assert.equal(glyph([view("done", true), view("working")]), "working");
  assert.equal(glyph([view("done", true)]), "done");
  assert.equal(glyph([view("done", false), view("idle")]), "done", "a read finished turn keeps its check, as in Orca");
  assert.equal(glyph([view("interrupted"), view("done")]), "interrupted");
  assert.equal(glyph([view("idle")]), "idle");
  assert.equal(glyph([{ ...view("idle"), source: "none" }]), "unverifiable", "a live pane without a signal");
  assert.equal(glyph([{ ...view("idle"), source: "heuristic" }]), "unverifiable");
  assert.equal(glyph([{ ...view("idle"), source: "none" }, view("idle")]), "unverifiable");
  assert.equal(glyph([view("exited")]), null);
  assert.equal(unread([view("done", true)]), true);
  assert.equal(unread([view("done", false)]), false, "only an unread turn is bold");
  assert.equal(unread([view("working", true)]), false);
  assert.equal(glyph([], { chatRunning: true }), "working", "a running chat turn is work");
  assert.equal(glyph([], { chatUnviewed: true }), "done", "an unviewed reply is an unread finished turn");
  assert.equal(workspaceGlyph([null, "done", "working"]), "working");
  assert.equal(workspaceGlyph([null, "failed", "attention"]), "attention");
  assert.equal(workspaceGlyph([null]), null);
});

test("short ages match Orca: now, minutes, hours, days", () => {
  const now = 10 * 86_400_000;
  assert.deepEqual(shortAge(now - 59_000, now), { unit: "now" });
  assert.deepEqual(shortAge(now - 18 * 60_000, now), { unit: "m", amount: 18 });
  assert.deepEqual(shortAge(now - (3 * 3_600_000 + 5), now), { unit: "h", amount: 3 });
  assert.deepEqual(shortAge(now - 5 * 86_400_000, now), { unit: "d", amount: 5 });
});

// ---------------------------------------------------------------- seed

test("seed: live and last-active always, recent saved agent scopes by recency up to the cap, creation order", () => {
  const day = 86_400_000;
  const now = 100 * day;
  const facts = makeFacts(
    [
      conv("live", 1, "primary-1", { createdAt: 5, updatedAt: 50 }),
      conv("last", 1, "primary-1", { createdAt: 1, updatedAt: 10, mode: "chat" }),
      conv("saved-new", 1, "primary-1", { createdAt: 3, updatedAt: 40 }),
      conv("saved-old", 1, "primary-1", { createdAt: 2, updatedAt: 20 }),
      conv("saved-stale", 1, "primary-1", { createdAt: 4, updatedAt: 30 }),
      conv("chat-saved", 1, "primary-1", { mode: "chat" }),
      conv("archived", 1, "primary-1", { archived: true }),
      conv("wt", 1, "wt-a", { createdAt: 9, updatedAt: 90 }),
      conv("no-project", null, null),
    ],
    baseWorkspaces,
  );
  const seeded = seedTabs({
    facts,
    savedConversationScopes: [
      { conversationId: "saved-new", updatedAt: now - 1 * day },
      { conversationId: "saved-old", updatedAt: now - 2 * day },
      { conversationId: "saved-stale", updatedAt: now - 30 * day },
      { conversationId: "chat-saved", updatedAt: now },
      { conversationId: "archived", updatedAt: now },
      { conversationId: "no-project", updatedAt: now },
      { conversationId: "wt", updatedAt: now },
      { conversationId: "deleted", updatedAt: now },
    ],
    liveConversationIds: new Set(["live", "no-project"]),
    lastActiveConversationId: "last",
    now,
    capPerWorkspace: 3,
  });
  const byWs = (id: string) =>
    seeded.rows
      .filter((row) => homeWorkspaceId(row, facts) === id)
      .sort((x, y) => x.position - y.position)
      .map((row) => row.key);
  assert.deepEqual(byWs("primary-1"), ["conversation:last", "conversation:saved-new", "conversation:live"]);
  assert.deepEqual(byWs("wt-a"), ["conversation:wt"]);
  assert.equal(seeded.rows.length, 4, "archived and project-less conversations never seed");
  assert.equal(seeded.activeByWorkspace["primary-1"], "conversation:last");
  assert.equal(seeded.activeByWorkspace["wt-a"], "conversation:wt");
  for (const row of seeded.rows) {
    assert.equal(applyBatchToDisk(EMPTY_TABS_STATE, [{ type: "insertTab", row }], facts).error, undefined, "seed rows pass the CHECK");
  }
});

// ---------------------------------------------------------------- navigation and the store protocol

test("navigation helpers: a click opens the tab in its home strip; the current conversation is derived", () => {
  const facts = makeFacts([conv("c1", 1, "wt-a"), conv("c2", 1, "primary-1", { mode: "chat" }), conv("loose", null, null)], baseWorkspaces);
  const first = openConversationIntent(selectWorkspaceNavigation("primary-1"), EMPTY_TABS_STATE, facts, "c1", 1);
  assert.deepEqual(first.nav, { kind: "workspace", workspaceId: "wt-a", composing: false }, "the strip follows the conversation");
  assert.equal(currentConversationOf(first.nav, first.result.state, facts), "c1");
  const again = openConversationIntent(first.nav, first.result.state, facts, "c1", 2);
  assert.ok(again.result.ops.every((op) => op.type !== "insertTab"), "a second click only activates");
  const loose = openConversationIntent(first.nav, first.result.state, facts, "loose", 3);
  assert.deepEqual(loose.nav, { kind: "loose", conversationId: "loose" });
  assert.deepEqual(loose.result.ops, [], "a project-less conversation never gets a tab");
  const composing = startComposingNavigation("primary-1");
  assert.equal(visibleTabKey(composing, first.result.state, facts), null, "a new task has no tab until the first send");
  assert.equal(currentConversationOf(composing, first.result.state, facts), null);
  const sent = openConversationIntent(composing, first.result.state, facts, "c2", 4);
  assert.equal(currentConversationOf(sent.nav, sent.result.state, facts), "c2");
  assert.deepEqual(stripView(sent.result.state, facts, "primary-1").keys, ["conversation:c2"]);
  assert.ok(openConversationIntent(first.nav, first.result.state, facts, "ghost", 5).result.rejected);
});

test("rebase: a read shows the snapshot plus the still-queued batches; a batch that would fail is left out", () => {
  const facts = makeFacts([conv("c1", 1, "primary-1"), conv("c2", 1, "primary-1")], baseWorkspaces);
  const disk = run(EMPTY_TABS_STATE, (s) => openTab(s, facts, { key: ck("c1"), now: 1 })).state;
  const queuedOpen = openTab(disk, facts, { key: ck("c2"), now: 2 }).ops;
  const ghost: TabsOp[] = [{ type: "insertTab", row: { ...disk.rows["conversation:c1"], key: ck("ghost"), conversationId: "ghost" } }];
  const view = rebase(disk, [queuedOpen, ghost], facts);
  assert.deepEqual(stripView(view, facts, "primary-1").keys, ["conversation:c1", "conversation:c2"]);
  assert.equal(stripView(view, facts, "primary-1").activeKey, "conversation:c2");
});

test("the disk model keeps SQL order: key check, CHECK, conflict, foreign keys", () => {
  const facts = makeFacts([conv("c1", 1, "primary-1")], baseWorkspaces);
  const row = run(EMPTY_TABS_STATE, (s) => openTab(s, facts, { key: ck("c1"), now: 1 })).state.rows[ck("c1")];
  const disk = applyBatchToDisk(EMPTY_TABS_STATE, [{ type: "insertTab", row }], facts).disk;
  assert.equal(applyBatchToDisk(disk, [{ type: "insertTab", row }], facts).error, undefined, "a duplicate is a no-op");
  assert.match(applyBatchToDisk(disk, [{ type: "insertTab", row: { ...row, customTitle: "x" } }], facts).error ?? "", /CHECK/, "CHECK before the conflict");
  assert.match(applyBatchToDisk(disk, [{ type: "insertTab", row: { ...row, key: ck("c2"), conversationId: "c2" } }], facts).error ?? "", /FOREIGN KEY/);
  assert.match(applyBatchToDisk(disk, [{ type: "insertTab", row: { ...row, key: "shell:x" as TabKey } }], facts).error ?? "", /not a tab key/);
  const failed = applyBatchToDisk(disk, [{ type: "deleteTab", key: ck("c1") }, { type: "insertTab", row: { ...row, key: ck("zz"), conversationId: "zz" } }], facts);
  assert.ok(failed.error);
  assert.equal(failed.disk, disk, "a failed batch rolls back");
});

test("a close never fails at rest: the successor pointer is a guarded no-op", () => {
  const facts = makeFacts([conv("a", 1, "primary-1"), conv("b", 1, "primary-1")], baseWorkspaces);
  const { state } = run(
    EMPTY_TABS_STATE,
    (s) => openTab(s, facts, { key: ck("a"), now: 1 }),
    (s) => openTab(s, facts, { key: ck("b"), now: 2 }),
  );
  const close = closeTab(state, facts, ck("b")).ops;
  const diskWithoutA = applyBatchToDisk(state, [{ type: "deleteTab", key: ck("a") }], facts).disk;
  const outcome = applyBatchToDisk(diskWithoutA, close, facts);
  assert.equal(outcome.error, undefined);
  assert.deepEqual(Object.keys(outcome.disk.rows), []);
});

test("naive concurrent writes can resurrect a closed tab; the serial queue cannot", () => {
  const facts = makeFacts([conv("a", 1, "primary-1")], baseWorkspaces);
  const opened = openTab(EMPTY_TABS_STATE, facts, { key: ck("a"), now: 1 });
  const closed = closeTab(opened.state, facts, ck("a"));
  const deliver = (order: TabsOp[][]) =>
    order.reduce<DiskState>((disk, ops) => applyBatchToDisk(disk, ops, facts).disk, EMPTY_TABS_STATE);
  assert.deepEqual(Object.keys(deliver([opened.ops, closed.ops]).rows), [], "FIFO delivery: closed stays closed");
  assert.deepEqual(Object.keys(deliver([closed.ops, opened.ops]).rows), ["conversation:a"], "out-of-order delivery leaves a zombie row");
});

test("the GC predicate: archived and project-less owners only, never a missing workspace", () => {
  const facts = makeFacts(
    [conv("ok", 1, "primary-1"), conv("arch", 1, "primary-1", { archived: true }), conv("loose", null, null), conv("homeless", 3, null)],
    baseWorkspaces,
  );
  const state: TabsState = {
    rows: Object.fromEntries(
      ["ok", "arch", "loose", "homeless"].map((id) => [
        ck(id),
        { key: ck(id), conversationId: id, shellWorkspaceId: null, customTitle: null, defaultTitle: null, position: 0, createdAt: 1, activatedAt: null },
      ]),
    ),
    activeByWorkspace: {},
  };
  assert.deepEqual(garbageTabKeys(state, facts).sort(), [ck("arch"), ck("loose")]);
  assert.equal(homeWorkspaceId(state.rows[ck("homeless")], facts), null, "hidden, but kept");
});

// ---------------------------------------------------------------- randomized interleavings

function prng(seed: number) {
  let x = seed >>> 0 || 1;
  return () => {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    return (x >>> 0) / 4294967296;
  };
}

type SimSession = SessionOwnerFact & { status: "running" | "exited" | "failed" };
type QueueItem = { kind: "batch"; ops: TabsOp[] } | { kind: "read" };

/** The parts under test. Each mutant replaces one part to prove that the
 *  randomized check catches the bug class that the design excludes. */
type Model = {
  openTab: typeof openTab;
  closeTab: typeof closeTab;
  /** Runs when a session appears. The design does nothing. */
  onSessionAppeared: (sim: Sim, session: SimSession) => void;
  /** Runs while the facts are incomplete. The design does nothing. */
  onLoading: (sim: Sim) => void;
  /** "fifo" is the serial queue of the design. */
  batchDelivery: "fifo" | "any";
  /** "queued": reads go through the same queue and rebase the queued batches. */
  reads: "queued" | "bypass";
  view: (sim: Sim, state: TabsState, facts: TabFacts, workspaceId: string) => StripView;
  globalPanelAccepts: (session: SimSession) => boolean;
  /** "derived": App keeps no copy of the current conversation. */
  currentTracking: "derived" | "stored";
};

const DESIGN: Model = {
  openTab,
  closeTab,
  onSessionAppeared: () => undefined,
  onLoading: () => undefined,
  batchDelivery: "fifo",
  reads: "queued",
  view: (_sim, state, facts, workspaceId) => stripView(state, facts, workspaceId),
  globalPanelAccepts: (session) => panelAcceptsSession({ kind: "unowned", scope: "panel:global" }, session),
  currentTracking: "derived",
};

type Sim = {
  model: Model;
  facts: { complete: boolean; conversations: Map<string, ConversationFact>; workspaces: Map<string, WorkspaceFact> };
  memory: TabsState;
  disk: DiskState;
  queue: QueueItem[];
  bypassReads: DiskState[];
  opened: Set<string>;
  closedPending: Set<string>;
  storedHomes: Map<string, string>;
  storedCurrent: string | null;
  nav: TabNavigation;
  sessions: SimSession[];
  now: number;
  counters: { shell: number; session: number; conversation: number };
  log: string[];
  random: () => number;
  stats: Record<string, number>;
};

const factsOf = (sim: Sim) => sim.facts as TabFacts;
const bump = (sim: Sim, name: string) => {
  sim.stats[name] = (sim.stats[name] ?? 0) + 1;
};

function viewsOf(sim: Sim, state: TabsState = sim.memory) {
  const facts = factsOf(sim);
  return [...facts.workspaces.keys()].sort().map((id) => {
    const view = sim.model.view(sim, state, facts, id);
    return { id, keys: view.keys, activeKey: view.activeKey };
  });
}

function modelVisibleKey(sim: Sim): TabKey | null {
  if (sim.nav.kind !== "workspace" || sim.nav.composing) return null;
  return sim.model.view(sim, sim.memory, factsOf(sim), sim.nav.workspaceId).activeKey;
}

function checkInvariants(sim: Sim, label: string) {
  const facts = factsOf(sim);
  const views = viewsOf(sim);
  const seenKey = new Map<string, string>();
  const seenOwner = new Map<string, string>();
  for (const view of views) {
    assert.equal(new Set(view.keys).size, view.keys.length, `${label}: duplicate inside strip ${view.id}`);
    for (const key of view.keys) {
      assert.ok(!seenKey.has(key), `${label}: ${key} in two strips (${seenKey.get(key)}, ${view.id})`);
      seenKey.set(key, view.id);
      const row = sim.memory.rows[key];
      const owner = row.conversationId !== null ? `conversation:${row.conversationId}` : key;
      assert.ok(!seenOwner.has(owner), `${label}: owner ${owner} has two tabs`);
      seenOwner.set(owner, key);
      if (row.conversationId !== null) {
        const conversation = facts.conversations.get(row.conversationId);
        const truth = conversation ? conversationHome(conversation, facts.workspaces) : null;
        assert.equal(view.id, truth, `${label}: ${key} outside its conversation's workspace`);
      } else {
        assert.equal(view.id, row.shellWorkspaceId, `${label}: ${key} outside its workspace`);
      }
    }
    assert.ok(
      view.keys.length === 0 ? view.activeKey === null : view.keys.includes(view.activeKey!),
      `${label}: invalid active tab in ${view.id}`,
    );
  }
  for (const key of Object.keys(sim.memory.rows)) assert.ok(sim.opened.has(key), `${label}: implicit tab ${key} in memory`);
  for (const key of Object.keys(sim.disk.rows)) assert.ok(sim.opened.has(key), `${label}: implicit tab ${key} at rest`);
  for (const key of sim.closedPending) assert.ok(!sim.memory.rows[key], `${label}: closed tab ${key} came back`);
  const looseScope = sim.nav.kind === "loose" && sim.nav.conversationId ? ck(sim.nav.conversationId) : null;
  for (const session of sim.sessions) {
    let holders = views
      .flatMap((view) => view.keys)
      .filter((key) => panelAcceptsSession({ kind: "tab", key }, session)).length;
    if (looseScope && ownerKeyOfSession(session) === looseScope) holders += 1;
    if (sim.model.globalPanelAccepts(session)) holders += 1;
    assert.ok(holders <= 1, `${label}: session ${session.id} in ${holders} surfaces`);
  }
  if (sim.model.currentTracking === "stored") {
    assert.equal(sim.storedCurrent, currentConversationOf(sim.nav, sim.memory, facts), `${label}: current conversation drifted`);
  }
  if (sim.nav.kind === "workspace") {
    assert.ok(facts.workspaces.has(sim.nav.workspaceId), `${label}: navigation to a missing workspace`);
  }
}

function enqueue(sim: Sim, result: IntentResult, explicit: boolean): boolean {
  if (result.rejected) {
    bump(sim, "rejected");
    return false;
  }
  for (const op of result.ops) {
    if (op.type === "insertTab") {
      if (explicit) sim.opened.add(op.row.key);
      sim.closedPending.delete(op.row.key);
      const home = homeWorkspaceId(op.row, factsOf(sim));
      if (home) sim.storedHomes.set(op.row.key, home);
    }
    if (op.type === "deleteTab") sim.closedPending.add(op.key);
  }
  sim.memory = result.state;
  if (result.ops.length > 0) sim.queue.push({ kind: "batch", ops: result.ops });
  return true;
}

function deliverOne(sim: Sim) {
  const facts = factsOf(sim);
  if (sim.model.reads === "bypass" && sim.bypassReads.length > 0 && sim.random() < 0.5) {
    sim.memory = sim.bypassReads.shift()!;
    bump(sim, "staleReadApplied");
    return;
  }
  if (sim.queue.length === 0) return;
  const index = sim.model.batchDelivery === "fifo" ? 0 : Math.floor(sim.random() * sim.queue.length);
  const [item] = sim.queue.splice(index, 1);
  const queuedBatches = () => sim.queue.flatMap((queued) => (queued.kind === "batch" ? [queued.ops] : []));
  if (item.kind === "read") {
    sim.memory = rebase(sim.disk, queuedBatches(), facts);
    bump(sim, "read");
    return;
  }
  const outcome = applyBatchToDisk(sim.disk, item.ops, facts);
  if (outcome.error) {
    // The store drops only the failed batch and shows the snapshot plus the rest.
    sim.log.push(`batch failed: ${outcome.error}`);
    bump(sim, "batchFailed");
    sim.memory = rebase(sim.disk, queuedBatches(), facts);
    return;
  }
  sim.disk = outcome.disk;
}

function deliver(sim: Sim, count: number) {
  for (let i = 0; i < count && (sim.queue.length > 0 || sim.bypassReads.length > 0); i += 1) deliverOne(sim);
}

function startupGc(sim: Sim) {
  const facts = factsOf(sim);
  const garbage = garbageTabKeys(sim.disk, facts);
  if (garbage.length === 0) return;
  bump(sim, "gcRows");
  sim.disk = applyBatchToDisk(sim.disk, garbage.map((key) => ({ type: "deleteTab", key })), facts).disk;
}

function simOpenConversation(sim: Sim, conversationId: string) {
  const facts = factsOf(sim);
  if (sim.model === DESIGN) {
    const reference = openConversationIntent(sim.nav, sim.memory, facts, conversationId, sim.now);
    if (!reference.result.rejected) {
      const conversation = facts.conversations.get(conversationId)!;
      assert.equal(reference.nav.kind, conversationHome(conversation, facts.workspaces) ? "workspace" : "loose");
    }
  }
  const conversation = facts.conversations.get(conversationId);
  if (!conversation) {
    bump(sim, "rejected");
    return;
  }
  const home = conversationHome(conversation, facts.workspaces);
  if (!home) {
    sim.nav = { kind: "loose", conversationId };
    sim.storedCurrent = conversationId;
    bump(sim, "openedLoose");
    return;
  }
  if (enqueue(sim, sim.model.openTab(sim.memory, facts, { key: ck(conversationId), now: sim.now }), true)) {
    sim.nav = { kind: "workspace", workspaceId: home, composing: false };
    sim.storedCurrent = conversationId;
  }
}

function spawn(sim: Sim, owner: { conversationId?: string | null; shellTabId?: string | null }, agentKind: string | null) {
  const session: SimSession = {
    id: `t${sim.counters.session++}`,
    conversationId: owner.conversationId ?? null,
    shellTabId: owner.shellTabId ?? null,
    agentKind,
    status: "running",
  };
  sim.sessions.push(session);
  const before = sim.memory;
  const queued = sim.queue.length;
  sim.model.onSessionAppeared(sim, session);
  if (sim.model === DESIGN) {
    assert.equal(sim.memory, before, "a session changed the tab state");
    assert.equal(sim.queue.length, queued, "a session queued a tab write");
  }
}

const EVENTS = [
  "sidebarOpen", "sidebarOpen", "newConversationSend", "startComposing", "openShellTab", "activate", "closeTab", "cmdW",
  "move", "cycle", "selectNumber", "switchWorkspace", "handoff", "archive", "toggleMode", "deleteConversation", "removeProject",
  "spawnSession", "spawnSession", "exitSession", "supervisorRestart", "refresh", "refetch", "incompleteFacts",
  "deliver", "deliver", "deliver", "crashReload",
] as const;

function simulate(seed: number, steps: number, model: Model = DESIGN) {
  const random = prng(seed);
  const pick = <T,>(items: readonly T[]): T | undefined => items[Math.floor(random() * items.length)];
  const workspaces = [ws("primary-1", 1, "primary"), ws("wt-a", 1), ws("wt-b", 1), ws("primary-2", 2, "primary")];
  const conversations = ["c1", "c2", "c3", "c4", "c5", "c6"].map((id, index) =>
    conv(id, index < 4 ? 1 : 2, index < 4 ? pick(["primary-1", "wt-a", "wt-b", null])! : "primary-2", {
      mode: random() < 0.7 ? "agent" : "chat",
      createdAt: index,
      updatedAt: index,
    }),
  );
  const sim: Sim = {
    model,
    facts: {
      complete: true,
      conversations: new Map(conversations.map((c) => [c.id, c])),
      workspaces: new Map(workspaces.map((w) => [w.id, w])),
    },
    memory: EMPTY_TABS_STATE,
    disk: EMPTY_TABS_STATE,
    queue: [],
    bypassReads: [],
    opened: new Set(),
    closedPending: new Set(),
    storedHomes: new Map(),
    storedCurrent: null,
    nav: selectWorkspaceNavigation("primary-1"),
    sessions: [],
    now: 100,
    counters: { shell: 0, session: 0, conversation: 0 },
    log: [],
    random,
    stats: {},
  };
  // The one-time seed: Rust writes the rows in one transaction before the first read.
  const seeded = seedTabs({
    facts: factsOf(sim),
    savedConversationScopes: conversations.filter(() => random() < 0.5).map((c) => ({ conversationId: c.id, updatedAt: sim.now })),
    liveConversationIds: new Set(conversations.filter(() => random() < 0.3).map((c) => c.id)),
    lastActiveConversationId: pick(conversations)!.id,
    now: sim.now,
    capPerWorkspace: 2,
    savedWithinMs: 1_000,
  });
  const seedState: TabsState = {
    rows: Object.fromEntries(seeded.rows.map((row) => [row.key, row])),
    activeByWorkspace: seeded.activeByWorkspace,
  };
  for (const row of seeded.rows) {
    sim.opened.add(row.key);
    const home = homeWorkspaceId(row, factsOf(sim));
    if (home) sim.storedHomes.set(row.key, home);
  }
  sim.memory = seedState;
  sim.disk = seedState;
  sim.storedCurrent = currentConversationOf(sim.nav, sim.memory, factsOf(sim));
  checkInvariants(sim, `seed ${seed}`);

  for (let step = 0; step < steps; step += 1) {
    sim.now += 1;
    const facts = factsOf(sim);
    const event = pick(EVENTS)!;
    bump(sim, event);
    const label = `seed ${seed} step ${step} ${event}`;
    const currentStrip = sim.nav.kind === "workspace" ? model.view(sim, sim.memory, facts, sim.nav.workspaceId) : null;
    switch (event) {
      case "sidebarOpen":
        simOpenConversation(sim, pick([...sim.facts.conversations.keys(), "ghost"])!);
        break;
      case "newConversationSend": {
        const workspace = sim.nav.kind === "workspace" ? sim.facts.workspaces.get(sim.nav.workspaceId) : undefined;
        const id = `n${sim.counters.conversation++}`;
        const mode = random() < 0.6 ? "agent" : "chat";
        sim.facts.conversations.set(
          id,
          conv(id, workspace?.projectId ?? null, workspace?.id ?? null, { mode, createdAt: sim.now, updatedAt: sim.now }),
        );
        simOpenConversation(sim, id);
        if (mode === "agent" && workspace) spawn(sim, { conversationId: id }, "claude");
        bump(sim, workspace ? "newInWorkspace" : "newLoose");
        break;
      }
      case "startComposing": {
        const workspaceId = sim.nav.kind === "workspace" ? sim.nav.workspaceId : pick([...sim.facts.workspaces.keys()])!;
        sim.nav = startComposingNavigation(workspaceId);
        sim.storedCurrent = null;
        break;
      }
      case "openShellTab": {
        const workspaceId = sim.nav.kind === "workspace" ? sim.nav.workspaceId : pick([...sim.facts.workspaces.keys(), "gone"])!;
        const id = uuid(sim.counters.shell++);
        const result = model.openTab(sim.memory, facts, {
          key: shellTabKey(id),
          shellWorkspaceId: workspaceId,
          afterKey: currentStrip?.activeKey ?? null,
          now: sim.now,
        });
        if (enqueue(sim, result, true)) {
          sim.nav = selectWorkspaceNavigation(workspaceId);
          spawn(sim, { shellTabId: id }, null);
        }
        break;
      }
      case "activate": {
        const key = pick(currentStrip?.keys ?? []);
        if (key) enqueue(sim, activateTab(sim.memory, facts, key, sim.now), true);
        if (key && sim.nav.kind === "workspace") sim.nav = selectWorkspaceNavigation(sim.nav.workspaceId);
        break;
      }
      case "closeTab": {
        const key = pick(currentStrip?.keys ?? []);
        if (!key || !currentStrip) break;
        const before = currentStrip;
        const remaining = before.rows.filter((row) => row.key !== key);
        const expectedMru = [...remaining]
          .filter((row) => row.activatedAt !== null)
          .sort(
            (a, b) =>
              b.activatedAt! - a.activatedAt! || b.position - a.position || b.createdAt - a.createdAt || (a.key < b.key ? 1 : -1),
          )[0]?.key;
        for (const session of sim.sessions) if (ownerKeyOfSession(session) === key) session.status = "exited";
        enqueue(sim, model.closeTab(sim.memory, facts, key), true);
        if (before.activeKey === key && sim.nav.kind === "workspace") {
          bump(sim, "closeActiveChecked");
          const after = model.view(sim, sim.memory, facts, sim.nav.workspaceId);
          const index = before.keys.indexOf(key);
          const expected = expectedMru ?? before.keys[index + 1] ?? before.keys[index - 1] ?? null;
          assert.equal(after.activeKey, expected, `${label}: close fallback`);
        }
        break;
      }
      case "cmdW": {
        // Close the focused pane. The tab closes when that pane was its last
        // one, or at once when the tab has no pane.
        const visible = modelVisibleKey(sim);
        if (!visible) break;
        const owned = sim.sessions.filter((session) => ownerKeyOfSession(session) === visible);
        if (owned.length === 0) {
          bump(sim, "cmdWEmptyTab");
          enqueue(sim, model.closeTab(sim.memory, facts, visible), true);
          break;
        }
        const victim = pick(owned)!;
        sim.sessions = sim.sessions.filter((session) => session !== victim);
        if (!sim.sessions.some((session) => ownerKeyOfSession(session) === visible)) {
          bump(sim, "lastPaneClosedTab");
          enqueue(sim, model.closeTab(sim.memory, facts, visible), true);
        }
        break;
      }
      case "move": {
        const key = pick(currentStrip?.keys ?? []);
        if (key) enqueue(sim, moveTab(sim.memory, facts, key, Math.floor(random() * 5)), true);
        break;
      }
      case "cycle":
        if (sim.nav.kind === "workspace") {
          enqueue(sim, cycleTab(sim.memory, facts, sim.nav.workspaceId, random() < 0.5 ? -1 : 1, sim.now), true);
        }
        break;
      case "selectNumber":
        if (sim.nav.kind === "workspace") {
          enqueue(sim, selectTabByNumber(sim.memory, facts, sim.nav.workspaceId, 1 + Math.floor(random() * 9), sim.now), true);
        }
        break;
      case "switchWorkspace":
        sim.nav = selectWorkspaceNavigation(pick([...sim.facts.workspaces.keys()])!);
        break;
      case "handoff": {
        const conversation = pick([...sim.facts.conversations.values()].filter((c) => c.projectId === 1));
        const target = pick([...sim.facts.workspaces.values()].filter((w) => w.projectId === 1));
        if (!conversation || !target) break;
        const wasCurrent = currentConversationOf(sim.nav, sim.memory, facts) === conversation.id;
        if (sim.memory.rows[ck(conversation.id)] && conversation.workspaceId !== target.id) bump(sim, "openTabMoved");
        sim.facts.conversations.set(conversation.id, { ...conversation, workspaceId: target.id });
        if (wasCurrent && random() < 0.7) {
          bump(sim, "followedHandoff");
          simOpenConversation(sim, conversation.id);
        }
        break;
      }
      case "archive": {
        const conversation = pick([...sim.facts.conversations.values()]);
        if (!conversation) break;
        if (!conversation.archived && sim.memory.rows[ck(conversation.id)] && random() < 0.7) {
          enqueue(sim, model.closeTab(sim.memory, facts, ck(conversation.id)), true);
        } else if (!conversation.archived) {
          bump(sim, "archivedWithoutClose");
        }
        sim.facts.conversations.set(conversation.id, { ...conversation, archived: !conversation.archived });
        if (conversation.archived && sim.nav.kind === "loose" && sim.nav.conversationId === conversation.id) {
          // Unarchive from the loose view opens the tab (spec 9.2).
          bump(sim, "unarchivedIntoStrip");
          simOpenConversation(sim, conversation.id);
        }
        break;
      }
      case "toggleMode": {
        const conversation = pick([...sim.facts.conversations.values()]);
        if (!conversation) break;
        const before = JSON.stringify(viewsOf(sim));
        sim.facts.conversations.set(conversation.id, { ...conversation, mode: conversation.mode === "agent" ? "chat" : "agent" });
        assert.equal(JSON.stringify(viewsOf(sim)), before, `${label}: a mode toggle changed a strip`);
        break;
      }
      case "deleteConversation": {
        const conversation = pick([...sim.facts.conversations.values()]);
        if (!conversation) break;
        sim.facts.conversations.delete(conversation.id);
        sim.disk = cascadeConversationDelete(sim.disk, conversation.id);
        if (random() < 0.7) sim.memory = cascadeConversationDelete(sim.memory, conversation.id);
        else bump(sim, "missedDeleteNotice");
        sim.sessions = sim.sessions.map((session) =>
          session.conversationId === conversation.id ? { ...session, status: "exited" } : session,
        );
        if (sim.nav.kind === "loose" && sim.nav.conversationId === conversation.id) sim.nav = { kind: "loose", conversationId: null };
        if (sim.storedCurrent === conversation.id && model.currentTracking === "derived") sim.storedCurrent = null;
        break;
      }
      case "removeProject": {
        if (!sim.facts.workspaces.has("primary-2")) break;
        sim.facts.workspaces.delete("primary-2");
        for (const c of [...sim.facts.conversations.values()]) {
          if (c.projectId === 2) sim.facts.conversations.set(c.id, { ...c, projectId: null, workspaceId: null });
        }
        sim.disk = cascadeWorkspaceDelete(sim.disk, "primary-2");
        if (random() < 0.7) sim.memory = cascadeWorkspaceDelete(sim.memory, "primary-2");
        if (sim.nav.kind === "workspace" && sim.nav.workspaceId === "primary-2") sim.nav = selectWorkspaceNavigation("primary-1");
        break;
      }
      case "spawnSession": {
        const roll = random();
        const shellRow = pick(Object.values(sim.memory.rows).filter((row) => row.shellWorkspaceId));
        if (roll < 0.45) {
          spawn(sim, { conversationId: pick([...sim.facts.conversations.keys(), "ghost"])! }, random() < 0.6 ? "claude" : null);
        } else if (roll < 0.75 && shellRow) {
          spawn(sim, { shellTabId: parseTabKey(shellRow.key)!.id }, null);
        } else {
          spawn(sim, {}, random() < 0.2 ? "codex" : null);
        }
        break;
      }
      case "exitSession": {
        const session = pick(sim.sessions);
        if (session) session.status = random() < 0.5 ? "exited" : "failed";
        break;
      }
      case "supervisorRestart": {
        // Transient emptiness: every PTY ends. No tab may close on its own.
        const before = sim.memory;
        for (const session of sim.sessions) session.status = "exited";
        assert.equal(sim.memory, before, `${label}: emptiness closed a tab`);
        break;
      }
      case "refresh": {
        // Pane reconcilers and every view run. The strip state must not change.
        const before = sim.memory;
        const queued = sim.queue.length;
        viewsOf(sim);
        for (const session of sim.sessions) ownerKeyOfSession(session);
        assert.equal(sim.memory, before, `${label}: a refresh wrote tab state`);
        assert.equal(sim.queue.length, queued, `${label}: a refresh queued a write`);
        break;
      }
      case "refetch":
        if (model.reads === "queued") sim.queue.push({ kind: "read" });
        else sim.bypassReads.push(sim.disk);
        break;
      case "incompleteFacts": {
        const rowsBefore = Object.keys(sim.memory.rows).length;
        sim.facts.complete = false;
        model.onLoading(sim);
        for (const view of viewsOf(sim)) assert.deepEqual(view.keys, [], `${label}: a view showed tabs on incomplete facts`);
        assert.ok(Object.keys(sim.memory.rows).length >= rowsBefore, `${label}: rows lost on incomplete facts`);
        sim.facts.complete = true;
        break;
      }
      case "deliver":
        deliver(sim, 1 + Math.floor(random() * 4));
        break;
      case "crashReload": {
        if (random() < 0.5) {
          deliver(sim, Number.POSITIVE_INFINITY);
          const memoryViews = JSON.stringify(viewsOf(sim));
          startupGc(sim);
          sim.memory = sim.disk;
          assert.equal(JSON.stringify(viewsOf(sim)), memoryViews, `${label}: reload changed the visible strips`);
        } else {
          bump(sim, "lostInFlightWrites");
          sim.queue = [];
          sim.bypassReads = [];
          startupGc(sim);
          sim.memory = sim.disk;
          for (const key of [...sim.closedPending]) if (sim.disk.rows[key]) sim.closedPending.delete(key);
        }
        break;
      }
    }
    checkInvariants(sim, label);
  }
  // Convergence: after the queue drains and a last read, memory and disk show the same strips.
  deliver(sim, Number.POSITIVE_INFINITY);
  sim.memory = rebase(sim.disk, [], factsOf(sim));
  assert.equal(JSON.stringify(viewsOf(sim)), JSON.stringify(viewsOf(sim, sim.disk)), `seed ${seed}: memory and disk diverged`);
  return sim;
}

test("randomized interleavings keep every invariant (300 seeds x 400 steps)", () => {
  const totals: Record<string, number> = {};
  let failedBatches = 0;
  for (let seed = 1; seed <= 300; seed += 1) {
    const sim = simulate(seed, 400);
    failedBatches += sim.log.length;
    for (const [name, count] of Object.entries(sim.stats)) totals[name] = (totals[name] ?? 0) + count;
  }
  for (const name of [
    "closeActiveChecked", "lastPaneClosedTab", "cmdWEmptyTab", "openTabMoved", "followedHandoff", "missedDeleteNotice",
    "lostInFlightWrites", "rejected", "openedLoose", "newInWorkspace", "archivedWithoutClose", "gcRows", "read",
  ]) {
    assert.ok((totals[name] ?? 0) > 50, `coverage: ${name} ran ${totals[name] ?? 0} times`);
  }
  assert.ok(failedBatches > 0, "the simulation exercised the failed-batch path");
});

// ---------------------------------------------------------------- mutants

function mutantCaught(model: Model, seeds = 200, steps = 400): string | null {
  for (let seed = 1; seed <= seeds; seed += 1) {
    try {
      simulate(seed, steps, model);
    } catch (error) {
      return `seed ${seed}: ${(error as Error).message.split("\n")[0]}`;
    }
  }
  return null;
}

test("mutant 1 (the August 2026 bug): a refresh opens a tab for a live session without one", () => {
  const caught = mutantCaught({
    ...DESIGN,
    onSessionAppeared: (sim, session) => {
      const key = ownerKeyOfSession(session);
      if (!key || sim.memory.rows[key]) return;
      enqueue(sim, openTab(sim.memory, factsOf(sim), { key, now: sim.now, activate: false }), false);
    },
  });
  assert.match(caught ?? "", /implicit tab/);
});

test("mutant 2: a non-idempotent open mints a second tab for one conversation", () => {
  let counter = 0;
  const caught = mutantCaught({
    ...DESIGN,
    openTab: (state, facts, intent) => {
      const original = state.rows[intent.key];
      if (!original || original.conversationId === null) return openTab(state, facts, intent);
      const twin = `${intent.key}#${counter++}` as TabKey;
      const rest = { ...state.rows };
      delete rest[intent.key];
      const result = openTab({ ...state, rows: rest }, facts, intent);
      if (result.rejected) return result;
      const swap = (key: TabKey | null) => (key === intent.key ? twin : key);
      const twinRow = { ...result.state.rows[intent.key], key: twin };
      const rows = { ...result.state.rows, [intent.key]: original, [twin]: twinRow };
      const activeByWorkspace = Object.fromEntries(
        Object.entries(result.state.activeByWorkspace).map(([workspaceId, key]) => [workspaceId, swap(key)]),
      );
      const ops = result.ops.map((op): TabsOp => {
        if (op.type === "insertTab") return { ...op, row: twinRow };
        if (op.type === "touchTab") return { ...op, key: swap(op.key)! };
        if (op.type === "setActive") return { ...op, key: swap(op.key) };
        if (op.type === "setPositions") return { ...op, positions: op.positions.map(([key, position]) => [swap(key)!, position]) };
        return op;
      });
      return { state: { rows, activeByWorkspace }, ops };
    },
  });
  assert.match(caught ?? "", /has two tabs/);
});

test("mutant 3: close falls back to the first tab instead of the MRU tab", () => {
  const caught = mutantCaught({
    ...DESIGN,
    closeTab: (state, facts, key) => {
      const result = closeTab(state, facts, key);
      const row = state.rows[key];
      const home = row ? homeWorkspaceId(row, facts) : null;
      if (!home) return result;
      const first = stripView(result.state, facts, home).keys[0] ?? null;
      return {
        state: { ...result.state, activeByWorkspace: { ...result.state.activeByWorkspace, [home]: first } },
        ops: [...result.ops.filter((op) => op.type !== "setActive"), { type: "setActive", workspaceId: home, key: first }],
      };
    },
  });
  assert.match(caught ?? "", /close fallback/);
});

test("mutant 4: batches reach Rust out of order (no serial queue)", () => {
  const caught = mutantCaught({ ...DESIGN, batchDelivery: "any" });
  assert.match(caught ?? "", /came back|diverged|reload changed|implicit tab/);
});

test("mutant 5: a loading pass deletes rows whose owner is not known yet", () => {
  const caught = mutantCaught({
    ...DESIGN,
    onLoading: (sim) => {
      const facts = factsOf(sim);
      for (const row of Object.values(sim.memory.rows)) {
        if (homeWorkspaceId(row, facts) === null) enqueue(sim, closeTab(sim.memory, facts, row.key), true);
      }
    },
  });
  assert.match(caught ?? "", /rows lost on incomplete facts/);
});

test("mutant 6: snapshot reads bypass the serial queue and land raw (stale overwrite)", () => {
  const caught = mutantCaught({ ...DESIGN, reads: "bypass" });
  assert.match(caught ?? "", /came back|diverged|reload changed/);
});

test("mutant 7: the tab row stores its workspace; a handoff leaves it in the old strip", () => {
  const caught = mutantCaught({
    ...DESIGN,
    view: (sim, state, facts, workspaceId) => {
      const rows = Object.values(state.rows)
        .filter((row) => facts.complete && homeWorkspaceId(row, facts) !== null && sim.storedHomes.get(row.key) === workspaceId)
        .sort((a, b) => a.position - b.position || a.createdAt - b.createdAt || (a.key < b.key ? -1 : 1));
      const keys = rows.map((row) => row.key);
      const stored = state.activeByWorkspace[workspaceId] ?? null;
      return { workspaceId, keys, rows, activeKey: stored && keys.includes(stored) ? stored : keys[0] ?? null };
    },
  });
  assert.match(caught ?? "", /outside its conversation's workspace/);
});

test("mutant 8: the global panel keeps the shipped unbound filter (no owner field)", () => {
  const caught = mutantCaught({ ...DESIGN, globalPanelAccepts: (session) => shippedUnboundPanelAccepts(session) });
  assert.match(caught ?? "", /in 2 surfaces/);
});

test("mutant 9: App keeps its own copy of the current conversation", () => {
  const caught = mutantCaught({ ...DESIGN, currentTracking: "stored" });
  assert.match(caught ?? "", /current conversation drifted/);
});

// ---------------------------------------------------------------- contracts

test("import contract: the tab modules and the panel layout modules never import each other", () => {
  const read = (path: string) => readFileSync(new URL(`../src/${path}`, import.meta.url), "utf8");
  const imports = (source: string) => [...source.matchAll(/from\s+["']([^"']+)["']/g)].map((match) => match[1]);
  for (const file of ["workspaceTabs.ts", "workspaceTabsStore.ts"]) {
    for (const specifier of imports(read(file))) {
      assert.ok(!/terminalLayout|TerminalPanel/.test(specifier), `${file} imports ${specifier}`);
    }
  }
  for (const file of ["terminalLayout.ts", "components/TerminalPanel.tsx"]) {
    for (const specifier of imports(read(file))) {
      assert.ok(!/workspaceTabs/.test(specifier), `${file} imports ${specifier}`);
    }
  }
});

test("structural rule: only the intent functions and the seed produce insert operations", () => {
  const source = readFileSync(new URL("../src/workspaceTabs.ts", import.meta.url), "utf8");
  const producers = [...source.matchAll(/export function (\w+)[\s\S]*?\n}\n/g)]
    .filter((match) => /type: "insertTab"/.test(match[0]))
    .map((match) => match[1])
    .sort();
  assert.deepEqual(producers, ["openTab"], "seedTabs returns rows; the store wraps them");
  assert.ok(!/export function \w*(sync|reconcile)\w*\(/i.test(source), "no sync or reconcile writer");
  assert.ok(!/export function \w+\([^)]*sessions\s*:/i.test(source), "no writer takes a session list");
});

import assert from "node:assert/strict";
import test from "node:test";
import type { AgentPaneView } from "../src/agentPaneStatus.ts";
import {
  EMPTY_LEDGER,
  attentionFinished,
  attentionWaits,
  buildQueue,
  chatCandidates,
  chatQueueKey,
  dockRows,
  itemInView,
  mergeCandidates,
  nextQueueItem,
  orderQueue,
  paneCandidate,
  paneQueueKey,
  queueDockRowLimit,
  queuePopoverPosition,
  reconcileLedger,
  visibleQueue,
  type AttentionCandidate,
  type AttentionItem,
  type QueueLedger,
  type QueueViewContext,
} from "../src/attentionQueue.ts";
import {
  defaultShortcut,
  shortcutConflict,
  shortcutDefinitions,
  shortcutMatchesEvent,
} from "../src/shortcuts.ts";

const NOW = 1_000_000;
const NOTHING_IN_VIEW: QueueViewContext = {
  focusedSessionId: null,
  viewedConversationId: null,
};

function view(overrides: Partial<AgentPaneView> = {}): AgentPaneView {
  return {
    status: "working",
    reason: null,
    unread: false,
    waitSeq: 0,
    turnSeq: 0,
    prompt: null,
    source: "presence",
    updatedAt: NOW - 60_000,
    ...overrides,
  };
}

function pane(
  sessionId: string,
  overrides: Partial<AgentPaneView>,
  conversationId: string | null = null,
) {
  return paneCandidate({ sessionId, conversationId, view: view(overrides) });
}

function item(
  key: string,
  enteredAt: number,
  kind: AttentionItem["kind"] = "done",
): AttentionItem {
  return {
    key,
    kind,
    enteredAt,
    instance: "x",
    at: null,
    conversationId: null,
    target: key.startsWith("pane:")
      ? { kind: "pane", sessionId: key.slice(5) }
      : { kind: "conversation", conversationId: key.slice(5) },
  };
}

function keyboardEvent(
  key: string,
  modifiers: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>>,
) {
  return {
    key,
    code: `Key${key.toUpperCase()}`,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...modifiers,
  };
}

test("a pane enters the queue when it waits or has an unread result", () => {
  assert.equal(pane("a", { status: "needsApproval" })?.kind, "approval");
  assert.equal(pane("a", { status: "needsInput" })?.kind, "input");
  assert.equal(pane("a", { status: "done", unread: true })?.kind, "done");
  assert.equal(pane("a", { status: "failed", unread: true })?.kind, "failed");
});

test("a working, idle, stopped, exited, or read pane stays out", () => {
  assert.equal(pane("a", { status: "working", unread: true }), null);
  assert.equal(pane("a", { status: "idle" }), null);
  assert.equal(pane("a", { status: "interrupted", unread: true }), null);
  assert.equal(pane("a", { status: "exited", unread: true }), null);
  assert.equal(pane("a", { status: "done", unread: false }), null);
  assert.equal(pane("a", { status: "failed", unread: false }), null);
  assert.equal(paneCandidate({ sessionId: "a", conversationId: null, view: null }), null);
});

test("a wait stays a candidate even when the pane is read", () => {
  // Reading never answers a question. The unread flag does not matter.
  assert.equal(pane("a", { status: "needsApproval", unread: false })?.kind, "approval");
  assert.equal(pane("a", { status: "needsInput", unread: true })?.kind, "input");
});

test("a chat enters for a pending approval, a question, or an unviewed reply", () => {
  const candidates = chatCandidates({
    unviewedConversationIds: new Set(["reply", "both"]),
    pendingApprovals: {
      shell: { callId: "c1", toolKind: "shell" },
      ask: { callId: "c2", toolKind: "mcp_elicitation" },
      both: { callId: "c3", toolKind: "apply_patch" },
    },
  });
  const byKey = new Map(candidates.map((candidate) => [candidate.key, candidate]));
  assert.equal(byKey.get(chatQueueKey("shell"))?.kind, "approval");
  assert.equal(byKey.get(chatQueueKey("ask"))?.kind, "input");
  assert.equal(byKey.get(chatQueueKey("reply"))?.kind, "done");
  // A conversation that waits and has an unviewed reply is one entry.
  assert.equal(byKey.get(chatQueueKey("both"))?.kind, "approval");
  assert.equal(candidates.length, 4);
});

test("a chat entry yields to a pane entry of the same conversation", () => {
  const panes = [pane("p1", { status: "needsInput" }, "conv-1")!];
  const chats = chatCandidates({
    unviewedConversationIds: new Set(["conv-1", "conv-2"]),
    pendingApprovals: {},
  });
  const merged = mergeCandidates(panes, chats);
  assert.deepEqual(
    merged.map((candidate) => candidate.key),
    [paneQueueKey("p1"), chatQueueKey("conv-2")],
  );
});

test("a loose pane does not hide any chat entry", () => {
  const panes = [pane("p1", { status: "done", unread: true }, null)!];
  const chats = chatCandidates({
    unviewedConversationIds: new Set(["conv-1"]),
    pendingApprovals: {},
  });
  assert.equal(mergeCandidates(panes, chats).length, 2);
});

test("the queue is first in, first out", () => {
  const first = pane("old", { status: "failed", unread: true, updatedAt: NOW - 600_000 })!;
  const second = pane("mid", { status: "needsInput", updatedAt: NOW - 120_000 })!;
  const third = pane("new", { status: "done", unread: true, updatedAt: NOW - 30_000 })!;
  const { items } = buildQueue({
    previous: EMPTY_LEDGER,
    candidates: [third, first, second],
    view: NOTHING_IN_VIEW,
    now: NOW,
  });
  assert.deepEqual(
    items.map((entry) => entry.key),
    ["pane:old", "pane:mid", "pane:new"],
  );
  // Colour tells the urgency. A question does not jump the line.
  assert.equal(items[1].kind, "input");
});

test("equal times sort by key", () => {
  const a = pane("b-pane", { status: "needsInput", updatedAt: NOW - 1000 })!;
  const b = pane("a-pane", { status: "needsInput", updatedAt: NOW - 1000 })!;
  const ledger = reconcileLedger(EMPTY_LEDGER, [a, b], NOW);
  assert.deepEqual(
    orderQueue([a, b], ledger).map((entry) => entry.key),
    ["pane:a-pane", "pane:b-pane"],
  );
});

test("a new pane gets the earlier of now and its update time", () => {
  const past = pane("past", { status: "needsInput", updatedAt: NOW - 5000 })!;
  const future = pane("future", { status: "needsInput", updatedAt: NOW + 5000 })!;
  const ledger = reconcileLedger(EMPTY_LEDGER, [past, future], NOW);
  assert.equal(ledger.get("pane:past")?.enteredAt, NOW - 5000);
  assert.equal(ledger.get("pane:future")?.enteredAt, NOW);
});

test("a chat entry gets the time that Halv first saw it", () => {
  const chats = chatCandidates({
    unviewedConversationIds: new Set(["c1"]),
    pendingApprovals: {},
  });
  const first = reconcileLedger(EMPTY_LEDGER, chats, NOW);
  assert.equal(first.get("chat:c1")?.enteredAt, NOW);
  const later = reconcileLedger(first, chats, NOW + 90_000);
  assert.equal(later, first);
  assert.equal(later.get("chat:c1")?.enteredAt, NOW);
});

test("an entry keeps its first time when Rust moves the update time", () => {
  const before = pane("a", { status: "needsApproval", waitSeq: 4, updatedAt: NOW - 50_000 })!;
  const ledger = reconcileLedger(EMPTY_LEDGER, [before], NOW);
  // The prompt detail arrives for the same wait and bumps updatedAt.
  const after = pane("a", { status: "needsApproval", waitSeq: 4, updatedAt: NOW - 1000 })!;
  const next = reconcileLedger(ledger, [after], NOW);
  assert.equal(next, ledger);
  assert.equal(next.get("pane:a")?.enteredAt, NOW - 50_000);
});

test("a new wait of the same pane enters at the back", () => {
  const other = pane("other", { status: "needsInput", updatedAt: NOW - 20_000 })!;
  const first = pane("a", { status: "needsApproval", waitSeq: 1, updatedAt: NOW - 90_000 })!;
  let ledger = reconcileLedger(EMPTY_LEDGER, [first, other], NOW);
  const second = pane("a", { status: "needsApproval", waitSeq: 2, updatedAt: NOW - 1000 })!;
  ledger = reconcileLedger(ledger, [second, other], NOW);
  assert.deepEqual(
    orderQueue([second, other], ledger).map((entry) => entry.key),
    ["pane:other", "pane:a"],
  );
});

test("a result that finishes again enters at the back", () => {
  const first = pane("a", { status: "done", unread: true, turnSeq: 1, updatedAt: NOW - 90_000 })!;
  const other = pane("b", { status: "done", unread: true, turnSeq: 1, updatedAt: NOW - 30_000 })!;
  let ledger = reconcileLedger(EMPTY_LEDGER, [first, other], NOW);
  // The user reads nothing. The agent runs and finishes a second turn.
  const second = pane("a", { status: "done", unread: true, turnSeq: 2, updatedAt: NOW - 100 })!;
  ledger = reconcileLedger(ledger, [second, other], NOW);
  assert.deepEqual(
    orderQueue([second, other], ledger).map((entry) => entry.key),
    ["pane:b", "pane:a"],
  );
});

test("an entry leaves the ledger when its state changes", () => {
  const waiting = pane("a", { status: "needsInput", updatedAt: NOW - 60_000 })!;
  let ledger: QueueLedger = reconcileLedger(EMPTY_LEDGER, [waiting], NOW);
  assert.equal(ledger.has("pane:a"), true);
  // The user answered. The agent works again, so the pane is no candidate.
  ledger = reconcileLedger(ledger, [], NOW + 1000);
  assert.equal(ledger.size, 0);
  // A later wait is a new entry with its own time.
  const again = pane("a", { status: "needsInput", waitSeq: 2, updatedAt: NOW + 4000 })!;
  ledger = reconcileLedger(ledger, [again], NOW + 5000);
  assert.equal(ledger.get("pane:a")?.enteredAt, NOW + 4000);
});

test("viewing hides a waiting item and it returns at its old place", () => {
  const first = pane("first", { status: "needsApproval", updatedAt: NOW - 300_000 })!;
  const second = pane("second", { status: "needsInput", updatedAt: NOW - 100_000 })!;
  let ledger: QueueLedger = EMPTY_LEDGER;
  let result = buildQueue({
    previous: ledger,
    candidates: [first, second],
    view: { focusedSessionId: "first", viewedConversationId: null },
    now: NOW,
  });
  assert.deepEqual(
    result.items.map((entry) => entry.key),
    ["pane:second"],
  );
  ledger = result.ledger;
  // The user looks away. The first item comes back in front.
  result = buildQueue({
    previous: ledger,
    candidates: [first, second],
    view: NOTHING_IN_VIEW,
    now: NOW + 60_000,
  });
  assert.deepEqual(
    result.items.map((entry) => entry.key),
    ["pane:first", "pane:second"],
  );
  assert.equal(result.ledger.get("pane:first")?.enteredAt, NOW - 300_000);
});

test("a chat is in view only for its own conversation", () => {
  const chat = item("chat:c1", 1, "approval");
  assert.equal(
    itemInView(chat, { focusedSessionId: null, viewedConversationId: "c1" }),
    true,
  );
  assert.equal(
    itemInView(chat, { focusedSessionId: null, viewedConversationId: "c2" }),
    false,
  );
  // A focused pane never hides a chat entry.
  assert.equal(
    itemInView(chat, { focusedSessionId: "c1", viewedConversationId: null }),
    false,
  );
  const terminal = item("pane:p1", 1, "input");
  assert.equal(
    itemInView(terminal, { focusedSessionId: "p1", viewedConversationId: null }),
    true,
  );
  assert.deepEqual(
    visibleQueue([chat, terminal], { focusedSessionId: "p1", viewedConversationId: "c1" }),
    [],
  );
});

test("the kinds split into waits and finished results", () => {
  assert.equal(attentionWaits("approval"), true);
  assert.equal(attentionWaits("input"), true);
  assert.equal(attentionWaits("done"), false);
  assert.equal(attentionFinished("done"), true);
  assert.equal(attentionFinished("failed"), true);
  assert.equal(attentionFinished("approval"), false);
});

test("the dock shows three rows, or one in a short window", () => {
  const items = ["a", "b", "c", "d", "e"].map((name, index) => item(`pane:${name}`, index));
  assert.equal(queueDockRowLimit(900), 3);
  assert.equal(queueDockRowLimit(700), 3);
  assert.equal(queueDockRowLimit(699), 1);
  const tall = dockRows(items, queueDockRowLimit(900));
  assert.deepEqual(
    tall.rows.map((entry) => entry.key),
    ["pane:a", "pane:b", "pane:c"],
  );
  assert.equal(tall.more, 2);
  const short = dockRows(items, queueDockRowLimit(600));
  assert.equal(short.rows.length, 1);
  assert.equal(short.more, 4);
  assert.deepEqual(dockRows(items.slice(0, 2), 3), { rows: items.slice(0, 2), more: 0 });
  assert.deepEqual(dockRows([], 3), { rows: [], more: 0 });
});

test("next opens the first item that is not the last opened one", () => {
  const items = [item("pane:a", 1, "input"), item("pane:b", 2, "approval"), item("chat:c", 3)];
  assert.equal(nextQueueItem(items, null)?.key, "pane:a");
  assert.equal(nextQueueItem(items, "pane:a")?.key, "pane:b");
  // Two waiting agents alternate: a is hidden while the user views it.
  assert.equal(nextQueueItem([items[1]], "pane:a")?.key, "pane:b");
  assert.equal(nextQueueItem([items[0]], "pane:b")?.key, "pane:a");
});

test("next opens the only item again, and does nothing for an empty queue", () => {
  const only = item("pane:a", 1, "input");
  assert.equal(nextQueueItem([only], "pane:a")?.key, "pane:a");
  assert.equal(nextQueueItem([], null), null);
  assert.equal(nextQueueItem([], "pane:a"), null);
});

test("the popover opens above its anchor and stays in the window", () => {
  const viewport = { width: 1280, height: 800 };
  const inSidebar = queuePopoverPosition({ left: 16, top: 700 }, viewport);
  assert.equal(inSidebar.left, 16);
  assert.equal(inSidebar.bottom, 800 - 700 + 6);
  assert.equal(queuePopoverPosition({ left: -40, top: 700 }, viewport).left, 8);
  assert.equal(queuePopoverPosition({ left: 1270, top: 700 }, viewport).left, 1280 - 360 - 8);
  assert.equal(queuePopoverPosition({ left: 16, top: 40 }, viewport).maxHeight, 120);
  assert.equal(queuePopoverPosition({ left: 16, top: 600 }, viewport).maxHeight, 600 - 6 - 8);
});

test("the next chord has no conflict and is not an AltGr or system chord", () => {
  const definition = shortcutDefinitions.find((entry) => entry.id === "task.next_in_queue");
  assert.ok(definition);
  for (const platform of ["MacIntel", "Linux x86_64", "Win32"]) {
    const binding = defaultShortcut(definition, platform);
    assert.notEqual(binding, "disabled");
    assert.equal(shortcutConflict(definition.id, binding, {}, platform), null);
  }
  // Ctrl+Alt is AltGr on many Windows layouts, so the chord avoids Alt.
  assert.equal(definition.defaultBinding.includes("alt"), false);
  assert.equal(
    shortcutMatchesEvent(
      keyboardEvent("u", { metaKey: true, shiftKey: true }),
      definition.defaultBinding,
      "MacIntel",
    ),
    true,
  );
  assert.equal(
    shortcutMatchesEvent(
      keyboardEvent("u", { metaKey: true }),
      definition.defaultBinding,
      "MacIntel",
    ),
    false,
  );
});

test("a candidate list built from panes and chats keeps unique keys", () => {
  const panes: AttentionCandidate[] = [
    pane("p1", { status: "needsInput" }, "c1")!,
    pane("p2", { status: "done", unread: true }, "c2")!,
  ];
  const chats = chatCandidates({
    unviewedConversationIds: new Set(["c1", "c2", "c3"]),
    pendingApprovals: { c3: { callId: "x", toolKind: "shell" } },
  });
  const keys = mergeCandidates(panes, chats).map((candidate) => candidate.key);
  assert.equal(new Set(keys).size, keys.length);
  assert.deepEqual(keys, ["pane:p1", "pane:p2", "chat:c3"]);
});

import type { AgentPaneView } from "./agentPaneStatus.ts";

// The attention queue. It lists what waits for the user, oldest first: an
// agent that asks for approval or input, and a finished result that the
// user did not view. The module is pure. The hook in useAttentionQueue.ts
// feeds it, and AttentionQueue.tsx draws it.

export type AttentionKind = "approval" | "input" | "done" | "failed";

export type AttentionTarget =
  | { kind: "pane"; sessionId: string }
  | { kind: "conversation"; conversationId: string };

/** A state that needs the user. The item stays until the state changes. */
export function attentionWaits(kind: AttentionKind) {
  return kind === "approval" || kind === "input";
}

/** A finished result. The user reads it to clear it. */
export function attentionFinished(kind: AttentionKind) {
  return kind === "done" || kind === "failed";
}

export type AttentionCandidate = {
  /** One key for each pane and each chat conversation. */
  key: string;
  target: AttentionTarget;
  kind: AttentionKind;
  /** The conversation that holds the item, or null for a loose pane. */
  conversationId: string | null;
  /** The identity of this wait or turn. A new value makes a new item. */
  instance: string;
  /** When the wait or turn happened, in ms. Null when Halv does not know. */
  at: number | null;
};

export type AttentionItem = AttentionCandidate & {
  /** When the item entered the queue. The queue sorts by this time. */
  enteredAt: number;
};

export function paneQueueKey(sessionId: string) {
  return `pane:${sessionId}`;
}

export function chatQueueKey(conversationId: string) {
  return `chat:${conversationId}`;
}

/**
 * The queue entry of one agent pane, or null. A pane enters when it needs
 * approval or input, or when it has a finished turn that the user did not
 * view. A working, idle, stopped, or read pane does not enter.
 */
export function paneCandidate(input: {
  sessionId: string;
  conversationId: string | null;
  view: AgentPaneView | null;
}): AttentionCandidate | null {
  const { view } = input;
  if (!view) return null;
  let kind: AttentionKind;
  let instance: string;
  if (view.status === "needsApproval" || view.status === "needsInput") {
    kind = view.status === "needsApproval" ? "approval" : "input";
    instance = `${view.status}:wait${view.waitSeq}`;
  } else if (view.unread && (view.status === "done" || view.status === "failed")) {
    kind = view.status;
    instance = `${view.status}:turn${view.turnSeq}`;
  } else {
    return null;
  }
  return {
    key: paneQueueKey(input.sessionId),
    target: { kind: "pane", sessionId: input.sessionId },
    kind,
    conversationId: input.conversationId,
    instance,
    at: Number.isFinite(view.updatedAt) && view.updatedAt > 0 ? view.updatedAt : null,
  };
}

/**
 * The queue entries of the chat conversations. A pending approval or an MCP
 * question waits for the user. An unviewed reply is a finished result. The
 * unviewed set cannot tell a reply from an error, so both read as done.
 */
export function chatCandidates(input: {
  unviewedConversationIds: ReadonlySet<string>;
  /** The pending tool call of each conversation. */
  pendingApprovals: Readonly<
    Record<string, { callId: string; toolKind: string }>
  >;
}): AttentionCandidate[] {
  const candidates: AttentionCandidate[] = [];
  const waiting = new Set<string>();
  for (const [conversationId, call] of Object.entries(input.pendingApprovals)) {
    waiting.add(conversationId);
    candidates.push({
      key: chatQueueKey(conversationId),
      target: { kind: "conversation", conversationId },
      kind: call.toolKind === "mcp_elicitation" ? "input" : "approval",
      conversationId,
      instance: `wait:${call.callId}`,
      at: null,
    });
  }
  for (const conversationId of input.unviewedConversationIds) {
    if (waiting.has(conversationId)) continue;
    candidates.push({
      key: chatQueueKey(conversationId),
      target: { kind: "conversation", conversationId },
      kind: "done",
      conversationId,
      instance: "unviewed",
      at: null,
    });
  }
  return candidates;
}

/**
 * One list of pane entries and chat entries. A chat entry yields to a pane
 * entry of the same conversation: the pane has the precise state.
 */
export function mergeCandidates(
  panes: readonly AttentionCandidate[],
  chats: readonly AttentionCandidate[],
): AttentionCandidate[] {
  const paneConversations = new Set<string>();
  for (const pane of panes) {
    if (pane.conversationId) paneConversations.add(pane.conversationId);
  }
  return [
    ...panes,
    ...chats.filter(
      (chat) => !chat.conversationId || !paneConversations.has(chat.conversationId),
    ),
  ];
}

/** The first time that Halv saw each entry. The queue sorts by it. */
export type QueueLedger = ReadonlyMap<
  string,
  { instance: string; enteredAt: number }
>;

export const EMPTY_LEDGER: QueueLedger = new Map();

/**
 * The ledger after one look at the candidates. An entry keeps its first
 * time while its wait or turn stays the same: Rust moves the update time
 * of a pane when the prompt detail arrives. A new wait or turn gets a new
 * time, so a result that finishes again enters at the back. An entry that
 * is no longer a candidate leaves the ledger. The first time of a pane is
 * the earlier of now and its update time. The first time of a chat entry
 * is now. Returns the same object when nothing changed.
 */
export function reconcileLedger(
  previous: QueueLedger,
  candidates: readonly AttentionCandidate[],
  now: number,
): QueueLedger {
  const next = new Map<string, { instance: string; enteredAt: number }>();
  let changed = previous.size !== candidates.length;
  for (const candidate of candidates) {
    const known = previous.get(candidate.key);
    if (known && known.instance === candidate.instance) {
      next.set(candidate.key, known);
      continue;
    }
    changed = true;
    next.set(candidate.key, {
      instance: candidate.instance,
      enteredAt: candidate.at === null ? now : Math.min(now, candidate.at),
    });
  }
  return changed ? next : previous;
}

/** The candidates in FIFO order: oldest first, then by key. */
export function orderQueue(
  candidates: readonly AttentionCandidate[],
  ledger: QueueLedger,
): AttentionItem[] {
  return candidates
    .flatMap((candidate) => {
      const entry = ledger.get(candidate.key);
      return entry ? [{ ...candidate, enteredAt: entry.enteredAt }] : [];
    })
    .sort(
      (left, right) =>
        left.enteredAt - right.enteredAt || left.key.localeCompare(right.key),
    );
}

/** What the user looks at now. A hidden or covered surface looks at nothing. */
export type QueueViewContext = {
  /** The pane that the user looks at in a focused window. */
  focusedSessionId: string | null;
  /** The chat conversation that the user reads in a focused window. */
  viewedConversationId: string | null;
};

/** True while the user looks at the item. The queue hides such an item. */
export function itemInView(item: AttentionItem, view: QueueViewContext) {
  return item.target.kind === "pane"
    ? view.focusedSessionId === item.target.sessionId
    : view.viewedConversationId === item.target.conversationId;
}

export function visibleQueue(items: readonly AttentionItem[], view: QueueViewContext) {
  return items.filter((item) => !itemInView(item, view));
}

/** The queue after one look: the new ledger and the items that show. */
export function buildQueue(input: {
  previous: QueueLedger;
  candidates: readonly AttentionCandidate[];
  view: QueueViewContext;
  now: number;
}): { ledger: QueueLedger; items: AttentionItem[] } {
  const ledger = reconcileLedger(input.previous, input.candidates, input.now);
  return { ledger, items: visibleQueue(orderQueue(input.candidates, ledger), input.view) };
}

export const QUEUE_DOCK_ROWS = 3;
export const QUEUE_DOCK_ROWS_COMPACT = 1;
/** A window under this height shows one dock row. */
export const QUEUE_COMPACT_HEIGHT_PX = 700;

export function queueDockRowLimit(viewportHeight: number) {
  return viewportHeight < QUEUE_COMPACT_HEIGHT_PX
    ? QUEUE_DOCK_ROWS_COMPACT
    : QUEUE_DOCK_ROWS;
}

/** The rows of the dock, and how many items the popover holds beyond them. */
export function dockRows<T extends AttentionItem>(items: readonly T[], limit: number) {
  return {
    rows: items.slice(0, limit),
    more: Math.max(0, items.length - limit),
  };
}

/**
 * The item that the "next" chord opens: the first item that is not the one
 * opened last. Two waiting agents alternate this way. When the last opened
 * item is the only one, it opens again: the user left it and it still waits.
 * An empty queue gives null.
 */
export function nextQueueItem<T extends AttentionItem>(
  items: readonly T[],
  lastOpenedKey: string | null,
): T | null {
  return items.find((item) => item.key !== lastOpenedKey) ?? items[0] ?? null;
}

export const QUEUE_POPOVER_WIDTH_PX = 360;
const QUEUE_POPOVER_MARGIN_PX = 8;
const QUEUE_POPOVER_GAP_PX = 6;

/**
 * The place of the popover: above its anchor, with its left edge at the
 * anchor and inside the window. `maxHeight` keeps it under the top edge.
 */
export function queuePopoverPosition(
  anchor: { left: number; top: number },
  viewport: { width: number; height: number },
  popoverWidth = QUEUE_POPOVER_WIDTH_PX,
) {
  const maxLeft = Math.max(
    QUEUE_POPOVER_MARGIN_PX,
    viewport.width - popoverWidth - QUEUE_POPOVER_MARGIN_PX,
  );
  return {
    left: Math.min(Math.max(anchor.left, QUEUE_POPOVER_MARGIN_PX), maxLeft),
    bottom: viewport.height - anchor.top + QUEUE_POPOVER_GAP_PX,
    maxHeight: Math.max(
      120,
      anchor.top - QUEUE_POPOVER_GAP_PX - QUEUE_POPOVER_MARGIN_PX,
    ),
  };
}

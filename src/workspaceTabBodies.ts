// Which tab bodies mount, and what each body shows (ADR 0010, retention v1).
//
// Retention v1 keeps today's cost: the visible tab, every tab whose owner
// runs a process, and every tab whose close is in progress stay mounted.
// A hidden body keeps its size and gets `inert`, so it never takes keys.
// A tab without a running process mounts again when the user shows it.

import { conversationTabKey, parseTabKey, type TabKey } from "./workspaceTabs.ts";

export type LiveSessionFacts = {
  agentEntries: ReadonlyArray<{
    session: { conversationId?: string | null; mode: string; status: string };
  }>;
  terminals: ReadonlyArray<{ conversationId?: string | null; status: string }>;
};

const LIVE_AGENT_STATUSES = new Set(["starting", "running", "waiting_input"]);

/** Conversations with a running non-GUI agent or a running owned terminal. */
export function liveConversationIds(facts: LiveSessionFacts): Set<string> {
  const ids = new Set<string>();
  for (const entry of facts.agentEntries) {
    const conversationId = entry.session.conversationId;
    if (
      conversationId &&
      entry.session.mode !== "gui" &&
      LIVE_AGENT_STATUSES.has(entry.session.status)
    ) {
      ids.add(conversationId);
    }
  }
  for (const terminal of facts.terminals) {
    if (terminal.conversationId && terminal.status === "running") {
      ids.add(terminal.conversationId);
    }
  }
  return ids;
}

/** Shell tabs with a running shell of their own. */
export function liveShellTabIds(
  terminals: ReadonlyArray<{ shellTabId?: string | null; status: string }>,
): Set<string> {
  return new Set(
    terminals.flatMap((terminal) =>
      terminal.shellTabId && terminal.status === "running" ? [terminal.shellTabId] : [],
    ),
  );
}

/** The tab keys whose bodies stay mounted, in a stable order. */
export function mountedTabKeys(input: {
  /** Every stored tab with a home now, in any strip. */
  keys: readonly TabKey[];
  visibleKey: TabKey | null;
  liveConversationIds: ReadonlySet<string>;
  /** Shell tabs with a running shell. */
  liveShellTabIds?: ReadonlySet<string>;
  closingKeys: ReadonlySet<TabKey>;
  /** The loose view shows a conversation without a tab. */
  looseConversationId?: string | null;
}): TabKey[] {
  const mounted = input.keys.filter((key) => {
    if (key === input.visibleKey || input.closingKeys.has(key)) return true;
    const parsed = parseTabKey(key);
    if (parsed?.kind === "shell") return input.liveShellTabIds?.has(parsed.id) ?? false;
    return parsed?.kind === "conversation" && input.liveConversationIds.has(parsed.id);
  });
  const extra: TabKey[] = [];
  if (input.visibleKey && !mounted.includes(input.visibleKey)) extra.push(input.visibleKey);
  if (input.looseConversationId) {
    const looseKey = conversationTabKey(input.looseConversationId);
    if (!mounted.includes(looseKey) && !extra.includes(looseKey)) extra.push(looseKey);
  }
  return [...mounted, ...extra].sort();
}

export type ConversationBodyKind = "ghostty" | "terminal" | "chat";

/** The body of a conversation tab. The mode selects it. It never decides
 *  whether the tab exists. */
export function conversationBodyKind(input: {
  mode: "chat" | "agent";
  /** The live agent session of the conversation, if any. */
  agentMode: string | null;
  runningOwnedTerminal: boolean;
  savedTerminals: boolean;
  /** The first send started an agent and it has no session yet. */
  opening: boolean;
}): ConversationBodyKind {
  if (input.mode !== "agent") return "chat";
  if (input.agentMode === "terminal_ghostty") return "ghostty";
  if (
    input.agentMode === "terminal_embedded" ||
    input.runningOwnedTerminal ||
    input.savedTerminals ||
    input.opening
  ) {
    return "terminal";
  }
  return "chat";
}

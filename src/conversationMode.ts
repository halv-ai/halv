import type { AgentBackend, ConversationSummary } from "./types";
import type { AgentKind, AgentSessionEntry } from "./agentSessions";

export type ConversationMode = "chat" | "agent";

export function modeForNewConversation(proModeDefault: boolean): ConversationMode {
  return proModeDefault ? "agent" : "chat";
}

/** The mode of a new conversation from the composer. A "Text conversation"
 *  of the launcher or the "+" menu (ADR 0020) is always a chat, also in Pro
 *  mode: the user picks the agent and the model in the composer. */
export function modeForComposerDraft(
  proModeDefault: boolean,
  textConversation: boolean,
): ConversationMode {
  return textConversation ? "chat" : modeForNewConversation(proModeDefault);
}

export function modeForConversationSelection(
  conversation: Pick<ConversationSummary, "mode"> | null | undefined,
): ConversationMode {
  return conversation?.mode === "agent" ? "agent" : "chat";
}

export function conversationUsesAgentSurface(
  conversation: Pick<ConversationSummary, "mode"> | null | undefined,
) {
  return modeForConversationSelection(conversation) === "agent";
}

export function agentKindForBackend(
  backend: AgentBackend | null | undefined,
): AgentKind | null {
  if (backend === "claude_session") return "claude";
  if (backend === "codex_session") return "codex";
  if (backend === "kimi_session") return "kimi";
  if (backend === "zai_session") return "zai";
  if (backend === "grok_session") return "grok";
  return null;
}

// The Pro toggle also applies to the open conversation, so an existing chat
// can hand off to a terminal without starting over. Only a real OFF→ON
// transition converts; app start and conversation switches never do.
export function conversationProModeSwitch(
  previousEnabled: boolean,
  enabled: boolean,
  conversation: Pick<ConversationSummary, "mode"> | null | undefined,
): ConversationMode | null {
  if (previousEnabled === enabled || !enabled) return null;
  if (!conversation || conversation.mode === "agent") return null;
  return "agent";
}

export function conversationDisplayTitle(
  conversation: Pick<ConversationSummary, "mode" | "title" | "titleSource">,
  agentTitle?: string | null,
) {
  if (conversation.titleSource === "manual") return conversation.title;
  const resolvedAgentTitle = agentTitle?.trim();
  return conversation.mode === "agent" && resolvedAgentTitle
    ? resolvedAgentTitle
    : conversation.title;
}

export function conversationNamingTerminal<
  T extends {
    id: string;
    conversationId: string | null;
    customTitle: string | null;
  },
>(terminals: readonly T[], conversationId: string): T | null {
  const boundTerminals = terminals
    .filter((terminal) => terminal.conversationId === conversationId)
    .sort((a, b) => a.id.localeCompare(b.id));
  return (
    boundTerminals.find((terminal) => terminal.customTitle?.trim()) ??
    boundTerminals[0] ??
    null
  );
}

// The terminal title that names an agent conversation. A plain shell names
// the conversation only when the user gave it a title: its default title
// (the project name) must not replace the agent title.
export function conversationNamingTitle<
  T extends {
    id: string;
    conversationId: string | null;
    customTitle: string | null;
    agentKind: string | null;
    title: string;
  },
>(terminals: readonly T[], conversationId: string): string | null {
  return (
    conversationNamingTerminal(
      terminals.filter(
        (terminal) => terminal.agentKind || terminal.customTitle?.trim(),
      ),
      conversationId,
    )?.title ?? null
  );
}

export type ConversationTerminalStatus =
  | "starting"
  | "running"
  | "waiting_input"
  | "exited"
  | "failed";

// Agent panes drive the conversation status. A plain shell has no turn
// tracking, so its "running" status would keep the spinner on forever.
export function conversationTerminalStatus<
  T extends {
    conversationId: string | null;
    status: ConversationTerminalStatus;
    agentKind: string | null;
  },
>(terminals: readonly T[], conversationId: string) {
  const statuses = terminals
    .filter(
      (terminal) =>
        terminal.conversationId === conversationId && terminal.agentKind,
    )
    .map((terminal) => terminal.status);
  for (const status of [
    "running",
    "starting",
    "waiting_input",
    "failed",
    "exited",
  ] as const) {
    if (statuses.includes(status)) return status;
  }
  return null;
}

export function agentSessionForConversationMode(
  entries: readonly AgentSessionEntry[],
  conversationId: string | null,
  mode: ConversationMode,
) {
  const candidates = entries.filter(
    (entry) => entry.session.conversationId === conversationId,
  );
  const active = (entry: AgentSessionEntry) =>
    ["starting", "running", "waiting_input"].includes(entry.session.status);
  const modeMatches = (entry: AgentSessionEntry) =>
    mode === "agent"
      ? entry.session.mode !== "gui"
      : entry.session.mode === "gui";
  return (
    candidates.find((entry) => modeMatches(entry) && active(entry)) ??
    candidates.find(modeMatches) ??
    candidates.find(active) ??
    candidates[0] ??
    null
  );
}

import { useEffect, useMemo, useRef, useSyncExternalStore } from "react";
import {
  agentPaneNotificationText,
  agentPaneTitle,
  agentPromptSummary,
  type AgentPaneGlyph,
} from "./agentPaneStatus.ts";
import { agentSessionAgentLabel } from "./agentSessions.ts";
import {
  EMPTY_LEDGER,
  buildQueue,
  chatCandidates,
  mergeCandidates,
  paneCandidate,
  type AttentionCandidate,
  type AttentionItem,
  type QueueLedger,
} from "./attentionQueue.ts";
import type { SidebarTerminalSession } from "./components/ThreadSidebar";
import {
  conversationDisplayTitle,
  conversationNamingTitle,
} from "./conversationMode.ts";
import { useT } from "./i18n/runtime";
import {
  shortcutForAction,
  shortcutMatchesEvent,
  type KeyboardShortcutOverrides,
} from "./shortcuts.ts";
import type { ConversationSummary, ToolCallUi } from "./types.ts";

/** One row of the queue, with the text that the UI shows. */
export type AttentionQueueRow = AttentionItem & {
  /** The conversation title, or the title of the terminal. */
  title: string;
  /** One sentence for the state, such as "Claude needs your approval". */
  heading: string;
  /** What the agent asks, or null when it gave no detail. */
  detail: string | null;
  glyph: AgentPaneGlyph;
};

export const NEXT_IN_QUEUE_SHORTCUT = "task.next_in_queue";

const CHAT_DETAIL_MAX_CHARS = 200;

function subscribeWindowFocus(listener: () => void) {
  window.addEventListener("focus", listener);
  window.addEventListener("blur", listener);
  document.addEventListener("visibilitychange", listener);
  return () => {
    window.removeEventListener("focus", listener);
    window.removeEventListener("blur", listener);
    document.removeEventListener("visibilitychange", listener);
  };
}

function windowFocusedNow() {
  return document.hasFocus() && document.visibilityState === "visible";
}

function glyphOf(kind: AttentionItem["kind"]): AgentPaneGlyph {
  return kind === "approval" || kind === "input" ? "attention" : kind;
}

/**
 * The rows of the attention queue that the user can act on, oldest first.
 * An item that the user looks at now is not in the list. A pane is in view
 * when it has focus in a focused window. A chat is in view when it is open
 * in a focused window and no full-window surface covers it.
 */
export function useAttentionQueue(input: {
  terminals: readonly SidebarTerminalSession[];
  conversations: readonly ConversationSummary[];
  unviewedConversationIds: ReadonlySet<string>;
  pendingApprovals: Readonly<Record<string, ToolCallUi>>;
  /** The pane that the user looks at, from the terminal activity store. */
  focusedSessionId: string | null;
  /** The chat conversation that the user reads, or null when a surface
   *  such as Settings, Dot, or a terminal tab covers the chat. */
  readingConversationId: string | null;
}): AttentionQueueRow[] {
  const t = useT();
  const windowFocused = useSyncExternalStore(
    subscribeWindowFocus,
    windowFocusedNow,
    () => false,
  );
  const ledgerRef = useRef<QueueLedger>(EMPTY_LEDGER);
  const {
    terminals,
    conversations,
    unviewedConversationIds,
    pendingApprovals,
    focusedSessionId,
    readingConversationId,
  } = input;

  return useMemo(() => {
    const conversationsById = new Map(
      conversations.map((conversation) => [conversation.id, conversation]),
    );
    const terminalsById = new Map(
      terminals.map((terminal) => [terminal.id, terminal]),
    );
    const panes: AttentionCandidate[] = [];
    for (const terminal of terminals) {
      if (!terminal.agentKind) continue;
      const candidate = paneCandidate({
        sessionId: terminal.id,
        conversationId: terminal.conversationId,
        view: terminal.pane ?? null,
      });
      if (candidate) panes.push(candidate);
    }
    // A chat entry needs a conversation that the user can open.
    const chats = chatCandidates({ unviewedConversationIds, pendingApprovals }).filter(
      (candidate) => {
        const conversation = candidate.conversationId
          ? conversationsById.get(candidate.conversationId)
          : undefined;
        return conversation !== undefined && conversation.archivedAt == null;
      },
    );
    const { ledger, items } = buildQueue({
      previous: ledgerRef.current,
      candidates: mergeCandidates(panes, chats),
      view: {
        focusedSessionId,
        viewedConversationId: windowFocused ? readingConversationId : null,
      },
      now: Date.now(),
    });
    ledgerRef.current = ledger;

    const titleOf = (conversationId: string | null, fallback: string) => {
      const conversation = conversationId
        ? conversationsById.get(conversationId)
        : undefined;
      return conversation
        ? conversationDisplayTitle(
            conversation,
            conversationNamingTitle(terminals, conversation.id),
          )
        : fallback;
    };

    return items.flatMap((item): AttentionQueueRow[] => {
      const glyph = glyphOf(item.kind);
      if (item.target.kind === "pane") {
        const terminal = terminalsById.get(item.target.sessionId);
        const view = terminal?.pane;
        if (!terminal || !terminal.agentKind || !view) return [];
        const agent = agentSessionAgentLabel(terminal.agentKind);
        const title = titleOf(item.conversationId, terminal.title);
        const text = agentPaneNotificationText(view, { agent, title }, t);
        return [
          {
            ...item,
            title,
            heading: text?.title ?? agentPaneTitle(view, t),
            detail: agentPromptSummary(view.prompt, agent, t),
            glyph,
          },
        ];
      }
      const conversation = conversationsById.get(item.target.conversationId);
      if (!conversation) return [];
      const heading =
        item.kind === "approval"
          ? t("Needs approval")
          : item.kind === "input"
            ? t("Needs input")
            : t("Unviewed response");
      const call = pendingApprovals[conversation.id];
      const detail =
        item.kind === "approval" || item.kind === "input"
          ? (call?.command.trim().slice(0, CHAT_DETAIL_MAX_CHARS) || null)
          : null;
      return [
        {
          ...item,
          title: titleOf(conversation.id, conversation.title),
          heading,
          detail,
          glyph,
        },
      ];
    });
  }, [
    conversations,
    focusedSessionId,
    pendingApprovals,
    readingConversationId,
    t,
    terminals,
    unviewedConversationIds,
    windowFocused,
  ]);
}

/**
 * The "open next in queue" chord. One window listener in the capture phase
 * owns it, so xterm never receives it as PTY input. The chord is consumed
 * also when the queue is empty. The listener ignores keys in a dialog, an
 * open palette, or an IME composition. The pinned cycle uses the same
 * pattern.
 */
export function useQueueNextShortcut(options: {
  overrides: KeyboardShortcutOverrides;
  /** A modal surface is open: the chord does nothing. */
  blocked: boolean;
  onNext: () => void;
}) {
  const optionsRef = useRef(options);
  optionsRef.current = options;
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const { overrides, blocked, onNext } = optionsRef.current;
      if (
        event.defaultPrevented ||
        event.isComposing ||
        blocked ||
        // The queue popover is a dialog too, but the chord works inside it.
        (event.target instanceof Element &&
          event.target.closest('[role="dialog"]:not([data-attention-queue-popover])'))
      ) {
        return;
      }
      if (
        !shortcutMatchesEvent(event, shortcutForAction(NEXT_IN_QUEUE_SHORTCUT, overrides))
      ) {
        return;
      }
      event.preventDefault();
      event.stopImmediatePropagation();
      onNext();
    }
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, []);
}

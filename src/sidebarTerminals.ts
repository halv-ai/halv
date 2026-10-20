import { conversationNamingTerminal } from "./conversationMode.ts";

type SidebarTerminal = {
  id: string;
  conversationId: string | null;
  customTitle: string | null;
  continuedFrom?: string | null;
};

type VisibleConversation = {
  id: string;
};

// A visible conversation already exposes its naming terminal through the
// conversation row. Keep every other terminal here, including orphaned panes,
// panes bound to archived conversations, and additional panes for one chat.
// A continuation shell and its ended source share one pane, so the pair is
// one row: the source never shows, and the shell is reachable through it.
export function standaloneSidebarTerminals<T extends SidebarTerminal>(
  terminals: readonly T[],
  visibleConversations: readonly VisibleConversation[],
): T[] {
  const reachableTerminalIds = new Set<string>();
  for (const conversation of visibleConversations) {
    const terminal = conversationNamingTerminal(terminals, conversation.id);
    if (terminal) reachableTerminalIds.add(terminal.id);
  }
  const replacedTerminalIds = new Set(
    terminals.flatMap((terminal) =>
      terminal.continuedFrom ? [terminal.continuedFrom] : [],
    ),
  );
  return terminals.filter(
    (terminal) =>
      !reachableTerminalIds.has(terminal.id) &&
      !replacedTerminalIds.has(terminal.id) &&
      !(
        terminal.continuedFrom &&
        reachableTerminalIds.has(terminal.continuedFrom)
      ),
  );
}

// A shell of an open shell tab shows in its tab (ADR 0010). It never shows
// in the Chats area of the sidebar (ADR 0020). A shell whose shell tab went
// away stays there, so it is still reachable.
export function tabbedShellSessionIds(
  terminals: ReadonlyArray<{ id: string; shellTabId?: string | null }>,
  openShellTabIds: ReadonlySet<string>,
): Set<string> {
  return new Set(
    terminals.flatMap((terminal) =>
      terminal.shellTabId && openShellTabIds.has(terminal.shellTabId) ? [terminal.id] : [],
    ),
  );
}

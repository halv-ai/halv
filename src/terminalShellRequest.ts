import { terminalPanelStorageKey } from "./conversationWorkspace.ts";
import { terminalPanelRegistry } from "./terminalPanelRegistry.ts";

// The "New shell" command reaches the terminal panel of the open
// conversation. The panel that holds the conversation scope in the
// mount registry gets the request first. Otherwise this older registry
// finds the panel. Each mounted panel registers one
// handler under its conversation id. A panel with no conversation uses "".
// Two panels can serve one conversation for a moment (the chat panel and a
// hidden agent panel). The newest panel gets the request. When it unmounts,
// the older panel gets the request again.
type TerminalShellRequestHandler = () => void;

const handlers = new Map<string, TerminalShellRequestHandler[]>();

function requestKey(conversationId: string | null) {
  return conversationId ?? "";
}

export function registerTerminalShellRequest(
  conversationId: string | null,
  handler: TerminalShellRequestHandler,
) {
  const key = requestKey(conversationId);
  handlers.set(key, [...(handlers.get(key) ?? []), handler]);
  return () => {
    const remaining = (handlers.get(key) ?? []).filter(
      (candidate) => candidate !== handler,
    );
    if (remaining.length > 0) handlers.set(key, remaining);
    else handlers.delete(key);
  };
}

// Returns false when no panel serves the conversation.
export function requestTerminalShell(conversationId: string | null) {
  const holder = conversationId
    ? terminalPanelRegistry.request(
        terminalPanelStorageKey({ conversationId, workspaceId: null, projectId: null }),
      )
    : null;
  if (holder) {
    holder.openShell();
    return true;
  }
  const handler = handlers.get(requestKey(conversationId))?.at(-1);
  if (!handler) return false;
  handler();
  return true;
}

export function conversationSendRoute(
  conversationId: string | null,
  runningConversationIds: ReadonlySet<string>,
  newThreadId: string,
) {
  return runningConversationIds.has(conversationId ?? newThreadId)
    ? "queue"
    : "send";
}

export function shouldAdoptPendingNewTurn(
  conversationId: string,
  knownConversationIds: { has(conversationId: string): boolean },
  conversationTurnStateIds: { has(conversationId: string): boolean },
  runningConversationIds: { has(conversationId: string): boolean },
): boolean {
  return (
    !knownConversationIds.has(conversationId) &&
    !conversationTurnStateIds.has(conversationId) &&
    !runningConversationIds.has(conversationId)
  );
}

type ConversationSurfaceMode =
  | "gui"
  | "terminal_embedded"
  | "terminal_ghostty";

export function conversationHandoffNotice(
  previousMode: ConversationSurfaceMode,
  nextMode: ConversationSurfaceMode,
) {
  if (previousMode === "gui" && nextMode !== "gui") {
    return "Continued in terminal — resumed session";
  }
  if (previousMode !== "gui" && nextMode === "gui") {
    return "Continued in chat — resumed session";
  }
  return null;
}

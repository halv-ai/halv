import type { StoredMessage, UiMessage } from "./types";

export function upsertStreamingAssistant(
  messages: UiMessage[],
  conversationId: string,
  delta: string,
) {
  const next = [...messages];
  const index = next.findIndex(
    (message) => message.role === "assistant" && message.streaming,
  );

  if (index >= 0) {
    next[index] = {
      ...next[index],
      conversationId,
      content: `${next[index].content}${delta}`,
    };
    return next;
  }

  next.push({
    id: `stream-${Date.now()}`,
    conversationId,
    role: "assistant",
    content: delta,
    createdAt: new Date().toISOString(),
    error: false,
    streaming: true,
  });
  return next;
}

export function mergePersistedMessagesWithStream(
  messages: StoredMessage[],
  conversationId: string,
  streamBuffer: string | undefined,
): UiMessage[] {
  if (!streamBuffer) {
    return messages;
  }

  // Storage is authoritative after it contains the assistant row.
  if (messages[messages.length - 1]?.role === "assistant") {
    return messages;
  }

  return upsertStreamingAssistant(messages, conversationId, streamBuffer);
}

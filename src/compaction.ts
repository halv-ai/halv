import type { ConversationCompaction, StoredMessage } from "./types";

export function isCompactionBoundaryMessage(
  message: Pick<StoredMessage, "sequence">,
  compaction: ConversationCompaction | null | undefined,
) {
  return (
    compaction !== null &&
    compaction !== undefined &&
    message.sequence !== undefined &&
    message.sequence === compaction.uptoMessageId
  );
}

export function completedTurnCount(
  messages: Array<
    Pick<StoredMessage, "error" | "role"> & { streaming?: boolean }
  >,
) {
  let awaitingAssistant = false;
  let completedTurns = 0;

  for (const message of messages) {
    if (message.role === "user") {
      awaitingAssistant = true;
    } else if (
      message.role === "assistant" &&
      awaitingAssistant &&
      !message.error &&
      !message.streaming
    ) {
      completedTurns += 1;
      awaitingAssistant = false;
    }
  }

  return completedTurns;
}

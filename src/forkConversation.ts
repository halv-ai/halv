import type { StoredMessage } from "./types";

type ForkPointMessage = Pick<StoredMessage, "role" | "sequence">;

export function userMessageForkSequence(
  message: ForkPointMessage,
): number | null {
  return message.role === "user" &&
    Number.isSafeInteger(message.sequence) &&
    (message.sequence ?? 0) > 0
    ? (message.sequence ?? null)
    : null;
}

export function latestUserForkSequence(
  messages: readonly ForkPointMessage[],
): number | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role === "user") {
      return userMessageForkSequence(message);
    }
  }
  return null;
}

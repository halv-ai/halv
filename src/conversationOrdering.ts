import type { ConversationSummary } from "./types";

const DAY_MS = 24 * 60 * 60 * 1_000;

export function compareConversations(
  left: ConversationSummary,
  right: ConversationSummary,
) {
  if (left.pinned !== right.pinned) {
    return left.pinned ? -1 : 1;
  }
  const updatedDifference =
    Date.parse(right.updatedAt) - Date.parse(left.updatedAt);
  if (Number.isFinite(updatedDifference) && updatedDifference !== 0) {
    return updatedDifference;
  }
  return Date.parse(right.createdAt) - Date.parse(left.createdAt);
}

export function sortConversations(conversations: ConversationSummary[]) {
  return [...conversations].sort(compareConversations);
}

export function conversationBuckets(
  conversations: ConversationSummary[],
  now = new Date(),
) {
  const today = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate(),
  ).getTime();
  const buckets = new Map<string, ConversationSummary[]>([
    ["PINNED", []],
    ["TODAY", []],
    ["YESTERDAY", []],
    ["PREVIOUS 7 DAYS", []],
    ["OLDER", []],
  ]);

  for (const conversation of sortConversations(conversations)) {
    if (conversation.pinned) {
      buckets.get("PINNED")?.push(conversation);
      continue;
    }
    const updatedAt = Date.parse(conversation.updatedAt);
    const updatedDate = new Date(updatedAt);
    const updatedDay = Number.isFinite(updatedAt)
      ? new Date(
          updatedDate.getFullYear(),
          updatedDate.getMonth(),
          updatedDate.getDate(),
        ).getTime()
      : Number.NaN;
    const age = Number.isFinite(updatedDay)
      ? Math.floor((today - updatedDay) / DAY_MS)
      : Number.POSITIVE_INFINITY;
    const label =
      age <= 0
        ? "TODAY"
        : age === 1
          ? "YESTERDAY"
          : age <= 7
            ? "PREVIOUS 7 DAYS"
            : "OLDER";
    buckets.get(label)?.push(conversation);
  }

  return [...buckets.entries()]
    .filter(([, items]) => items.length > 0)
    .map(([label, items]) => ({ label, conversations: items }));
}

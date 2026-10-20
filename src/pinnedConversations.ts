import { sortConversations } from "./conversationOrdering.ts";
import type { ConversationSummary } from "./types";

export function pinnedConversations(conversations: ConversationSummary[]) {
  return sortConversations(
    conversations.filter((conversation) => conversation.pinned && !conversation.archivedAt),
  );
}

export function nextPinnedConversation(
  conversations: ConversationSummary[],
  currentId: string | null,
  offset: -1 | 1,
) {
  const pinned = pinnedConversations(conversations);
  if (!pinned.length) return null;
  const index = pinned.findIndex((conversation) => conversation.id === currentId);
  const nextIndex = index === -1
    ? offset === 1 ? 0 : pinned.length - 1
    : (index + offset + pinned.length) % pinned.length;
  return pinned[nextIndex].id;
}

export function pinnedConversationInitial(title: string) {
  return title.match(/[\p{L}\p{N}]/u)?.[0].toLocaleUpperCase() ?? "?";
}

export function pinnedConversationColor(id: string) {
  let hash = 0;
  for (const character of id) hash = (hash * 31 + character.codePointAt(0)!) >>> 0;
  return ["peach", "sage", "lavender", "sand", "blue", "rose"][hash % 6];
}

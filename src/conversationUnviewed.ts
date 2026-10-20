export function markConversationUnviewed(
  current: ReadonlySet<string>,
  conversationId: string,
  selectedConversationId: string | null,
) {
  const next = new Set(current);
  if (conversationId === selectedConversationId) {
    next.delete(conversationId);
  } else {
    next.add(conversationId);
  }
  return next;
}

export function clearConversationUnviewed(
  current: ReadonlySet<string>,
  conversationId: string,
) {
  const next = new Set(current);
  next.delete(conversationId);
  return next;
}

export function migrateConversationUnviewed(
  current: ReadonlySet<string>,
  fromConversationId: string,
  toConversationId: string,
) {
  const next = new Set(current);
  if (!next.delete(fromConversationId)) {
    return next;
  }
  next.add(toConversationId);
  return next;
}

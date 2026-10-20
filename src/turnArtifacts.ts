export function artifactBelongsToUserTurn(
  artifactTurnId: string,
  artifactCreatedAt: string,
  messageId: string,
  messageCreatedAt: string,
  latestUserMessageId: string | null,
  streaming: boolean,
  knownUserMessageIds: ReadonlySet<string>,
) {
  const artifactTime = Date.parse(artifactCreatedAt);
  const messageTime = Date.parse(messageCreatedAt);
  const belongsToLiveOptimisticTurn =
    messageId === latestUserMessageId &&
    streaming &&
    !knownUserMessageIds.has(artifactTurnId) &&
    Number.isFinite(artifactTime) &&
    Number.isFinite(messageTime) &&
    artifactTime >= messageTime - 1_000;
  return (
    artifactTurnId === messageId || belongsToLiveOptimisticTurn
  );
}

export function shouldShowTaskPlanForUserTurn(
  messageId: string,
  latestUserMessageId: string | null,
  streaming: boolean,
  hasTaskPlan: boolean,
) {
  return messageId === latestUserMessageId && streaming && hasTaskPlan;
}

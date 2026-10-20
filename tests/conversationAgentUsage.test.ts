import assert from "node:assert/strict";
import test from "node:test";

import {
  conversationAgentTurns,
  conversationAgentUsageTotals,
} from "../src/conversationAgentUsage.ts";
import type { UiMessage } from "../src/types.ts";

test("maps Claude transcript usage into session meter turns", () => {
  const messages: UiMessage[] = [
    {
      id: "assistant-1",
      conversationId: "conversation-1",
      role: "assistant",
      content: "ok",
      createdAt: "2026-08-16T12:00:00Z",
      inputTokens: 100,
      outputTokens: 2,
      error: false,
      tokensSaved: 40,
      usage: {
        inputTokens: 100,
        outputTokens: 2,
        totalTokens: 102,
        cachedTokens: 30,
        reasoningTokens: 0,
      },
    },
  ];
  const turns = conversationAgentTurns(messages);
  assert.deepEqual(turns, [
    {
      inputTokens: 100,
      outputTokens: 2,
      cacheReadTokens: 30,
      cacheCreationTokens: 0,
      savingsTokens: 40,
      savingsSource: "measured",
    },
  ]);
  assert.equal(conversationAgentUsageTotals(turns).savingsTokens, 40);
});

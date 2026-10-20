import assert from "node:assert/strict";
import test from "node:test";

import { replySavings } from "../src/messageSavings.ts";
import type { TokenUsage, UiMessage } from "../src/types.ts";

const usage: TokenUsage = {
  inputTokens: 120,
  outputTokens: 30,
  totalTokens: 150,
  cachedTokens: 45,
  reasoningTokens: 0,
};

function assistant(overrides: Partial<UiMessage> = {}): UiMessage {
  return {
    id: "assistant-1",
    conversationId: "conversation-1",
    role: "assistant",
    content: "Done",
    createdAt: "2026-08-17T00:00:00Z",
    error: false,
    ...overrides,
  };
}

test("a saving with a dollar method leads with dollars", () => {
  assert.deepEqual(
    replySavings(
      assistant({
        inputTokens: 120,
        tokensSaved: 80,
        savingsUsd: 0.04,
        savingsSource: "measured",
        pricingSource: "model_table",
        usdMethod: "effective_rate",
      }),
      usage,
      true,
    ),
    { kind: "dollars", amountUsd: 0.04, tokens: 80 },
  );
});

test("an older saving shows the compression in tokens", () => {
  assert.deepEqual(
    replySavings(
      assistant({
        inputTokens: 120,
        tokensSaved: 80,
        savingsUsd: 0.04,
        savingsSource: "measured",
        pricingSource: "headroom",
      }),
      usage,
      true,
    ),
    {
      kind: "compressed",
      originalInputTokens: 200,
      compressedInputTokens: 120,
      percent: 40,
    },
  );
});

test("maps measure-only session usage to an always-visible reply line", () => {
  assert.deepEqual(
    replySavings(assistant({ savingsSource: "usage" }), usage, true),
    { kind: "usage", input: 120, output: 30, cached: 45 },
  );
  assert.equal(replySavings(assistant(), usage, false), null);
});

test("keeps a measured zero distinct from a measure-only reply", () => {
  assert.deepEqual(
    replySavings(
      assistant({
        savingsSource: "measured",
        tokensSaved: 0,
        savingsUsd: 0,
      }),
      usage,
      true,
    ),
    { kind: "tokens", label: "saved", tokens: 0 },
  );
});

test("uses RTK and Crux labels when compression is absent", () => {
  assert.deepEqual(
    replySavings(
      assistant({
        tokensSaved: 50,
        savingsUsd: 0.01,
        pricingSource: "model_table",
        rtkTokensSaved: 50,
        rtkIncludedInTotal: true,
      }),
      usage,
      true,
    ),
    { kind: "tokens", label: "RTK", tokens: 50 },
  );
  assert.deepEqual(
    replySavings(
      assistant({
        tokensSaved: 30,
        savingsUsd: 0.01,
        pricingSource: "model_table",
        cruxTokensSaved: 30,
        cruxIncludedInTotal: true,
      }),
      usage,
      true,
    ),
    { kind: "tokens", label: "crux (est.)", tokens: 30 },
  );
});

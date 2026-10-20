import assert from "node:assert/strict";
import test from "node:test";

import {
  getUsageSectionVisibility,
  summarizeSavingsByLane,
  summarizeUsageByLane,
} from "../src/usageByLane.ts";
import type { LaneUsage } from "../src/types.ts";

test("groups lane usage by billing and provider", () => {
  const usage: LaneUsage[] = [
    {
      lane: "codex_chatgpt",
      billing: "plan",
      provider: "openai",
      requests: 4,
      inputTokens: 400,
      outputTokens: null,
      costUsd: null,
      tokensSaved: 400,
    },
    {
      lane: "claude_subscription",
      billing: "plan",
      provider: "claude",
      requests: 3,
      inputTokens: 300,
      outputTokens: 30,
      costUsd: null,
      tokensSaved: 300,
    },
    {
      lane: "claude_api_key",
      billing: "api",
      provider: "claude",
      requests: 2,
      inputTokens: 200,
      outputTokens: null,
      costUsd: 1.25,
      tokensSaved: 200,
    },
  ];

  assert.deepEqual(summarizeUsageByLane(usage), {
    plan: {
      providers: [
        { id: "openai", label: "OpenAI", requests: 4 },
        { id: "claude", label: "Claude", requests: 3 },
      ],
      inputTokens: 700,
      outputTokens: 30,
      costUsd: 0,
    },
    api: {
      providers: [{ id: "claude", label: "Claude", requests: 2 }],
      inputTokens: 200,
      outputTokens: null,
      costUsd: 1.25,
    },
  });
});

test("combines lanes for the same billing provider", () => {
  const usage: LaneUsage[] = [
    {
      lane: "custom-one",
      billing: "api",
      provider: "custom",
      requests: 2,
      inputTokens: 100,
      outputTokens: 10,
      costUsd: 0.4,
      tokensSaved: 80,
    },
    {
      lane: "custom-two",
      billing: "api",
      provider: "custom",
      requests: 5,
      inputTokens: 250,
      outputTokens: 20,
      costUsd: 0.6,
      tokensSaved: 120,
    },
  ];

  assert.deepEqual(summarizeUsageByLane(usage).api, {
    providers: [{ id: "custom", label: "Custom", requests: 7 }],
    inputTokens: 350,
    outputTokens: 30,
    costUsd: 1,
  });
});

test("ignores unattributed rows in billing summaries", () => {
  const usage: LaneUsage[] = [
    {
      lane: "unknown",
      billing: "unknown",
      provider: "custom",
      requests: 8,
      inputTokens: 900,
      outputTokens: null,
      costUsd: null,
      tokensSaved: 50,
    },
  ];

  assert.deepEqual(summarizeUsageByLane(usage), {
    plan: {
      providers: [],
      inputTokens: 0,
      outputTokens: null,
      costUsd: 0,
    },
    api: {
      providers: [],
      inputTokens: 0,
      outputTokens: null,
      costUsd: 0,
    },
  });
});

test("shows API usage only for a configured API key", () => {
  assert.deepEqual(getUsageSectionVisibility(true, 0), {
    showApi: true,
    showUnattributedFallback: false,
  });
  assert.deepEqual(getUsageSectionVisibility(false, 0), {
    showApi: false,
    showUnattributedFallback: false,
  });
});

test("shows unattributed usage without a configured API key", () => {
  assert.deepEqual(getUsageSectionVisibility(false, 3), {
    showApi: false,
    showUnattributedFallback: true,
  });
  assert.deepEqual(getUsageSectionVisibility(true, 3), {
    showApi: true,
    showUnattributedFallback: false,
  });
});

test("summarizes savings by lane in descending token order", () => {
  const usage: LaneUsage[] = [
    {
      lane: "claude_subscription",
      billing: "plan",
      provider: "claude",
      requests: 3,
      inputTokens: 300,
      outputTokens: null,
      costUsd: null,
      tokensSaved: 250,
    },
    {
      lane: "codex_chatgpt",
      billing: "plan",
      provider: "openai",
      requests: 7,
      inputTokens: 700,
      outputTokens: null,
      costUsd: null,
      tokensSaved: 750,
    },
  ];

  assert.deepEqual(summarizeSavingsByLane(usage), [
    {
      lane: "codex_chatgpt",
      label: "Codex",
      requests: 7,
      tokensSaved: 750,
      percent: 75,
    },
    {
      lane: "claude_subscription",
      label: "Claude Code",
      requests: 3,
      tokensSaved: 250,
      percent: 25,
    },
  ]);
});

test("summarizes empty and zero-token lane savings", () => {
  assert.deepEqual(summarizeSavingsByLane([]), []);
  assert.deepEqual(
    summarizeSavingsByLane([
      {
        lane: "gemini_cli",
        billing: "plan",
        provider: "gemini",
        requests: 2,
        inputTokens: 100,
        outputTokens: null,
        costUsd: null,
        tokensSaved: 0,
      },
    ]),
    [
      {
        lane: "gemini_cli",
        label: "Gemini CLI",
        requests: 2,
        tokensSaved: 0,
        percent: 0,
      },
    ],
  );
});

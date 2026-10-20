import assert from "node:assert/strict";
import test from "node:test";
import {
  FIRST_RESULT_STORAGE_KEY,
  lastTurnSavingsPercent,
  readFirstResultSeen,
  trialGoalProgress,
  trialPromptFacts,
  valueDelivered,
  writeFirstResultSeen,
} from "../src/activation.ts";
import type { UiMessage } from "../src/types.ts";

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
}

test("value is delivered by a first result, a guided completion, or a saving", () => {
  const nothing = {
    firstResultSeen: false,
    onboardingCompleted: false,
    tokensSaved: 0,
  };
  assert.equal(valueDelivered(nothing), false);
  assert.equal(valueDelivered({ ...nothing, firstResultSeen: true }), true);
  assert.equal(valueDelivered({ ...nothing, onboardingCompleted: true }), true);
  assert.equal(valueDelivered({ ...nothing, tokensSaved: 1 }), true);
});

test("the first result persists across launches", () => {
  const storage = memoryStorage();
  assert.equal(readFirstResultSeen(storage), false);
  writeFirstResultSeen(storage);
  assert.equal(storage.getItem(FIRST_RESULT_STORAGE_KEY), "1");
  assert.equal(readFirstResultSeen(storage), true);
});

test("the last turn savings percent is positive when context was removed", () => {
  const messages = [
    { role: "user", content: "hi" },
    {
      role: "assistant",
      content: "done",
      inputTokens: 600,
      tokensSaved: 400,
    },
  ] as unknown as UiMessage[];
  assert.equal(lastTurnSavingsPercent(messages), 40);
});

test("the last turn savings percent is null without a measurement", () => {
  const messages = [
    { role: "assistant", content: "done", tokensSaved: null },
  ] as unknown as UiMessage[];
  assert.equal(lastTurnSavingsPercent(messages), null);
  assert.equal(lastTurnSavingsPercent([]), null);
});

test("trial card facts lead with dollars, then tokens, then conversations", () => {
  assert.deepEqual(
    trialPromptFacts({ conversations: 3, tokensSaved: 9_000, dollarsUsd: 0.42 }),
    { kind: "dollars", amountUsd: 0.42, conversations: 3 },
  );
  assert.deepEqual(
    trialPromptFacts({ conversations: 3, tokensSaved: 9_000, dollarsUsd: 0 }),
    { kind: "tokens", tokens: 9_000, conversations: 3 },
  );
  assert.deepEqual(
    trialPromptFacts({ conversations: 2, tokensSaved: 0, dollarsUsd: 0 }),
    { kind: "conversations", conversations: 2 },
  );
  assert.deepEqual(
    trialPromptFacts({ conversations: 0, tokensSaved: 0, dollarsUsd: 0 }),
    { kind: "none" },
  );
});

test("the trial goal counts active days inside the trial window", () => {
  const trialEndMs = Date.parse("2026-10-23T12:00:00Z");
  const progress = trialGoalProgress({
    trialEndMs,
    nowMs: Date.parse("2026-10-18T09:00:00Z"),
    activeDayKeys: ["2026-10-15", "2026-10-16", "2026-10-16", "2026-10-18"],
  });
  assert.equal(progress.day, 2);
  assert.equal(progress.totalDays, 7);
  assert.equal(progress.daysLeft, 6);
  // 10-15 is before the trial; 10-16 counts once.
  assert.equal(progress.activeDays, 2);
  assert.equal(progress.targetDays, 5);
  assert.equal(progress.reached, false);
  assert.equal(progress.ended, false);
});

test("the trial goal reports a reached goal and an ended trial", () => {
  const trialEndMs = Date.parse("2026-10-23T12:00:00Z");
  const reached = trialGoalProgress({
    trialEndMs,
    nowMs: Date.parse("2026-10-22T12:00:00Z"),
    activeDayKeys: ["2026-10-17", "2026-10-18", "2026-10-19", "2026-10-20", "2026-10-21"],
  });
  assert.equal(reached.day, 7);
  assert.equal(reached.reached, true);
  const ended = trialGoalProgress({
    trialEndMs,
    nowMs: Date.parse("2026-10-24T00:00:00Z"),
    activeDayKeys: [],
  });
  assert.equal(ended.ended, true);
  assert.equal(ended.daysLeft, 0);
  assert.equal(ended.day, 7);
});

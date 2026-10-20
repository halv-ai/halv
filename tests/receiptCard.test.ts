import assert from "node:assert/strict";
import test from "node:test";
import {
  buildReceiptData,
  formatReceiptPeriod,
  formatReceiptSummary,
  formatReceiptTokens,
  formatReceiptUsd,
  getReceiptCardLayout,
} from "../src/receiptCard.ts";
import type {
  AccountAuthStatus,
  DailyTokenActivity,
  LaneUsage,
  SavingsTotals,
} from "../src/types.ts";

test("receipt token formatting stays compact", () => {
  assert.equal(formatReceiptTokens(482_000), "482K");
  assert.equal(formatReceiptTokens(1_200_000), "1.2M");
  assert.equal(formatReceiptTokens(Number.NaN), "0");
});

test("receipt footer uses the content left padding", () => {
  const layout = getReceiptCardLayout();

  assert.equal(layout.footer.x, layout.content.left);
  assert.deepEqual(layout.card, { width: 1080, height: 1080, radius: 28 });
});

test("receipt period formatting uses the requested short form", () => {
  assert.equal(formatReceiptPeriod("2026-08-25", "2026-08-31"), "AUG 25 \u2013 AUG 31");
});

test("receipt data covers seven local days and summarizes agent savings", () => {
  const activity: DailyTokenActivity[] = [
    day("2026-08-25", 100, 400),
    day("2026-08-27", 300, 600),
    day("2026-08-31", 600, 2_000),
  ];
  const lanes: LaneUsage[] = [
    lane("codex_chatgpt", "openai", 700),
    lane("claude_subscription", "anthropic", 300),
    lane("codex_chatgpt", "openai", 200),
  ];
  const totals = {
    tokensSaved: 10_000,
    savingsUsd: 20,
  } as SavingsTotals;
  const account = {
    signedIn: true,
    referral: {
      code: "K7PQ2M4X",
      link: "https://halv.ai/i/K7PQ2M4X",
      trialDays: 14,
      invited: 3,
      rewarded: 1,
      claimedCode: null,
    },
  } as AccountAuthStatus;

  const receipt = buildReceiptData({
    dailyTokenActivity: activity,
    usageByLane: lanes,
    weeklyTokensSaved: 1_000,
    totals,
    account,
    now: new Date(2026, 7, 31, 23, 30),
  });

  assert.equal(receipt.periodStart, "2026-08-25");
  assert.equal(receipt.periodEnd, "2026-08-31");
  assert.deepEqual(receipt.daily, [100, 0, 300, 0, 0, 0, 600]);
  assert.equal(receipt.percentSaved, 25);
  // No dollars in the period: the receipt does not pro-rate all-time dollars.
  assert.equal(receipt.savingsUsd, null);
  assert.equal(receipt.inviteCode, "K7PQ2M4X");
  assert.deepEqual(receipt.agents, [
    { label: "Codex", tokensSaved: 900, percent: 75 },
    { label: "Claude Code", tokensSaved: 300, percent: 25 },
  ]);
  assert.equal(
    formatReceiptSummary(receipt),
    "Cached once, reused all week. 25% of my agent traffic ran through halv \u2014 Codex 75%, Claude Code 25%.",
  );
});

test("receipt leads with the period's own dollars", () => {
  const activity: DailyTokenActivity[] = [
    { ...day("2026-08-30", 400, 400), dollarsUsd: 0.25 },
    { ...day("2026-08-31", 600, 2_000), dollarsUsd: 1.0 },
  ];
  const receipt = buildReceiptData({
    dailyTokenActivity: activity,
    usageByLane: [],
    weeklyTokensSaved: 1_000,
    weeklyDollarsUsd: 1.25,
    totals: { tokensSaved: 50_000, savingsUsd: 900 } as SavingsTotals,
    account: null,
    now: new Date(2026, 7, 31, 23, 30),
  });

  assert.equal(receipt.savingsUsd, 1.25);
  assert.deepEqual(receipt.dailyUsd, [0, 0, 0, 0, 0, 0.25, 1]);
  assert.equal(formatReceiptUsd(receipt.savingsUsd ?? 0), "$1.25");
  assert.ok(
    formatReceiptSummary(receipt).startsWith(
      "1K tokens kept out of context, at API prices. ",
    ),
  );
});

function day(
  date: string,
  tokensSaved: number,
  inputTokens: number,
): DailyTokenActivity {
  return { date, inputTokens, outputTokens: 0, tokensSaved };
}

function lane(
  laneName: LaneUsage["lane"],
  provider: LaneUsage["provider"],
  tokensSaved: number,
): LaneUsage {
  return {
    lane: laneName,
    provider,
    billing: "plan",
    requests: 1,
    inputTokens: 0,
    outputTokens: 0,
    costUsd: 0,
    tokensSaved,
  };
}

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { calculateInputSavings } from "../src/savings.ts";

test("calculates savings from original input tokens only", () => {
  const savings = calculateInputSavings(16_239, 1_556);

  assert.deepEqual(savings, {
    compressedInputTokens: 16_239,
    originalInputTokens: 17_795,
    percent: (1_556 / 17_795) * 100,
    savedTokens: 1_556,
  });
  assert.equal(Math.round(savings!.percent), 9);
});

test("does not fold output tokens into input savings", () => {
  const savings = calculateInputSavings(16_239, 1_556);
  const outputTokens = 3_619;

  assert.equal(Math.round(savings!.percent), 9);
  assert.equal(
    Math.round((1_556 / (16_239 + outputTokens + 1_556)) * 100),
    7,
  );
});

test("normalizes negative savings and rejects invalid input", () => {
  assert.deepEqual(calculateInputSavings(100, -5), {
    compressedInputTokens: 100,
    originalInputTokens: 100,
    percent: 0,
    savedTokens: 0,
  });
  assert.equal(calculateInputSavings(null, 10), null);
  assert.equal(calculateInputSavings(-1, 10), null);
  assert.equal(calculateInputSavings(0, 0), null);
  assert.equal(calculateInputSavings(Number.NaN, 10), null);
});

test("labels rolling weekly savings as seven days", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const panel = readFileSync(
    new URL("../src/components/SavingsPanel.tsx", import.meta.url),
    "utf8",
  );
  const statusBar = readFileSync(
    new URL("../src/components/StatusBar.tsx", import.meta.url),
    "utf8",
  );

  assert.match(app, /ipc\.storage\.savingsWeek\(\)/);
  assert.doesNotMatch(app, /↓ \{tokens\} last 7 days/);
  assert.match(panel, /last 7 days/);
  assert.match(statusBar, /t\("7d"\)/);
  assert.doesNotMatch(app, /this week/i);
  assert.doesNotMatch(panel, /this week/i);
});

test("removes the dead browser weekly savings cache", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const savings = readFileSync(
    new URL("../src/savings.ts", import.meta.url),
    "utf8",
  );

  assert.doesNotMatch(app, /weekSavings|recordDailySavings/);
  assert.doesNotMatch(savings, /DAILY_SAVINGS_HISTORY|TrailingWeekSavings/);
});

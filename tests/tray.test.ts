import assert from "node:assert/strict";
import { test } from "node:test";
import { formatTrayPercent, formatTrayReset, formatTrayTokens, remainingQuota, trayIsFresh, trayWindows, type TraySnapshot, type TrayWindow } from "../src/tray/model.ts";

const now = Date.parse("2026-09-29T18:00:00Z");
const quota: TrayWindow = { provider: "Codex", label: "5h", used: 32, reset: new Date(now + 3600_000).toISOString() };
const snapshot: TraySnapshot = { today: 0, total: 1_000_000, windows: [quota], checked_at: new Date(now).toISOString() };

test("tray capacity expires with its reading and reset", () => {
  assert.equal(trayIsFresh(snapshot, now + 119_999), true);
  assert.equal(trayIsFresh(snapshot, now + 120_000), false);
  assert.equal(trayIsFresh({ ...snapshot, checked_at: null }, now), false);
  assert.equal(trayIsFresh({ ...snapshot, checked_at: "bad" }, now), false);
  assert.equal(remainingQuota(quota, true, now), 68);
  assert.equal(remainingQuota(quota, false, now), null);
  assert.equal(remainingQuota(quota, true, now + 3600_000), null);
  for (const used of [null, Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.equal(remainingQuota({ ...quota, used }, true, now), null);
  }
  assert.equal(remainingQuota({ ...quota, reset: "bad" }, true, now), null);
});

test("tray distinguishes exhausted, fractional, and unavailable quotas", () => {
  assert.equal(remainingQuota({ ...quota, used: 110 }, true, now), 0);
  assert.equal(remainingQuota({ ...quota, used: -10 }, true, now), 100);
  assert.equal(formatTrayPercent(0.2), "<1");
  assert.equal(formatTrayPercent(99.9), "99");
  assert.equal(formatTrayPercent(0), "0");
  assert.equal(formatTrayTokens(0, "en-US"), "0");
  assert.equal(formatTrayTokens(null, "en-US"), "—");
});

test("provider windows stay separate and preserve their actual labels", () => {
  const windows = [
    { ...quota, provider: "Claude" as const, label: "7d", used: 61 },
    { ...quota, label: "24h", used: 5 },
    { ...quota, label: "30d", used: 70 },
  ];
  const sample = { ...snapshot, windows };
  assert.equal(trayWindows(sample, "Claude")[0].used, null);
  assert.equal(trayWindows(sample, "Claude")[1].used, 61);
  assert.deepEqual(trayWindows(sample, "Codex").map(w => w.label), ["24h", "30d"]);
  const weeklyOnly = trayWindows({ ...snapshot, windows: [{ ...quota, label: "7d" }] }, "Codex");
  assert.deepEqual(weeklyOnly.map(window => window.label), ["5h", "7d"]);
  assert.equal(weeklyOnly[0].used, null);
  assert.equal(weeklyOnly[1].used, 32);
  assert.equal(formatTrayReset(new Date(now + 1).toISOString(), now), "1m");
  assert.equal(formatTrayReset(new Date(now + 25 * 3600_000).toISOString(), now), "1d 1h");
});

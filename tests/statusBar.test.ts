import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import type { WalletAccountUsage } from "../src/accountWallet.ts";
import type { ProviderPlanWindowGroup } from "../src/providerPlanWindows.ts";
import {
  canRefreshUsage,
  findWalletUsage,
  formatStatusCountdown,
  popoverLeft,
  providerLetter,
  statusBarDensity,
  statusBarProviderGroups,
  statusProviderUsage,
  usageTone,
  walletUsageWindows,
  windowLengthLabel,
} from "../src/statusBar.ts";

const NOW_MS = Date.parse("2026-08-13T15:00:00.000Z");
const NOW_SECONDS = NOW_MS / 1000;
const MINUTE = 60;
const labels = { week: "wk" };

function group(
  id: ProviderPlanWindowGroup["id"],
  windows: ProviderPlanWindowGroup["windows"],
): ProviderPlanWindowGroup {
  return { id, label: id, windows, unavailableReason: null };
}

test("density steps down at 900 and 500 pixels", () => {
  assert.equal(statusBarDensity(1280), "full");
  assert.equal(statusBarDensity(900), "full");
  assert.equal(statusBarDensity(899), "compact");
  assert.equal(statusBarDensity(500), "compact");
  assert.equal(statusBarDensity(499), "minimal");
});

test("usage tone is neutral below 60, warning below 80, then danger", () => {
  assert.equal(usageTone(0), "normal");
  assert.equal(usageTone(59), "normal");
  assert.equal(usageTone(60), "warning");
  assert.equal(usageTone(79), "warning");
  assert.equal(usageTone(80), "danger");
  assert.equal(usageTone(100), "danger");
});

test("countdowns use minutes, hours and minutes, then days and hours", () => {
  assert.equal(formatStatusCountdown(NOW_MS + 12 * 60_000, NOW_MS), "12m");
  assert.equal(formatStatusCountdown(NOW_MS + 234 * 60_000, NOW_MS), "3h 54m");
  assert.equal(formatStatusCountdown(NOW_MS + 9_060 * 60_000, NOW_MS), "6d 7h");
  assert.equal(formatStatusCountdown(NOW_MS - 60_000, NOW_MS), "0m");
});

test("weekly windows use the week label when the reset is unknown", () => {
  assert.equal(windowLengthLabel("7d", labels), "wk");
  assert.equal(windowLengthLabel("5h", labels), "5h");
});

test("provider usage lists known windows and finds the tightest", () => {
  const usage = statusProviderUsage(
    group("claude", [
      { label: "5h", remainingPercent: 87, resetsAt: NOW_SECONDS + 234 * MINUTE },
      { label: "7d", remainingPercent: 59, resetsAt: NOW_SECONDS + 9_060 * MINUTE },
    ]),
    NOW_MS,
    labels,
  );

  assert.deepEqual(usage.windows, [
    { key: "5h", usedPercent: 13, resetLabel: "3h 54m", tone: "normal" },
    { key: "7d", usedPercent: 41, resetLabel: "6d 7h", tone: "normal" },
  ]);
  assert.equal(usage.tightest?.key, "7d");
});

test("provider usage drops windows without data and labels unknown resets", () => {
  const usage = statusProviderUsage(
    group("codex", [
      { label: "5h", remainingPercent: null, resetsAt: null },
      { label: "7d", remainingPercent: 15, resetsAt: null },
    ]),
    NOW_MS,
    labels,
  );

  assert.deepEqual(usage.windows, [
    { key: "7d", usedPercent: 85, resetLabel: "wk", tone: "danger" },
  ]);
  assert.equal(usage.tightest?.tone, "danger");

  const empty = statusProviderUsage(
    group("codex", [{ label: "", remainingPercent: null, resetsAt: null }]),
    NOW_MS,
    labels,
  );
  assert.deepEqual(empty.windows, []);
  assert.equal(empty.tightest, null);
});

test("wallet providers always get a pill and others only with data", () => {
  const groups = [
    group("claude", [{ label: "5h", remainingPercent: null, resetsAt: null }]),
    group("codex", [{ label: "", remainingPercent: null, resetsAt: null }]),
    group("kimi", [{ label: "", remainingPercent: null, resetsAt: null }]),
    group("glm", [
      { label: "requests", remainingPercent: 40, resetsAt: NOW_SECONDS + MINUTE },
    ]),
  ];

  assert.deepEqual(
    statusBarProviderGroups(groups).map((entry) => entry.id),
    ["claude", "codex", "glm"],
  );
});

test("letters match the sidebar terminal badges", () => {
  assert.equal(providerLetter("claude"), "C");
  assert.equal(providerLetter("codex"), "X");
  assert.equal(providerLetter("grok"), "Gr");
});

test("wallet usage orders windows and skips windows that already reset", () => {
  const usage: WalletAccountUsage = {
    provider: "claude",
    accountId: "work",
    windows: [
      { kind: "fableWeekly", usedPercent: 35, resetsAt: NOW_MS + 60_000 },
      { kind: "weekly", usedPercent: 92.4, resetsAt: NOW_MS + 60_000 },
      { kind: "session", usedPercent: 64, resetsAt: NOW_MS + 41 * 60_000 },
      { kind: "session", usedPercent: 5, resetsAt: NOW_MS - 60_000 },
    ],
    updatedAt: NOW_MS,
  };

  assert.deepEqual(
    walletUsageWindows(usage, NOW_MS, { week: "wk", fable: "Fable" }),
    [
      { key: "session", usedPercent: 64, resetLabel: "41m", tone: "warning" },
      { key: "weekly", usedPercent: 92, resetLabel: "wk", tone: "danger" },
      { key: "fableWeekly", usedPercent: 35, resetLabel: "Fable", tone: "normal" },
    ],
  );
  assert.deepEqual(walletUsageWindows(undefined, NOW_MS, { week: "wk", fable: "Fable" }), []);
});

test("wallet usage shows a window that already reset as empty instead of hiding it", () => {
  // Orca cached this inactive account hours ago: the 5h window reset
  // since, the weekly windows did not.
  const usage: WalletAccountUsage = {
    provider: "claude",
    accountId: "work-2",
    windows: [
      { kind: "session", usedPercent: 71, resetsAt: NOW_MS - 3 * 3_600_000 },
      { kind: "weekly", usedPercent: 18, resetsAt: NOW_MS + 2 * 86_400_000 },
      { kind: "fableWeekly", usedPercent: 44, resetsAt: NOW_MS + 86_400_000 },
    ],
    updatedAt: NOW_MS - 4 * 3_600_000,
  };

  assert.deepEqual(
    walletUsageWindows(usage, NOW_MS, { week: "wk", fable: "Fable" }),
    [
      { key: "session", usedPercent: 0, resetLabel: "5h", tone: "normal" },
      { key: "weekly", usedPercent: 18, resetLabel: "wk", tone: "normal" },
      { key: "fableWeekly", usedPercent: 44, resetLabel: "Fable", tone: "normal" },
    ],
  );
});

test("wallet usage lookup matches provider and account", () => {
  const entries: WalletAccountUsage[] = [
    { provider: "codex", accountId: "a", windows: [], updatedAt: NOW_MS },
    { provider: "claude", accountId: "a", windows: [], updatedAt: NOW_MS },
  ];

  assert.equal(findWalletUsage(entries, "claude", "a"), entries[1]);
  assert.equal(findWalletUsage(entries, "claude", "b"), undefined);
});

test("manual refresh runs at most once per 30 seconds", () => {
  assert.equal(canRefreshUsage(null, NOW_MS), true);
  assert.equal(canRefreshUsage(NOW_MS - 29_999, NOW_MS), false);
  assert.equal(canRefreshUsage(NOW_MS - 30_000, NOW_MS), true);
});

test("the popover starts at its pill and stays on screen", () => {
  assert.equal(popoverLeft(120, 1280), 120);
  assert.equal(popoverLeft(2, 1280), 8);
  assert.equal(popoverLeft(1100, 1280), 912);
  assert.equal(popoverLeft(100, 300), 8);
});

test("the app mounts the status line below the body and keeps it uncovered", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const sidebar = readFileSync(
    new URL("../src/components/ThreadSidebar.tsx", import.meta.url),
    "utf8",
  );
  const statusBar = readFileSync(
    new URL("../src/components/StatusBar.tsx", import.meta.url),
    "utf8",
  );

  assert.ok(app.indexOf("<StatusBar") > app.indexOf("<ThreadSidebar"));
  assert.ok(app.indexOf("<StatusBar") < app.indexOf("<SettingsPanel"));
  // The drawer stops at the top edge of the status line.
  assert.match(app, /fixed inset-x-0 bottom-\[max\(1\.5rem,24px\)\] top-9 z-50/);
  assert.doesNotMatch(sidebar, /SavingsStrip/);
  assert.match(statusBar, /flex h-6 min-h-\[24px\]/);
  assert.match(statusBar, /aria-haspopup="dialog"/);
  assert.match(statusBar, /aria-expanded=\{open\}/);
  assert.match(statusBar, /ipc\.wallet\s*\.usage\(\)/);
});

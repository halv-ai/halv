import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  AUTO_ROTATION_RETRY_MS,
  AUTO_ROTATION_STORAGE_KEY,
  AUTO_ROTATION_UNKNOWN_RESET_MS,
  DEFAULT_AUTO_ROTATION_SETTINGS,
  INITIAL_AUTO_ROTATION_WATCH,
  assistedSwapInput,
  autoRotationTarget,
  claudePanesBusy,
  claudeStaleSessionIds,
  claudeWalletPaneIds,
  createAutoRotationSetting,
  finishAutoRotation,
  nextAutoRotationStep,
  normalizeAutoRotationSettings,
  rememberRotatedAway,
  rotatedAwayIds,
  rotatedAwayUntil,
  type AutoRotationWatch,
} from "../src/accountAutoRotation.ts";
// @ts-expect-error Node's test runner needs the source extension.
import type {
  WalletAccount,
  WalletAccountUsage,
  WalletProviderState,
} from "../src/accountWallet.ts";

const NOW = Date.parse("2026-10-15T12:00:00Z");
const MINUTE = 60_000;
const SETTINGS = { startPercent: 92, latestPercent: 99 };

function account(id: string, extra: Partial<WalletAccount> = {}): WalletAccount {
  return {
    id,
    email: `${id}@example.com`,
    organizationName: null,
    plan: "max",
    missing: false,
    createdAt: "2026-10-01T00:00:00Z",
    ...extra,
  };
}

function usage(accountId: string, sessionUsed: number, resetMinutes = 60): WalletAccountUsage {
  return {
    provider: "claude",
    accountId,
    windows: [{ kind: "session", usedPercent: sessionUsed, resetsAt: NOW + resetMinutes * MINUTE }],
    updatedAt: NOW,
  };
}

function state(ids: string[], activeId = "active"): WalletProviderState {
  return {
    activeId,
    systemDefault: { email: null, plan: null },
    accounts: [account("active"), ...ids.map((id) => account(id))],
  };
}

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    values,
    storage: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => void values.set(key, value),
    },
  };
}

test("the setting is off by default and clamps bad input", () => {
  assert.deepEqual(DEFAULT_AUTO_ROTATION_SETTINGS, {
    enabled: false,
    startPercent: 92,
    latestPercent: 99,
  });
  assert.deepEqual(normalizeAutoRotationSettings(null), DEFAULT_AUTO_ROTATION_SETTINGS);
  assert.deepEqual(
    normalizeAutoRotationSettings({ enabled: true, startPercent: 20, latestPercent: 140 }),
    { enabled: true, startPercent: 50, latestPercent: 100 },
  );
  // The latest percent never goes below the start percent.
  assert.deepEqual(
    normalizeAutoRotationSettings({ enabled: true, startPercent: 95, latestPercent: 90 }),
    { enabled: true, startPercent: 95, latestPercent: 95 },
  );
  assert.deepEqual(
    normalizeAutoRotationSettings({ enabled: "yes", startPercent: 93.6, latestPercent: "98" }),
    { enabled: false, startPercent: 94, latestPercent: 99 },
  );
  // A value that is not a number keeps the fallback, never NaN.
  const fallback = { enabled: true, startPercent: 80, latestPercent: 90 };
  assert.deepEqual(
    normalizeAutoRotationSettings(
      { enabled: true, startPercent: Number.NaN, latestPercent: Number.POSITIVE_INFINITY },
      fallback,
    ),
    fallback,
  );
});

test("the setting persists one JSON value, and a throwing storage keeps it off", () => {
  const { values, storage } = memoryStorage();
  const setting = createAutoRotationSetting(() => storage);
  let changes = 0;
  setting.subscribe(() => {
    changes += 1;
  });
  assert.equal(setting.getSnapshot(), setting.getSnapshot());
  assert.deepEqual(setting.getSnapshot(), DEFAULT_AUTO_ROTATION_SETTINGS);
  setting.set({ enabled: true });
  setting.set({ startPercent: Number.NaN });
  setting.set({ latestPercent: 96 });
  assert.deepEqual(setting.getSnapshot(), { enabled: true, startPercent: 92, latestPercent: 96 });
  assert.equal(changes, 3);
  assert.deepEqual(JSON.parse(values.get(AUTO_ROTATION_STORAGE_KEY)!), {
    enabled: true,
    startPercent: 92,
    latestPercent: 96,
  });
  assert.deepEqual(
    createAutoRotationSetting(() => storage).getSnapshot(),
    { enabled: true, startPercent: 92, latestPercent: 96 },
  );

  values.set(AUTO_ROTATION_STORAGE_KEY, "{not json");
  assert.deepEqual(
    createAutoRotationSetting(() => storage).getSnapshot(),
    DEFAULT_AUTO_ROTATION_SETTINGS,
  );
  const broken = createAutoRotationSetting(() => {
    throw new Error("blocked");
  });
  assert.deepEqual(broken.getSnapshot(), DEFAULT_AUTO_ROTATION_SETTINGS);
  broken.set({ enabled: true });
  assert.equal(broken.getSnapshot().enabled, true);
});

test("only live Claude subscription panes in Halv's terminals count as busy", () => {
  const pane = (
    id: string,
    extra: Partial<{
      agentKind: "claude" | "codex";
      laneKind: "claudeSubscription" | "claudeApiKey";
      mode: "terminal_embedded" | "gui";
      status: "running" | "exited";
    }> = {},
  ) => ({
    session: {
      id,
      agentKind: "claude" as const,
      laneKind: "claudeSubscription" as const,
      mode: "terminal_embedded" as const,
      status: "running" as const,
      ...extra,
    },
  });
  const entries = [
    pane("claude-pane"),
    pane("codex-pane", { agentKind: "codex" }),
    pane("api-key-pane", { laneKind: "claudeApiKey" }),
    pane("chat", { mode: "gui" }),
    pane("ended", { status: "exited" }),
  ];
  assert.deepEqual(claudeWalletPaneIds(entries), ["claude-pane"]);
  assert.equal(claudePanesBusy(entries, (id) => id !== "claude-pane"), false);
  assert.equal(claudePanesBusy(entries, (id) => id === "claude-pane"), true);
  assert.equal(claudePanesBusy([], () => true), false);
  assert.deepEqual(
    claudeStaleSessionIds([
      { sessionId: "a", provider: "claude", previousAccountId: "x", nextAccountId: "y" },
      { sessionId: "b", provider: "codex", previousAccountId: "x", nextAccountId: "y" },
    ]),
    ["a"],
  );
});

test("a known target must be below the start percent", () => {
  const target = (usages: WalletAccountUsage[], ids = ["near", "far"]) =>
    autoRotationTarget({
      state: state(ids),
      usage: usages,
      activeUsedPercent: 97,
      startPercent: 92,
      rotatedAway: new Set(),
      nowMs: NOW,
    });
  // 93 % is below the active use, so the assisted card offers it, but it is
  // already past the start percent.
  assert.equal(target([usage("near", 93), usage("far", 95)]), null);
  assert.equal(target([usage("near", 93), usage("far", 40)])?.account.id, "far");
  assert.equal(target([usage("near", 91)])?.account.id, "near");
  // Unknown usage still counts, after the known accounts.
  assert.equal(target([usage("near", 93)])?.account.id, "far");
  assert.equal(target([usage("near", 93)])?.maxUsedPercent, null);
});

test("an account that Halv left recently is not a target until its reset", () => {
  let memory = rememberRotatedAway({}, "first", NOW + 30 * MINUTE, NOW);
  const target = (nowMs: number) =>
    autoRotationTarget({
      state: state(["first", "second"], "third"),
      usage: [],
      activeUsedPercent: 95,
      startPercent: 92,
      rotatedAway: rotatedAwayIds(memory, nowMs),
      nowMs,
    })?.account.id;
  assert.equal(target(NOW), "active");
  memory = rememberRotatedAway(memory, "active", NOW + 300 * MINUTE, NOW);
  assert.equal(target(NOW), "second");
  assert.equal(target(NOW + 31 * MINUTE), "first");
  // A passed entry drops out of the memory.
  assert.deepEqual(
    Object.keys(rememberRotatedAway(memory, "second", NOW + 600 * MINUTE, NOW + 31 * MINUTE)).sort(),
    ["active", "second"],
  );
});

test("the left account stays out until the last high window resets", () => {
  const seconds = (minutes: number) => Math.round((NOW + minutes * MINUTE) / 1000);
  assert.equal(
    rotatedAwayUntil(
      [
        { label: "5h", remainingPercent: 5, resetsAt: seconds(41) },
        { label: "7d", remainingPercent: 60, resetsAt: seconds(9_000) },
      ],
      92,
      NOW,
    ),
    NOW + 41 * MINUTE,
  );
  assert.equal(
    rotatedAwayUntil(
      [
        { label: "5h", remainingPercent: 5, resetsAt: seconds(41) },
        { label: "7d", remainingPercent: 7, resetsAt: seconds(9_000) },
      ],
      92,
      NOW,
    ),
    NOW + 9_000 * MINUTE,
  );
  assert.equal(
    rotatedAwayUntil([{ label: "5h", remainingPercent: 3, resetsAt: null }], 92, NOW),
    NOW + AUTO_ROTATION_UNKNOWN_RESET_MS,
  );
  assert.equal(rotatedAwayUntil([], 92, NOW), NOW + AUTO_ROTATION_UNKNOWN_RESET_MS);
});

test("a rotation waits for a pause until the latest percent", () => {
  const step = (
    watch: AutoRotationWatch,
    usedPercent: number | null,
    busy: boolean,
    accountKey = "work",
  ) =>
    nextAutoRotationStep(watch, { accountKey, usedPercent, busy, nowMs: NOW, settings: SETTINGS });
  const initial = INITIAL_AUTO_ROTATION_WATCH;
  assert.equal(step(initial, 91, false).rotate, false);
  assert.equal(step(initial, 92, true).rotate, false);
  assert.equal(step(initial, 98, true).rotate, false);
  assert.equal(step(initial, 92, false).rotate, true);
  assert.equal(step(initial, 99, true).rotate, true);
  assert.equal(step(initial, 100, true).rotate, true);
  // No reading or System default never rotates.
  assert.equal(step(initial, null, false).rotate, false);
  assert.equal(step(initial, 99, false, "system").rotate, false);
});

test("one rotation per crossing, and a failure tries again after a minute", () => {
  const read = (
    watch: AutoRotationWatch,
    usedPercent: number | null,
    nowMs = NOW,
    accountKey = "work",
  ) =>
    nextAutoRotationStep(watch, { accountKey, usedPercent, busy: false, nowMs, settings: SETTINGS });

  let { watch, rotate } = read(INITIAL_AUTO_ROTATION_WATCH, 95);
  assert.equal(rotate, true);
  assert.equal(watch.phase, "rotating");
  // No second rotation while the first one runs.
  assert.equal(read(watch, 99).rotate, false);

  // A switch that did not change the account ends the crossing.
  watch = finishAutoRotation(watch, "work", "switched", NOW);
  assert.equal(read(watch, 99).rotate, false);
  const dropped = read(watch, 40);
  assert.equal(dropped.rearmed, true);
  assert.equal(read(dropped.watch, 93).rotate, true);

  // A failure waits one minute.
  ({ watch } = read(INITIAL_AUTO_ROTATION_WATCH, 95));
  watch = finishAutoRotation(watch, "work", "failed", NOW);
  assert.equal(watch.retryAtMs, NOW + AUTO_ROTATION_RETRY_MS);
  assert.equal(read(watch, 95, NOW + AUTO_ROTATION_RETRY_MS - 1).rotate, false);
  assert.equal(read(watch, 95, NOW + AUTO_ROTATION_RETRY_MS).rotate, true);

  // A crossing without a target waits one minute too.
  ({ watch } = read(INITIAL_AUTO_ROTATION_WATCH, 95));
  watch = finishAutoRotation(watch, "work", "noTarget", NOW);
  assert.equal(watch.phase, "retry");

  // Another account starts a new crossing.
  const next = read(watch, null, NOW, "personal");
  assert.equal(next.rearmed, true);
  assert.deepEqual(next.watch, { accountKey: "personal", phase: "armed", retryAtMs: 0 });

  // The end of a switch for an account that is no longer active does nothing.
  assert.deepEqual(finishAutoRotation(next.watch, "work", "failed", NOW), next.watch);
  ({ watch } = read(INITIAL_AUTO_ROTATION_WATCH, 95));
  assert.equal(finishAutoRotation(watch, "work", "cancelled", NOW).phase, "armed");
});

test("with rotation on, the assisted card shows only as the fallback", () => {
  const reading = { accountKey: "work", usedPercent: 95 };
  assert.deepEqual(
    assistedSwapInput(reading, { enabled: false, fallback: false, startPercent: 92 }),
    { reading, threshold: 90 },
  );
  assert.deepEqual(
    assistedSwapInput(reading, { enabled: true, fallback: false, startPercent: 92 }),
    { reading: { accountKey: "work", usedPercent: null }, threshold: 92 },
  );
  assert.deepEqual(
    assistedSwapInput(reading, { enabled: true, fallback: true, startPercent: 92 }),
    { reading, threshold: 92 },
  );
  // A low reading still passes, so a shown card hides.
  const low = { accountKey: "work", usedPercent: 91 };
  assert.deepEqual(
    assistedSwapInput(low, { enabled: true, fallback: false, startPercent: 92 }),
    { reading: low, threshold: 92 },
  );
});

import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  ACCOUNT_SWAP_NOTICE_PERCENT,
  ACCOUNT_SWAP_STORAGE_KEY,
  INITIAL_ACCOUNT_SWAP_WATCH,
  accountSwapTarget,
  createAccountSwapSetting,
  nextAccountSwapWatch,
  type AccountSwapWatch,
} from "../src/accountSwap.ts";
// @ts-expect-error Node's test runner needs the source extension.
import type {
  WalletAccount,
  WalletAccountUsage,
  WalletProviderState,
} from "../src/accountWallet.ts";

const NOW = Date.parse("2026-10-15T12:00:00Z");
const MINUTE = 60_000;

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

function usage(
  accountId: string,
  windows: Array<[kind: "session" | "weekly" | "fableWeekly", used: number, resetMinutes: number | null]>,
): WalletAccountUsage {
  return {
    provider: "claude",
    accountId,
    windows: windows.map(([kind, usedPercent, resetMinutes]) => ({
      kind,
      usedPercent,
      resetsAt: resetMinutes === null ? null : NOW + resetMinutes * MINUTE,
    })),
    updatedAt: NOW,
  };
}

function state(accounts: WalletAccount[], activeId = "active"): WalletProviderState {
  return {
    activeId,
    systemDefault: { email: null, plan: null },
    accounts: [account("active"), ...accounts],
  };
}

function target(
  accounts: WalletAccount[],
  usages: WalletAccountUsage[],
  activeUsedPercent = 92,
) {
  return accountSwapTarget({
    state: state(accounts),
    provider: "claude",
    usage: usages,
    activeUsedPercent,
    nowMs: NOW,
  });
}

test("the account with the lowest highest use wins", () => {
  const result = target(
    [account("busy"), account("free"), account("medium")],
    [
      usage("busy", [["session", 86, 41], ["weekly", 92, 1_680]]),
      usage("free", [["session", 8, 131], ["weekly", 64, 4_380]]),
      usage("medium", [["session", 70, 20], ["weekly", 70, 900]]),
    ],
  );
  assert.equal(result?.account.id, "free");
  assert.equal(result?.maxUsedPercent, 64);
});

test("a tie goes to the soonest reset", () => {
  const result = target(
    [account("later"), account("sooner")],
    [
      usage("later", [["session", 40, 200]]),
      usage("sooner", [["session", 40, 30]]),
    ],
  );
  assert.equal(result?.account.id, "sooner");
});

test("a full week, a sign-in, a missing folder, or more use rule an account out", () => {
  const result = target(
    [
      account("full-week"),
      account("needs-sign-in", { needsSignIn: true }),
      account("missing", { missing: true }),
      account("worse"),
    ],
    [
      usage("full-week", [["session", 0, 300], ["weekly", 100, 600]]),
      usage("needs-sign-in", [["session", 1, 300]]),
      usage("missing", [["session", 1, 300]]),
      usage("worse", [["session", 95, 300]]),
    ],
  );
  assert.equal(result, null);
});

test("a window whose reset passed counts as empty", () => {
  const result = target(
    [account("reset")],
    [usage("reset", [["session", 99, -5], ["weekly", 30, 600]])],
  );
  assert.equal(result?.account.id, "reset");
  assert.equal(result?.maxUsedPercent, 30);
});

test("accounts with unknown usage come after the known ones", () => {
  // Halv's own wallet knows no usage of the accounts that are not active.
  assert.equal(target([account("first"), account("second")], [])?.account.id, "first");
  assert.equal(target([account("first"), account("second")], [])?.maxUsedPercent, null);
  assert.equal(
    target(
      [account("unknown"), account("known")],
      [usage("known", [["session", 50, 60]])],
    )?.account.id,
    "known",
  );
});

test("the notice shows once per crossing of the threshold", () => {
  let watch: AccountSwapWatch = INITIAL_ACCOUNT_SWAP_WATCH;
  const read = (accountKey: string, usedPercent: number | null) => {
    const next = nextAccountSwapWatch(watch, { accountKey, usedPercent });
    watch = next.watch;
    return { show: next.show, hide: next.hide };
  };
  assert.equal(ACCOUNT_SWAP_NOTICE_PERCENT, 90);
  assert.deepEqual(read("work", 60), { show: false, hide: true });
  assert.deepEqual(read("work", 90), { show: true, hide: false });
  // The notice stays once: higher use or a reload does not show it again.
  assert.deepEqual(read("work", 95), { show: false, hide: false });
  assert.deepEqual(read("work", null), { show: false, hide: false });
  // A drop below the threshold arms it again.
  assert.deepEqual(read("work", 12), { show: false, hide: true });
  assert.deepEqual(read("work", 91), { show: true, hide: false });
  // Another account arms it again and hides the old notice.
  assert.deepEqual(read("personal", null), { show: false, hide: true });
  assert.deepEqual(read("personal", 93), { show: true, hide: false });
});

test("the setting turns the notice off for this viewer", () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  };
  const setting = createAccountSwapSetting(() => storage);
  let changes = 0;
  setting.subscribe(() => {
    changes += 1;
  });
  assert.equal(setting.getSnapshot(), true);
  setting.set(false);
  assert.equal(setting.getSnapshot(), false);
  assert.equal(values.get(ACCOUNT_SWAP_STORAGE_KEY), "off");
  assert.equal(changes, 1);
  assert.equal(createAccountSwapSetting(() => storage).getSnapshot(), false);
  const broken = createAccountSwapSetting(() => {
    throw new Error("blocked");
  });
  assert.equal(broken.getSnapshot(), true);
});

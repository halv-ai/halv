// The assisted account switch near a usage limit. When the binding usage
// window of the active account reaches the threshold, Halv offers a
// one-click switch to the account with the most headroom. The card itself
// never switches without that click. Only the opt-in automatic rotation of
// Claude accounts switches without a click (accountAutoRotation.ts).
import type {
  WalletAccount,
  WalletAccountUsage,
  WalletProvider,
  WalletProviderState,
} from "./accountWallet.ts";

/** The used percent of the binding window that shows the notice. */
export const ACCOUNT_SWAP_NOTICE_PERCENT = 90;
/** An account at or above this weekly use cannot take over. */
export const ACCOUNT_SWAP_WEEKLY_FULL_PERCENT = 100;
export const ACCOUNT_SWAP_STORAGE_KEY = "halv.account-swap-notice.v1";

export type AccountSwapTarget = {
  account: WalletAccount;
  // The highest used percent of its windows, or null when Halv does not
  // know its usage.
  maxUsedPercent: number | null;
};

type Candidate = AccountSwapTarget & { resetsAt: number | null };

/**
 * The account with the most headroom: the lowest highest used percent.
 * Ties go to the soonest reset. An account that needs a sign-in, an account
 * without its folder, and an account with a full weekly window drop out. An
 * account with more use than the active account drops out too. Accounts
 * with unknown usage come after the known ones, in wallet order.
 */
export function accountSwapTarget(input: {
  state: WalletProviderState;
  provider: WalletProvider;
  usage: readonly WalletAccountUsage[];
  activeUsedPercent: number;
  nowMs: number;
}): AccountSwapTarget | null {
  const known: Candidate[] = [];
  const unknown: Candidate[] = [];
  for (const account of input.state.accounts) {
    if (account.id === input.state.activeId) continue;
    if (account.missing || account.needsSignIn) continue;
    const usage = input.usage.find(
      (entry) => entry.provider === input.provider && entry.accountId === account.id,
    );
    // A window whose reset passed starts again from zero.
    const windows = (usage?.windows ?? [])
      .filter((window) => Number.isFinite(window.usedPercent))
      .map((window) =>
        window.resetsAt !== null && window.resetsAt <= input.nowMs
          ? { ...window, usedPercent: 0 }
          : window,
      );
    if (windows.length === 0) {
      unknown.push({ account, maxUsedPercent: null, resetsAt: null });
      continue;
    }
    const weeklyFull = windows.some(
      (window) =>
        window.kind === "weekly" &&
        window.usedPercent >= ACCOUNT_SWAP_WEEKLY_FULL_PERCENT,
    );
    if (weeklyFull) continue;
    const maxUsedPercent = Math.max(...windows.map((window) => window.usedPercent));
    if (maxUsedPercent >= input.activeUsedPercent) continue;
    const resets = windows
      .filter((window) => window.usedPercent === maxUsedPercent && window.resetsAt !== null)
      .map((window) => window.resetsAt!);
    known.push({
      account,
      maxUsedPercent,
      resetsAt: resets.length > 0 ? Math.min(...resets) : null,
    });
  }
  known.sort(
    (left, right) =>
      left.maxUsedPercent! - right.maxUsedPercent! ||
      (left.resetsAt ?? Number.POSITIVE_INFINITY) -
        (right.resetsAt ?? Number.POSITIVE_INFINITY),
  );
  const best = known[0] ?? unknown[0];
  return best ? { account: best.account, maxUsedPercent: best.maxUsedPercent } : null;
}

/** The notice state of one provider. */
export type AccountSwapWatch = {
  // The active account at the last reading. Null before the first reading.
  accountKey: string | null;
  // True while the next crossing of the threshold shows the notice.
  armed: boolean;
};

export const INITIAL_ACCOUNT_SWAP_WATCH: AccountSwapWatch = {
  accountKey: null,
  armed: true,
};

export type AccountSwapReading = {
  // The active account id, or "system" for System default.
  accountKey: string;
  // The used percent of the binding window of the active account. Null
  // while Halv has no current usage for this account.
  usedPercent: number | null;
};

/**
 * Shows the notice once per crossing of the threshold. A drop below the
 * threshold or another active account arms it again, and hides a shown
 * notice.
 */
export function nextAccountSwapWatch(
  previous: AccountSwapWatch,
  reading: AccountSwapReading,
  threshold = ACCOUNT_SWAP_NOTICE_PERCENT,
): { watch: AccountSwapWatch; show: boolean; hide: boolean } {
  let watch = previous;
  let hide = false;
  if (previous.accountKey !== reading.accountKey) {
    watch = { accountKey: reading.accountKey, armed: true };
    hide = previous.accountKey !== null;
  }
  if (reading.usedPercent === null) return { watch, show: false, hide };
  if (reading.usedPercent < threshold) {
    return { watch: { ...watch, armed: true }, show: false, hide: true };
  }
  if (!watch.armed) return { watch, show: false, hide };
  return { watch: { ...watch, armed: false }, show: true, hide: false };
}

type SwapStorage = Pick<Storage, "getItem" | "setItem">;

function browserStorage(): SwapStorage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

/**
 * The setting that turns the notice off. It lives in the localStorage of
 * this viewer. A storage that throws keeps the notice on.
 */
export function createAccountSwapSetting(storage: () => SwapStorage | null = browserStorage) {
  const listeners = new Set<() => void>();
  let enabled: boolean | null = null;
  const read = () => {
    try {
      return storage()?.getItem(ACCOUNT_SWAP_STORAGE_KEY) !== "off";
    } catch {
      return true;
    }
  };
  return {
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    getSnapshot() {
      enabled ??= read();
      return enabled;
    },
    set(next: boolean) {
      enabled = next;
      try {
        storage()?.setItem(ACCOUNT_SWAP_STORAGE_KEY, next ? "on" : "off");
      } catch {
        // The value still applies to this window.
      }
      for (const listener of listeners) listener();
    },
  };
}

export const accountSwapSetting = createAccountSwapSetting();

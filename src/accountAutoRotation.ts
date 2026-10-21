// Automatic rotation of Claude accounts near a usage limit (ADR 0017). The
// setting is off by default. When the user turns it on, Halv switches the
// active Claude account between the start percent and the latest percent of
// the binding window. Below the latest percent, Halv waits until no Claude
// pane is busy. At the latest percent, Halv switches at once. The switch
// uses the normal select path, so Orca stays the single writer (ADR 0013).
import {
  ACCOUNT_SWAP_NOTICE_PERCENT,
  accountSwapTarget,
  type AccountSwapReading,
  type AccountSwapTarget,
} from "./accountSwap.ts";
import type {
  WalletAccountUsage,
  WalletProviderState,
  WalletStaleSession,
} from "./accountWallet.ts";
import type { AgentSession } from "./agentSessions.ts";
import type { CloudAccess } from "./halvCloud.ts";
import type { ProviderPlanWindow } from "./providerPlanWindows.ts";

export const AUTO_ROTATION_STORAGE_KEY = "halv.account-auto-rotation.v1";
export const AUTO_ROTATION_DEFAULT_START_PERCENT = 92;
export const AUTO_ROTATION_DEFAULT_LATEST_PERCENT = 99;
/** The lowest start percent that the setting accepts. */
export const AUTO_ROTATION_MIN_START_PERCENT = 50;
export const AUTO_ROTATION_MAX_PERCENT = 100;
/** A failed switch, or a crossing without a target, tries again after this delay. */
export const AUTO_ROTATION_RETRY_MS = 60_000;
/** An account that Halv left without a known reset stays out for this time. */
export const AUTO_ROTATION_UNKNOWN_RESET_MS = 5 * 60 * 60_000;

export type AutoRotationSettings = {
  enabled: boolean;
  // From this used percent, Halv switches when no Claude pane is busy.
  startPercent: number;
  // From this used percent, Halv switches at once, also during a turn.
  latestPercent: number;
};

export const DEFAULT_AUTO_ROTATION_SETTINGS: AutoRotationSettings = {
  enabled: false,
  startPercent: AUTO_ROTATION_DEFAULT_START_PERCENT,
  latestPercent: AUTO_ROTATION_DEFAULT_LATEST_PERCENT,
};

function clampPercent(value: unknown, min: number, fallback: number) {
  const number = typeof value === "number" ? value : Number.NaN;
  const safe = Number.isFinite(number) ? Math.round(number) : fallback;
  return Math.min(AUTO_ROTATION_MAX_PERCENT, Math.max(min, safe));
}

/**
 * Makes a stored or typed value valid. The start percent stays in
 * [50, 100], and the latest percent stays in [start, 100]. A value that is
 * not a finite number takes the value of `fallback`.
 */
export function normalizeAutoRotationSettings(
  value: unknown,
  fallback: AutoRotationSettings = DEFAULT_AUTO_ROTATION_SETTINGS,
): AutoRotationSettings {
  const record =
    value !== null && typeof value === "object"
      ? (value as Record<string, unknown>)
      : {};
  const startPercent = clampPercent(
    record.startPercent,
    AUTO_ROTATION_MIN_START_PERCENT,
    fallback.startPercent,
  );
  const latestPercent = clampPercent(
    record.latestPercent,
    startPercent,
    fallback.latestPercent,
  );
  return { enabled: record.enabled === true, startPercent, latestPercent };
}

type RotationStorage = Pick<Storage, "getItem" | "setItem">;

function browserStorage(): RotationStorage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

/**
 * The automatic rotation setting. It lives in the localStorage of this
 * viewer as one JSON value. A storage that throws keeps the rotation off.
 */
export function createAutoRotationSetting(
  storage: () => RotationStorage | null = browserStorage,
) {
  const listeners = new Set<() => void>();
  let settings: AutoRotationSettings | null = null;
  const read = () => {
    try {
      const raw = storage()?.getItem(AUTO_ROTATION_STORAGE_KEY);
      return raw
        ? normalizeAutoRotationSettings(JSON.parse(raw))
        : DEFAULT_AUTO_ROTATION_SETTINGS;
    } catch {
      return DEFAULT_AUTO_ROTATION_SETTINGS;
    }
  };
  const getSnapshot = () => {
    settings ??= read();
    return settings;
  };
  return {
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    getSnapshot,
    /** Merges the change. An invalid number keeps the current value. */
    set(change: Partial<AutoRotationSettings>) {
      const current = getSnapshot();
      settings = normalizeAutoRotationSettings({ ...current, ...change }, current);
      try {
        storage()?.setItem(AUTO_ROTATION_STORAGE_KEY, JSON.stringify(settings));
      } catch {
        // The value still applies to this window.
      }
      for (const listener of listeners) listener();
    },
  };
}

export const autoRotationSetting = createAutoRotationSetting();

/**
 * True when Halv may switch accounts without a click. Automatic rotation is
 * part of Halv Cloud (ADR 0021). Without access, the setting can stay on,
 * but Halv does not switch, and the assisted card works as before.
 */
export function autoRotationActive(
  settings: Pick<AutoRotationSettings, "enabled">,
  access: Pick<CloudAccess, "granted">,
) {
  return settings.enabled && access.granted;
}

/**
 * True when Settings shows the Halv Cloud card under the rotation checkbox:
 * the setting is on, the account status is loaded, the account has no
 * access, and the user did not choose "Keep going without it" in this visit.
 */
export function rotationCloudCardVisible({
  ready,
  settings,
  access,
  dismissedThisVisit,
}: {
  ready: boolean;
  settings: Pick<AutoRotationSettings, "enabled">;
  access: Pick<CloudAccess, "granted">;
  dismissedThisVisit: boolean;
}) {
  return ready && settings.enabled && !access.granted && !dismissedThisVisit;
}

const LIVE_AGENT_STATUSES = new Set(["starting", "running", "waiting_input"]);

type PaneEntry = {
  session: Pick<AgentSession, "id" | "agentKind" | "laneKind" | "mode" | "status">;
};

/**
 * The live Claude panes in Halv's terminals that use the wallet account.
 * Only the subscription lane uses the wallet. An agent session id is also
 * the id of its terminal pane.
 */
export function claudeWalletPaneIds(entries: readonly PaneEntry[]): string[] {
  return entries
    .filter(
      ({ session }) =>
        session.agentKind === "claude" &&
        session.laneKind === "claudeSubscription" &&
        session.mode === "terminal_embedded" &&
        LIVE_AGENT_STATUSES.has(session.status),
    )
    .map(({ session }) => session.id);
}

/**
 * True while a Claude pane is busy: a turn runs, or an approval or a
 * question waits. A pane without an activity signal rests.
 */
export function claudePanesBusy(
  entries: readonly PaneEntry[],
  busy: (sessionId: string) => boolean,
): boolean {
  return claudeWalletPaneIds(entries).some(busy);
}

/** The panes of a switch result that Halv restarts between turns. */
export function claudeStaleSessionIds(stale: readonly WalletStaleSession[]): string[] {
  return stale
    .filter((session) => session.provider === "claude")
    .map((session) => session.sessionId);
}

/**
 * The accounts that Halv rotated away from, with the time in Unix
 * milliseconds until which each one is not an automatic target.
 */
export type RotatedAwayAccounts = Readonly<Record<string, number>>;

/**
 * The time until which a left account stays out: the last reset of the
 * windows at or above the start percent. Usually that is the reset of the
 * binding window. Without a known reset, the account stays out for 5 hours.
 * Plan window resets are Unix seconds.
 */
export function rotatedAwayUntil(
  windows: readonly ProviderPlanWindow[],
  startPercent: number,
  nowMs: number,
): number {
  const unknown = nowMs + AUTO_ROTATION_UNKNOWN_RESET_MS;
  let until: number | null = null;
  for (const window of windows) {
    if (window.remainingPercent == null) continue;
    if (100 - window.remainingPercent < startPercent) continue;
    const resetMs = window.resetsAt == null ? null : window.resetsAt * 1000;
    const windowUntil = resetMs !== null && resetMs > nowMs ? resetMs : unknown;
    until = Math.max(until ?? windowUntil, windowUntil);
  }
  return until ?? unknown;
}

/** Adds a left account, and drops the entries whose time passed. */
export function rememberRotatedAway(
  memory: RotatedAwayAccounts,
  accountId: string,
  untilMs: number,
  nowMs: number,
): RotatedAwayAccounts {
  const next: Record<string, number> = {};
  for (const [id, until] of Object.entries(memory)) {
    if (until > nowMs) next[id] = until;
  }
  next[accountId] = untilMs;
  return next;
}

/** The accounts that are not an automatic target now. */
export function rotatedAwayIds(
  memory: RotatedAwayAccounts,
  nowMs: number,
): ReadonlySet<string> {
  return new Set(
    Object.entries(memory)
      .filter(([, until]) => until > nowMs)
      .map(([id]) => id),
  );
}

/**
 * The target of an automatic rotation. It uses the rules of the assisted
 * switch, with two more rules: a target with known usage must be below the
 * start percent, and an account that Halv left recently is not a target.
 * A target with unknown usage still counts, after the known ones.
 */
export function autoRotationTarget(input: {
  state: WalletProviderState;
  usage: readonly WalletAccountUsage[];
  activeUsedPercent: number;
  startPercent: number;
  rotatedAway: ReadonlySet<string>;
  nowMs: number;
}): AccountSwapTarget | null {
  return accountSwapTarget({
    state: {
      ...input.state,
      accounts: input.state.accounts.filter(
        (account) => !input.rotatedAway.has(account.id),
      ),
    },
    provider: "claude",
    usage: input.usage,
    // A known target must be below both the start percent and the active use.
    activeUsedPercent: Math.min(input.activeUsedPercent, input.startPercent),
    nowMs: input.nowMs,
  });
}

/** The rotation state of the active Claude account in one crossing. */
export type AutoRotationWatch = {
  // The active account at the last reading. Null before the first reading.
  accountKey: string | null;
  // "armed" switches at the next chance. "rotating" waits for a switch.
  // "done" waits for another account or a drop below the start percent.
  // "retry" tries again at `retryAtMs`.
  phase: "armed" | "rotating" | "done" | "retry";
  retryAtMs: number;
};

export const INITIAL_AUTO_ROTATION_WATCH: AutoRotationWatch = {
  accountKey: null,
  phase: "armed",
  retryAtMs: 0,
};

/**
 * Decides if Halv switches now. Halv never switches away from System
 * default, and it ignores usage that does not belong to the active account.
 * `rearmed` is true when another account or a drop below the start percent
 * starts a new crossing.
 */
export function nextAutoRotationStep(
  previous: AutoRotationWatch,
  input: AccountSwapReading & {
    busy: boolean;
    nowMs: number;
    settings: Pick<AutoRotationSettings, "startPercent" | "latestPercent">;
  },
): { watch: AutoRotationWatch; rotate: boolean; rearmed: boolean } {
  const armed = (accountKey: string): AutoRotationWatch => ({
    accountKey,
    phase: "armed",
    retryAtMs: 0,
  });
  let watch = previous;
  let rearmed = false;
  if (previous.accountKey !== input.accountKey) {
    watch = armed(input.accountKey);
    rearmed = previous.accountKey !== null;
  }
  if (input.accountKey === "system" || input.usedPercent === null) {
    return { watch, rotate: false, rearmed };
  }
  if (input.usedPercent < input.settings.startPercent) {
    if (watch.phase === "armed" || watch.phase === "rotating") {
      return { watch, rotate: false, rearmed };
    }
    return { watch: armed(input.accountKey), rotate: false, rearmed: true };
  }
  if (watch.phase === "rotating" || watch.phase === "done") {
    return { watch, rotate: false, rearmed };
  }
  if (watch.phase === "retry" && input.nowMs < watch.retryAtMs) {
    return { watch, rotate: false, rearmed };
  }
  if (input.busy && input.usedPercent < input.settings.latestPercent) {
    return { watch, rotate: false, rearmed };
  }
  return { watch: { ...watch, phase: "rotating" }, rotate: true, rearmed };
}

/**
 * Records the end of a switch that started for `accountKey`. A switch ends
 * the crossing. A failure or a missing target tries again after one
 * minute. "cancelled" arms the watch again.
 */
export function finishAutoRotation(
  watch: AutoRotationWatch,
  accountKey: string,
  outcome: "switched" | "failed" | "noTarget" | "cancelled",
  nowMs: number,
): AutoRotationWatch {
  if (watch.accountKey !== accountKey || watch.phase !== "rotating") return watch;
  if (outcome === "switched") return { ...watch, phase: "done", retryAtMs: 0 };
  if (outcome === "cancelled") return { ...watch, phase: "armed", retryAtMs: 0 };
  return { ...watch, phase: "retry", retryAtMs: nowMs + AUTO_ROTATION_RETRY_MS };
}

/**
 * The reading and threshold of the assisted Claude card. With automatic
 * rotation on, the card shows only when the rotation found no target, and
 * its threshold is the start percent. A reading below the threshold still
 * passes, so the card hides and arms again.
 */
export function assistedSwapInput(
  reading: AccountSwapReading,
  autoRotation: { enabled: boolean; fallback: boolean; startPercent: number },
): { reading: AccountSwapReading; threshold: number } {
  if (!autoRotation.enabled) {
    return { reading, threshold: ACCOUNT_SWAP_NOTICE_PERCENT };
  }
  const threshold = autoRotation.startPercent;
  const hidden =
    !autoRotation.fallback &&
    reading.usedPercent !== null &&
    reading.usedPercent >= threshold;
  return { reading: hidden ? { ...reading, usedPercent: null } : reading, threshold };
}

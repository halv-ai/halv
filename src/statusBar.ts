// Pure logic for the status line at the bottom of the window. The layout
// adapts Orca's StatusBar (stablyai/orca, MIT).
import type {
  WalletAccountUsage,
  WalletUsageWindow,
  WalletUsageWindowKind,
} from "./accountWallet.ts";
import type {
  PlanProviderId,
  ProviderPlanWindowGroup,
} from "./providerPlanWindows.ts";

export const STATUS_BAR_COMPACT_BELOW_PX = 900;
export const STATUS_BAR_MINIMAL_BELOW_PX = 500;
export const USAGE_WARNING_PERCENT = 60;
export const USAGE_DANGER_PERCENT = 80;
export const USAGE_REFRESH_THROTTLE_MS = 30_000;
export const STATUS_POPOVER_WIDTH_PX = 360;
const STATUS_POPOVER_MARGIN_PX = 8;

// The providers that hold a wallet. The status line always shows them.
const WALLET_PLAN_PROVIDERS: readonly PlanProviderId[] = ["claude", "codex"];

const PROVIDER_LETTERS: Record<PlanProviderId, string> = {
  claude: "C",
  codex: "X",
  kimi: "K",
  glm: "G",
  grok: "Gr",
};

/**
 * "full" shows every window and a mini bar. "compact" shows only the tightest
 * window. "minimal" shows a letter badge and a percentage.
 */
export type StatusBarDensity = "full" | "compact" | "minimal";

export type UsageTone = "normal" | "warning" | "danger";

/** One plan window, ready for the status line. */
export type StatusUsageWindow = {
  key: string;
  usedPercent: number;
  // The time to the reset, or the window length when the reset is unknown.
  resetLabel: string;
  tone: UsageTone;
};

export type StatusProviderUsage = {
  id: PlanProviderId;
  windows: StatusUsageWindow[];
  // The window with the highest use, or null when no window has data.
  tightest: StatusUsageWindow | null;
};

export type StatusUsageLabels = {
  // The short label of a weekly window, for example "wk".
  week: string;
};

export function statusBarDensity(windowWidth: number): StatusBarDensity {
  if (windowWidth < STATUS_BAR_MINIMAL_BELOW_PX) return "minimal";
  if (windowWidth < STATUS_BAR_COMPACT_BELOW_PX) return "compact";
  return "full";
}

export function usageTone(usedPercent: number): UsageTone {
  if (usedPercent >= USAGE_DANGER_PERCENT) return "danger";
  if (usedPercent >= USAGE_WARNING_PERCENT) return "warning";
  return "normal";
}

/** A short countdown such as "12m", "3h 54m", or "6d 7h". */
export function formatStatusCountdown(resetsAtMs: number, nowMs: number) {
  const minutes = Math.max(0, Math.ceil((resetsAtMs - nowMs) / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

/** The window length for a window without a known reset: "5h" or "wk". */
export function windowLengthLabel(label: string, labels: StatusUsageLabels) {
  return label === "7d" ? labels.week : label;
}

/**
 * Converts plan windows to status windows. Windows without data drop out.
 * Plan window resets are Unix seconds.
 */
export function statusProviderUsage(
  group: ProviderPlanWindowGroup,
  nowMs: number,
  labels: StatusUsageLabels,
): StatusProviderUsage {
  const windows = group.windows.flatMap((window, index) => {
    if (window.remainingPercent == null) return [];
    const usedPercent = clampPercent(100 - window.remainingPercent);
    return [
      {
        key: window.label || String(index),
        usedPercent,
        resetLabel:
          window.resetsAt == null
            ? windowLengthLabel(window.label, labels)
            : formatStatusCountdown(window.resetsAt * 1000, nowMs),
        tone: usageTone(usedPercent),
      },
    ];
  });
  return { id: group.id, windows, tightest: tightestWindow(windows) };
}

/**
 * The providers that get a pill. Claude and Codex always show when they are
 * connected. Other providers show only when a window has data.
 */
export function statusBarProviderGroups(groups: ProviderPlanWindowGroup[]) {
  return groups.filter(
    (group) =>
      WALLET_PLAN_PROVIDERS.includes(group.id) ||
      group.windows.some((window) => window.remainingPercent != null),
  );
}

export function isWalletPlanProvider(
  id: PlanProviderId,
): id is "claude" | "codex" {
  return WALLET_PLAN_PROVIDERS.includes(id);
}

export function providerLetter(id: PlanProviderId) {
  return PROVIDER_LETTERS[id];
}

/**
 * The usage windows of an account that is not active. Wallet resets are Unix
 * milliseconds. A window whose reset is in the past has no current data.
 */
export function walletUsageWindows(
  usage: WalletAccountUsage | undefined,
  nowMs: number,
  labels: StatusUsageLabels & { fable: string },
): StatusUsageWindow[] {
  if (!usage) return [];
  const order: WalletUsageWindowKind[] = ["session", "weekly", "fableWeekly"];
  // One window per kind. A live window wins over one that already reset.
  // Orca caches an inactive account, so its session window often reset
  // since the last fetch. That window starts again at zero, so the row
  // keeps it at 0% instead of hiding the limit.
  const byKind = new Map<WalletUsageWindowKind, WalletUsageWindow>();
  for (const window of usage.windows) {
    if (!Number.isFinite(window.usedPercent)) continue;
    const current = byKind.get(window.kind);
    const live = window.resetsAt == null || window.resetsAt > nowMs;
    const currentLive =
      current !== undefined && (current.resetsAt == null || current.resetsAt > nowMs);
    if (!current || (live && !currentLive)) byKind.set(window.kind, window);
  }
  return [...byKind.values()]
    .sort((left, right) => order.indexOf(left.kind) - order.indexOf(right.kind))
    .map((window) => {
      const reset = window.resetsAt != null && window.resetsAt <= nowMs;
      const usedPercent = reset ? 0 : clampPercent(window.usedPercent);
      // Orca shows the session countdown and names the weekly windows.
      const resetLabel =
        window.kind === "session"
          ? window.resetsAt == null || reset
            ? "5h"
            : formatStatusCountdown(window.resetsAt, nowMs)
          : window.kind === "weekly"
            ? labels.week
            : labels.fable;
      return {
        key: window.kind,
        usedPercent,
        resetLabel,
        tone: usageTone(usedPercent),
      };
    });
}

export function findWalletUsage(
  usage: readonly WalletAccountUsage[],
  provider: "claude" | "codex",
  accountId: string,
) {
  return usage.find(
    (entry) => entry.provider === provider && entry.accountId === accountId,
  );
}

/** True when the last manual refresh is older than the throttle. */
export function canRefreshUsage(
  lastRefreshMs: number | null,
  nowMs: number,
  throttleMs = USAGE_REFRESH_THROTTLE_MS,
) {
  return lastRefreshMs === null || nowMs - lastRefreshMs >= throttleMs;
}

/** The left edge of a popover that starts at its anchor and stays on screen. */
export function popoverLeft(
  anchorLeft: number,
  viewportWidth: number,
  popoverWidth = STATUS_POPOVER_WIDTH_PX,
) {
  const maxLeft = Math.max(
    STATUS_POPOVER_MARGIN_PX,
    viewportWidth - popoverWidth - STATUS_POPOVER_MARGIN_PX,
  );
  return Math.min(Math.max(anchorLeft, STATUS_POPOVER_MARGIN_PX), maxLeft);
}

function tightestWindow(windows: StatusUsageWindow[]) {
  return windows.reduce<StatusUsageWindow | null>(
    (tightest, window) =>
      tightest === null || window.usedPercent > tightest.usedPercent
        ? window
        : tightest,
    null,
  );
}

function clampPercent(value: number) {
  return Math.max(0, Math.min(100, Math.round(value)));
}

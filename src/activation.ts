// Activation facts (ADR 0018). The app asks for a Halv account only after it
// delivered value. These functions decide that moment and describe it.
import { calculateInputSavings } from "./savings.ts";
import type { UiMessage } from "./types.ts";

export const FIRST_RESULT_STORAGE_KEY = "halv.firstResult";

type ActivationStorage = Pick<Storage, "getItem" | "setItem">;

export const TRIAL_DAYS = 7;
export const TRIAL_GOAL_ACTIVE_DAYS = 5;
const DAY_MS = 86_400_000;

/** True when a first task completed on any lane, or a saving was measured. */
export function valueDelivered({
  firstResultSeen,
  onboardingCompleted,
  tokensSaved,
}: {
  firstResultSeen: boolean;
  onboardingCompleted: boolean;
  tokensSaved: number;
}) {
  return firstResultSeen || onboardingCompleted || tokensSaved > 0;
}

/** The percent of input context that Halv removed on the last reply. */
export function lastTurnSavingsPercent(messages: readonly UiMessage[]) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role !== "assistant") {
      continue;
    }
    if (message.tokensSaved == null) {
      return null;
    }
    const savings = calculateInputSavings(
      message.inputTokens ?? message.usage?.inputTokens,
      message.tokensSaved,
    );
    if (!savings) {
      return null;
    }
    return savings.percent;
  }
  return null;
}

/**
 * What the trial card can say about this device, in order of strength:
 * dollars saved, tokens saved, then the conversations the user keeps.
 */
export type TrialPromptFacts =
  | { kind: "dollars"; amountUsd: number; conversations: number }
  | { kind: "tokens"; tokens: number; conversations: number }
  | { kind: "conversations"; conversations: number }
  | { kind: "none" };

export function trialPromptFacts({
  conversations,
  tokensSaved,
  dollarsUsd,
}: {
  conversations: number;
  tokensSaved: number;
  dollarsUsd: number;
}): TrialPromptFacts {
  const kept = Math.max(0, Math.floor(conversations));
  if (dollarsUsd >= 0.01) {
    return { kind: "dollars", amountUsd: dollarsUsd, conversations: kept };
  }
  if (tokensSaved > 0) {
    return { kind: "tokens", tokens: tokensSaved, conversations: kept };
  }
  if (kept > 0) {
    return { kind: "conversations", conversations: kept };
  }
  return { kind: "none" };
}

export type TrialGoalProgress = {
  /** 1-based day of the trial. */
  day: number;
  totalDays: number;
  daysLeft: number;
  /** UTC days with a task since the trial began. */
  activeDays: number;
  targetDays: number;
  reached: boolean;
  ended: boolean;
};

/**
 * The trial goal: a task on `targetDays` of the `totalDays` trial days. The
 * server gives `trialEndMs`; the trial began `totalDays` before it.
 */
export function trialGoalProgress({
  trialEndMs,
  nowMs,
  activeDayKeys,
  totalDays = TRIAL_DAYS,
  targetDays = TRIAL_GOAL_ACTIVE_DAYS,
}: {
  trialEndMs: number;
  nowMs: number;
  /** `YYYY-MM-DD` keys of the days with a task. */
  activeDayKeys: readonly string[];
  totalDays?: number;
  targetDays?: number;
}): TrialGoalProgress {
  const startMs = trialEndMs - totalDays * DAY_MS;
  const elapsedDays = Math.floor((nowMs - startMs) / DAY_MS);
  const day = Math.min(totalDays, Math.max(1, elapsedDays + 1));
  const daysLeft = Math.max(0, Math.ceil((trialEndMs - nowMs) / DAY_MS));
  const startKey = dayKey(startMs);
  const endKey = dayKey(trialEndMs);
  const activeDays = new Set(
    activeDayKeys.filter((key) => key >= startKey && key <= endKey),
  ).size;
  return {
    day,
    totalDays,
    daysLeft,
    activeDays,
    targetDays,
    reached: activeDays >= targetDays,
    ended: nowMs >= trialEndMs,
  };
}

export function dayKey(ms: number) {
  return new Date(ms).toISOString().slice(0, 10);
}

export function readFirstResultSeen(
  storage: ActivationStorage | null = browserStorage(),
) {
  try {
    return storage?.getItem(FIRST_RESULT_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

export function writeFirstResultSeen(
  storage: ActivationStorage | null = browserStorage(),
) {
  try {
    storage?.setItem(FIRST_RESULT_STORAGE_KEY, "1");
  } catch {
    // The in-memory flag still gates this launch.
  }
}

function browserStorage(): ActivationStorage | null {
  if (typeof window === "undefined") {
    return null;
  }
  return window.localStorage;
}

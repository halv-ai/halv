import type { BillingPlan } from "./billing";
import type { AccountAuthStatus } from "./types";

export const BILLING_ACTIVATION_POLL_INTERVAL_MS = 3_000;
export const BILLING_ACTIVATION_TIMEOUT_MS = 90_000;

export type BillingActivationPollResult =
  | { outcome: "activated"; status: AccountAuthStatus }
  | { outcome: "timedOut"; lastError: unknown }
  | { outcome: "cancelled" };

type PollForBillingActivationOptions = {
  refresh: () => Promise<AccountAuthStatus>;
  onStatus: (status: AccountAuthStatus) => void;
  expectedPlan?: BillingPlan;
  intervalMs?: number;
  timeoutMs?: number;
  now?: () => number;
  sleep?: (durationMs: number) => Promise<void>;
  cancelled?: () => boolean;
};

export function billingActivationComplete(
  status: AccountAuthStatus,
  expectedPlan?: BillingPlan,
) {
  if (status.entitlements?.canUseApp !== true) {
    return false;
  }
  if (!expectedPlan) {
    return true;
  }
  return (status.entitlements.plan ?? status.plan) === expectedPlan;
}

export async function pollForBillingActivation({
  refresh,
  onStatus,
  expectedPlan,
  intervalMs = BILLING_ACTIVATION_POLL_INTERVAL_MS,
  timeoutMs = BILLING_ACTIVATION_TIMEOUT_MS,
  now = Date.now,
  sleep = defaultSleep,
  cancelled = () => false,
}: PollForBillingActivationOptions): Promise<BillingActivationPollResult> {
  const startedAt = now();
  let lastError: unknown = null;

  while (!cancelled()) {
    if (now() - startedAt >= timeoutMs) {
      return { outcome: "timedOut", lastError };
    }

    try {
      const status = await refresh();
      if (cancelled()) {
        return { outcome: "cancelled" };
      }
      onStatus(status);
      if (billingActivationComplete(status, expectedPlan)) {
        return { outcome: "activated", status };
      }
    } catch (error) {
      lastError = error;
    }

    const elapsedMs = now() - startedAt;
    if (elapsedMs >= timeoutMs) {
      return { outcome: "timedOut", lastError };
    }
    await sleep(Math.min(intervalMs, timeoutMs - elapsedMs));
  }

  return { outcome: "cancelled" };
}

function defaultSleep(durationMs: number) {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, durationMs);
  });
}

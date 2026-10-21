import type { AccountAuthStatus } from "./types";

export type GateDecision = "verify" | "app";
export type VerificationGateReason = "missingSync" | "expiredGrace";

const DEFAULT_OFFLINE_GRACE_HOURS = 72;
const HOUR_MS = 3_600_000;

// Halv never blocks local work (ADR 0021). No account state returns a wall.
// "verify" means that the entitlements are missing or old. The app then
// refreshes them in the background, and the user keeps working.
export function gateDecision(
  accountStatus: AccountAuthStatus,
  nowMs: number,
): GateDecision {
  if (!accountStatus.signedIn) {
    return "app";
  }

  const entitlements = accountStatus.entitlements;
  if (!entitlements || !accountStatus.syncedAt) {
    return "verify";
  }

  const syncedMs = Date.parse(accountStatus.syncedAt);
  const graceHours = entitlements.offlineGraceHours ?? DEFAULT_OFFLINE_GRACE_HOURS;
  const ageMs = nowMs - syncedMs;

  return ageMs <= graceHours * HOUR_MS ? "app" : "verify";
}

export function verificationGateReason(
  accountStatus: AccountAuthStatus,
): VerificationGateReason {
  return !accountStatus.entitlements || !accountStatus.syncedAt
    ? "missingSync"
    : "expiredGrace";
}

/**
 * The account status that App knows. Components outside the App props tree
 * (for example the status bar cards) read it here. App publishes each change.
 */
export function createAccountStatusStore() {
  const listeners = new Set<() => void>();
  let status: AccountAuthStatus | null = null;
  return {
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    getSnapshot: () => status,
    publish(next: AccountAuthStatus | null) {
      if (next === status) return;
      status = next;
      for (const listener of listeners) listener();
    },
  };
}

export const accountStatusStore = createAccountStatusStore();

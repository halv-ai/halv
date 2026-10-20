import type { AccountAuthStatus, Entitlements } from "./types";

export type GateDecision = "paywall" | "verify" | "app";
export type VerificationGateReason = "missingSync" | "expiredGrace";

const DEFAULT_OFFLINE_GRACE_HOURS = 72;
const HOUR_MS = 3_600_000;

// A signed-out user always reaches the app (ADR 0018). The trial card, not a
// launch wall, asks for the account after the first result.
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

  if (entitlements.canUseApp === false) {
    return "paywall";
  }

  const syncedMs = Date.parse(accountStatus.syncedAt);
  const graceHours = entitlements.offlineGraceHours ?? DEFAULT_OFFLINE_GRACE_HOURS;
  const ageMs = nowMs - syncedMs;

  return ageMs <= graceHours * HOUR_MS ? "app" : "verify";
}

// An account with no payment history can leave the paywall and read its
// chats. An account that paid, or that owes a payment, stays on it.
export function paywallDismissable(
  entitlements: Entitlements | null | undefined,
): boolean {
  if (!entitlements) {
    return false;
  }
  return (
    entitlements.trialConfirmed !== true &&
    entitlements.paymentConfirmed !== true &&
    !entitlements.paymentIssue
  );
}

export function verificationGateReason(
  accountStatus: AccountAuthStatus,
): VerificationGateReason {
  return !accountStatus.entitlements || !accountStatus.syncedAt
    ? "missingSync"
    : "expiredGrace";
}

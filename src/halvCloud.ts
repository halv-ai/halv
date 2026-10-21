import type { AccountAuthStatus, CloudAccess, HalvPlan } from "./types.ts";

export type { CloudAccess, CloudAccessReason } from "./types.ts";

const PAID_PLANS: ReadonlySet<HalvPlan> = new Set<HalvPlan>([
  "basic",
  "unlimited",
  "lifetime",
]);

/**
 * Tells whether the account can use Halv Cloud: JEV task routing, the
 * savings MCP, account rotation and sync (ADR 0021). Local work never
 * depends on this answer.
 *
 * `granted` decides. `reason` explains it:
 * - not granted: `signed_out`, `no_plan`, `payment_issue`
 * - granted: `trial`, `paid`, `beta`
 */
export function cloudAccess(status: AccountAuthStatus | null): CloudAccess {
  if (!status?.signedIn) {
    return { granted: false, reason: "signed_out" };
  }
  const entitlements = status.entitlements;
  if (!entitlements) {
    // The entitlements did not sync yet. A cached paid plan or a cached
    // legacy beta plan counts.
    if (status.plan && PAID_PLANS.has(status.plan)) {
      return { granted: true, reason: "paid" };
    }
    return status.plan === "beta_free"
      ? { granted: true, reason: "beta" }
      : { granted: false, reason: "no_plan" };
  }
  // An unpaid subscription has no access, also when the server is late.
  if (entitlements.paymentIssue === "unpaid") {
    return { granted: false, reason: "payment_issue" };
  }
  // Legacy beta accounts of 0.8.x keep Halv Cloud. The server reports
  // `canUseApp: false` for them while billing is required, so the plan
  // decides here.
  const plan = entitlements.plan ?? status.plan;
  if (!entitlements.canUseApp) {
    if (plan === "beta_free" && !entitlements.paymentIssue) {
      return { granted: true, reason: "beta" };
    }
    return {
      granted: false,
      reason: entitlements.paymentIssue ? "payment_issue" : "no_plan",
    };
  }
  if (entitlements.trialEnd != null && entitlements.paymentConfirmed !== true) {
    return { granted: true, reason: "trial" };
  }
  if (plan && PAID_PLANS.has(plan)) {
    return { granted: true, reason: "paid" };
  }
  // The server allows the app without a paid plan: beta or no billing.
  return { granted: true, reason: "beta" };
}

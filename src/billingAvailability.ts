import type { Entitlements } from "./types";

export function canManageBilling(entitlements?: Entitlements | null): boolean {
  if (!entitlements) return false;
  if (entitlements.billingPortalUrl?.trim()) return true;
  // Older servers only expose a static portal URL, while sessions are created on demand.
  return entitlements.billingPortalAvailable ?? (
    entitlements.billingRequired && (
      entitlements.plan === "unlimited" || entitlements.plan === "basic" || Boolean(entitlements.paymentIssue)
    )
  );
}

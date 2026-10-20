import { openExternalLink } from "./externalLinks.ts";
import { ipc } from "./ipc.ts";

export type BillingPlan = "basic" | "unlimited" | "lifetime";
export type BillingInterval = "month" | "year";

export function billingCheckout(
  plan: BillingPlan,
  interval: BillingInterval,
) {
  return ipc.billing.checkout({ plan, interval });
}

export function billingCheckoutInBrowser(
  plan: BillingPlan,
  interval: BillingInterval,
) {
  return ipc.billing.checkoutBrowser({ plan, interval });
}

export function billingPrices() {
  return ipc.billing.prices();
}

export function billingPortal() {
  return ipc.billing.portal();
}

export function openBillingSupport() {
  return openExternalLink("https://halv.ai");
}

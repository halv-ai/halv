import type { Translate } from "./i18n/runtime";
import type { PaymentIssue } from "./types";

export function shouldPromptForPaymentIssue(
  issue: PaymentIssue | null | undefined,
  canUseApp: boolean,
  stale: boolean,
) {
  return issue === "past_due" && canUseApp && !stale;
}

export function paymentIssueMessage(
  issue: PaymentIssue,
  t: Translate,
) {
  return issue === "unpaid"
    ? t(
        "Your Halv subscription is unpaid. Update your payment method to restore access.",
      )
    : t(
        "We couldn't renew your Halv subscription. Update your payment method to avoid losing access.",
      );
}

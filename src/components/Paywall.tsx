import { canManageBilling } from "../billingAvailability";
import { ipc } from "../ipc.ts";
import { useState } from "react";
import { billingPortal } from "../billing";
import { paymentIssueMessage } from "../billingIssue";
import { useT, type Translate } from "../i18n/runtime";
import type { AccountAuthStatus } from "../types";
import { useBillingCheckout } from "../useBillingCheckout";
import { LifetimeOffer } from "./LifetimeOffer";
import { SubscriptionPlans } from "./SubscriptionPlans";

type PaywallProps = {
  authStatus: AccountAuthStatus;
  onBack?: () => void;
  onCheckoutOpened?: () => void;
  onStatusChange: (status: AccountAuthStatus) => void;
};

export function Paywall({
  authStatus,
  onBack,
  onCheckoutOpened,
  onStatusChange,
}: PaywallProps) {
  const t = useT();
  const [pending, setPending] = useState<
    "refresh" | "signout" | "portal" | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const entitlements = authStatus.entitlements;
  const paymentIssue = entitlements?.paymentIssue ?? null;
  const billingPortalAvailable = canManageBilling(entitlements);
  const checkout = useBillingCheckout(onStatusChange, onCheckoutOpened);

  async function refreshStatus() {
    setPending("refresh");
    setError(null);
    try {
      const status = await ipc.auth.refreshEntitlements();
      onStatusChange(status);
    } catch (refreshError) {
      setError(errorText(refreshError));
    } finally {
      setPending(null);
    }
  }

  async function signOut() {
    setPending("signout");
    setError(null);
    try {
      const status = await ipc.auth.signout();
      onStatusChange(status);
    } catch (signoutError) {
      setError(errorText(signoutError));
    } finally {
      setPending(null);
    }
  }

  async function openBillingPortal() {
    setPending("portal");
    setError(null);
    try {
      await billingPortal();
    } catch (portalError) {
      setError(errorText(portalError));
    } finally {
      setPending(null);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-surface-base px-4 py-8 text-content-primary">
      <section className="w-full max-w-4xl rounded-2xl border border-surface-border/30 bg-surface-panel p-5 sm:p-8 shadow-2xl shadow-[rgb(var(--shadow-color)/0.24)]">
        {onBack ? (
          <button
            className="mb-5 rounded-md border border-surface-border/30 px-2.5 py-1.5 text-xs text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={onBack}
            type="button"
          >
            {t("Back to chat")}
          </button>
        ) : null}
        <p className="font-mono text-xs uppercase tracking-[0.22em] text-accent">
          {t("Halv")}
        </p>
        <h1 className="mt-2 text-2xl font-semibold text-content-primary">
          {paymentIssue
            ? t("Payment needs attention")
            : t("Your next chapter. Fully equipped.")}
        </h1>
        <p className="mt-3 text-sm leading-6 text-content-secondary">
          {paymentIssue
            ? paymentIssueMessage(paymentIssue, t)
            : paywallMessage(entitlements?.message, t)}
        </p>

        <p className="mt-3 text-xs text-content-muted">{authStatus.email ?? t("Signed in")}</p>

        <div className="mt-6">
          {paymentIssue && billingPortalAvailable ? (
            <div className="rounded-md border border-warning/35 bg-warning-surface/45 px-4 py-4">
              <p className="text-sm leading-6 text-content-secondary">
                {t(
                  "Open billing to update your card, then refresh your account status here.",
                )}
              </p>
              <button
                className="mt-4 rounded-md bg-accent px-4 py-2 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
                disabled={pending !== null}
                onClick={() => void openBillingPortal()}
                type="button"
              >
                {pending === "portal"
                  ? t("Opening billing...")
                  : t("Fix payment")}
              </button>
            </div>
          ) : (
            <div className="grid items-stretch gap-4 [&:has([data-lifetime-offer])]:md:grid-cols-[1.35fr_1fr]"><LifetimeOffer account={authStatus} checkout={checkout} /><SubscriptionPlans checkout={checkout} secondary /></div>
          )}
        </div>

        <div className="mt-5 flex flex-col gap-2 border-t border-surface-border/25 pt-4 sm:flex-row">
          <button
            className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={pending !== null || checkout.phase !== "idle"}
            onClick={() => void refreshStatus()}
            type="button"
          >
            {pending === "refresh" ? t("Refreshing...") : t("Refresh status")}
          </button>
          <button
            className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={pending !== null || checkout.phase !== "idle"}
            onClick={() => void signOut()}
            type="button"
          >
            {pending === "signout" ? t("Signing out...") : t("Sign out")}
          </button>
        </div>

        {billingPortalAvailable && !paymentIssue ? (
          <div className="mt-3 text-center">
            <button
              className="text-xs text-content-muted underline decoration-surface-border underline-offset-4 transition-colors hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-50"
              disabled={pending !== null || checkout.phase !== "idle"}
              onClick={() => void openBillingPortal()}
              type="button"
            >
              {pending === "portal"
                ? t("Opening billing...")
                : t("Manage or cancel your subscription")}
            </button>
          </div>
        ) : null}

        {error ? (
          <div className="mt-5 rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-2 text-sm text-danger">
            {error}
          </div>
        ) : null}
      </section>
    </main>
  );
}

function paywallMessage(
  message: string | null | undefined,
  t: Translate,
) {
  if (message === "Choose a plan to keep using Halv.") {
    return t("All your agents. Every feature. Choose how you make it yours.");
  }
  return message ?? t("Your account needs an active subscription.");
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

// The one Halv Cloud card (ADR 0021). Halv Cloud is the paid plan: task
// routing, the halv-savings MCP server, automatic account rotation, and the
// cloud dashboard. The card is always inline. It never blocks the chat or a
// terminal. It has one button that starts the trial and one button that
// keeps going without it.
//
// Placements:
// - "pitch": once after the first result, at the top of the chat column.
// - "inline": at the point of use, under the switch of a Halv Cloud feature.
import { useEffect, useId, useState } from "react";
import { recordAnalyticsEvent } from "../analytics";
import { billingPortal } from "../billing";
import { formatPrice } from "../billingCurrency";
import type { CloudAccess } from "../halvCloud.ts";
import { useLocale, useT, type Translate } from "../i18n/runtime";
import { ipc } from "../ipc.ts";
import type { AccountAuthStatus } from "../types";
import { useBillingCheckout } from "../useBillingCheckout";
import { useBillingPrices } from "./SubscriptionPlans";

export type HalvCloudFeature = "routing" | "rotation";

type HalvCloudCardProps = {
  placement: "pitch" | "inline";
  /** The feature at the point of use. Only the "inline" placement uses it. */
  feature?: HalvCloudFeature;
  account: AccountAuthStatus | null;
  access: CloudAccess;
  /** The result line of the first task. Only the "pitch" placement uses it. */
  payoff?: string;
  onSignIn: () => void;
  onStatusChange: (status: AccountAuthStatus) => void;
  /** "Keep going without it". */
  onDismiss: () => void;
  onCheckoutOpened?: () => void;
  onAccountRefresh?: () => Promise<void>;
};

const PENDING_INVITE_CODE_KEY = "halv.pendingInviteCode";
const APPLY_INVITE_AFTER_SIGN_IN_KEY = "halv.applyInviteAfterSignIn";
const TRIAL_PLAN = "unlimited";
const TRIAL_INTERVAL = "month";

export function HalvCloudCard({
  placement,
  feature,
  account,
  access,
  payoff,
  onSignIn,
  onStatusChange,
  onDismiss,
  onCheckoutOpened,
  onAccountRefresh,
}: HalvCloudCardProps) {
  const t = useT();
  const locale = useLocale();
  const prices = useBillingPrices();
  const checkout = useBillingCheckout(onStatusChange, () => {
    void recordAnalyticsEvent("checkout_opened");
    onCheckoutOpened?.();
  });
  const [portalPending, setPortalPending] = useState(false);
  const [portalError, setPortalError] = useState<string | null>(null);
  const entitlements = account?.signedIn ? account.entitlements : null;
  const trialUsed =
    entitlements?.trialConfirmed === true ||
    entitlements?.paymentConfirmed === true;
  const monthlyPrice = prices
    ? formatPrice(prices.plans.unlimited.month, prices.currency, locale)
    : null;
  const titleId = useId();

  // The pitch records `trial_prompted` in App. The point-of-use cards record
  // it here. Native storage keeps one event for each install.
  useEffect(() => {
    if (placement === "inline" && !access.granted) {
      void recordAnalyticsEvent("trial_prompted");
    }
  }, [access.granted, placement]);

  if (access.granted) {
    return null;
  }

  async function openBillingPortal() {
    setPortalPending(true);
    setPortalError(null);
    try {
      await billingPortal();
    } catch (error) {
      setPortalError(error instanceof Error ? error.message : String(error));
    } finally {
      setPortalPending(false);
    }
  }

  function startTrial() {
    void checkout.openInBrowser(TRIAL_PLAN, TRIAL_INTERVAL);
  }

  const checkoutBusy =
    checkout.phase === "opening" || checkout.phase === "openingExternal";
  const primary =
    access.reason === "signed_out"
      ? { label: t("Sign in to start my trial"), run: onSignIn }
      : access.reason === "payment_issue"
        ? {
            label: portalPending ? t("Opening billing...") : t("Fix payment"),
            run: () => void openBillingPortal(),
          }
        : {
            label: checkoutBusy
              ? t("Opening browser…")
              : trialUsed
                ? t("Start Halv Cloud")
                : t("Start 7-day trial"),
            run: startTrial,
          };
  const error = checkout.error ?? portalError;

  return (
    <section
      aria-labelledby={titleId}
      className={
        placement === "pitch"
          ? "mx-5 mt-3 rounded-xl border border-accent/35 bg-accent/5 px-4 py-3"
          : "rounded-md border border-accent/35 bg-accent/5 px-3 py-3"
      }
      data-halv-cloud-card={placement === "pitch" ? "pitch" : feature}
    >
      <p className="font-mono text-[11px] uppercase tracking-[0.2em] text-accent">
        {t("Halv Cloud")}
      </p>
      <h3
        className="mt-1 text-sm font-semibold text-content-primary"
        id={titleId}
      >
        {cardTitle({ placement, feature, access, trialUsed }, t)}
      </h3>
      <div className="mt-1 space-y-1 text-xs leading-5 text-content-secondary">
        {placement === "pitch" && payoff ? <p>{payoff}</p> : null}
        <p>{cardLine({ placement, feature }, t)}</p>
      </div>
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          className="rounded-md bg-accent px-3 py-1.5 text-xs font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
          disabled={checkoutBusy || portalPending}
          onClick={primary.run}
          type="button"
        >
          {primary.label}
        </button>
        <button
          className="rounded-md border border-surface-border/30 px-3 py-1.5 text-xs text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
          onClick={onDismiss}
          type="button"
        >
          {t("Keep going without it")}
        </button>
      </div>
      {checkout.phase === "activating" ? (
        <p className="mt-2 text-xs text-content-muted" role="status">
          {t("Finish checkout in your browser. Halv Cloud turns on when it completes.")}
        </p>
      ) : checkout.phase === "timedOut" ? (
        <p className="mt-2 text-xs text-content-muted" role="status">
          {t("Checkout is taking longer than expected.")}{" "}
          <button
            className="underline underline-offset-4 hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={checkout.retryActivation}
            type="button"
          >
            {t("Retry activation")}
          </button>
        </p>
      ) : access.reason !== "payment_issue" && !trialUsed ? (
        <p className="mt-2 text-[11px] text-content-muted">
          {monthlyPrice
            ? t("Card required · {price}/month after 7 days · cancel anytime", {
                price: monthlyPrice,
              })
            : t("7-day free trial · card required · cancel anytime")}
        </p>
      ) : null}
      {error ? (
        <p className="mt-2 text-xs text-danger" role="alert">
          {error}
        </p>
      ) : null}
      {placement === "pitch" && access.reason !== "payment_issue" ? (
        <InviteCode
          account={account}
          onAccountRefresh={onAccountRefresh}
          onSignIn={onSignIn}
        />
      ) : null}
    </section>
  );
}

function cardTitle(
  {
    placement,
    feature,
    access,
    trialUsed,
  }: {
    placement: "pitch" | "inline";
    feature?: HalvCloudFeature;
    access: CloudAccess;
    trialUsed: boolean;
  },
  t: Translate,
) {
  if (access.reason === "payment_issue") {
    return t("Payment needs attention");
  }
  if (placement === "inline" && feature === "routing") {
    return t("Task routing is part of Halv Cloud");
  }
  if (placement === "inline" && feature === "rotation") {
    return t("Automatic rotation is part of Halv Cloud");
  }
  return trialUsed
    ? t("Turn on Halv Cloud")
    : t("Try Halv Cloud free for 7 days");
}

function cardLine(
  { placement, feature }: { placement: "pitch" | "inline"; feature?: HalvCloudFeature },
  t: Translate,
) {
  if (placement === "inline" && feature === "routing") {
    return t(
      "Your choice stays on. Until Halv Cloud starts, each task runs on the coordinator model.",
    );
  }
  if (placement === "inline" && feature === "rotation") {
    return t(
      "Your choice stays on. Until Halv Cloud starts, you switch Claude accounts yourself.",
    );
  }
  return t(
    "Everything on this computer stays free. Halv Cloud adds task routing to cheaper models, automatic account rotation, and a synced savings dashboard.",
  );
}

// An invite code from a friend extends the trial. The code waits in session
// storage while the user signs in.
function InviteCode({
  account,
  onAccountRefresh,
  onSignIn,
}: {
  account: AccountAuthStatus | null;
  onAccountRefresh?: () => Promise<void>;
  onSignIn: () => void;
}) {
  const t = useT();
  const [inviteCode, setInviteCode] = useState(readPendingInviteCode);
  const [open, setOpen] = useState(() => inviteCode.length > 0);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [appliedTrialDays, setAppliedTrialDays] = useState<number | null>(null);
  const claimedCode =
    account?.referral?.claimedCode ??
    account?.entitlements?.referral?.claimedCode ??
    null;

  useEffect(() => {
    if (
      !account?.signedIn ||
      inviteCode.length !== 8 ||
      !shouldApplyInviteAfterSignIn()
    ) {
      return;
    }
    clearApplyInviteAfterSignIn();
    void claimInviteCode(inviteCode);
  }, [account?.signedIn, inviteCode]);

  async function claimInviteCode(code: string) {
    setPending(true);
    setError(null);
    try {
      const result = await ipc.account.claimReferral({ code });
      setAppliedTrialDays(result.trialDays);
      clearPendingInviteCode();
      await onAccountRefresh?.();
    } catch (claimError) {
      setError(referralClaimError(claimError, t));
    } finally {
      setPending(false);
    }
  }

  async function applyInviteCode(event: React.FormEvent) {
    event.preventDefault();
    const code = inviteCode.trim().toUpperCase();
    if (code.length !== 8) {
      return;
    }
    if (!account?.signedIn) {
      markApplyInviteAfterSignIn();
      onSignIn();
      return;
    }
    await claimInviteCode(code);
  }

  if (appliedTrialDays !== null) {
    return (
      <p className="mt-2 text-xs text-accent">
        {t("Invite applied: {trialDays}-day trial.", {
          trialDays: appliedTrialDays,
        })}
      </p>
    );
  }
  if (claimedCode) {
    return null;
  }
  return (
    <div className="mt-2">
      <button
        aria-expanded={open}
        className="text-[11px] text-content-muted underline decoration-surface-border underline-offset-4 transition-colors hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
        onClick={() => {
          setOpen((current) => !current);
          setError(null);
        }}
        type="button"
      >
        {t("Have an invite code?")}
      </button>
      {open ? (
        <form
          className="mt-2 flex flex-wrap items-start gap-2"
          onSubmit={(event) => void applyInviteCode(event)}
        >
          <label className="min-w-0 max-w-[14rem] flex-1">
            <span className="sr-only">{t("Invite code")}</span>
            <input
              autoCapitalize="characters"
              autoComplete="off"
              className="w-full rounded-md border border-surface-border/30 bg-surface-raised px-2.5 py-1.5 font-mono text-xs uppercase tracking-[0.12em] text-content-primary outline-none placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
              disabled={pending}
              maxLength={8}
              onChange={(event) => {
                const nextCode = event.target.value
                  .trim()
                  .replace(/\s+/g, "")
                  .toUpperCase()
                  .slice(0, 8);
                setInviteCode(nextCode);
                writePendingInviteCode(nextCode);
              }}
              placeholder={t("8-character code")}
              value={inviteCode}
            />
          </label>
          <button
            className="rounded-md border border-surface-border/30 px-2.5 py-1.5 text-xs text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={pending || inviteCode.length !== 8}
            type="submit"
          >
            {pending ? t("Applying…") : t("Apply")}
          </button>
          {error ? <p className="w-full text-xs text-danger">{error}</p> : null}
        </form>
      ) : null}
    </div>
  );
}

function readPendingInviteCode() {
  try {
    return window.sessionStorage.getItem(PENDING_INVITE_CODE_KEY) ?? "";
  } catch {
    return "";
  }
}

function writePendingInviteCode(code: string) {
  try {
    if (code) {
      window.sessionStorage.setItem(PENDING_INVITE_CODE_KEY, code);
    } else {
      window.sessionStorage.removeItem(PENDING_INVITE_CODE_KEY);
    }
  } catch {
    // The invite remains available in component state.
  }
}

function markApplyInviteAfterSignIn() {
  try {
    window.sessionStorage.setItem(APPLY_INVITE_AFTER_SIGN_IN_KEY, "1");
  } catch {
    // The user can apply the retained code after sign-in.
  }
}

function shouldApplyInviteAfterSignIn() {
  try {
    return window.sessionStorage.getItem(APPLY_INVITE_AFTER_SIGN_IN_KEY) === "1";
  } catch {
    return false;
  }
}

function clearApplyInviteAfterSignIn() {
  try {
    window.sessionStorage.removeItem(APPLY_INVITE_AFTER_SIGN_IN_KEY);
  } catch {
    // Session storage is optional.
  }
}

function clearPendingInviteCode() {
  try {
    window.sessionStorage.removeItem(PENDING_INVITE_CODE_KEY);
    window.sessionStorage.removeItem(APPLY_INVITE_AFTER_SIGN_IN_KEY);
  } catch {
    // Session storage is optional.
  }
}

function referralClaimError(error: unknown, t: Translate) {
  const rawMessage = error instanceof Error ? error.message : String(error);
  const prefix = "Could not apply referral code: ";
  const message = rawMessage.startsWith(prefix)
    ? rawMessage.slice(prefix.length)
    : rawMessage;
  switch (message) {
    case "invalid referral code":
      return t("Invalid invite code.");
    case "referral code not found":
      return t("Invite code not found.");
    case "cannot claim your own code":
      return t("You cannot use your own invite code.");
    case "referral already claimed":
      return t("An invite code was already applied.");
    case "referral must be claimed before the first subscription":
      return t("Apply an invite code before your first subscription.");
    default:
      return t("Could not apply invite code.");
  }
}

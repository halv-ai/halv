import { useEffect, useState } from "react";
import type { TrialPromptFacts } from "../activation.ts";
import { formatPrice } from "../billingCurrency";
import { formatMoney, useLocale, useT, type Translate } from "../i18n/runtime";
import { ipc } from "../ipc.ts";
import type { AccountAuthStatus } from "../types";
import { useBillingPrices } from "./SubscriptionPlans";

type TrialPromptCardProps = {
  payoff: string;
  /** What this device keeps: savings and conversations (ADR 0018). */
  facts: TrialPromptFacts;
  account: AccountAuthStatus | null;
  onAccountRefresh: () => Promise<void>;
  onSignIn: () => void;
  onClose: () => void;
};

const PENDING_INVITE_CODE_KEY = "halv.pendingInviteCode";
const APPLY_INVITE_AFTER_SIGN_IN_KEY = "halv.applyInviteAfterSignIn";

export function TrialPromptCard({
  payoff,
  facts,
  account,
  onAccountRefresh,
  onSignIn,
  onClose,
}: TrialPromptCardProps) {
  const t = useT();
  const locale = useLocale();
  const prices = useBillingPrices();
  const monthlyPrice = prices
    ? formatPrice(prices.plans.unlimited.month, prices.currency, locale)
    : null;
  const [inviteCode, setInviteCode] = useState(readPendingInviteCode);
  const [inviteOpen, setInviteOpen] = useState(() => inviteCode.length > 0);
  const [invitePending, setInvitePending] = useState(false);
  const [inviteError, setInviteError] = useState<string | null>(null);
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

  async function claimInviteCode(code: string) {
    setInvitePending(true);
    setInviteError(null);
    try {
      const result = await ipc.account.claimReferral({ code });
      setAppliedTrialDays(result.trialDays);
      clearPendingInviteCode();
      await onAccountRefresh();
    } catch (error) {
      setInviteError(referralClaimError(error, t));
    } finally {
      setInvitePending(false);
    }
  }

  function closePrompt() {
    clearPendingInviteCode();
    onClose();
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 backdrop-blur-md">
      <section
        aria-labelledby="trial-prompt-title"
        aria-modal="true"
        className="relative w-full max-w-[440px] rounded-lg border border-surface-border/30 bg-surface-panel p-7 shadow-2xl shadow-[rgb(var(--shadow-color)/0.32)]"
        role="dialog"
      >
        <button
          aria-label={t("Close")}
          className="absolute right-4 top-4 rounded p-1 text-content-muted transition-colors duration-150 hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
          onClick={closePrompt}
          type="button"
        >
          <svg
            aria-hidden="true"
            className="h-4 w-4"
            fill="none"
            viewBox="0 0 24 24"
          >
            <path
              d="M6 6l12 12M18 6L6 18"
              stroke="currentColor"
              strokeLinecap="round"
              strokeWidth="1.5"
            />
          </svg>
        </button>
        <p className="font-mono text-xs uppercase tracking-[0.22em] text-accent">
          {t("Halv")}
        </p>
        <h2
          className="mt-2 text-2xl font-semibold text-content-primary"
          id="trial-prompt-title"
        >
          {t("Start your free 7-day trial")}
        </h2>
        <div className="mt-4 space-y-3 text-sm leading-6 text-content-secondary">
          <p>{payoff}</p>
          <p>{keepLine(facts, t)}</p>
        </div>
        <div className="mt-6">
          <button
            className="rounded-md bg-accent px-4 py-2.5 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40"
            onClick={account?.signedIn ? closePrompt : onSignIn}
            type="button"
          >
            {account?.signedIn
              ? t("Continue")
              : t("Sign in to start my trial")}
          </button>
        </div>
        {appliedTrialDays !== null || !claimedCode ? (
          <div className="mt-4 border-t border-surface-border/25 pt-4">
            {appliedTrialDays !== null ? (
              <p className="text-sm text-accent">
                {t("Invite applied: {trialDays}-day trial.", {
                  trialDays: appliedTrialDays,
                })}
              </p>
            ) : (
              <>
                <button
                  aria-expanded={inviteOpen}
                  className="text-xs text-content-muted underline decoration-surface-border underline-offset-4 transition-colors hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                  onClick={() => {
                    setInviteOpen((current) => !current);
                    setInviteError(null);
                  }}
                  type="button"
                >
                  {t("Have an invite code?")}
                </button>
                {inviteOpen ? (
                  <form
                    className="mt-3 flex flex-wrap items-start gap-2"
                    onSubmit={(event) => void applyInviteCode(event)}
                  >
                    <label className="min-w-0 flex-1">
                      <span className="sr-only">{t("Invite code")}</span>
                      <input
                        autoCapitalize="characters"
                        autoComplete="off"
                        className="w-full rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 font-mono text-sm uppercase tracking-[0.12em] text-content-primary outline-none placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
                        disabled={invitePending}
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
                      className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
                      disabled={invitePending || inviteCode.length !== 8}
                      type="submit"
                    >
                      {invitePending ? t("Applying\u2026") : t("Apply")}
                    </button>
                    {inviteError ? (
                      <p className="w-full text-xs text-danger">
                        {inviteError}
                      </p>
                    ) : null}
                  </form>
                ) : null}
              </>
            )}
          </div>
        ) : null}
        <div className="mt-4 text-center text-xs text-content-muted">
          <p>
            {monthlyPrice
              ? t("Card required · {price}/month after 7 days · cancel anytime", {
                  price: monthlyPrice,
                })
              : t("Card required — cancel anytime · Your chats stay on this device")}
          </p>
        </div>
      </section>
    </div>
  );
}

// Loss framing: the user keeps what this device already produced
// (ADR 0018). Dollars lead, then tokens, then the conversations.
function keepLine(facts: TrialPromptFacts, t: Translate) {
  switch (facts.kind) {
    case "dollars":
      return t(
        "Sign in to keep going with your {count} conversations and the {amount} you saved on this device.",
        { count: facts.conversations, amount: formatMoney(facts.amountUsd) },
      );
    case "tokens":
      return t(
        "Sign in to keep going with your {count} conversations and the {tokens} tokens you saved on this device.",
        {
          count: facts.conversations,
          tokens: Math.round(facts.tokens).toLocaleString(),
        },
      );
    case "conversations":
      return t(
        "Sign in to keep going with the {count} conversations on this device. Your free 7-day trial starts now.",
        { count: facts.conversations },
      );
    case "none":
      return t("Keep working on your project with a free 7-day trial.");
  }
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

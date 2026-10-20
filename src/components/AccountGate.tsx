import { ipc } from "../ipc.ts";
import { useEffect, useRef, useState } from "react";
import {
  availableAccountOAuthButtons,
  type AccountOAuthButton,
  type AccountOAuthProvider,
} from "../accountOAuth";
import { requestSignInCode } from "../emailSignIn.ts";
import { useT, type Translate } from "../i18n/runtime";
import type { AccountAuthStatus } from "../types";

type AccountGateProps = {
  authStatus: AccountAuthStatus;
  onBack?: () => void;
  onStatusChange: (status: AccountAuthStatus) => void;
  reason?: "trialRequired";
};

const OAUTH_LOGIN_TIMEOUT_MS = 90_000;

export function AccountGate({
  authStatus,
  onBack,
  onStatusChange,
  reason,
}: AccountGateProps) {
  const t = useT();
  const [email, setEmail] = useState(authStatus.email ?? "");
  const [code, setCode] = useState("");
  const [codeSent, setCodeSent] = useState(false);
  const [devCode, setDevCode] = useState<string | null>(null);
  const [oauthButtons, setOauthButtons] = useState<AccountOAuthButton[]>([]);
  const [pending, setPending] = useState<
    "request" | "challenge" | "verify" | AccountOAuthProvider | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const oauthAttemptRef = useRef(0);
  const oauthTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    let active = true;

    void ipc.auth.oauthProviders()
      .then((providers) => {
        if (active) {
          setOauthButtons(availableAccountOAuthButtons(providers));
        }
      })
      .catch(() => {
        if (active) {
          setOauthButtons([]);
        }
      });

    return () => {
      active = false;
      oauthAttemptRef.current += 1;
      clearOauthTimeout();
    };
  }, []);

  async function oauthLogin(provider: AccountOAuthProvider) {
    const attemptId = oauthAttemptRef.current + 1;
    oauthAttemptRef.current = attemptId;
    clearOauthTimeout();
    setPending(provider);
    setError(null);

    oauthTimeoutRef.current = setTimeout(() => {
      if (oauthAttemptRef.current !== attemptId) {
        return;
      }

      oauthAttemptRef.current += 1;
      oauthTimeoutRef.current = null;
      void ipc.auth.oauthCancel().catch(() => undefined);
      setPending(null);
      setError(t("Sign-in timed out — please try again."));
    }, OAUTH_LOGIN_TIMEOUT_MS);

    try {
      const status = await ipc.auth.oauthLogin({
        provider,
      });
      if (oauthAttemptRef.current !== attemptId) {
        return;
      }
      clearOauthTimeout();
      onStatusChange(status);
    } catch {
      if (oauthAttemptRef.current !== attemptId) {
        return;
      }
      clearOauthTimeout();
      setError(t("Sign-in wasn’t completed — please try again."));
    } finally {
      if (oauthAttemptRef.current === attemptId) {
        clearOauthTimeout();
        setPending(null);
      }
    }
  }

  function cancelOauthLogin() {
    if (pending !== "google" && pending !== "apple") {
      return;
    }

    oauthAttemptRef.current += 1;
    clearOauthTimeout();
    void ipc.auth.oauthCancel().catch(() => undefined);
    setPending(null);
    setError(null);
  }

  function clearOauthTimeout() {
    if (oauthTimeoutRef.current !== null) {
      clearTimeout(oauthTimeoutRef.current);
      oauthTimeoutRef.current = null;
    }
  }

  async function requestCode(event: React.FormEvent) {
    event.preventDefault();
    const trimmedEmail = email.trim();
    if (!trimmedEmail) {
      return;
    }
    setPending("request");
    setError(null);
    setDevCode(null);
    try {
      const outcome = await requestSignInCode(ipc.auth, trimmedEmail, () =>
        setPending("challenge"),
      );
      if (outcome.status === "challengeIncomplete") {
        setCodeSent(false);
        setError(t("The security check wasn’t completed — please try again."));
        return;
      }
      setCodeSent(outcome.result.ok);
      setDevCode(outcome.result.devCode ?? null);
      setCode("");
    } catch (requestError) {
      setCodeSent(false);
      setError(errorText(requestError));
    } finally {
      setPending(null);
    }
  }

  async function verifyCode(event: React.FormEvent) {
    event.preventDefault();
    const trimmedEmail = email.trim();
    if (!trimmedEmail || code.length !== 6) {
      return;
    }
    setPending("verify");
    setError(null);
    try {
      const status = await ipc.auth.verify({
        email: trimmedEmail,
        code,
      });
      onStatusChange(status);
    } catch (verifyError) {
      setError(errorText(verifyError));
    } finally {
      setPending(null);
    }
  }

  const pendingOauthProvider =
    pending === "google" || pending === "apple" ? pending : null;

  return (
    <main className="flex min-h-screen items-center justify-center bg-surface-base px-4 py-8 text-content-primary">
      <section className="w-full max-w-md rounded-lg border border-surface-border/30 bg-surface-panel p-6 shadow-2xl shadow-[rgb(var(--shadow-color)/0.24)]">
        {onBack ? (
          <button
            className="mb-5 rounded-md border border-surface-border/30 px-2.5 py-1.5 text-xs text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={onBack}
            type="button"
          >
            {t("Back to chat")}
          </button>
        ) : null}
        <div className="mb-6">
          <p className="font-mono text-xs uppercase tracking-[0.22em] text-accent">
            {t("Halv")}
          </p>
          <h1 className="mt-2 text-2xl font-semibold text-content-primary">
            {reason === "trialRequired"
              ? t("Start your free 7-day trial")
              : t("Sign in")}
          </h1>
          {reason === "trialRequired" ? (
            <p className="mt-2 whitespace-pre-line text-sm leading-6 text-content-secondary">
              {t(
                "Sign in to start your trial — 7 days free.\nYour chats stay on this device.",
              )}
            </p>
          ) : null}
        </div>

        {oauthButtons.length > 0 ? (
          <>
            <div aria-label={t("Sign in providers")} className="space-y-3">
              {oauthButtons.map((button) => (
                <button
                  className="w-full rounded-md border border-surface-border/40 bg-surface-raised px-3 py-2 text-sm font-semibold text-content-primary transition-colors duration-150 hover:border-surface-border/70 hover:bg-surface-panel focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60"
                  disabled={pending !== null}
                  key={button.provider}
                  onClick={() => void oauthLogin(button.provider)}
                  type="button"
                >
                  {pending === button.provider
                    ? t("Waiting for {provider}...", {
                        provider: providerName(button.provider, t),
                      })
                    : providerButtonLabel(button.provider, t)}
                </button>
              ))}
            </div>

            {pendingOauthProvider ? (
              <div className="mt-3 text-center">
                <button
                  aria-label={t("Cancel {provider} sign-in", {
                    provider: providerName(pendingOauthProvider, t),
                  })}
                  className="text-sm font-medium text-content-secondary underline decoration-surface-border underline-offset-4 transition-colors duration-150 hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                  onClick={cancelOauthLogin}
                  type="button"
                >
                  {t("Cancel")}
                </button>
              </div>
            ) : null}

            <div className="my-5 flex items-center gap-3 text-content-muted">
              <span className="h-px flex-1 bg-surface-border/30" />
              <span className="text-xs uppercase tracking-[0.16em]">
                {t("or")}
              </span>
              <span className="h-px flex-1 bg-surface-border/30" />
            </div>
          </>
        ) : null}

        <form className="space-y-4" onSubmit={requestCode}>
          <label className="block space-y-2">
            <span className="text-sm font-medium text-content-secondary">
              {t("Email")}
            </span>
            <input
              autoComplete="email"
              className="w-full rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none transition-colors duration-150 placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
              disabled={pending !== null}
              onChange={(event) => setEmail(event.target.value)}
              placeholder="you@example.com"
              type="email"
              value={email}
            />
          </label>

          <button
            className="w-full rounded-md bg-accent px-3 py-2 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
            disabled={pending !== null || email.trim().length === 0}
            type="submit"
          >
            {pending === "request" || pending === "challenge"
              ? t("Sending...")
              : t("Send code")}
          </button>
          {pending === "challenge" ? (
            <p className="text-center text-sm text-content-secondary">
              {t("Complete the security check in the window that opened.")}
            </p>
          ) : null}
        </form>

        {codeSent ? (
          <form className="mt-5 space-y-4" onSubmit={verifyCode}>
            {devCode ? (
              <div className="rounded-md border border-accent/35 bg-accent/10 px-3 py-2 font-mono text-sm text-accent">
                {t("Dev code: {code}", { code: devCode })}
              </div>
            ) : null}
            <label className="block space-y-2">
              <span className="text-sm font-medium text-content-secondary">
                {t("Code")}
              </span>
              <input
                autoComplete="one-time-code"
                className="w-full rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-center font-mono text-lg tracking-[0.22em] text-content-primary outline-none transition-colors duration-150 placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
                disabled={pending !== null}
                inputMode="numeric"
                maxLength={6}
                onChange={(event) =>
                  setCode(event.target.value.replace(/\D/g, "").slice(0, 6))
                }
                pattern="\d{6}"
                placeholder="000000"
                value={code}
              />
            </label>
            <button
              className="w-full rounded-md bg-content-primary px-3 py-2 text-sm font-semibold text-surface-base transition-colors duration-150 hover:bg-content-secondary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60"
              disabled={pending !== null || code.length !== 6}
              type="submit"
            >
              {pending === "verify" ? t("Verifying...") : t("Verify")}
            </button>
          </form>
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

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function providerName(provider: AccountOAuthProvider, t: Translate) {
  return provider === "google" ? t("Google") : t("Apple");
}

function providerButtonLabel(provider: AccountOAuthProvider, t: Translate) {
  return provider === "google"
    ? t("Continue with Google")
    : t("Continue with Apple");
}

import { ipc } from "../ipc.ts";
import { useEffect, useRef, useState } from "react";
import type { VerificationGateReason } from "../gate";
import { useT, type Translate } from "../i18n/runtime";
import type { AccountAuthStatus, HalvPlan } from "../types";

type VerificationGateProps = {
  authStatus: AccountAuthStatus;
  reason: VerificationGateReason;
  onStatusChange: (status: AccountAuthStatus) => void;
};

export function VerificationGate({
  authStatus,
  reason,
  onStatusChange,
}: VerificationGateProps) {
  const t = useT();
  const [pending, setPending] = useState<"refresh" | "signout" | null>(null);
  const [autoRefreshing, setAutoRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  const refreshInFlightRef = useRef(false);
  const onStatusChangeRef = useRef(onStatusChange);
  const copy = verificationGateCopy(reason, t);

  useEffect(() => {
    onStatusChangeRef.current = onStatusChange;
  }, [onStatusChange]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  async function refreshStatus(source: "manual" | "auto") {
    if (refreshInFlightRef.current) {
      return;
    }

    refreshInFlightRef.current = true;
    if (source === "manual") {
      setPending("refresh");
    } else {
      setAutoRefreshing(true);
    }
    setError(null);

    try {
      const status = await ipc.auth.refreshEntitlements();
      if (mountedRef.current) {
        onStatusChangeRef.current(status);
      }
    } catch (refreshError) {
      if (mountedRef.current) {
        setError(errorText(refreshError));
      }
    } finally {
      refreshInFlightRef.current = false;
      if (mountedRef.current) {
        if (source === "manual") {
          setPending(null);
        } else {
          setAutoRefreshing(false);
        }
      }
    }
  }

  async function signOut() {
    setPending("signout");
    setError(null);
    try {
      const status = await ipc.auth.signout();
      if (mountedRef.current) {
        onStatusChangeRef.current(status);
      }
    } catch (signoutError) {
      if (mountedRef.current) {
        setError(errorText(signoutError));
      }
    } finally {
      if (mountedRef.current) {
        setPending(null);
      }
    }
  }

  useEffect(() => {
    void refreshStatus("auto");
    const interval = window.setInterval(() => void refreshStatus("auto"), 30_000);
    return () => window.clearInterval(interval);
  }, []);

  return (
    <main className="flex min-h-screen items-center justify-center bg-surface-base px-4 py-8 text-content-primary">
      <section className="w-full max-w-lg rounded-lg border border-surface-border/30 bg-surface-panel p-6 shadow-2xl shadow-[rgb(var(--shadow-color)/0.24)]">
        <p className="font-mono text-xs uppercase tracking-[0.22em] text-accent">
          {t("Halv")}
        </p>
        <h1 className="mt-2 text-2xl font-semibold text-content-primary">
          {copy.title}
        </h1>
        <p className="mt-3 text-sm leading-6 text-content-secondary">
          {copy.message}
        </p>

        <div className="mt-5 rounded-md border border-surface-border/30 bg-surface-raised/50 px-3 py-2 text-sm text-content-secondary">
          <span className="font-mono text-xs uppercase tracking-[0.16em] text-content-muted">
            {t("Account")}
          </span>
          <p className="mt-1 text-content-primary">
            {authStatus.email ?? t("Signed in")}
          </p>
          <p className="mt-1 text-xs text-content-muted">
            {accountPlanSummary(
              authStatus.plan ?? authStatus.entitlements?.plan,
              t,
            )}
          </p>
        </div>

        <div className="mt-6 flex flex-col gap-2 sm:flex-row">
          <button
            className="rounded-md bg-accent px-3 py-2 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
            disabled={pending !== null || autoRefreshing}
            onClick={() => void refreshStatus("manual")}
            type="button"
          >
            {pending === "refresh" || autoRefreshing
              ? t("Retrying...")
              : t("Retry")}
          </button>
          <button
            className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={pending !== null}
            onClick={() => void signOut()}
            type="button"
          >
            {pending === "signout" ? t("Signing out...") : t("Sign out")}
          </button>
        </div>

        {error ? (
          <div className="mt-5 rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-2 text-sm text-danger">
            {error}
          </div>
        ) : null}
      </section>
    </main>
  );
}

function verificationGateCopy(reason: VerificationGateReason, t: Translate) {
  switch (reason) {
    case "missingSync":
      return {
        title: t("Verifying your subscription..."),
        message: t(
          "Halv needs a successful subscription check before the app can be used on this device.",
        ),
      };
    case "expiredGrace":
      return {
        title: t("Can't verify your subscription — reconnect"),
        message: t(
          "Reconnect to the Halv server and retry. The cached subscription check is past its offline grace window.",
        ),
      };
  }
}

function accountPlanSummary(
  plan: HalvPlan | null | undefined,
  t: Translate,
) {
  switch (plan) {
    case "beta_free":
      return t("Plan Beta Free");
    case "basic":
      return t("Plan Basic");
    case "lifetime":
      return t("Lifetime membership");
    case "unlimited":
      return t("Plan Unlimited");
    default:
      return t("Plan Unknown");
  }
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

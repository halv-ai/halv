// The assisted account switch near a usage limit (see accountSwap.ts). The
// card sits above the provider pills of the status bar. One click switches
// through the normal select path. The card never switches without that
// click. The opt-in automatic rotation of Claude accounts (ADR 0017) shows
// its result in a card at the same place.
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { assistedSwapInput } from "../accountAutoRotation";
import {
  ACCOUNT_SWAP_NOTICE_PERCENT,
  accountSwapSetting,
  accountSwapTarget,
  INITIAL_ACCOUNT_SWAP_WATCH,
  nextAccountSwapWatch,
  type AccountSwapTarget,
  type AccountSwapWatch,
} from "../accountSwap";
import { accountRestartStore } from "../accountRestart";
import {
  activeWalletLabel,
  WALLET_PROVIDERS,
  walletProviderName,
  walletStore,
  type WalletProvider,
} from "../accountWallet";
import { formatNumber, useT } from "../i18n/runtime";
import { ipc } from "../ipc.ts";
import type { ProviderPlanWindowGroup } from "../providerPlanWindows";
import { statusProviderUsage } from "../statusBar";
import {
  useAccountAutoRotation,
  type AutoRotationNotice,
} from "../useAccountAutoRotation";
import { errorText, useWalletSnapshot } from "./AccountWallet";

type SwapNotice = {
  provider: WalletProvider;
  accountKey: string;
  usedPercent: number;
  resetLabel: string;
  target: AccountSwapTarget;
};

export function AccountSwapNotice({
  groups,
  nowMs,
  walletVersions,
}: {
  groups: readonly ProviderPlanWindowGroup[];
  nowMs: number;
  walletVersions: Record<WalletProvider, number | null>;
}) {
  const t = useT();
  const wallet = useWalletSnapshot();
  const enabled = useSyncExternalStore(
    accountSwapSetting.subscribe,
    accountSwapSetting.getSnapshot,
    accountSwapSetting.getSnapshot,
  );
  const watches = useRef<Record<WalletProvider, AccountSwapWatch>>({
    claude: INITIAL_ACCOUNT_SWAP_WATCH,
    codex: INITIAL_ACCOUNT_SWAP_WATCH,
  });
  const [notices, setNotices] = useState<SwapNotice[]>([]);
  const [switching, setSwitching] = useState<WalletProvider | null>(null);
  const [errors, setErrors] = useState<Partial<Record<WalletProvider, string>>>({});

  // Only usage that belongs to the active account counts. Right after a
  // switch, the shown windows are those of the old account.
  const readings = WALLET_PROVIDERS.map((provider) => {
    const group = groups.find((candidate) => candidate.id === provider);
    const current = wallet.loaded && walletVersions[provider] === wallet.version;
    const window =
      group && current
        ? statusProviderUsage(group, nowMs, { week: t("wk") }).tightest
        : null;
    return {
      provider,
      accountKey: wallet.state[provider].activeId ?? "system",
      usedPercent: window?.usedPercent ?? null,
      resetLabel: window?.resetLabel ?? "",
    };
  });
  const claudeReading = readings.find((reading) => reading.provider === "claude")!;
  const autoRotation = useAccountAutoRotation({
    reading: claudeReading,
    windows: groups.find((group) => group.id === "claude")?.windows ?? [],
    loaded: wallet.loaded,
    nowMs,
  });
  const autoEnabled = autoRotation.settings.enabled;
  // With automatic rotation on, the Claude card shows only when the
  // rotation found no target.
  const assisted = readings.map((reading) => {
    const input =
      reading.provider === "claude"
        ? assistedSwapInput(reading, {
            enabled: autoEnabled,
            fallback: autoRotation.fallback,
            startPercent: autoRotation.settings.startPercent,
          })
        : { reading, threshold: ACCOUNT_SWAP_NOTICE_PERCENT };
    return { ...reading, ...input.reading, threshold: input.threshold };
  });
  const readingKey = JSON.stringify(
    assisted.map(({ provider, accountKey, usedPercent, threshold }) => [
      provider,
      accountKey,
      usedPercent,
      threshold,
    ]),
  );
  const readingsRef = useRef(assisted);
  readingsRef.current = assisted;

  // The rotation takes over the Claude card, or gives it back. This effect
  // runs before the reading effect, so the new watch takes the reading.
  const autoEnabledRef = useRef(autoEnabled);
  useEffect(() => {
    if (autoEnabledRef.current === autoEnabled) return;
    autoEnabledRef.current = autoEnabled;
    setNotices((current) => current.filter((notice) => notice.provider !== "claude"));
    watches.current = { ...watches.current, claude: INITIAL_ACCOUNT_SWAP_WATCH };
  }, [autoEnabled]);

  useEffect(() => {
    // A notice that is off never uses up a crossing. Turning it on while
    // the use stays high shows the notice.
    if (!wallet.loaded || !enabled) return;
    for (const reading of readingsRef.current) {
      const { watch, show, hide } = nextAccountSwapWatch(
        watches.current[reading.provider],
        reading,
        reading.threshold,
      );
      watches.current[reading.provider] = watch;
      if (hide) {
        setNotices((current) =>
          current.filter((notice) => notice.provider !== reading.provider),
        );
      }
      if (!show || reading.usedPercent === null) continue;
      const { provider, accountKey, usedPercent, resetLabel } = reading;
      void loadTarget(provider, usedPercent).then((target) => {
        // The active account or its use can change while the usage loads.
        const watch = watches.current[provider];
        if (!target || watch.accountKey !== accountKey || watch.armed) return;
        setErrors((current) => ({ ...current, [provider]: undefined }));
        setNotices((current) => [
          ...current.filter((notice) => notice.provider !== provider),
          { provider, accountKey, usedPercent, resetLabel, target },
        ]);
      });
    }
  }, [enabled, readingKey, wallet.loaded]);

  useEffect(() => {
    if (enabled) return;
    setNotices([]);
    watches.current = {
      claude: INITIAL_ACCOUNT_SWAP_WATCH,
      codex: INITIAL_ACCOUNT_SWAP_WATCH,
    };
  }, [enabled]);

  async function loadTarget(provider: WalletProvider, activeUsedPercent: number) {
    const state = walletStore.getSnapshot().state;
    // Only Orca knows the usage of the accounts that are not active.
    const usage =
      state.store === "orca" ? await ipc.wallet.usage().catch(() => []) : [];
    return accountSwapTarget({
      state: state[provider],
      provider,
      usage,
      activeUsedPercent,
      nowMs: Date.now(),
    });
  }

  async function switchAccount(notice: SwapNotice) {
    if (switching) return;
    setSwitching(notice.provider);
    setErrors((current) => ({ ...current, [notice.provider]: undefined }));
    try {
      const result = await walletStore.select(notice.provider, notice.target.account.id);
      accountRestartStore.apply(result.staleSessions);
      setNotices((current) => current.filter((entry) => entry !== notice));
    } catch (cause) {
      setErrors((current) => ({ ...current, [notice.provider]: errorText(cause) }));
    } finally {
      setSwitching(null);
    }
  }

  if (notices.length === 0 && !autoRotation.notice) return null;
  return (
    <div className="pointer-events-none fixed bottom-[36px] left-[12px] z-40 flex w-[340px] max-w-[calc(100vw-24px)] select-text flex-col gap-2 whitespace-normal">
      {autoRotation.notice ? (
        <AutoRotationCard notice={autoRotation.notice} onDismiss={autoRotation.dismiss} />
      ) : null}
      {notices.map((notice) => {
        const provider = walletProviderName(notice.provider);
        const error = errors[notice.provider];
        const busy = switching === notice.provider;
        return (
          <section
            aria-label={t("{provider} usage limit", { provider })}
            className="pointer-events-auto overflow-hidden rounded-lg border border-surface-border/30 bg-surface-panel text-content-primary shadow-2xl shadow-black/25"
            data-account-swap-notice={notice.provider}
            key={notice.provider}
            onKeyDown={(event) => {
              if (event.key !== "Escape") return;
              event.stopPropagation();
              setNotices((current) => current.filter((entry) => entry !== notice));
            }}
            role="region"
          >
            <div className="flex items-start gap-2.5 px-3.5 pb-3 pt-3">
              <span
                aria-hidden
                className="mt-0.5 flex h-5 w-5 flex-none items-center justify-center rounded-full bg-warning/15 text-warning"
              >
                <svg fill="none" height="12" viewBox="0 0 12 12" width="12">
                  <path d="M6 3v3.5M6 8.6v.1" stroke="currentColor" strokeLinecap="round" strokeWidth="1.6" />
                </svg>
              </span>
              <div aria-live="polite" className="min-w-0 flex-1" role="status">
                <p className="text-[13px] font-semibold leading-5 text-content-primary">
                  {t("{provider} usage is at {percent}%", {
                    provider,
                    percent: formatNumber(notice.usedPercent),
                  })}
                </p>
                <p className="mt-0.5 truncate text-xs leading-5 text-content-secondary">
                  {t("{account} · resets in {reset}", {
                    account: activeWalletLabel(wallet.state[notice.provider], t),
                    reset:
                      readings.find((reading) => reading.provider === notice.provider)
                        ?.resetLabel || notice.resetLabel,
                  })}
                </p>
                <p className="mt-1 text-xs leading-5 text-content-secondary">
                  {notice.target.maxUsedPercent === null
                    ? t("Switch to {account}? Halv does not know its usage yet.", {
                        account: notice.target.account.email,
                      })
                    : t("Switch to {account}? It has used {percent}% of its limits.", {
                        account: notice.target.account.email,
                        percent: formatNumber(notice.target.maxUsedPercent),
                      })}
                </p>
                {error ? (
                  <p className="mt-1 text-xs leading-5 text-danger" role="alert">
                    {error}
                  </p>
                ) : null}
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-2 border-t border-surface-border/20 bg-surface-sunken/40 px-3.5 py-2.5">
              <button
                className="rounded-md bg-accent px-2.5 py-1 text-xs font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
                disabled={switching !== null}
                onClick={() => void switchAccount(notice)}
                type="button"
              >
                {busy ? t("Switching…") : t("Switch account")}
              </button>
              <button
                className="rounded-md px-2.5 py-1 text-xs font-medium text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
                disabled={busy}
                onClick={() =>
                  setNotices((current) => current.filter((entry) => entry !== notice))
                }
                type="button"
              >
                {t("Later")}
              </button>
            </div>
          </section>
        );
      })}
    </div>
  );
}

/** The result of an automatic Claude switch. It never takes the focus. */
function AutoRotationCard({
  notice,
  onDismiss,
}: {
  notice: AutoRotationNotice;
  onDismiss: () => void;
}) {
  const t = useT();
  const failed = notice.kind === "failed";
  return (
    <section
      aria-label={t("Automatic account switch")}
      className="pointer-events-auto overflow-hidden rounded-lg border border-surface-border/30 bg-surface-panel text-content-primary shadow-2xl shadow-black/25"
      data-account-auto-rotation={notice.kind}
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        event.stopPropagation();
        onDismiss();
      }}
      role="region"
    >
      <div className="flex items-start gap-2.5 px-3.5 pb-3 pt-3">
        <span
          aria-hidden
          className={`mt-0.5 flex h-5 w-5 flex-none items-center justify-center rounded-full ${
            failed ? "bg-danger/15 text-danger" : "bg-accent/15 text-accent"
          }`}
        >
          {failed ? (
            <svg fill="none" height="12" viewBox="0 0 12 12" width="12">
              <path d="M6 3v3.5M6 8.6v.1" stroke="currentColor" strokeLinecap="round" strokeWidth="1.6" />
            </svg>
          ) : (
            <svg fill="none" height="12" viewBox="0 0 12 12" width="12">
              <path
                d="M2.5 4.5h6.5M7.5 2.5l2 2-2 2M9.5 7.5H3M4.5 5.5l-2 2 2 2"
                stroke="currentColor"
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth="1.3"
              />
            </svg>
          )}
        </span>
        <div aria-live="polite" className="min-w-0 flex-1" role="status">
          {notice.kind === "switched" ? (
            <>
              <p className="break-words text-[13px] font-semibold leading-5 text-content-primary">
                {t("Switched Claude to {account}", { account: notice.to })}
              </p>
              <p className="mt-0.5 text-xs leading-5 text-content-secondary">
                {t("{account} reached {percent}% of its limit.", {
                  account: notice.from,
                  percent: formatNumber(notice.usedPercent),
                })}
              </p>
            </>
          ) : (
            <>
              <p className="text-[13px] font-semibold leading-5 text-content-primary">
                {t("Halv could not switch Claude accounts")}
              </p>
              <p className="mt-0.5 text-xs leading-5 text-danger" role="alert">
                {notice.error}
              </p>
            </>
          )}
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2 border-t border-surface-border/20 bg-surface-sunken/40 px-3.5 py-2.5">
        <button
          className="rounded-md px-2.5 py-1 text-xs font-medium text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
          onClick={onDismiss}
          type="button"
        >
          {t("Dismiss")}
        </button>
      </div>
    </section>
  );
}

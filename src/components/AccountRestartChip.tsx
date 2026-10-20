// The account prompt of one agent pane after an account switch. It adapts
// Orca's CodexRestartChip (stablyai/orca, MIT), but it never blocks input:
// Halv restarts a pane only while the pane rests between turns.
import { useEffect, useSyncExternalStore } from "react";
import { accountRestartStore, accountRestartSummary } from "../accountRestart";
import {
  walletAccountLabel,
  walletProviderName,
  type WalletProvider,
} from "../accountWallet";
import { useT } from "../i18n/runtime";
import { useWalletSnapshot } from "./AccountWallet";

export function AccountRestartChip({ sessionId }: { sessionId: string }) {
  const t = useT();
  const entry = useSyncExternalStore(
    accountRestartStore.subscribe,
    () => accountRestartStore.getSnapshot().entries[sessionId],
  );
  const wallet = useWalletSnapshot();
  useEffect(() => accountRestartStore.ensureStarted(), []);
  if (!entry) return null;

  const providerState = wallet.state[entry.provider];
  const provider = walletProviderName(entry.provider);
  const previous = walletAccountLabel(providerState, entry.previousAccountId, t);
  const next = walletAccountLabel(providerState, entry.nextAccountId, t);
  const body =
    entry.phase === "queued"
      ? t("Restarts on {account} when the current turn ends.", { account: next })
      : entry.phase === "restarting"
        ? t("Restarting on {account}…", { account: next })
        : entry.phase === "failed" && entry.error
          ? entry.error
          : t("Restart this session to use {account}. It resumes the same conversation.", {
              account: next,
            });

  return (
    <section
      aria-label={t("Account switched")}
      className="terminal-account-chip"
      data-account-restart={entry.phase}
    >
      <div className="terminal-account-chip-eyebrow">
        <RestartIcon spinning={entry.phase === "restarting"} />
        {t("Account switched")}
      </div>
      <p className="terminal-account-chip-title">
        {t("{provider} is still signed in as {account}", {
          provider,
          account: previous,
        })}
      </p>
      <p
        className="terminal-account-chip-body"
        role={entry.phase === "failed" ? "alert" : "status"}
      >
        {body}
      </p>
      {entry.phase === "restarting" ? null : (
        <div className="terminal-account-chip-actions">
          <button
            onClick={() => accountRestartStore.keep([sessionId])}
            type="button"
          >
            {t("Keep old account")}
          </button>
          {entry.phase === "queued" ? null : (
            <button
              data-primary
              onClick={() => accountRestartStore.queue([sessionId])}
              type="button"
            >
              {entry.phase === "failed" ? t("Try again") : t("Restart")}
            </button>
          )}
        </div>
      )}
    </section>
  );
}

/**
 * The status bar prompt for the panes of one provider. It adapts Orca's
 * CodexRestartStatusPrompt: one click restarts every pane that asks.
 */
export function AccountRestartPrompt({ provider }: { provider: WalletProvider }) {
  const t = useT();
  const entries = useSyncExternalStore(
    accountRestartStore.subscribe,
    () => accountRestartStore.getSnapshot().entries,
  );
  useEffect(() => accountRestartStore.ensureStarted(), []);
  const { askIds, pendingCount } = accountRestartSummary(entries, provider);
  const count = askIds.length + pendingCount;
  if (count === 0) return null;
  const name = walletProviderName(provider);
  return (
    <div
      className="border-t border-surface-border/25 px-3 py-2.5"
      data-account-restart-prompt={provider}
    >
      <p className="text-[11px] leading-4 text-content-secondary">
        {count === 1
          ? t("1 {provider} session is still on the old account.", { provider: name })
          : t("{count} {provider} sessions are still on the old account.", {
              count,
              provider: name,
            })}
      </p>
      <p className="mt-0.5 text-[10px] leading-4 text-content-muted">
        {t("Each session restarts when its current turn ends.")}
      </p>
      {askIds.length > 0 ? (
        <button
          className="mt-2 inline-flex w-full items-center justify-center rounded-md border border-surface-border/50 px-2.5 py-1.5 text-xs font-medium text-content-primary transition-colors hover:bg-surface-raised focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
          onClick={() => accountRestartStore.queue(askIds)}
          type="button"
        >
          {askIds.length === 1
            ? t("Restart session")
            : t("Restart {count} sessions", { count: askIds.length })}
        </button>
      ) : null}
    </div>
  );
}

function RestartIcon({ spinning }: { spinning: boolean }) {
  return (
    <svg
      aria-hidden
      className={spinning ? "animate-spin" : undefined}
      fill="none"
      height={10}
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2.2"
      viewBox="0 0 24 24"
      width={10}
    >
      <path d="M21 12a9 9 0 0 1-15.5 6.2M3 12A9 9 0 0 1 18.5 5.8" />
      <path d="M21 3v6h-6M3 21v-6h6" />
    </svg>
  );
}

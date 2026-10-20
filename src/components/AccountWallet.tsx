import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  formatWalletPlan,
  walletAccountDetail,
  walletAccountProblem,
  walletProviderName,
  walletStore,
  walletSwitchNotice,
  walletSyncedWithOrca,
  type WalletAccount,
  type WalletProvider,
} from "../accountWallet";
import { accountRestartStore } from "../accountRestart";
import { confirmDialog } from "../dialogs";
import { useT } from "../i18n/runtime";

const SYSTEM_DEFAULT_KEY = "system-default";
const NOTICE_TIMEOUT_MS = 9_000;

export function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

export function useWalletSnapshot() {
  const snapshot = useSyncExternalStore(
    walletStore.subscribe,
    walletStore.getSnapshot,
    walletStore.getSnapshot,
  );
  useEffect(() => {
    walletStore.ensureLoaded();
  }, []);
  return snapshot;
}

/** Shows a message for a few seconds. A new message restarts the timer. */
export function useTransientNotice() {
  const [notice, setNotice] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  function show(message: string | null) {
    if (timer.current) clearTimeout(timer.current);
    setNotice(message);
    timer.current = message
      ? setTimeout(() => setNotice(null), NOTICE_TIMEOUT_MS)
      : null;
  }
  return [notice, show] as const;
}

type AccountWalletListProps = {
  provider: WalletProvider;
  disabled: boolean;
  onAdd: (provider: WalletProvider) => void;
};

/** The account list of one provider in Settings > Connections. */
export function AccountWalletList({
  provider,
  disabled,
  onAdd,
}: AccountWalletListProps) {
  const t = useT();
  const { state, loaded } = useWalletSnapshot();
  const providerState = state[provider];
  const [pendingKey, setPendingKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, showNotice] = useTransientNotice();
  const name = walletProviderName(provider);
  const busy = disabled || pendingKey !== null || !loaded;

  // Orca can change the accounts while Settings is closed.
  useEffect(() => {
    void walletStore.refresh().catch(() => undefined);
  }, []);

  async function choose(accountId: string | null) {
    if (accountId === providerState.activeId) return;
    setPendingKey(accountId ?? SYSTEM_DEFAULT_KEY);
    setError(null);
    showNotice(null);
    try {
      const result = await walletStore.select(provider, accountId);
      accountRestartStore.apply(result.staleSessions);
      showNotice(walletSwitchNotice(provider, result.staleSessions, t));
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setPendingKey(null);
    }
  }

  async function remove(account: WalletAccount) {
    const confirmed = await confirmDialog({
      title: t("Remove account"),
      message: t(
        "Remove {email} from Halv? Halv deletes its sign-in on this computer. Open sessions that use this account stop working.",
        { email: account.email },
      ),
      confirmLabel: t("Remove"),
      danger: true,
    });
    if (!confirmed) return;
    setPendingKey(account.id);
    setError(null);
    showNotice(null);
    try {
      await walletStore.remove(provider, account.id);
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setPendingKey(null);
    }
  }

  const systemDetail = [
    providerState.systemDefault.email,
    formatWalletPlan(providerState.systemDefault.plan),
  ]
    .filter((part): part is string => Boolean(part))
    .join(" · ");

  return (
    <div className="mt-4 space-y-2">
      <div className="font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
        {t("Accounts")}
      </div>
      {walletSyncedWithOrca(state) ? (
        <p className="text-[11px] leading-4 text-content-muted">
          {t("Synced with Orca")}
        </p>
      ) : null}
      <fieldset
        aria-label={t("{provider} accounts", { provider: name })}
        className="space-y-1"
        disabled={busy}
      >
        <AccountRow
          checked={providerState.activeId === null}
          detail={systemDetail || t("Your own {provider} login", { provider: name })}
          label={t("System default")}
          name={`wallet-${provider}`}
          onSelect={() => void choose(null)}
          pending={pendingKey === SYSTEM_DEFAULT_KEY}
        />
        {providerState.accounts.map((account) => {
          const problem = walletAccountProblem(account, t);
          return (
            <AccountRow
              blocked={problem !== null}
              checked={providerState.activeId === account.id}
              detail={problem ?? walletAccountDetail(account)}
              key={account.id}
              label={account.email}
              name={`wallet-${provider}`}
              onRemove={() => void remove(account)}
              onSelect={() => void choose(account.id)}
              pending={pendingKey === account.id}
              removeLabel={t("Remove {email}", { email: account.email })}
            />
          );
        })}
      </fieldset>
      <button
        className="w-full rounded-lg border border-dashed border-surface-border/60 px-3 py-1.5 text-xs text-content-secondary transition-colors hover:border-accent/50 hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
        disabled={busy}
        onClick={() => onAdd(provider)}
        type="button"
      >
        {t("Add account")}
      </button>
      {state.recovered ? (
        <p className="text-[11px] leading-4 text-warning" role="status">
          {t(
            "Halv could not read the account list, so it rebuilt the list from the account folders. The active account is System default.",
          )}
        </p>
      ) : null}
      {notice ? (
        <p className="text-[11px] leading-4 text-content-muted" role="status">
          {notice}
        </p>
      ) : null}
      {error ? (
        <p className="text-[11px] leading-4 text-danger" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

function AccountRow({
  blocked = false,
  checked,
  detail,
  label,
  name,
  onRemove,
  onSelect,
  pending,
  removeLabel,
}: {
  // The account cannot be picked now. The detail tells why.
  blocked?: boolean;
  checked: boolean;
  detail: string;
  label: string;
  name: string;
  onRemove?: () => void;
  onSelect: () => void;
  pending: boolean;
  removeLabel?: string;
}) {
  return (
    <div
      className={`flex items-center gap-2 rounded-lg border px-2.5 py-1.5 transition-colors ${
        checked
          ? "border-accent/50 bg-accent/[0.08]"
          : "border-surface-border/35 hover:bg-surface-raised/60"
      } ${pending ? "opacity-60" : ""}`}
    >
      <label className="flex min-w-0 flex-1 cursor-pointer items-center gap-2">
        <input
          checked={checked}
          className="accent-accent"
          disabled={blocked && !checked}
          name={name}
          onChange={onSelect}
          type="radio"
        />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-xs text-content-primary">
            {label}
          </span>
          {detail ? (
            <span
              className={`block truncate text-[10px] ${
                blocked ? "text-warning" : "text-content-muted"
              }`}
            >
              {detail}
            </span>
          ) : null}
        </span>
      </label>
      {onRemove ? (
        <button
          aria-label={removeLabel}
          className="shrink-0 rounded px-1.5 text-sm leading-none text-content-muted transition-colors hover:text-danger focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
          onClick={onRemove}
          title={removeLabel}
          type="button"
        >
          {"×"}
        </button>
      ) : null}
    </div>
  );
}

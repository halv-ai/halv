// Account wallet: several Claude Code and Codex logins, one active account
// per provider. Rust owns the registry; this store mirrors the last state it
// reported. The design adapts Orca's managed accounts (stablyai/orca, MIT).
import { ipc } from "./ipc.ts";
import type { Translate } from "./i18n/runtime.tsx";

export type WalletProvider = "claude" | "codex";

export const WALLET_PROVIDERS: readonly WalletProvider[] = ["claude", "codex"];
export const WALLET_CHANGED_EVENT = "wallet-changed";

export type WalletAccount = {
  id: string;
  email: string;
  organizationName: string | null;
  plan: string | null;
  missing: boolean;
  // Orca refuses to select the account until the user signs in to it again
  // in Orca. Rust sends the field only when it is true.
  needsSignIn?: boolean;
  createdAt: string;
};

export type WalletSystemDefault = {
  email: string | null;
  plan: string | null;
};

export type WalletProviderState = {
  activeId: string | null;
  systemDefault: WalletSystemDefault;
  accounts: WalletAccount[];
};

// "halv" keeps the accounts in Halv's own registry. "orca" reads the
// accounts, the active account, and switches from the shared Orca store.
export type WalletStoreKind = "halv" | "orca";

export type WalletState = {
  claude: WalletProviderState;
  codex: WalletProviderState;
  // Halv could not read the registry at startup and rebuilt it from disk.
  recovered: boolean;
  store: WalletStoreKind;
};

export type WalletUsageWindowKind = "session" | "weekly" | "fableWeekly";

export type WalletUsageWindow = {
  kind: WalletUsageWindowKind;
  usedPercent: number;
  // Unix milliseconds.
  resetsAt: number | null;
};

/** The plan usage of one wallet account that is not active. */
export type WalletAccountUsage = {
  provider: WalletProvider;
  accountId: string;
  windows: WalletUsageWindow[];
  // Unix milliseconds.
  updatedAt: number;
};

/** A live agent pane that still runs an account that is no longer active. */
export type WalletStaleSession = {
  sessionId: string;
  provider: WalletProvider;
  // The active account when the pane started. Null is System default.
  previousAccountId: string | null;
  // The active account now. Null is System default.
  nextAccountId: string | null;
};

export type WalletSelectResult = {
  state: WalletState;
  // Agent panes of both providers that still run an old account.
  staleSessions: WalletStaleSession[];
};

export type WalletAddFinishResult = {
  state: WalletState;
  // False while the sign-in folder holds no login yet.
  added: boolean;
};

export type WalletPendingAdd = {
  pendingId: string;
  terminalId: string;
};

export type WalletSelectArgs = {
  provider: WalletProvider;
  accountId: string | null;
};
export type WalletProviderArgs = { provider: WalletProvider };
export type WalletPendingArgs = { provider: WalletProvider; pendingId: string };
export type WalletAccountArgs = { provider: WalletProvider; accountId: string };
export type WalletKeepSessionsArgs = { sessionIds: string[] };

export const EMPTY_WALLET_STATE: WalletState = {
  claude: { activeId: null, systemDefault: { email: null, plan: null }, accounts: [] },
  codex: { activeId: null, systemDefault: { email: null, plan: null }, accounts: [] },
  recovered: false,
  store: "halv",
};

type WalletListener = () => void;

export type WalletSnapshot = {
  state: WalletState;
  loaded: boolean;
  // Increments when the wallet changes after the first load, so views can
  // refresh the usage and lanes of the active account.
  version: number;
};

/**
 * Commit-based store. Every commit takes the state Rust returned; it never
 * merges with a snapshot captured before an await, so a late response can
 * only replace the state with a newer server answer.
 */
export class WalletStore {
  private snapshot: WalletSnapshot = {
    state: EMPTY_WALLET_STATE,
    loaded: false,
    version: 0,
  };
  private listeners = new Set<WalletListener>();
  private listening = false;
  private refreshingOnFocus = false;
  private loadRequest = 0;

  subscribe = (listener: WalletListener) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = () => this.snapshot;

  commit(state: WalletState) {
    const previous = this.snapshot;
    const changed =
      previous.loaded && JSON.stringify(previous.state) !== JSON.stringify(state);
    if (previous.loaded && !changed) return;
    this.snapshot = {
      state,
      loaded: true,
      version: previous.version + (changed ? 1 : 0),
    };
    for (const listener of this.listeners) listener();
  }

  /** Starts the change listener once and loads the first state. */
  ensureLoaded() {
    if (!this.listening) {
      this.listening = true;
      void ipc.events
        .listen<WalletState>(WALLET_CHANGED_EVENT, (state) => {
          // The event is newer than any refresh still in flight.
          this.loadRequest += 1;
          this.commit(state);
        })
        .catch(() => {
          this.listening = false;
        });
    }
    // The user can switch accounts in Orca while Halv is in the background.
    if (!this.refreshingOnFocus && typeof window !== "undefined") {
      this.refreshingOnFocus = true;
      window.addEventListener("focus", () => {
        void this.refresh().catch(() => undefined);
      });
    }
    if (!this.snapshot.loaded) void this.refresh().catch(() => undefined);
  }

  async refresh() {
    const request = ++this.loadRequest;
    const state = await ipc.wallet.list();
    // A newer refresh or a mutation result already committed fresher data.
    if (request === this.loadRequest) this.commit(state);
    return state;
  }

  async select(provider: WalletProvider, accountId: string | null) {
    this.loadRequest += 1;
    const result = await ipc.wallet.select({ provider, accountId });
    this.commit(result.state);
    return result;
  }

  async startAdd(provider: WalletProvider) {
    return ipc.wallet.addLaunch({ provider });
  }

  async finishAdd(provider: WalletProvider, pendingId: string) {
    this.loadRequest += 1;
    const result = await ipc.wallet.addFinish({ provider, pendingId });
    this.commit(result.state);
    return result;
  }

  async cancelAdd(provider: WalletProvider, pendingId: string) {
    await ipc.wallet.addCancel({ provider, pendingId });
  }

  async remove(provider: WalletProvider, accountId: string) {
    this.loadRequest += 1;
    const state = await ipc.wallet.remove({ provider, accountId });
    this.commit(state);
    return state;
  }
}

export const walletStore = new WalletStore();

const PLAN_LABELS: Record<string, string> = {
  free: "Free",
  go: "Go",
  plus: "Plus",
  pro: "Pro",
  max: "Max",
  team: "Team",
  business: "Business",
  enterprise: "Enterprise",
  edu: "Edu",
};

export function formatWalletPlan(plan: string | null | undefined): string | null {
  const value = plan?.trim();
  if (!value) return null;
  return (
    PLAN_LABELS[value.toLowerCase()] ??
    `${value.slice(0, 1).toUpperCase()}${value.slice(1)}`
  );
}

export function walletProviderName(provider: WalletProvider) {
  return provider === "claude" ? "Claude" : "Codex";
}

export function activeWalletAccount(state: WalletProviderState) {
  return state.activeId
    ? state.accounts.find((account) => account.id === state.activeId) ?? null
    : null;
}

/** The short label of the active account, for a compact switcher. */
export function activeWalletLabel(state: WalletProviderState, t: Translate) {
  const account = activeWalletAccount(state);
  if (account) return account.email;
  return state.systemDefault.email
    ? t("System default · {email}", { email: state.systemDefault.email })
    : t("System default");
}

export function walletAccountDetail(account: WalletAccount) {
  return [account.organizationName, formatWalletPlan(account.plan)]
    .filter((part): part is string => Boolean(part))
    .join(" · ");
}

/** Why the user cannot pick the account now, or null when they can. */
export function walletAccountProblem(account: WalletAccount, t: Translate) {
  if (account.missing) {
    return t("Account folder missing. Remove it and add it again.");
  }
  if (account.needsSignIn) return t("Sign in again in Orca");
  return null;
}

/**
 * Non-blocking notice after a switch. It counts the agent panes of the
 * provider that still run the old account. Panes that follow the switch
 * (Claude in Orca's shared default folder) never count.
 */
export function walletSwitchNotice(
  provider: WalletProvider,
  staleSessions: readonly WalletStaleSession[],
  t: Translate,
): string | null {
  const count = staleSessions.filter(
    (session) => session.provider === provider,
  ).length;
  if (count <= 0) return null;
  const name = walletProviderName(provider);
  return count === 1
    ? t(
        "1 {provider} session is still on the old account. Restart it from its pane or the status bar.",
        { provider: name },
      )
    : t(
        "{count} {provider} sessions are still on the old account. Restart them from their panes or the status bar.",
        { count, provider: name },
      );
}

/** The label of an account id in prompts. Null is System default. */
export function walletAccountLabel(
  state: WalletProviderState,
  accountId: string | null,
  t: Translate,
) {
  if (accountId === null) return t("System default");
  return (
    state.accounts.find((account) => account.id === accountId)?.email ??
    t("the old account")
  );
}

export function hasManagedAccounts(state: WalletState, provider: WalletProvider) {
  return state[provider].accounts.length > 0;
}

/** True when the accounts come from the shared Orca store. */
export function walletSyncedWithOrca(state: WalletState) {
  return state.store === "orca";
}

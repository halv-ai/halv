// Agent panes that still run an old account after an account switch. The
// prompt adapts Orca's Codex restart notice (stablyai/orca, MIT): each pane
// asks once, and a restart resumes the same CLI session on the active
// account. Halv restarts a pane only while the pane rests between turns.
// Rust decides which panes are stale (see src-tauri/src/wallet/launches.rs).
import { ipc } from "./ipc.ts";
import {
  walletStore,
  type WalletProvider,
  type WalletStaleSession,
} from "./accountWallet.ts";
import { agentSessionStore } from "./agentSessions.ts";
import type { TerminalSession } from "./types.ts";

/**
 * "ask" waits for the user. "queued" restarts when the pane rests.
 * "restarting" runs now. "failed" shows the error and asks again.
 */
export type AccountRestartPhase = "ask" | "queued" | "restarting" | "failed";

export type AccountRestartEntry = WalletStaleSession & {
  phase: AccountRestartPhase;
  error: string | null;
};

export type AccountRestartEntries = Readonly<Record<string, AccountRestartEntry>>;

export type AccountRestartSnapshot = {
  entries: AccountRestartEntries;
};

const LIVE_AGENT_STATUSES = new Set(["starting", "running", "waiting_input"]);

/**
 * Merges the stale panes that Rust reported into the entries. A pane keeps
 * its answer while the target account stays the same. A pane that Rust no
 * longer reports goes, unless its restart runs now.
 */
export function mergeStaleSessions(
  current: AccountRestartEntries,
  stale: readonly WalletStaleSession[],
): Record<string, AccountRestartEntry> {
  const next: Record<string, AccountRestartEntry> = {};
  for (const session of stale) {
    const previous = current[session.sessionId];
    const sameTarget =
      previous !== undefined &&
      previous.provider === session.provider &&
      previous.nextAccountId === session.nextAccountId;
    next[session.sessionId] = sameTarget
      ? { ...previous, ...session }
      : { ...session, phase: "ask", error: null };
  }
  for (const [sessionId, entry] of Object.entries(current)) {
    if (!(sessionId in next) && entry.phase === "restarting") {
      next[sessionId] = entry;
    }
  }
  return next;
}

/** True while the pane prompt has a question for the user. */
export function restartAwaitsAnswer(entry: AccountRestartEntry | undefined) {
  return entry?.phase === "ask" || entry?.phase === "failed";
}

export type AccountRestartSummary = {
  // Panes that wait for an answer.
  askIds: string[];
  // Panes that restart when they rest, or restart now.
  pendingCount: number;
};

/** The panes of one provider, for the status bar prompt. */
export function accountRestartSummary(
  entries: AccountRestartEntries,
  provider: WalletProvider,
): AccountRestartSummary {
  const askIds: string[] = [];
  let pendingCount = 0;
  for (const entry of Object.values(entries)) {
    if (entry.provider !== provider) continue;
    if (restartAwaitsAnswer(entry)) askIds.push(entry.sessionId);
    else pendingCount += 1;
  }
  askIds.sort();
  return { askIds, pendingCount };
}

/**
 * The next queued pane that this panel can restart now. A pane in a turn,
 * or a pane that waits for an approval or an answer, is busy.
 */
export function nextRestartablePane(
  entries: AccountRestartEntries,
  owned: ReadonlySet<string>,
  busy: (sessionId: string) => boolean,
): string | null {
  const queued = Object.values(entries)
    .filter(
      (entry) =>
        entry.phase === "queued" &&
        owned.has(entry.sessionId) &&
        !busy(entry.sessionId),
    )
    .map((entry) => entry.sessionId)
    .sort();
  return queued[0] ?? null;
}

/** The prompt panes whose agent session is no longer live. */
export function endedSessionIds(
  sessionIds: readonly string[],
  agents: readonly { session: { id: string; status: string } }[],
) {
  const live = new Set(
    agents
      .filter((entry) => LIVE_AGENT_STATUSES.has(entry.session.status))
      .map((entry) => entry.session.id),
  );
  return sessionIds.filter((sessionId) => !live.has(sessionId)).sort();
}

/** Waits until the terminal process of a pane is gone. */
export async function waitForTerminalExit(
  sessionId: string,
  list: () => Promise<Pick<TerminalSession, "id" | "status">[]>,
  {
    timeoutMs = 10_000,
    pollMs = 150,
    sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)),
  }: {
    timeoutMs?: number;
    pollMs?: number;
    sleep?: (ms: number) => Promise<void>;
  } = {},
) {
  for (let waited = 0; ; waited += pollMs) {
    const rows = await list();
    if (!rows.some((row) => row.id === sessionId && row.status === "running")) {
      return;
    }
    if (waited >= timeoutMs) {
      throw new Error("The old session did not stop in time.");
    }
    await sleep(pollMs);
  }
}

/** A live pane that a restart replaces. */
export type RestartablePane<Row, Saved> = {
  paneId: string;
  row: Row;
  // The saved pane state that resumes the CLI session.
  saved: Saved;
  cliSessionId: string | null;
};

export type PaneRestartSteps<Row, Saved> = {
  // The pane of the session in this panel, or null when it is gone.
  find: () => RestartablePane<Row, Saved> | null;
  // True while another resume runs in this panel.
  resumeBusy: () => boolean;
  stop: () => Promise<unknown>;
  waitForExit: () => Promise<void>;
  resume: (row: Row, saved: Saved) => Promise<void>;
  // The session that the pane shows now.
  boundSessionId: (paneId: string) => string | null;
  // Keeps the saved state on the pane, so the user can resume it by hand.
  keepForRetry: (paneId: string, saved: Saved) => void;
  forgetOld: () => Promise<void>;
  messages: { noCliSession: string; resumeFailed: string };
};

/**
 * Restarts one agent pane on the active account: it stops the old
 * process, then resumes the same CLI session in the same pane. The new
 * launch takes the active account. "later" means that another resume runs
 * in the panel, and nothing changed.
 */
export async function restartPaneOnActiveAccount<Row, Saved>(
  sessionId: string,
  steps: PaneRestartSteps<Row, Saved>,
): Promise<"done" | "later" | "gone"> {
  const pane = steps.find();
  if (!pane) return "gone";
  // Without the CLI session id, a restart would lose the conversation.
  if (!pane.cliSessionId) throw new Error(steps.messages.noCliSession);
  if (steps.resumeBusy()) return "later";
  await steps.stop();
  await steps.waitForExit();
  await steps.resume(pane.row, pane.saved);
  if (steps.boundSessionId(pane.paneId) === sessionId) {
    steps.keepForRetry(pane.paneId, pane.saved);
    throw new Error(steps.messages.resumeFailed);
  }
  await steps.forgetOld().catch(() => undefined);
  return "done";
}

type AccountRestartDependencies = {
  list: () => Promise<WalletStaleSession[]>;
  keep: (sessionIds: string[]) => Promise<unknown>;
};

const DEFAULT_DEPENDENCIES: AccountRestartDependencies = {
  list: () => ipc.wallet.staleSessions(),
  keep: (sessionIds) => ipc.wallet.keepSessions({ sessionIds }),
};

export function createAccountRestartStore(
  dependencies: AccountRestartDependencies = DEFAULT_DEPENDENCIES,
) {
  let snapshot: AccountRestartSnapshot = { entries: {} };
  const listeners = new Set<() => void>();
  let request = 0;
  let started = false;

  const commit = (entries: Record<string, AccountRestartEntry>) => {
    if (JSON.stringify(entries) === JSON.stringify(snapshot.entries)) return;
    snapshot = { entries };
    for (const listener of listeners) listener();
  };

  const update = (
    sessionId: string,
    change: (entry: AccountRestartEntry) => AccountRestartEntry | null,
  ) => {
    const entry = snapshot.entries[sessionId];
    if (!entry) return false;
    const next = { ...snapshot.entries };
    const changed = change(entry);
    if (changed) next[sessionId] = changed;
    else delete next[sessionId];
    commit(next);
    return true;
  };

  const store = {
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    getSnapshot: () => snapshot,
    /** Takes the stale list of a switch result. It is newer than a refresh in flight. */
    apply(stale: readonly WalletStaleSession[]) {
      request += 1;
      commit(mergeStaleSessions(snapshot.entries, stale));
    },
    async refresh() {
      const current = ++request;
      const stale = await dependencies.list();
      if (current === request) commit(mergeStaleSessions(snapshot.entries, stale));
    },
    /** Restarts the panes when they rest. */
    queue(sessionIds: readonly string[]) {
      const next = { ...snapshot.entries };
      for (const sessionId of sessionIds) {
        const entry = next[sessionId];
        if (restartAwaitsAnswer(entry)) {
          next[sessionId] = { ...entry!, phase: "queued", error: null };
        }
      }
      commit(next);
    },
    /** Keeps the panes on their account until the next switch. */
    keep(sessionIds: readonly string[]) {
      const next = { ...snapshot.entries };
      for (const sessionId of sessionIds) {
        if (next[sessionId]?.phase !== "restarting") delete next[sessionId];
      }
      commit(next);
      void dependencies.keep([...sessionIds]).catch(() => undefined);
    },
    /** Claims a queued pane. Only one panel restarts it. */
    begin(sessionId: string) {
      if (snapshot.entries[sessionId]?.phase !== "queued") return false;
      return update(sessionId, (entry) => ({ ...entry, phase: "restarting" }));
    },
    finish(sessionId: string) {
      update(sessionId, () => null);
    },
    fail(sessionId: string, error: string) {
      update(sessionId, (entry) => ({ ...entry, phase: "failed", error }));
    },
    /** Gives a claimed pane back to the queue. */
    requeue(sessionId: string) {
      update(sessionId, (entry) =>
        entry.phase === "restarting" ? { ...entry, phase: "queued" } : entry,
      );
    },
    /**
     * Follows the wallet: each change, also a switch in Orca, asks Rust
     * again. A pane that stops or closes asks Rust again too, so its prompt
     * goes.
     */
    ensureStarted() {
      if (started) return;
      started = true;
      let walletVersion = walletStore.getSnapshot().version;
      walletStore.subscribe(() => {
        const { version } = walletStore.getSnapshot();
        if (version === walletVersion) return;
        walletVersion = version;
        void store.refresh().catch(() => undefined);
      });
      let lastGone = "";
      agentSessionStore.subscribe(() => {
        const ids = Object.keys(snapshot.entries);
        const agents = agentSessionStore.getSnapshot();
        if (ids.length === 0 || agents.loading) return;
        const gone = endedSessionIds(ids, agents.entries).join("\n");
        if (!gone || gone === lastGone) return;
        lastGone = gone;
        void store.refresh().catch(() => undefined);
      });
      if (typeof window !== "undefined") {
        window.addEventListener("focus", () => {
          void store.refresh().catch(() => undefined);
        });
      }
      void store.refresh().catch(() => undefined);
    },
  };
  return store;
}

export type AccountRestartStore = ReturnType<typeof createAccountRestartStore>;

export const accountRestartStore = createAccountRestartStore();

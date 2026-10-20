import type {
  AgentSessionEntry,
  AgentSessionLaneOptions,
  AgentSessionStatus,
} from "./agentSessions.ts";
import {
  closeTerminalLeaf,
  collectTerminalLeafIds,
  reconcileTerminalPanelLayout,
  type TerminalPanelLayoutSnapshot,
} from "./terminalLayout.ts";
import {
  terminalPaneStateInput,
  type TerminalPaneState,
  type TerminalPaneStateInput,
} from "./terminalRestore.ts";
import type { TerminalSession } from "./types.ts";

export type TerminalCloseSnapshot = {
  scope: string;
  layout: TerminalPanelLayoutSnapshot;
  sessions: TerminalSession[];
  paneSessionIds: Record<string, string>;
  agentEntries: AgentSessionEntry[];
  laneOptions: AgentSessionLaneOptions;
  restoredPanes: Record<string, TerminalPaneState>;
  resumeFallbackPanes: Record<string, TerminalPaneState>;
  excludedPaneIds: ReadonlySet<string>;
};

export type TerminalRestoreWritePayload = {
  scope: string;
  layout: TerminalPanelLayoutSnapshot;
  panes: TerminalPaneStateInput[];
};

export function closeTerminalPanelSessions(
  layout: TerminalPanelLayoutSnapshot,
  paneIds: ReadonlySet<string>,
) {
  const tabs = layout.tabs.flatMap((tab) => {
    let root = tab.root;
    for (const paneId of paneIds) {
      const next = closeTerminalLeaf(root, paneId);
      if (!next) return [];
      root = next;
    }
    return [{ ...tab, root }];
  });
  return reconcileTerminalPanelLayout(
    { ...layout, tabs },
    tabs.flatMap((tab) => collectTerminalLeafIds(tab.root)),
    null,
  );
}

export function closedTerminalRestorePayload(
  snapshot: TerminalCloseSnapshot,
): TerminalRestoreWritePayload {
  const layout = closeTerminalPanelSessions(
    snapshot.layout,
    snapshot.excludedPaneIds,
  );
  const paneIdBySessionId = new Map(
    Object.entries(snapshot.paneSessionIds).map(([paneId, sessionId]) => [
      sessionId,
      paneId,
    ]),
  );
  const panes = snapshot.sessions
    .flatMap((session) => {
      const paneId = paneIdBySessionId.get(session.id);
      if (!paneId || snapshot.excludedPaneIds.has(paneId)) return [];
      const entry =
        snapshot.agentEntries.find(
          (candidate) => candidate.session.id === session.id,
        ) ?? null;
      return [
        terminalPaneStateInput(
          session,
          entry,
          snapshot.restoredPanes[paneId] ??
            snapshot.resumeFallbackPanes[paneId] ??
            null,
          snapshot.laneOptions,
          paneId,
        ),
      ];
    });
  return { scope: snapshot.scope, layout, panes };
}

export type TerminalParkSnapshot = Omit<TerminalCloseSnapshot, "excludedPaneIds">;

/**
 * The park payload of a tab close (ADR 0010). Each agent pane with a CLI
 * session id stays saved: agent kind, lane, credential profile, CLI session
 * id, cwd and titles. Shell panes and agent panes without a CLI session id
 * drop. The layout keeps only the parked panes, so a reopen resumes each
 * parked agent once with `--resume`. An empty pane list drops the scope.
 */
export function parkedTerminalRestorePayload(
  snapshot: TerminalParkSnapshot,
): TerminalRestoreWritePayload {
  const sessionById = new Map(snapshot.sessions.map((session) => [session.id, session]));
  const excludedPaneIds = new Set<string>();
  const paneIds = new Set([
    ...snapshot.layout.tabs.flatMap((tab) => collectTerminalLeafIds(tab.root)),
    ...Object.keys(snapshot.paneSessionIds),
  ]);
  for (const paneId of paneIds) {
    const sessionId = snapshot.paneSessionIds[paneId];
    const session = sessionId ? sessionById.get(sessionId) : undefined;
    const saved =
      snapshot.restoredPanes[paneId] ?? snapshot.resumeFallbackPanes[paneId] ?? null;
    const entry = session
      ? snapshot.agentEntries.find((candidate) => candidate.session.id === session.id)
      : undefined;
    const agentKind = entry?.session.agentKind ?? session?.agentKind ?? saved?.agentKind;
    const cliSessionId = entry?.session.cliSessionId ?? saved?.cliSessionId;
    if (!session || !agentKind || !cliSessionId) excludedPaneIds.add(paneId);
  }
  return closedTerminalRestorePayload({ ...snapshot, excludedPaneIds });
}

export async function writeClosedTerminalRestore(
  snapshot: TerminalCloseSnapshot,
  dependencies: {
    writeLayout: (
      scope: string,
      layout: TerminalPanelLayoutSnapshot,
    ) => void;
    writeRestore: (payload: TerminalRestoreWritePayload) => Promise<unknown>;
  },
) {
  const payload = closedTerminalRestorePayload(snapshot);
  dependencies.writeLayout(payload.scope, payload.layout);
  await dependencies.writeRestore(payload);
  return payload;
}

export function markPendingTerminalForget(
  terminalIds: Set<string>,
  agentIds: Set<string>,
  sessionId: string,
  agent: boolean,
) {
  terminalIds.add(sessionId);
  if (agent) agentIds.add(sessionId);
}

export function clearPendingTerminalForget(
  terminalIds: Set<string>,
  agentIds: Set<string>,
  sessionId: string,
) {
  terminalIds.delete(sessionId);
  agentIds.delete(sessionId);
}

export function agentSessionWaitsForExit(status: AgentSessionStatus) {
  return ["starting", "running", "waiting_input"].includes(status);
}

export async function closeRunningTerminalSession(
  sessionId: string,
  dependencies: {
    closeTerminal: (sessionId: string) => Promise<unknown>;
    forgetTerminal: (sessionId: string) => Promise<unknown>;
  },
): Promise<"closing" | "forgotten"> {
  try {
    await dependencies.closeTerminal(sessionId);
    return "closing";
  } catch (closeError) {
    try {
      await dependencies.forgetTerminal(sessionId);
    } catch (forgetError) {
      if (
        !isTerminalSessionNotFound(closeError) ||
        !isTerminalSessionNotFound(forgetError)
      ) {
        throw forgetError;
      }
    }
    return "forgotten";
  }
}

function isTerminalSessionNotFound(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return /terminal session.*was not found/i.test(message);
}

export async function forgetTerminalSession(
  sessionId: string,
  forgetTerminal: (sessionId: string) => Promise<unknown>,
) {
  try {
    await forgetTerminal(sessionId);
  } catch (cause) {
    if (!isTerminalSessionNotFound(cause)) throw cause;
  }
}

function isAgentSessionNotFound(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return /agent session.*not found/i.test(message);
}

function isAgentSessionRunning(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return /agent session.*running/i.test(message);
}

function waitForAgentExit() {
  return new Promise<void>((resolve) => globalThis.setTimeout(resolve, 50));
}

export async function forgetStoppedAgentSession(
  sessionId: string,
  dependencies: {
    forgetAgent: (sessionId: string) => Promise<unknown>;
    refreshAgentSessions: () => Promise<unknown>;
    waitForExit?: () => Promise<void>;
  },
) {
  let failure: unknown = null;
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      await dependencies.forgetAgent(sessionId);
      break;
    } catch (cause) {
      if (isAgentSessionNotFound(cause)) break;
      if (isAgentSessionRunning(cause) && attempt < 9) {
        await (dependencies.waitForExit ?? waitForAgentExit)();
        continue;
      }
      failure = cause;
      break;
    }
  }
  try {
    await dependencies.refreshAgentSessions();
  } catch (cause) {
    failure ??= cause;
  }
  if (failure) throw failure;
}

export async function forgetPendingTerminalExit(
  terminalIds: Set<string>,
  agentIds: Set<string>,
  sessionId: string,
  dependencies: {
    forgetTerminal: (sessionId: string) => Promise<unknown>;
    forgetAgent: (sessionId: string) => Promise<unknown>;
    refreshAgentSessions: () => Promise<unknown>;
    refreshTerminalSessions: () => Promise<unknown>;
    waitForExit?: () => Promise<void>;
  },
) {
  if (!terminalIds.delete(sessionId)) return false;
  const forgetAgent = agentIds.delete(sessionId);
  await forgetTerminalSession(sessionId, dependencies.forgetTerminal);

  let failure: unknown = null;
  if (forgetAgent) {
    try {
      await forgetStoppedAgentSession(sessionId, dependencies);
    } catch (cause) {
      failure ??= cause;
    }
  }
  try {
    await dependencies.refreshTerminalSessions();
  } catch (cause) {
    failure ??= cause;
  }
  if (failure) throw failure;
  return true;
}

function isTerminalStillRunning(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return /close the terminal before removing it/i.test(message);
}

// A continuation shell whose source pane is gone has no pane of its own.
// Close the shell and drop its row. An in-process shell can need a moment
// to exit after the close, so the forget retries a few times.
export async function closeOrphanTerminalSession(
  sessionId: string,
  dependencies: {
    closeTerminal: (sessionId: string) => Promise<unknown>;
    forgetTerminal: (sessionId: string) => Promise<unknown>;
    waitForExit?: () => Promise<void>;
  },
) {
  try {
    await dependencies.closeTerminal(sessionId);
  } catch (cause) {
    if (isTerminalSessionNotFound(cause)) return;
    throw cause;
  }
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      await dependencies.forgetTerminal(sessionId);
      return;
    } catch (cause) {
      if (isTerminalSessionNotFound(cause)) return;
      if (!isTerminalStillRunning(cause) || attempt === 9) throw cause;
      await (dependencies.waitForExit ?? waitForAgentExit)();
    }
  }
}

// A continuation shell replaces its ended source in the same pane. When the
// shell pane closes, the source terminal and its agent row go too.
// Otherwise the ended agent comes back as a pane of its own.
export async function forgetContinuationSource(
  sourceId: string,
  dependencies: {
    forgetTerminal: (sessionId: string) => Promise<unknown>;
    forgetAgent: (sessionId: string) => Promise<unknown>;
    refreshAgentSessions: () => Promise<unknown>;
    waitForExit?: () => Promise<void>;
  },
) {
  await forgetTerminalSession(sourceId, dependencies.forgetTerminal);
  await forgetStoppedAgentSession(sourceId, dependencies);
}

// One running session of a tab that closes. An agent stops through its
// agent session, a shell through `terminal_close`. Both rows go away, as a
// pane close does, so nothing comes back as a pane of its own. A process
// can need a moment to exit, so each forget retries a few times.
export async function stopTerminalSessionForTabClose(
  session: Pick<TerminalSession, "id" | "agentKind" | "status">,
  dependencies: {
    killAgent: (sessionId: string) => Promise<unknown>;
    closeTerminal: (sessionId: string) => Promise<unknown>;
    forgetTerminal: (sessionId: string) => Promise<unknown>;
    forgetAgent: (sessionId: string) => Promise<unknown>;
    refreshAgentSessions: () => Promise<unknown>;
    waitForExit?: () => Promise<void>;
  },
) {
  if (session.agentKind) {
    if (session.status === "running") {
      try {
        await dependencies.killAgent(session.id);
      } catch (cause) {
        if (!isAgentSessionNotFound(cause)) throw cause;
        // A plain agent terminal without an agent row stops as a shell.
        await closeOrphanTerminalSession(session.id, dependencies);
        return;
      }
    }
    for (let attempt = 0; attempt < 10; attempt += 1) {
      try {
        await forgetTerminalSession(session.id, dependencies.forgetTerminal);
        break;
      } catch (cause) {
        if (!isTerminalStillRunning(cause) || attempt === 9) throw cause;
        await (dependencies.waitForExit ?? waitForAgentExit)();
      }
    }
    await forgetStoppedAgentSession(session.id, dependencies);
    return;
  }
  if (session.status === "running") {
    await closeOrphanTerminalSession(session.id, dependencies);
  } else {
    await forgetTerminalSession(session.id, dependencies.forgetTerminal);
  }
}

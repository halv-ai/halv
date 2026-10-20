import type {
  AgentSessionEntry,
  AgentSessionStatus,
} from "./agentSessions.ts";
import {
  createTerminalPaneId,
  type TerminalPaneState,
} from "./terminalRestore.ts";
import type { TerminalSession } from "./types.ts";

export type ListedTerminalLiveness = Pick<TerminalSession, "id" | "status"> &
  Partial<Pick<TerminalSession, "continuedFrom">>;
export type AgentSessionLiveness = {
  session: Pick<AgentSessionEntry["session"], "cliSessionId" | "status">;
};

/** What a panel shows (ADR 0010, P2 and P3). A shell tab panel and a
 *  conversation panel show their owner's sessions. An agent panel shows its
 *  agent and the panes of its saved layout. A classic panel has no owner. */
export type TerminalPanelBinding =
  | { kind: "shellTab"; shellTabId: string }
  | { kind: "conversation"; conversationId: string }
  | { kind: "agent"; agentSessionId: string }
  | { kind: "classic" };

export type PanelSessionFact = Pick<TerminalSession, "id"> &
  Partial<Pick<TerminalSession, "conversationId" | "shellTabId" | "agentKind">>;

/**
 * The session filter of a panel, in the order that the panel checks it.
 * - A session that a continuation shell replaced never shows.
 * - A session has one owner: its conversation, else its shell tab. The
 *   conversation wins when corrupt data sets both (P1).
 * - An owned session never shows in a panel of another owner or in a panel
 *   without an owner, also not through a stale saved pane (P3).
 * `inLayout` is true when the saved layout of the panel holds the pane.
 */
export function panelListsSession(
  panel: TerminalPanelBinding,
  session: PanelSessionFact,
  options: { inLayout?: boolean; replaced?: ReadonlySet<string> } = {},
): boolean {
  if (options.replaced?.has(session.id)) return false;
  const conversationId = session.conversationId || null;
  const shellTabId = conversationId ? null : session.shellTabId || null;
  switch (panel.kind) {
    case "conversation":
      return conversationId === panel.conversationId;
    case "shellTab":
      return shellTabId === panel.shellTabId;
    case "agent":
      return (
        !shellTabId &&
        (session.id === panel.agentSessionId || Boolean(options.inLayout))
      );
    case "classic":
      return (
        !conversationId &&
        !shellTabId &&
        (!session.agentKind || Boolean(options.inLayout))
      );
  }
}

export function createTerminalResumeGuard() {
  let inFlight = 0;
  return {
    active: () => inFlight > 0,
    begin: () => {
      inFlight += 1;
      let ended = false;
      return () => {
        if (ended) return;
        ended = true;
        inFlight = Math.max(0, inFlight - 1);
      };
    },
  };
}

/**
 * An agent pane does not resume on its own while a supervisor with another
 * protocol holds the socket. That supervisor can still run the agent, and a
 * second copy would write to the same transcript (ADR 0008, rule 5). The
 * pane keeps the manual Resume / Start fresh choice.
 */
export function paneWaitsForOldSupervisor(
  pane: Pick<TerminalPaneState, "agentKind">,
  supervisorProtocolMismatch: boolean,
) {
  return supervisorProtocolMismatch && Boolean(pane.agentKind);
}

export function filterRestoredPanesForAutoResume(
  panes: TerminalPaneState[],
  listedRows: ListedTerminalLiveness[],
  agentEntries: AgentSessionLiveness[],
  { supervisorProtocolMismatch = false }: { supervisorProtocolMismatch?: boolean } = {},
) {
  const runningTerminalIds = new Set(
    listedRows
      .filter((row) => row.status === "running")
      .map((row) => row.id),
  );
  const liveAgentStatuses = new Set<AgentSessionStatus>([
    "starting",
    "running",
    "waiting_input",
  ]);
  const liveAgentCliSessionIds = new Set(
    agentEntries
      .filter((entry) => liveAgentStatuses.has(entry.session.status))
      .map((entry) => entry.session.cliSessionId)
      .filter((cliSessionId): cliSessionId is string => Boolean(cliSessionId)),
  );
  // A shell that continues the saved session already owns the pane. A
  // resume would start the ended agent again over that shell.
  const continuedSessionIds = new Set(
    listedRows
      .map((row) => row.continuedFrom)
      .filter((sessionId): sessionId is string => Boolean(sessionId)),
  );
  const droppedPaneIds: string[] = [];
  const resumablePanes = panes.filter((pane) => {
    const terminalIsRunning = runningTerminalIds.has(pane.sessionId);
    const agentIsLive = Boolean(
      pane.agentKind &&
        pane.cliSessionId &&
        liveAgentCliSessionIds.has(pane.cliSessionId),
    );
    const continued = continuedSessionIds.has(pane.sessionId);
    const heldByOldSupervisor = paneWaitsForOldSupervisor(
      pane,
      supervisorProtocolMismatch,
    );
    if (!terminalIsRunning && !agentIsLive && !continued && !heldByOldSupervisor) {
      return true;
    }
    droppedPaneIds.push(pane.paneId);
    return false;
  });
  return { droppedPaneIds, resumablePanes };
}

export function releaseAutoResumePaneClaims(
  claimedPaneIds: Set<string>,
  paneIds: Iterable<string>,
) {
  for (const paneId of paneIds) claimedPaneIds.delete(paneId);
}

export function paneIdForSession(
  paneSessionIds: Readonly<Record<string, string>>,
  sessionId: string,
) {
  return (
    Object.entries(paneSessionIds).find(
      ([, candidateSessionId]) => candidateSessionId === sessionId,
    )?.[0] ?? null
  );
}

export function bindPaneToSession(
  paneSessionIds: Readonly<Record<string, string>>,
  paneId: string,
  sessionId: string,
) {
  const uniqueBindings = Object.fromEntries(
    Object.entries(paneSessionIds).filter(
      ([candidatePaneId, candidateSessionId]) =>
        candidatePaneId === paneId || candidateSessionId !== sessionId,
    ),
  );
  return { ...uniqueBindings, [paneId]: sessionId };
}

export type ContinuationRow = Pick<TerminalSession, "id"> &
  Partial<Pick<TerminalSession, "continuedFrom">>;

/**
 * Binds each pane that shows an ended agent session to the shell that
 * continues it. The shell keeps the pane id, so the layout stays the same.
 * A source with a continuation never gets its own pane again. A shell whose
 * source pane was closed (dismissed) or shows a resume failure (blocked) is
 * an orphan: the panel closes it, and the source keeps its pane.
 */
export function applyTerminalContinuations(
  rows: readonly ContinuationRow[],
  paneSessionIds: Readonly<Record<string, string>>,
  blockedPaneIds: ReadonlySet<string> = new Set(),
  dismissedSessionIds: ReadonlySet<string> = new Set(),
) {
  let nextPaneSessionIds = paneSessionIds;
  const replacedSessionIds = new Set<string>();
  const continuationBySourceId = new Map<string, string>();
  const reboundPaneIds: string[] = [];
  const orphanSessionIds: string[] = [];
  for (const row of rows) {
    const sourceId = row.continuedFrom;
    if (!sourceId) continue;
    const sourcePaneId = paneIdForSession(nextPaneSessionIds, sourceId);
    const orphan =
      dismissedSessionIds.has(sourceId) ||
      Boolean(sourcePaneId && blockedPaneIds.has(sourcePaneId));
    if (orphan) {
      if (!dismissedSessionIds.has(row.id)) orphanSessionIds.push(row.id);
      continue;
    }
    replacedSessionIds.add(sourceId);
    continuationBySourceId.set(sourceId, row.id);
    if (
      !sourcePaneId ||
      dismissedSessionIds.has(row.id) ||
      paneIdForSession(nextPaneSessionIds, row.id)
    ) {
      continue;
    }
    nextPaneSessionIds = bindPaneToSession(
      nextPaneSessionIds,
      sourcePaneId,
      row.id,
    );
    reboundPaneIds.push(sourcePaneId);
  }
  return {
    paneSessionIds: nextPaneSessionIds,
    replacedSessionIds,
    continuationBySourceId,
    reboundPaneIds,
    orphanSessionIds,
  };
}

/**
 * True when a new continuation shell concerns this panel: a pane shows its
 * source, or the panel closed the source just before the shell started. In
 * the second case the shell is an orphan, and the next refresh closes it.
 */
export function panelOwnsContinuationSource(
  sourceId: string,
  paneSessionIds: Readonly<Record<string, string>>,
  dismissedSessionIds: ReadonlySet<string>,
) {
  return (
    Boolean(paneIdForSession(paneSessionIds, sourceId)) ||
    dismissedSessionIds.has(sourceId)
  );
}

export type PaneSessionAgent = {
  session: Pick<AgentSessionEntry["session"], "id" | "cliSessionId">;
};

export function reconcileListedTerminalPanes(
  rows: TerminalSession[],
  paneSessionIds: Readonly<Record<string, string>>,
  restoredPanes: Readonly<
    Record<string, Pick<TerminalPaneState, "cliSessionId">>
  >,
  agentEntries: PaneSessionAgent[],
  expectedSessionIds: ReadonlySet<string>,
  createPaneId: () => string = createTerminalPaneId,
) {
  const visibleRows: TerminalSession[] = [];
  const claimedPaneIds = new Set<string>();
  const appendablePaneIds = new Set<string>();
  let nextPaneSessionIds = paneSessionIds;
  for (const row of rows) {
    let paneId = paneIdForSession(nextPaneSessionIds, row.id);
    let adoptedSavedPane = false;
    let createdPane = false;
    if (paneId) claimedPaneIds.add(paneId);
    if (!paneId) {
      const cliSessionId = agentEntries.find(
        (entry) => entry.session.id === row.id,
      )?.session.cliSessionId;
      const savedPane = cliSessionId
        ? Object.entries(restoredPanes).find(
            ([candidatePaneId, pane]) =>
              pane.cliSessionId === cliSessionId &&
              !claimedPaneIds.has(candidatePaneId),
          )
        : null;
      if (savedPane) {
        paneId = savedPane[0];
        claimedPaneIds.add(paneId);
        adoptedSavedPane = true;
      } else {
        if (row.status !== "running" && !expectedSessionIds.has(row.id)) {
          continue;
        }
        paneId = createPaneId();
        createdPane = true;
      }
      nextPaneSessionIds = bindPaneToSession(
        nextPaneSessionIds,
        paneId,
        row.id,
      );
    }
    if (
      adoptedSavedPane ||
      createdPane ||
      expectedSessionIds.has(row.id)
    ) {
      appendablePaneIds.add(paneId);
    }
    visibleRows.push(row);
  }
  return {
    rows: visibleRows,
    paneSessionIds: nextPaneSessionIds,
    appendablePaneIds,
  };
}

// Returns the record without the given panes. The same object comes back
// when no pane matches, so React state setters can skip a render.
export function omitPaneIds<T>(
  record: Readonly<Record<string, T>>,
  paneIds: Iterable<string>,
): Record<string, T> {
  const ids = [...paneIds].filter((paneId) => paneId in record);
  if (ids.length === 0) return record as Record<string, T>;
  const next = { ...record };
  for (const paneId of ids) delete next[paneId];
  return next;
}

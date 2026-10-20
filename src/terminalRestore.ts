import {
  buildAgentSessionLane,
  resolveAgentTitleLayers,
  type AgentKind,
  type AgentSessionCreateLane,
  type AgentSessionCreateLaneKind,
  type AgentSessionEntry,
  type AgentSessionLaneOptions,
  type AgentSessionTitleState,
} from "./agentSessions.ts";
import {
  closeTerminalLeaf,
  collectTerminalLeafIds,
  deserializeTerminalPanelLayout,
  reconcileTerminalPanelLayout,
  type TerminalPaneLayoutNode,
  type TerminalPanelLayoutSnapshot,
} from "./terminalLayout.ts";
import type {
  TerminalSession,
  TerminalSupervisorStatus,
} from "./types.ts";

export type TerminalPaneStateInput = {
  paneId: string;
  sessionId: string;
  projectId: number | null;
  conversationId: string | null;
  projectName: string;
  cwd: string;
  executionProfile: string;
  agentKind: AgentKind | null;
  laneKind: AgentSessionCreateLaneKind | null;
  credentialProfileId: string | null;
  customTitle: string | null;
  defaultTitle?: string | null;
  generatedTitle?: string | null;
  aiVaultTitle?: AgentSessionTitleState["aiVaultTitle"];
  generation?: number;
  cliSessionId: string | null;
  cols: number;
  rows: number;
};

export type TerminalPaneState = TerminalPaneStateInput & {
  scrollback: string;
  updatedAt: string;
};

export type TerminalPanelState = {
  layout: unknown;
  panes: TerminalPaneState[];
  updatedAt: string;
};

export type ScopedTerminalPanelState = {
  scope: string;
  panel: TerminalPanelState;
};

export type RestoredTerminalPanel = {
  layout: TerminalPanelLayoutSnapshot;
  panes: Record<string, TerminalPaneState>;
  paneSessionIds: Record<string, string>;
  sessions: TerminalSession[];
};

export type TerminalResumeAttempt = {
  createdAtMs: number;
  createdByResume: boolean;
  userWroteInput: boolean;
};

export type TerminalResumeExit = {
  exitCode: number | null;
  exitedAtMs: number;
};

export const TERMINAL_RESUME_FAILURE_WINDOW_MS = 20_000;

const SUPERVISOR_REPLAY_NOTICE =
  "Halv could not start the terminal supervisor. Saved agent sessions will resume from their transcripts; in-flight work may be lost.";
const SUPERVISOR_PROTOCOL_NOTICE =
  "Agents from an older Halv are still running. Halv cannot connect to them.";

export type SupervisorReplayNotice = {
  message: string;
  detail?: string;
};

const EMPTY_LAYOUT: TerminalPanelLayoutSnapshot = {
  version: 1,
  activeTabId: null,
  activeLeafId: null,
  tabs: [],
};

const AGENT_KINDS = new Set<AgentKind>([
  "antigravity",
  "claude",
  "codex",
  "gemini",
  "kimi",
  "zai",
  "grok",
]);

export function supervisorReplayNotice(
  savedPanes: TerminalPaneState[],
  status: TerminalSupervisorStatus,
): SupervisorReplayNotice | null {
  const detail = status.message?.trim();
  if (status.available) {
    // Settings shows a supervisor from an older build. The panel does not.
    return detail && !status.supervisorStale ? { message: detail } : null;
  }
  if (!savedPanes.some((pane) => Boolean(pane.agentKind))) {
    return null;
  }
  // The old supervisor still runs these agents, so they do not resume.
  return {
    message: status.protocolMismatch
      ? SUPERVISOR_PROTOCOL_NOTICE
      : SUPERVISOR_REPLAY_NOTICE,
    ...(detail ? { detail } : {}),
  };
}

export function isTerminalResumeFailure(
  attempt: TerminalResumeAttempt | null | undefined,
  exit: TerminalResumeExit,
) {
  if (!attempt?.createdByResume || attempt.userWroteInput) return false;
  const elapsedMs = exit.exitedAtMs - attempt.createdAtMs;
  return elapsedMs >= 0 && elapsedMs <= TERMINAL_RESUME_FAILURE_WINDOW_MS;
}

export function restoreTerminalPanel(
  saved: TerminalPanelState | null,
  conversationId?: string | null,
): RestoredTerminalPanel {
  if (!saved || !Array.isArray(saved.panes)) {
    return {
      layout: EMPTY_LAYOUT,
      panes: {},
      paneSessionIds: {},
      sessions: [],
    };
  }
  const layout = deserializeTerminalPanelLayout(JSON.stringify(saved.layout));
  const scopedPanes = saved.panes
    .filter(validTerminalPane)
    .map((pane) => ({
      ...pane,
      paneId:
        typeof pane.paneId === "string" && pane.paneId.trim()
          ? pane.paneId
          : pane.sessionId,
      hadPersistedPaneId:
        typeof pane.paneId === "string" && Boolean(pane.paneId.trim()),
      conversationId: pane.conversationId ?? null,
    }))
    .map((pane) =>
      conversationId && pane.conversationId === null
        ? { ...pane, conversationId }
        : pane,
    )
    .filter(
      (pane) =>
        conversationId === undefined || pane.conversationId === conversationId,
    );
  // Two saved panes with one CLI session id are one terminal saved twice (a
  // ghost resume raced the real one). Keep the pane whose leaf sits in the
  // richest tab - the real split - and drop the clones together with their
  // leaves, so the saved corruption collapses back to the true shape.
  const tabLeafSizes = new Map<string, [number, number]>();
  (layout?.tabs ?? []).forEach((tab, tabIndex) => {
    const leafIds = collectTerminalLeafIds(tab.root);
    for (const leafId of leafIds) {
      tabLeafSizes.set(leafId, [leafIds.length, -tabIndex]);
    }
  });
  const duplicateKey = (pane: (typeof scopedPanes)[number]) =>
    pane.hadPersistedPaneId
      ? `pane:${pane.paneId}`
      : pane.cliSessionId
        ? `legacy-cli:${pane.cliSessionId}`
        : `pane:${pane.paneId}`;
  const keeperByIdentity = new Map<string, (typeof scopedPanes)[number]>();
  for (const pane of scopedPanes) {
    const key = duplicateKey(pane);
    const current = keeperByIdentity.get(key);
    if (!current) {
      keeperByIdentity.set(key, pane);
      continue;
    }
    const score = (candidate: TerminalPaneState) =>
      tabLeafSizes.get(candidate.paneId) ?? [0, 0];
    const [aLeaves, aTab] = score(current);
    const [bLeaves, bTab] = score(pane);
    if (bLeaves > aLeaves || (bLeaves === aLeaves && bTab > aTab)) {
      keeperByIdentity.set(key, pane);
    }
  }
  // A duplicate that lives alone in its own tab is a ghost-resume clone:
  // drop it with its tab. A duplicate inside a shared split is a pane the
  // user arranged; it stays, minus the id it cannot own.
  const droppedLeafIds = new Set<string>();
  const paneList = scopedPanes.flatMap((pane) => {
    const keeper = keeperByIdentity.get(duplicateKey(pane));
    if (keeper === pane) {
      return [pane];
    }
    const [tabLeafCount] = tabLeafSizes.get(pane.paneId) ?? [0, 0];
    if (tabLeafCount <= 1) {
      droppedLeafIds.add(pane.paneId);
      return [];
    }
    return [{ ...pane, cliSessionId: null }];
  });
  const panes = Object.fromEntries(
    paneList.map(({ hadPersistedPaneId: _, ...pane }) => [pane.paneId, pane]),
  );
  const paneIds = Object.keys(panes);
  // Leaves without a matching pane stay in the layout: their sessions can be
  // live (a mid-session remount) and pruning them here rebuilds the panel
  // from an empty base, which flattens every split into loose tabs. The
  // panel's refresh prunes leaves whose sessions are truly gone.
  const savedLeafIds = layout
    ? layout.tabs.flatMap((tab) =>
        collectTerminalLeafIds(tab.root).filter(
          (leafId) => !droppedLeafIds.has(leafId),
        ),
      )
    : [];
  const knownIds = [...new Set([...paneIds, ...savedLeafIds])];
  const reconciled = reconcileTerminalPanelLayout(layout, knownIds);
  const orderedIds = reconciled.tabs.flatMap((tab) =>
    collectTerminalLeafIds(tab.root),
  );
  const sessions = orderedIds.flatMap((paneId) => {
    const pane = panes[paneId];
    return pane ? [restoredTerminalSession(pane)] : [];
  });
  const paneSessionIds = Object.fromEntries(
    Object.values(panes).map((pane) => [pane.paneId, pane.sessionId]),
  );
  return { layout: reconciled, panes, paneSessionIds, sessions };
}

export function restoreTerminalPanels(
  savedPanels: ScopedTerminalPanelState[],
): RestoredTerminalPanel {
  const restoredPanels = savedPanels.map(({ panel }) =>
    restoreTerminalPanel(panel),
  );
  const panes = Object.assign(
    {},
    ...restoredPanels.map((restored) => restored.panes),
  ) as Record<string, TerminalPaneState>;
  const paneSessionIds = Object.assign(
    {},
    ...restoredPanels.map((restored) => restored.paneSessionIds),
  ) as Record<string, string>;
  const first = restoredPanels.find((restored) => restored.sessions.length > 0);
  const combined: TerminalPanelLayoutSnapshot = {
    version: 1,
    activeTabId: first?.layout.activeTabId ?? null,
    activeLeafId: first?.layout.activeLeafId ?? null,
    tabs: restoredPanels.flatMap((restored) => restored.layout.tabs),
  };
  const layout = reconcileTerminalPanelLayout(combined, Object.keys(panes));
  const orderedIds = layout.tabs.flatMap((tab) =>
    collectTerminalLeafIds(tab.root),
  );
  const sessions = orderedIds.flatMap((paneId) => {
    const pane = panes[paneId];
    return pane ? [restoredTerminalSession(pane)] : [];
  });
  return { layout, panes, paneSessionIds, sessions };
}

export function replaceTerminalSessionId(
  layout: TerminalPanelLayoutSnapshot,
  previousId: string,
  nextId: string,
): TerminalPanelLayoutSnapshot {
  // TODO(pane-id): Remove session-id leaf replacement after legacy callers retire.
  if (!previousId || !nextId || previousId === nextId) {
    return layout;
  }
  // A racing session-list refresh can append the resumed session as its own
  // tab before this rename runs. Drop those leaves first, or the rename
  // leaves the same session id in two tabs and every restore duplicates.
  const tabs = layout.tabs.flatMap((tab) => {
    const root = closeTerminalLeaf(tab.root, nextId);
    return root ? [{ ...tab, root }] : [];
  });
  return {
    ...layout,
    activeTabId:
      layout.activeTabId === previousId ? nextId : layout.activeTabId,
    activeLeafId:
      layout.activeLeafId === previousId ? nextId : layout.activeLeafId,
    tabs: tabs.map((tab) => ({
      tabId: tab.tabId === previousId ? nextId : tab.tabId,
      root: replaceLeafId(tab.root, previousId, nextId),
    })),
  };
}

export function terminalPaneStateInput(
  session: TerminalSession,
  entry: AgentSessionEntry | null,
  restored: TerminalPaneState | null,
  laneOptions: AgentSessionLaneOptions,
  paneId = restored?.paneId ?? session.id,
): TerminalPaneStateInput {
  const laneKind = entry?.session.laneKind ?? restored?.laneKind ?? null;
  return {
    paneId,
    sessionId: session.id,
    projectId: session.projectId ?? restored?.projectId ?? null,
    conversationId:
      entry?.session.conversationId ??
      session.conversationId ??
      restored?.conversationId ??
      null,
    projectName: session.projectName || restored?.projectName || "Terminal",
    cwd: session.cwd || restored?.cwd || "",
    executionProfile:
      session.executionProfile || restored?.executionProfile || "full_access",
    agentKind: entry?.session.agentKind ?? session.agentKind ?? restored?.agentKind ?? null,
    laneKind,
    credentialProfileId:
      restored?.credentialProfileId ??
      activeCredentialProfileId(laneOptions, entry?.session.agentKind, laneKind),
    customTitle:
      entry?.session.customTitle?.trim() ||
      session.title?.trim() ||
      restored?.customTitle ||
      null,
    defaultTitle:
      entry?.session.defaultTitle?.trim() || restored?.defaultTitle || null,
    generatedTitle:
      entry?.session.generatedTitle?.trim() || restored?.generatedTitle || null,
    aiVaultTitle: entry?.session.aiVaultTitle ?? restored?.aiVaultTitle ?? null,
    generation: entry?.session.generation ?? restored?.generation ?? 0,
    cliSessionId:
      entry?.session.cliSessionId ?? restored?.cliSessionId ?? null,
    cols: session.cols,
    rows: session.rows,
  };
}

export function buildTerminalRestoreWritePayload(input: {
  scope: string;
  layout: TerminalPanelLayoutSnapshot;
  sessions: TerminalSession[];
  paneSessionIds: Record<string, string>;
  agentEntries: AgentSessionEntry[];
  restoredPanes: Record<string, TerminalPaneState>;
  resumeFallbackPanes: Record<string, TerminalPaneState>;
  laneOptions: AgentSessionLaneOptions;
}) {
  const paneIdBySessionId = new Map(
    Object.entries(input.paneSessionIds).map(([paneId, sessionId]) => [
      sessionId,
      paneId,
    ]),
  );
  const panes = input.sessions.map((session) => {
    const paneId = paneIdBySessionId.get(session.id) ?? session.id;
    const entry =
      input.agentEntries.find(
        (candidate) => candidate.session.id === session.id,
      ) ?? null;
    return terminalPaneStateInput(
      session,
      entry,
      input.restoredPanes[paneId] ??
        input.resumeFallbackPanes[paneId] ??
        null,
      input.laneOptions,
      paneId,
    );
  });
  return { scope: input.scope, layout: input.layout, panes };
}

export function bindTerminalPaneSession(
  paneSessionIds: Readonly<Record<string, string>>,
  paneId: string,
  sessionId: string,
) {
  return { ...paneSessionIds, [paneId]: sessionId };
}

export function createTerminalPaneId() {
  return (
    globalThis.crypto?.randomUUID?.() ??
    `pane-${Date.now()}-${Math.random().toString(36).slice(2)}`
  );
}

export function resumeLaneForTerminalPane(
  pane: TerminalPaneState,
  laneOptions: AgentSessionLaneOptions,
): AgentSessionCreateLane | null {
  if (!pane.agentKind || !pane.laneKind || pane.laneKind === "custom") {
    return null;
  }
  const credentialProfileId =
    activeCredentialProfileId(
      laneOptions,
      pane.agentKind,
      pane.laneKind,
      pane.credentialProfileId,
    ) ?? pane.credentialProfileId;
  try {
    return buildAgentSessionLane(pane.laneKind, credentialProfileId, "");
  } catch {
    return null;
  }
}

export function terminalPaneTitleState(
  pane: TerminalPaneState,
): AgentSessionTitleState {
  return {
    defaultTitle: pane.defaultTitle || agentFallbackTitle(pane.agentKind),
    generatedTitle: pane.generatedTitle,
    aiVaultTitle: pane.aiVaultTitle,
    customTitle: pane.customTitle,
    generation: pane.generation ?? 0,
  };
}

export function terminalPaneResolvedTitle(pane: TerminalPaneState) {
  return resolveAgentTitleLayers(
    terminalPaneTitleState(pane),
    pane.projectName || "Terminal",
  );
}

export function restoredTerminalSession(
  pane: TerminalPaneState,
): TerminalSession {
  return {
    id: pane.sessionId,
    title: pane.customTitle,
    projectId: pane.projectId,
    conversationId: pane.conversationId,
    projectName: pane.projectName,
    cwd: pane.cwd,
    shell: "",
    pid: 0,
    executionProfile: "full_access",
    agentKind: pane.agentKind,
    status: "exited",
    cols: pane.cols,
    rows: pane.rows,
    startedAt: pane.updatedAt,
    exitedAt: pane.updatedAt,
  };
}

function replaceLeafId(
  node: TerminalPaneLayoutNode,
  previousId: string,
  nextId: string,
): TerminalPaneLayoutNode {
  if (node.type === "leaf") {
    return node.leafId === previousId ? { ...node, leafId: nextId } : node;
  }
  return {
    ...node,
    first: replaceLeafId(node.first, previousId, nextId),
    second: replaceLeafId(node.second, previousId, nextId),
  };
}

function activeCredentialProfileId(
  laneOptions: AgentSessionLaneOptions,
  agentKind: AgentKind | undefined,
  laneKind: AgentSessionCreateLaneKind | null,
  preferredId?: string | null,
) {
  if (!agentKind || !laneKind) return null;
  const profiles =
    laneOptions
      .find((option) => option.agentKind === agentKind)
      ?.lanes.find((lane) => lane.kind === laneKind)?.credentialProfiles ?? [];
  return (
    profiles.find(
      (profile) => profile.id === preferredId && profile.status === "active",
    )?.id ??
    profiles.find((profile) => profile.status === "active")?.id ??
    null
  );
}

function validTerminalPane(
  value: TerminalPaneState & { paneId?: unknown },
) {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof value.sessionId === "string" &&
    value.sessionId.length > 0 &&
    (value.paneId === undefined || typeof value.paneId === "string") &&
    (value.conversationId === undefined ||
      value.conversationId === null ||
      typeof value.conversationId === "string") &&
    typeof value.projectName === "string" &&
    typeof value.cwd === "string" &&
    typeof value.executionProfile === "string" &&
    (value.agentKind === null || AGENT_KINDS.has(value.agentKind)) &&
    typeof value.cols === "number" &&
    typeof value.rows === "number" &&
    typeof value.scrollback === "string" &&
    typeof value.updatedAt === "string"
  );
}

function agentFallbackTitle(agentKind: AgentKind | null) {
  if (agentKind === "claude") return "Claude";
  if (agentKind === "codex") return "Codex";
  if (agentKind === "gemini") return "Gemini";
  if (agentKind === "antigravity") return "Antigravity";
  if (agentKind === "kimi") return "Kimi";
  if (agentKind === "zai") return "GLM";
  if (agentKind === "grok") return "Grok";
  return "Terminal";
}

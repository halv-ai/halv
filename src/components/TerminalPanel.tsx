import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import {
  agentPaneViewFor,
  terminalActivityStore,
} from "../terminalActivityStore";
import {
  agentPaneLabel,
  agentPaneTitle,
  type AgentPaneView,
} from "../agentPaneStatus";
import {
  focusTerminalPaneElement,
  terminalFocusBridge,
} from "../terminalFocusBridge";
import { AgentStateGlyph } from "./AgentStateGlyph";
import { ipc, type TerminalOpenArgs } from "../ipc.ts";
import {
  agentSessionTitle,
  agentSessionStore,
  forgetAgentSession,
  type AgentKind,
  type AgentSessionEntry,
  type AgentSessionStatus,
  sessionSavingsMeasurementNote,
  sessionSavingsUnmeasured,
  conversationSavingsTotals,
  listAgentSessionSavedTotals,
} from "../agentSessions";
import {
  closeTerminalLeaf,
  collectTerminalLeafIds,
  deserializeTerminalPanelLayout,
  reconcileTerminalPanelLayout,
  serializeTerminalPanelLayout,
  setTerminalSplitRatio,
  splitTerminalLeaf,
  terminalLayoutHasLeaf,
  type TerminalPaneLayoutNode,
  type TerminalPaneSplitDirection,
  type TerminalPanelLayoutSnapshot,
  type TerminalSplitPath,
  swapTerminalSiblings,
  rotateTerminalSplitAt,
} from "../terminalLayout";
import { middleTruncate, terminalAgentLabel } from "../terminalSurface";
import {
  openTerminalSplitChoice,
  resolveTerminalSplitAgentLane,
  TERMINAL_SPLIT_TERMINAL_CHOICE_ID,
  terminalSplitAgentChoice,
  terminalSplitMenuChoices,
} from "../terminalSplit";
import { openPlainTerminalSession } from "../terminalOpening";
import {
  buildTerminalRestoreWritePayload,
  createTerminalPaneId,
  isTerminalResumeFailure,
  restoreTerminalPanel,
  restoredTerminalSession,
  resumeLaneForTerminalPane,
  supervisorReplayNotice,
  terminalPaneResolvedTitle,
  terminalPaneTitleState,
  type TerminalPaneState,
  type TerminalPanelState,
  type TerminalResumeAttempt,
} from "../terminalRestore";
import {
  applyTerminalContinuations,
  bindPaneToSession,
  createTerminalResumeGuard,
  filterRestoredPanesForAutoResume,
  omitPaneIds,
  paneIdForSession,
  panelOwnsContinuationSource,
  panelListsSession,
  paneWaitsForOldSupervisor,
  reconcileListedTerminalPanes,
  releaseAutoResumePaneClaims,
  type TerminalPanelBinding,
} from "../terminalPaneReconcile";
import {
  agentSessionWaitsForExit,
  parkedTerminalRestorePayload,
  stopTerminalSessionForTabClose,
  clearPendingTerminalForget,
  closeOrphanTerminalSession,
  closeRunningTerminalSession,
  closeTerminalPanelSessions,
  forgetContinuationSource,
  forgetTerminalSession,
  forgetPendingTerminalExit,
  forgetStoppedAgentSession,
  markPendingTerminalForget,
  writeClosedTerminalRestore,
  type TerminalCloseSnapshot,
} from "../terminalClose";
import { registerTerminalShellRequest } from "../terminalShellRequest";
import { restartPaneOnActiveAccount, waitForTerminalExit } from "../accountRestart";
import { useAccountRestartRunner } from "../useAccountRestartRunner";
import {
  terminalPanelRegistry,
  type TerminalPanelPaneFact,
} from "../terminalPanelRegistry";
import { terminalPanelStorageKey } from "../conversationWorkspace";
import { confirmDialog } from "../dialogs";
import { diffRailStore, type DiffRailPaneDescriptor } from "../diffRail";
import {
  collectEditedPaths,
  collectWorkedDirectories,
  createGitRepoDiscoveryCache,
} from "../diffRailRepos";
import {
  effectiveTerminalStatus,
  type TerminalActivityStatus,
} from "../terminalActivity";
import { useT, type Translate } from "../i18n/runtime";
import type {
  Project,
  TerminalContinuedEvent,
  TerminalEvent,
  TerminalSession,
  TerminalSupervisorReconnect,
  TerminalSupervisorReconnectedEvent,
  TerminalSupervisorReconnectingEvent,
  TerminalSupervisorStatus,
} from "../types";
import {
  shortcutForAction,
  shortcutMatchesEvent,
  type KeyboardShortcutOverrides,
} from "../shortcuts";
import { agentLabel } from "./AgentSessionView";
import { SessionSavingsMeter } from "./SessionSavingsMeter";
import { useRoutingSavingsReport } from "../useRoutingSavings";
import { laneIsPlan, routingSavingsFromTotals } from "../savingsMethod";
import { TerminalPane, type TerminalPaneSplitChoice } from "./TerminalPane";

type TerminalPanelProps = {
  agentEntry?: AgentSessionEntry | null;
  conversationApprovalPolicy?: string | null;
  conversationId?: string | null;
  conversationModel?: string | null;
  conversationTitle?: string | null;
  open: boolean;
  project: Project | null;
  projects?: Project[];
  workspaceId: string | null;
  focusSessionId?: string | null;
  shortcutOverrides: KeyboardShortcutOverrides;
  onClose: () => void;
  onActivePaneChange?: (sessionId: string | null) => void;
  onOpenAgentTerminal?: () => void;
  onPromptSubmitGate?: () => boolean;
  onPromptSubmitSignalHandled?: (sessionId: string, signal: number) => void;
  onPanesChange?: (panes: DiffRailPaneDescriptor[]) => void;
  onRestoreAvailable?: () => void;
  onSessionsChanged?: (sessions: TerminalSession[]) => void;
  onSessionAttached?: (sessionId: string) => void;
  onSessionAttachError?: (sessionId: string, message: string) => void;
  promptSubmitSignals?: Readonly<Record<string, number>>;
  // False while an inactive conversation surface hides the panel.
  visible?: boolean;
  /** A tab body of the workspace strip: the panel fills the body. */
  fill?: boolean;
  /** With workspace tabs, the panel takes part in the mount registry:
   *  one mounted panel per scope (ADR 0010, P4). Without it, the panel
   *  runs as before. */
  claimScope?: boolean;
  /** The agent that the tab starts now. The empty panel says so instead
   *  of offering a choice. */
  startingLabel?: string | null;
  /** A shell tab body (ADR 0010): the panel shows the shells of this shell
   *  tab only, and its new shells belong to it. */
  shellTabId?: string | null;
  /** A new shell tab: the panel opens one shell when it finds no pane. */
  openShellOnMount?: boolean;
  /** The shell that `openShellOnMount` opened. */
  onShellOpenedOnMount?: (session: TerminalSession) => void;
  /** The shell tab rules (ADR 0010, 9.4 and 9.8): a plain shell that exits
   *  with 0 closes its pane, a shell that fails offers Restart, an idle
   *  shell closes without a prompt, and the last closed pane closes the
   *  tab through `onLastPaneClosed`. */
  shellExitRules?: boolean;
  onLastPaneClosed?: () => void;
  /** The live title (OSC) of the active pane, for the title of a shell tab. */
  onActiveTitleChange?: (title: string | null) => void;
  /** A tab body has no header of its own (ADR 0010, S7): its pane title
   *  bars are the only header, as in Orca. The visible body shows its
   *  savings meter in this element of the app header. */
  headerSlot?: HTMLElement | null;
};

type SurfaceStatus = AgentSessionStatus | TerminalSession["status"];

const EMPTY_LAYOUT: TerminalPanelLayoutSnapshot = {
  version: 1,
  activeTabId: null,
  activeLeafId: null,
  tabs: [],
};
// A runaway backstop, not a product limit — Orca runs with no cap at all.
const MAX_TERMINAL_SESSIONS = 32;
const MIN_PANE_SIZE = 200;
const SPLIT_AGENT_KINDS: AgentKind[] = [
  "claude",
  "codex",
  "gemini",
  "antigravity",
  "kimi",
  "zai",
  "grok",
];
const NOOP = () => undefined;
const EMPTY_PROMPT_SUBMIT_SIGNALS: Readonly<Record<string, number>> = {};
// Module scope on purpose: a remount or a second panel on the same saved
// state must never auto-resume the same pane twice. Manual Resume stays
// available for panes whose automatic attempt failed.
const autoResumeClaimedPaneIds = new Set<string>();
// Bounded retry for auto-resume: transient startup failures (the sidecar
// still booting) release the claim after a delay, capped so a persistent
// failure never loops create/kill. Manual Resume ignores all of this.
const autoResumeAttempts = new Map<string, number>();
const AUTO_RESUME_MAX_ATTEMPTS = 3;
const AUTO_RESUME_RETRY_DELAY_MS = 8_000;
const gitRootByDirectory = new Map<string, Promise<string | null>>();
const discoverGitRepos = createGitRepoDiscoveryCache((directory) =>
  ipc.project.gitReposUnderPath({
    path: directory,
  }).catch(() => []),
);

type PaneRootInput = {
  sessionId: string;
  directories: string[];
  discoveryGeneration: number;
  key: string;
};

type PaneRootResolution = {
  key: string;
  gitRoot: string | null;
  gitRoots: string[];
};

type TrackedResumeAttempt = TerminalResumeAttempt & {
  pane: TerminalPaneState;
};

type TerminalResumeFailure = {
  exitCode: number | null;
  exitedAt: string;
};

function scheduleAutoResumeRetry(paneId: string) {
  const attempts = (autoResumeAttempts.get(paneId) ?? 0) + 1;
  autoResumeAttempts.set(paneId, attempts);
  if (attempts >= AUTO_RESUME_MAX_ATTEMPTS) return;
  setTimeout(() => {
    autoResumeClaimedPaneIds.delete(paneId);
  }, AUTO_RESUME_RETRY_DELAY_MS * attempts);
}

function editedPathParent(path: string) {
  const separator = path.includes("\\") ? "\\" : "/";
  const index = path.lastIndexOf(separator);
  if (index < 0) return path;
  if (index === 0) return separator;
  if (index === 2 && /^[A-Za-z]:/.test(path)) return path.slice(0, 3);
  return path.slice(0, index);
}

function resolveGitRoot(directory: string) {
  const cached = gitRootByDirectory.get(directory);
  if (cached) return cached;
  const request = ipc.project.gitRootForPath({
    path: directory,
  }).catch(() => {
    // Do not cache a backend failure. The next call retries.
    gitRootByDirectory.delete(directory);
    return null;
  });
  gitRootByDirectory.set(directory, request);
  return request;
}

// Sessions the user explicitly created (splits, new tabs, sends, resumes).
// Only these may APPEND as new tabs; any other live session must replace a
// saved leaf or stay out of the layout - implicit appends are how restore
// corruption spread.
export const expectedTerminalSessionIds = new Set<string>();

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function isSupervisorTerminalFailure(message: string | undefined) {
  return message?.toLocaleLowerCase().includes("supervisor") ?? false;
}

function terminalStatusLabel(session: TerminalSession, t: Translate) {
  if (session.status === "running") return t("running");
  if (session.status === "failed") return t("failed");
  if (session.signal) return session.signal;
  return t("exit {exitCode}", { exitCode: session.exitCode ?? "?" });
}

/** The display state of a running agent pane, or null for other panes. */
function runningAgentPaneView(
  snapshot: ReturnType<typeof terminalActivityStore.getSnapshot>,
  session: TerminalSession | null | undefined,
): AgentPaneView | null {
  return session?.agentKind && session.status === "running"
    ? agentPaneViewFor(snapshot, session.id)
    : null;
}

function surfaceStatusLabel(status: SurfaceStatus, t: Translate) {
  if (status === "waiting_input") return t("waiting");
  if (status === "starting") return t("starting");
  if (status === "running") return t("running");
  if (status === "exited") return t("exited");
  return t("failed");
}

function terminalSavingsMeasurementNote(
  laneKind: string,
  savingsSources: AgentSessionEntry["session"]["savingsSources"],
  t: Translate,
) {
  const note = sessionSavingsMeasurementNote(laneKind, savingsSources);
  if (note === "crux + RTK only · proxy off") {
    return t("crux + RTK only · proxy off");
  }
  if (note === "crux + RTK · proxy on") {
    return t("crux + RTK · proxy on");
  }
  if (note === "RTK hook not active") {
    return t("RTK hook not active");
  }
  if (note === "crux + RTK (by instruction) · proxy on") {
    return t("crux + RTK (by instruction) · proxy on");
  }
  if (note === "crux + RTK (by instruction) · proxy off") {
    return t("crux + RTK (by instruction) · proxy off");
  }
  if (note === "crux only · proxy off") {
    return t("crux only · proxy off");
  }
  return note;
}

function terminalSupervisorReplayMessage(
  notice: NonNullable<ReturnType<typeof supervisorReplayNotice>>,
  t: Translate,
) {
  const { message } = notice;
  if (
    message ===
    "Halv could not start the terminal supervisor. Saved agent sessions will resume from their transcripts; in-flight work may be lost."
  ) {
    return t(
      "Halv could not start the terminal supervisor. Saved agent sessions will resume from their transcripts; in-flight work may be lost.",
    );
  }
  if (
    message ===
    "Agents from an older Halv are still running. Halv cannot connect to them."
  ) {
    return t(
      "Agents from an older Halv are still running. Halv cannot connect to them.",
    );
  }
  return message;
}

function terminalAgentMark(
  entry: AgentSessionEntry | null,
  agentKind?: TerminalSession["agentKind"],
) {
  const kind = entry?.session.agentKind ?? agentKind;
  if (!kind) return ">_";
  if (kind === "claude") return "C";
  if (kind === "codex") return "X";
  if (kind === "gemini") return "✦";
  if (kind === "antigravity") return "A";
  if (kind === "kimi") return "K";
  return "G";
}

function terminalTitle(
  session: TerminalSession,
  titles: Record<string, string>,
  agentEntry: AgentSessionEntry | null,
) {
  if (agentEntry) return agentSessionTitle(agentEntry.session);
  const terminalCustom = session.title?.trim();
  if (terminalCustom) return terminalCustom;
  const reported = titles[session.id]?.trim();
  if (reported) return reported;
  return terminalAgentLabel(session.agentKind) ?? session.projectName;
}

function readStoredLayout(key: string) {
  try {
    return deserializeTerminalPanelLayout(window.localStorage.getItem(key));
  } catch {
    return null;
  }
}

function writeStoredLayout(key: string, layout: TerminalPanelLayoutSnapshot) {
  try {
    window.localStorage.setItem(key, serializeTerminalPanelLayout(layout));
  } catch {
    // Storage can be unavailable in restricted webviews.
  }
}

function allLayoutLeafIds(layout: TerminalPanelLayoutSnapshot) {
  return layout.tabs.flatMap((tab) => collectTerminalLeafIds(tab.root));
}

export function TerminalPanel({
  agentEntry = null,
  conversationApprovalPolicy = null,
  conversationId = null,
  conversationModel = null,
  conversationTitle = null,
  open,
  project,
  projects,
  workspaceId,
  focusSessionId = null,
  shortcutOverrides,
  onClose,
  onActivePaneChange = NOOP,
  onOpenAgentTerminal,
  onPromptSubmitGate,
  onPromptSubmitSignalHandled = NOOP,
  onPanesChange = NOOP,
  onRestoreAvailable = NOOP,
  onSessionsChanged = NOOP,
  onSessionAttached = NOOP,
  onSessionAttachError = NOOP,
  promptSubmitSignals = EMPTY_PROMPT_SUBMIT_SIGNALS,
  visible = true,
  fill = false,
  claimScope = false,
  startingLabel = null,
  shellTabId = null,
  openShellOnMount = false,
  onShellOpenedOnMount,
  shellExitRules = false,
  onLastPaneClosed,
  onActiveTitleChange,
  headerSlot = null,
}: TerminalPanelProps) {
  const t = useT();
  const agentSessionsState = useSyncExternalStore(
    agentSessionStore.subscribe,
    agentSessionStore.getSnapshot,
  );
  const diffRailState = useSyncExternalStore(
    diffRailStore.subscribe,
    diffRailStore.getSnapshot,
  );
  const diffRailDiscoveryRefreshKey =
    open && diffRailState.ui.open
      ? JSON.stringify(
          Object.entries(diffRailState.snapshots).map(([key, snapshot]) => [
            key,
            snapshot.requestId,
            snapshot.fetchState,
            snapshot.stale,
          ]),
        )
      : null;
  // A shell tab owns plain shells only. It has no conversation.
  const boundConversationId = shellTabId
    ? null
    : (conversationId ?? agentEntry?.session.conversationId ?? null);
  const boundConversationTitle =
    conversationTitle ?? agentEntry?.session.conversationTitle ?? t("Conversation");
  const boundConversationModel =
    conversationModel ?? agentEntry?.session.model ?? "gpt-5.5";
  const storageKey = terminalPanelStorageKey({
    conversationId: boundConversationId,
    shellTabId,
    agentSessionId: agentEntry?.session.id ?? null,
    workspaceId,
    projectId: project?.id ?? null,
  });
  // Mount registry (ADR 0010, P4): only the panel that holds the scope
  // restores, refreshes, persists and resumes it. A second panel on the
  // same scope waits and shows a short note.
  const [claimToken] = useState(() => Symbol("terminal-panel"));
  const [registryClaimed, setClaimed] = useState(false);
  const claimed = claimScope ? registryClaimed : true;
  useLayoutEffect(() => {
    if (!claimScope) return undefined;
    const tryClaim = () =>
      setClaimed(terminalPanelRegistry.claim(storageKey, claimToken));
    tryClaim();
    const unsubscribe = terminalPanelRegistry.subscribe(() => {
      if (!terminalPanelRegistry.holds(storageKey, claimToken)) tryClaim();
    });
    return () => {
      unsubscribe();
      terminalPanelRegistry.release(storageKey, claimToken);
      setClaimed(false);
    };
  }, [claimScope, claimToken, storageKey]);
  // A tab close freezes the panel: no refresh, no persistence and no
  // auto-resume until it unmounts, or until the close fails.
  const closingGuardRef = useRef(false);
  const [sessions, setSessions] = useState<TerminalSession[]>([]);
  const [paneSessionIds, setPaneSessionIds] = useState<Record<string, string>>(
    {},
  );
  const [restoredPanes, setRestoredPanes] = useState<
    Record<string, TerminalPaneState>
  >({});
  const [resumeFallbackPanes, setResumeFallbackPanes] = useState<
    Record<string, TerminalPaneState>
  >({});
  const [resumeFailures, setResumeFailures] = useState<
    Record<string, TerminalResumeFailure>
  >({});
  const [layout, setLayout] = useState<TerminalPanelLayoutSnapshot>(() =>
    readStoredLayout(storageKey) ?? { ...EMPTY_LAYOUT },
  );
  const [sessionTitles, setSessionTitles] = useState<Record<string, string>>({});
  const [opening, setOpening] = useState(false);
  const [openingLeafIds, setOpeningLeafIds] = useState<Set<string>>(new Set());
  const [closingSessionIds, setClosingSessionIds] = useState<Set<string>>(
    new Set(),
  );
  const [error, setError] = useState<string | null>(null);
  const [supervisorFallbackMessage, setSupervisorFallbackMessage] = useState<
    string | null
  >(null);
  const [supervisorReconnectBySession, setSupervisorReconnectBySession] =
    useState<Record<string, TerminalSupervisorReconnect>>({});
  const [supervisorFailureBySession, setSupervisorFailureBySession] = useState<
    Record<string, string>
  >({});
  const [retryingSupervisorSessionIds, setRetryingSupervisorSessionIds] =
    useState<Set<string>>(new Set());
  const [terminalAttachGenerations, setTerminalAttachGenerations] = useState<
    Record<string, number>
  >({});
  const [supervisorReplayAlert, setSupervisorReplayAlert] = useState<
    ReturnType<typeof supervisorReplayNotice>
  >(null);
  const [allRunningCount, setAllRunningCount] = useState(0);
  const [restoreReadyKey, setRestoreReadyKey] = useState<string | null>(null);
  const [refreshedKey, setRefreshedKey] = useState<string | null>(null);
  const [resumingSessionIds, setResumingSessionIds] = useState<Set<string>>(
    new Set(),
  );
  const [resumingAll, setResumingAll] = useState(false);
  // A tab body shows "Resume all" as a banner. Auto-resume handles most
  // saved panes within a moment, so the banner waits and shows only for
  // two or more panes that stay saved.
  const restoredPaneCount = Object.keys(restoredPanes).length;
  const [resumeBannerShown, setResumeBannerShown] = useState(false);
  useEffect(() => {
    if (!fill || restoredPaneCount < 2) {
      setResumeBannerShown(false);
      return undefined;
    }
    const timer = window.setTimeout(() => setResumeBannerShown(true), 600);
    return () => window.clearTimeout(timer);
  }, [fill, restoredPaneCount]);
  const [terminalFocused, setTerminalFocused] = useState(false);
  const activitySnapshot = useSyncExternalStore(
    terminalActivityStore.subscribe,
    terminalActivityStore.getSnapshot,
  );
  const activityStatusBySession = activitySnapshot.statuses;
  const [rootResolutionBySession, setRootResolutionBySession] = useState<
    Record<string, PaneRootResolution>
  >({});
  const [rootDiscoveryRefreshSignal, setRootDiscoveryRefreshSignal] =
    useState(0);
  const layoutRef = useRef(layout);
  const loadedStorageKeyRef = useRef(storageKey);
  const skipPersistKeyRef = useRef<string | null>(storageKey);
  const pendingForgetRef = useRef(new Set<string>());
  const pendingAgentForgetRef = useRef(new Set<string>());
  const dismissedSessionIdsRef = useRef(new Set<string>());
  const resumeAttemptsRef = useRef(new Map<string, TrackedResumeAttempt>());
  // A supervisor with another protocol can still run the saved agents.
  const supervisorProtocolMismatchRef = useRef(false);
  const resumeFailuresRef = useRef(resumeFailures);
  // Every resume is a frozen layout transaction. Session-list refreshes and
  // persistence must stand down until the replacement is bound to its saved
  // pane, or the replacement can be appended as a second pane mid-resume.
  const resumeGuardRef = useRef(createTerminalResumeGuard());
  // A scope whose terminal_restore_read failed must stay read-only: a write
  // in that state replaces the saved panes with an empty list and destroys
  // the only copy of the restore state.
  const restoreWriteBlockedKeysRef = useRef(new Set<string>());
  const lastRestoreWriteRef = useRef<string | null>(null);
  // Async flows check this ref after every await. Leaving the conversation
  // mid-flight must abort their state writes, or the persistence effect
  // saves one conversation's panes under the other conversation's scope.
  const storageKeyRef = useRef(storageKey);
  const paneSessionIdsRef = useRef(paneSessionIds);
  const restoredPanesRef = useRef(restoredPanes);
  const resumeFallbackPanesRef = useRef(resumeFallbackPanes);
  const resumingSessionIdsRef = useRef(resumingSessionIds);
  const sessionsRef = useRef(sessions);
  const onSessionsChangedRef = useRef(onSessionsChanged);
  const shellExitRulesRef = useRef(shellExitRules);
  const onLastPaneClosedRef = useRef(onLastPaneClosed);
  const onShellOpenedOnMountRef = useRef(onShellOpenedOnMount);
  // The exit listener closes a pane through the current closeSession.
  const closeSessionRef = useRef<
    (session: TerminalSession, options?: { prompt?: boolean }) => Promise<void>
  >(async () => undefined);
  const rootDiscoveryActiveRef = useRef(open && diffRailState.ui.open);
  layoutRef.current = layout;
  storageKeyRef.current = storageKey;
  paneSessionIdsRef.current = paneSessionIds;
  restoredPanesRef.current = restoredPanes;
  resumeFallbackPanesRef.current = resumeFallbackPanes;
  resumeFailuresRef.current = resumeFailures;
  resumingSessionIdsRef.current = resumingSessionIds;
  sessionsRef.current = sessions;
  onSessionsChangedRef.current = onSessionsChanged;
  shellExitRulesRef.current = shellExitRules;
  onLastPaneClosedRef.current = onLastPaneClosed;
  onShellOpenedOnMountRef.current = onShellOpenedOnMount;
  rootDiscoveryActiveRef.current = open && diffRailState.ui.open;

  useEffect(
    () => terminalActivityStore.subscribePanes((_sessionId, view, previous) => {
      // A turn that rests or waits can have touched new repositories.
      if (
        previous?.status === "working" &&
        view?.status !== "working" &&
        rootDiscoveryActiveRef.current
      ) {
        setRootDiscoveryRefreshSignal((current) => current + 1);
      }
    }),
    [],
  );

  useEffect(() => {
    const refreshDiscovery = () => {
      if (!rootDiscoveryActiveRef.current) return;
      setRootDiscoveryRefreshSignal((current) => current + 1);
    };
    window.addEventListener("focus", refreshDiscovery);
    return () => window.removeEventListener("focus", refreshDiscovery);
  }, []);

  const agentSessionId = agentEntry?.session.id ?? null;
  const activeTab = useMemo(
    () => layout.tabs.find((tab) => tab.tabId === layout.activeTabId) ?? null,
    [layout.activeTabId, layout.tabs],
  );
  const activeSession = useMemo(
    () => {
      const sessionId = layout.activeLeafId
        ? paneSessionIds[layout.activeLeafId]
        : null;
      return sessions.find((session) => session.id === sessionId) ?? null;
    },
    [layout.activeLeafId, paneSessionIds, sessions],
  );
  const agentEntryForSession = (sessionId: string) =>
    agentSessionsState.entries.find(
      (entry) => entry.session.id === sessionId,
    ) ?? (sessionId === agentSessionId ? agentEntry : null);
  const activeAgentEntry = activeSession
    ? agentEntryForSession(activeSession.id)
    : null;
  const nextPaneRootInputs = useMemo<PaneRootInput[]>(
    () =>
      sessions.map((session) => {
        const entry =
          agentSessionsState.entries.find(
            (candidate) => candidate.session.id === session.id,
          ) ?? (session.id === agentSessionId ? agentEntry : null);
        const directories = [
          session.cwd,
          ...collectWorkedDirectories(entry?.transcript ?? []),
          ...collectEditedPaths(entry?.transcript ?? []).map(editedPathParent),
        ].filter(
          (directory, index, values) => values.indexOf(directory) === index,
        );
        // The completed turn count stays stable during a turn and advances when the turn ends.
        const discoveryGeneration = entry
          ? Math.max(0, entry.turnSequence - (entry.turnActive ? 1 : 0))
          : 0;
        return {
          sessionId: session.id,
          directories,
          discoveryGeneration,
          key: JSON.stringify([directories, discoveryGeneration]),
        };
      }),
    [agentEntry, agentSessionId, agentSessionsState.entries, sessions],
  );
  const paneRootInputsKey = JSON.stringify(
    nextPaneRootInputs.map((input) => [input.sessionId, input.key]),
  );
  const paneRootInputsRef = useRef<{
    key: string;
    inputs: PaneRootInput[];
  } | null>(null);
  if (paneRootInputsRef.current?.key !== paneRootInputsKey) {
    paneRootInputsRef.current = {
      key: paneRootInputsKey,
      inputs: nextPaneRootInputs,
    };
  }
  const paneRootInputs = paneRootInputsRef.current.inputs;
  useEffect(() => {
    let cancelled = false;
    void Promise.all(
      paneRootInputs.map(async (input) => {
        const roots = await Promise.all(input.directories.map(resolveGitRoot));
        const discoveredRoots = roots[0]
          ? []
          : await discoverGitRepos(
              input.directories[0],
              input.discoveryGeneration,
            );
        return {
          sessionId: input.sessionId,
          resolution: {
            key: input.key,
            gitRoot: roots[0] ?? null,
            gitRoots: [
              ...new Set(
                [
                  ...roots.filter((root): root is string => Boolean(root)),
                  ...discoveredRoots,
                ],
              ),
            ],
          },
        };
      }),
    ).then((rows) => {
      if (cancelled) return;
      setRootResolutionBySession((current) => {
        const next = Object.fromEntries(
          rows.map(({ sessionId, resolution }) => [sessionId, resolution]),
        );
        return JSON.stringify(current) === JSON.stringify(next) ? current : next;
      });
    });
    return () => {
      cancelled = true;
    };
  }, [
    diffRailDiscoveryRefreshKey,
    paneRootInputs,
    rootDiscoveryRefreshSignal,
  ]);
  const publishedPanes = useMemo<DiffRailPaneDescriptor[]>(
    () =>
      sessions.map((session) => {
        const entry =
          agentSessionsState.entries.find(
            (candidate) => candidate.session.id === session.id,
          ) ?? (session.id === agentSessionId ? agentEntry : null);
        const rootInput = paneRootInputs.find(
          (candidate) => candidate.sessionId === session.id,
        );
        const rootResolution = rootResolutionBySession[session.id];
        const resolvedRoots =
          rootResolution?.key === rootInput?.key ? rootResolution : null;
        return {
          sessionId: session.id,
          cwd: session.cwd,
          gitRoot: resolvedRoots?.gitRoot ?? null,
          gitRoots: resolvedRoots?.gitRoots ?? [],
          projectId:
            session.projectId ?? entry?.session.projectId ?? project?.id ?? null,
          title: terminalTitle(session, sessionTitles, entry),
          agentKind: entry?.session.agentKind ?? session.agentKind ?? null,
          sessionEnded:
            session.status !== "running" || entry?.session.status === "exited",
        };
      }),
    [
      agentEntry,
      agentSessionId,
      agentSessionsState.entries,
      paneRootInputs,
      project?.id,
      rootResolutionBySession,
      sessionTitles,
      sessions,
    ],
  );
  const publishedPanesKey = JSON.stringify(publishedPanes);
  useEffect(() => {
    onPanesChange(open ? publishedPanes : []);
    // Session refreshes can replace descriptors without changing their content.
    // Publishing those copies would notify this panel's store subscription again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onPanesChange, open, publishedPanesKey]);
  useEffect(() => () => onPanesChange([]), [onPanesChange]);
  useEffect(() => {
    onActivePaneChange(open ? activeSession?.id ?? null : null);
  }, [activeSession?.id, onActivePaneChange, open]);
  useEffect(() => () => onActivePaneChange(null), [onActivePaneChange]);
  // A shell tab shows the live title of its active pane.
  const activeLiveTitle = activeSession
    ? sessionTitles[activeSession.id]?.trim() || null
    : null;
  const onActiveTitleChangeRef = useRef(onActiveTitleChange);
  onActiveTitleChangeRef.current = onActiveTitleChange;
  useEffect(() => {
    onActiveTitleChangeRef.current?.(activeLiveTitle);
  }, [activeLiveTitle]);
  const displayedStatus: SurfaceStatus | null = activeSession
    ? effectiveTerminalStatus(
        activeSession.status,
        activeAgentEntry?.session.status,
        activityStatusBySession[activeSession.id],
        activeAgentEntry?.session.mode,
      )
    : null;
  const activePaneView = runningAgentPaneView(activitySnapshot, activeSession);
  const [conversationSavedTotals, setConversationSavedTotals] = useState<{
    savingsTokens: number;
    cacheReadTokens: number;
    dollarsUsd: number;
  } | null>(null);
  // Cheap refetch key: the query only reruns when savings actually move,
  // not on every store commit during streaming.
  const conversationSavingsKey = useMemo(
    () =>
      agentSessionsState.entries.reduce(
        (total, entry) => total + entry.totals.savingsTokens,
        0,
      ),
    [agentSessionsState.entries],
  );
  // The conversation meter survives closed tabs: it sums the persisted
  // turns of every terminal session this conversation ever ran.
  useEffect(() => {
    let cancelled = false;
    if (!boundConversationId) {
      setConversationSavedTotals(null);
      return;
    }
    void listAgentSessionSavedTotals()
      .then((rows) => {
        if (cancelled) return;
        setConversationSavedTotals(
          conversationSavingsTotals(rows, boundConversationId),
        );
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [boundConversationId, conversationSavingsKey]);

  // The JEV routing dollars of the conversation, else of the session.
  const routingReport = useRoutingSavingsReport(
    boundConversationId,
    agentEntry?.session.id ?? null,
  );

  const [projectPicker, setProjectPicker] = useState<{
    source: TerminalSession;
    direction: TerminalPaneSplitDirection;
    projectId: number | null;
    agentKind: AgentKind;
  } | null>(null);

  const splitAgentChoices = useMemo<TerminalPaneSplitChoice[]>(
    () =>
      SPLIT_AGENT_KINDS.map((agentKind) => {
        const choice = terminalSplitAgentChoice(
          agentSessionsState.laneOptions,
          agentKind,
        );
        return {
          id: agentKind,
          label: choice.label,
          ready: choice.ready,
          readiness:
            choice.readiness === "Login or saved key required"
              ? t("Login or saved key required")
              : choice.readiness === "Signed in"
                ? t("Signed in")
                : choice.readiness,
        };
      }),
    [agentSessionsState.laneOptions, t],
  );

  // One entry opens a small picker: project + agent for the new pane.
  const combinedSplitChoices = useMemo<TerminalPaneSplitChoice[]>(
    () =>
      // The pane is the source of the shell, so the choice works in every
      // pane, with or without a selected project.
      // A shell tab owns plain shells only, so its split menu offers
      // "Terminal" only.
      terminalSplitMenuChoices(
        true,
        t("Terminal"),
        t("opens a clean shell"),
        shellTabId ? [] : splitAgentChoices,
        !shellTabId && (projects ?? []).some((item) => item.id !== project?.id)
          ? {
              id: "other-project",
              label: t("Open agent in another project\u2026"),
              ready: true,
              readiness: t("pick project & agent"),
            }
          : null,
      ),
    [splitAgentChoices, projects, project?.id, shellTabId, t],
  );

  useEffect(() => {
    if (!claimed) return undefined;
    skipPersistKeyRef.current = storageKey;
    loadedStorageKeyRef.current = storageKey;
    const stored = readStoredLayout(storageKey) ?? { ...EMPTY_LAYOUT };
    layoutRef.current = stored;
    setLayout(stored);
    setSessions([]);
    setPaneSessionIds({});
    paneSessionIdsRef.current = {};
    setRestoredPanes({});
    setResumeFallbackPanes({});
    setResumeFailures({});
    resumeFailuresRef.current = {};
    setRestoreReadyKey(null);
    setRefreshedKey(null);
    setSupervisorReplayAlert(null);
    setSupervisorReconnectBySession({});
    setSupervisorFailureBySession({});
    setRetryingSupervisorSessionIds(new Set());
    setTerminalAttachGenerations({});
    dismissedSessionIdsRef.current.clear();
    resumeAttemptsRef.current.clear();
    supervisorProtocolMismatchRef.current = false;
    let disposed = false;
    const restoreRequest = ipc.terminal.restoreRead({ scope: storageKey }).then(async (saved) => {
      const status = await ipc.terminal.supervisorStatus().catch(
          (): TerminalSupervisorStatus => ({
            available: false,
            socketPath: "",
            sessions: 0,
            runningSessions: 0,
            supervisorVersion: null,
            supervisorStale: false,
            appVersion: "",
            message: null,
          }),
      );
      if (!disposed) {
        supervisorProtocolMismatchRef.current = status.protocolMismatch === true;
        setSupervisorReplayAlert(
          supervisorReplayNotice(saved?.panes ?? [], status),
        );
      }
      return restoreTerminalPanel(saved, boundConversationId);
    });
    void restoreRequest
      .then((restored) => {
        if (disposed) return;
        // The saved layout applies even when every pane was filtered out:
        // its leaves can belong to live sessions the refresh attaches next,
        // and starting from an empty layout flattens their splits.
        if (restored.sessions.length > 0 || restored.layout.tabs.length > 0) {
          layoutRef.current = restored.layout;
          setLayout(restored.layout);
          setSessions(restored.sessions);
          setPaneSessionIds(restored.paneSessionIds);
          paneSessionIdsRef.current = restored.paneSessionIds;
          setRestoredPanes(restored.panes);
          setSessionTitles(
            Object.fromEntries(
              Object.entries(restored.panes).map(([paneId, pane]) => [
                restored.paneSessionIds[paneId] ?? pane.sessionId,
                terminalPaneResolvedTitle(pane),
              ]),
            ),
          );
        }
        if (restored.sessions.length > 0) {
          onSessionsChanged(restored.sessions);
          onRestoreAvailable();
        }
        restoreWriteBlockedKeysRef.current.delete(storageKey);
        setRestoreReadyKey(storageKey);
      })
      .catch((cause) => {
        if (disposed) return;
        restoreWriteBlockedKeysRef.current.add(storageKey);
        setError(errorMessage(cause));
        setRestoreReadyKey(storageKey);
      });
    return () => {
      disposed = true;
    };
  }, [claimed, storageKey]);

  useEffect(() => {
    if (!claimed || closingGuardRef.current) return;
    if (skipPersistKeyRef.current === storageKey) {
      skipPersistKeyRef.current = null;
      return;
    }
    if (loadedStorageKeyRef.current === storageKey) {
      writeStoredLayout(storageKey, layout);
    }
  }, [claimed, layout, storageKey]);

  useEffect(() => {
    if (!claimed || closingGuardRef.current) return;
    if (restoreReadyKey !== storageKey) return;
    if (restoreWriteBlockedKeysRef.current.has(storageKey)) return;
    if (resumeGuardRef.current.active()) return;
    const write = buildTerminalRestoreWritePayload({
      scope: storageKey,
      layout,
      sessions,
      paneSessionIds,
      agentEntries: agentSessionsState.entries,
      restoredPanes,
      resumeFallbackPanes,
      laneOptions: agentSessionsState.laneOptions,
    });
    // The poller commits entries every few seconds; identical payloads
    // must not rewrite the whole state file each tick.
    const payload = JSON.stringify(write);
    if (lastRestoreWriteRef.current === payload) return;
    lastRestoreWriteRef.current = payload;
    void ipc.terminal.restoreWrite(write).catch((cause) =>
      setError(errorMessage(cause)),
    );
  }, [
    agentSessionsState.entries,
    agentSessionsState.laneOptions,
    claimed,
    layout,
    paneSessionIds,
    restoreReadyKey,
    restoredPanes,
    resumeFallbackPanes,
    sessions,
    storageKey,
  ]);

  useEffect(() => {
    setResumeFallbackPanes((current) => {
      const next = { ...current };
      let changed = false;
      for (const entry of agentSessionsState.entries) {
        const paneId = paneIdForSession(
          paneSessionIdsRef.current,
          entry.session.id,
        );
        if (entry.session.cliSessionId && paneId && next[paneId]) {
          delete next[paneId];
          changed = true;
        }
      }
      return changed ? next : current;
    });
    if (resumeGuardRef.current.active()) return;
    // A live session can resume while another conversation is visible.
    // The saved pane then adopts that session without changing its leaf.
    const liveCliSessionIds = new Set(
      agentSessionsState.entries
        .filter((entry) =>
          ["starting", "running", "waiting_input"].includes(
            entry.session.status,
          ),
        )
        .map((entry) => entry.session.cliSessionId)
        .filter((cliSessionId): cliSessionId is string =>
          Boolean(cliSessionId),
        ),
    );
    const stalePaneIds = new Set(
      Object.entries(restoredPanesRef.current)
        .filter(
          ([paneId, pane]) =>
            pane.cliSessionId &&
            liveCliSessionIds.has(pane.cliSessionId) &&
            // A resume in flight still owns its current session binding.
            !resumingSessionIds.has(
              paneSessionIdsRef.current[paneId] ?? pane.sessionId,
            ) &&
            !agentSessionsState.entries.some(
              (entry) =>
                entry.session.id ===
                (paneSessionIdsRef.current[paneId] ?? pane.sessionId),
            ),
        )
        .map(([paneId]) => paneId),
    );
    if (stalePaneIds.size === 0) return;
    // A live CLI session adopts one saved pane. Other panes with the same
    // CLI session id stay saved because their pane ids remain distinct.
    const bindings = new Map<string, string>();
    const takenLive = new Set<string>();
    for (const paneId of stalePaneIds) {
      const pane = restoredPanesRef.current[paneId];
      if (!pane?.cliSessionId) continue;
      const previousSessionId =
        paneSessionIdsRef.current[paneId] ?? pane.sessionId;
      if (dismissedSessionIdsRef.current.has(previousSessionId)) continue;
      const live = agentSessionsState.entries.find(
        (entry) =>
          entry.session.cliSessionId === pane.cliSessionId &&
          entry.session.id !== previousSessionId &&
          !takenLive.has(entry.session.id) &&
          ["starting", "running", "waiting_input"].includes(
            entry.session.status,
          ),
      );
      if (live) {
        takenLive.add(live.session.id);
        bindings.set(paneId, live.session.id);
      }
    }
    if (bindings.size === 0) return;
    const oldSessionIds = new Set(
      [...bindings.keys()].flatMap((paneId) => {
        const sessionId = paneSessionIdsRef.current[paneId];
        return sessionId ? [sessionId] : [];
      }),
    );
    setRestoredPanes((current) => {
      const next = { ...current };
      for (const paneId of bindings.keys()) delete next[paneId];
      return next;
    });
    setSessions((current) =>
      current.filter((row) => !oldSessionIds.has(row.id)),
    );
    const nextPaneSessionIds = [...bindings].reduce(
      (current, [paneId, sessionId]) =>
        bindPaneToSession(current, paneId, sessionId),
      paneSessionIdsRef.current,
    );
    paneSessionIdsRef.current = nextPaneSessionIds;
    setPaneSessionIds(nextPaneSessionIds);
  }, [agentSessionsState.entries, resumingSessionIds]);

  // A new shell tab opens its first shell once, when the panel has read its
  // scope and found no pane. A reload of an empty tab shows "Open shell".
  const [firstShellKey, setFirstShellKey] = useState<string | null>(null);
  const firstShellPending = openShellOnMount && firstShellKey !== storageKey;
  useEffect(() => {
    if (
      !firstShellPending ||
      !claimed ||
      !open ||
      closingGuardRef.current ||
      restoreReadyKey !== storageKey ||
      refreshedKey !== storageKey
    ) {
      return;
    }
    setFirstShellKey(storageKey);
    if (allLayoutLeafIds(layoutRef.current).length > 0 || sessionsRef.current.length > 0) {
      return;
    }
    void openTerminal().then((session) => {
      if (session) onShellOpenedOnMountRef.current?.(session);
    });
    // openTerminal reads refs and the current props.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [claimed, firstShellPending, open, refreshedKey, restoreReadyKey, storageKey]);

  // Reopening the app must come back to the previous live state, so saved
  // panes resume on their own: shells right away, agent panes once the lane
  // is ready and the saved CLI session id is known. A pane with no saved CLI
  // session keeps the manual Resume / Start fresh choice.
  useEffect(() => {
    if (
      !claimed ||
      closingGuardRef.current ||
      !open ||
      restoreReadyKey !== storageKey ||
      refreshedKey !== storageKey ||
      resumeGuardRef.current.active()
    ) {
      return;
    }
    const panes = Object.values(restoredPanes).filter((pane) => {
      if (autoResumeClaimedPaneIds.has(pane.paneId)) return false;
      if (
        dismissedSessionIdsRef.current.has(
          paneSessionIds[pane.paneId] ?? pane.sessionId,
        )
      ) {
        return false;
      }
      if (!pane.agentKind) return true;
      if (paneWaitsForOldSupervisor(pane, supervisorProtocolMismatchRef.current)) {
        return false;
      }
      return (
        Boolean(pane.cliSessionId) &&
        resumeLaneForTerminalPane(pane, agentSessionsState.laneOptions) !== null
      );
    });
    if (panes.length === 0) return;
    for (const pane of panes) {
      autoResumeClaimedPaneIds.add(pane.paneId);
    }
    void resumeRestoredPanesAtomically(panes);
  }, [
    agentSessionsState.laneOptions,
    claimed,
    open,
    refreshedKey,
    restoreReadyKey,
    restoredPanes,
    paneSessionIds,
    storageKey,
  ]);

  const refreshSessions = useCallback(
    async (preferredId?: string | null) => {
      const scopeKey = storageKeyRef.current;
      if (resumeGuardRef.current.active() || closingGuardRef.current) return [];
      const listedRows = await ipc.terminal.list();
      if (
        storageKeyRef.current !== scopeKey ||
        resumeGuardRef.current.active() ||
        closingGuardRef.current
      ) {
        return [];
      }
      // A shell that continues an ended agent takes over the agent's pane.
      // The new binding must exist before the stale-restore cleanup and the
      // row filter below.
      const continuations = applyTerminalContinuations(
        listedRows,
        paneSessionIdsRef.current,
        new Set(Object.keys(resumeFailuresRef.current)),
        dismissedSessionIdsRef.current,
      );
      if (continuations.reboundPaneIds.length > 0) {
        paneSessionIdsRef.current = continuations.paneSessionIds;
        setPaneSessionIds(continuations.paneSessionIds);
        // The pane now shows a plain shell. Saved agent state for the pane
        // must go, or the next launch resumes the agent over the shell.
        const reboundPaneIds = continuations.reboundPaneIds;
        restoredPanesRef.current = omitPaneIds(
          restoredPanesRef.current,
          reboundPaneIds,
        );
        setRestoredPanes((current) => omitPaneIds(current, reboundPaneIds));
        resumeFallbackPanesRef.current = omitPaneIds(
          resumeFallbackPanesRef.current,
          reboundPaneIds,
        );
        setResumeFallbackPanes((current) =>
          omitPaneIds(current, reboundPaneIds),
        );
        resumeFailuresRef.current = omitPaneIds(
          resumeFailuresRef.current,
          reboundPaneIds,
        );
        setResumeFailures((current) => omitPaneIds(current, reboundPaneIds));
      }
      for (const orphanId of continuations.orphanSessionIds) {
        dismissedSessionIdsRef.current.add(orphanId);
        void closeOrphanTerminalSession(orphanId, {
          closeTerminal: (sessionId) => ipc.terminal.close({ sessionId }),
          forgetTerminal: (sessionId) => ipc.terminal.forget({ sessionId }),
        }).catch((cause) => setError(errorMessage(cause)));
      }
      const preferredSessionId = preferredId
        ? (continuations.continuationBySourceId.get(preferredId) ??
          preferredId)
        : preferredId;
      // A live session needs no saved pane metadata.
      // The pane then attaches to the PTY instead of frozen scrollback.
      const runningIds = new Set(
        listedRows
          .filter((session) => session.status === "running")
          .map((session) => session.id),
      );
      setRestoredPanes((current) => {
        const stalePaneIds = Object.keys(current).filter((paneId) =>
          runningIds.has(paneSessionIdsRef.current[paneId]),
        );
        if (stalePaneIds.length === 0) return current;
        const next = { ...current };
        for (const paneId of stalePaneIds) delete next[paneId];
        return next;
      });
      const detachedRows = Object.entries(restoredPanesRef.current)
        .filter(([paneId, pane]) =>
          !dismissedSessionIdsRef.current.has(
            paneSessionIdsRef.current[paneId] ?? pane.sessionId,
          ),
        )
        .map(([paneId, pane]) =>
          restoredTerminalSession({
            ...pane,
            sessionId: paneSessionIdsRef.current[paneId] ?? pane.sessionId,
          }),
        );
      onSessionsChangedRef.current([...listedRows, ...detachedRows]);
      setAllRunningCount(
        listedRows.filter((session) => session.status === "running").length,
      );
      const storedLeafIds = new Set(allLayoutLeafIds(layoutRef.current));
      const binding: TerminalPanelBinding = shellTabId
        ? { kind: "shellTab", shellTabId }
        : boundConversationId
          ? { kind: "conversation", conversationId: boundConversationId }
          : agentSessionId
            ? { kind: "agent", agentSessionId }
            : { kind: "classic" };
      let rows = listedRows.filter((session) => {
        if (dismissedSessionIdsRef.current.has(session.id)) return false;
        const paneId = paneIdForSession(paneSessionIdsRef.current, session.id);
        return panelListsSession(binding, session, {
          inLayout: Boolean(paneId && storedLeafIds.has(paneId)),
          replaced: continuations.replacedSessionIds,
        });
      });
      const listedIds = new Set(rows.map((session) => session.id));
      rows.push(...detachedRows.filter((session) => !listedIds.has(session.id)));
      const agentEntries = agentSessionStore.getSnapshot().entries;
      const reconciledPanes = reconcileListedTerminalPanes(
        rows,
        paneSessionIdsRef.current,
        restoredPanesRef.current,
        agentEntries,
        expectedTerminalSessionIds,
      );
      rows = reconciledPanes.rows;
      const nextPaneSessionIds = reconciledPanes.paneSessionIds;
      const appendablePaneIds = reconciledPanes.appendablePaneIds;
      paneSessionIdsRef.current = nextPaneSessionIds;
      setPaneSessionIds(nextPaneSessionIds);
      // A resume can leave its pane briefly in neither backend list.
      // Keep the pane while its session changes or remains locally live.
      const rowIds = new Set(rows.map((session) => session.id));
      const preservedRow = (session: TerminalSession) =>
        !dismissedSessionIdsRef.current.has(session.id) &&
        !rowIds.has(session.id) &&
        (resumingSessionIdsRef.current.has(session.id) ||
          session.status === "running");
      setSessions((current) => [...rows, ...current.filter(preservedRow)]);
      setLayout((current) => {
        const validPaneIds = new Set(
          rows.flatMap((row) => {
            const paneId = paneIdForSession(nextPaneSessionIds, row.id);
            return paneId ? [paneId] : [];
          }),
        );
        const preservedIds = allLayoutLeafIds(current).filter((leafId) => {
          const sessionId = nextPaneSessionIds[leafId];
          if (!sessionId) return false;
          if (dismissedSessionIdsRef.current.has(sessionId)) return false;
          if (rowIds.has(sessionId)) return true;
          if (resumingSessionIdsRef.current.has(sessionId)) return true;
          const session = sessionsRef.current.find(
            (candidate) => candidate.id === sessionId,
          );
          return session?.status === "running";
        });
        const preferredPaneId = preferredSessionId
          ? paneIdForSession(nextPaneSessionIds, preferredSessionId)
          : null;
        return reconcileTerminalPanelLayout(
          current,
          [...validPaneIds, ...preservedIds],
          preferredPaneId,
          null,
          appendablePaneIds,
        );
      });
      setRefreshedKey(scopeKey);
      return rows;
    },
    [agentSessionId, boundConversationId, shellTabId],
  );

  async function retrySupervisorConnection(session: TerminalSession) {
    if (retryingSupervisorSessionIds.has(session.id)) return;
    setRetryingSupervisorSessionIds((current) =>
      new Set(current).add(session.id),
    );
    setError(null);
    try {
      const status = await ipc.terminal.supervisorRetry();
      if (!status.available) {
        throw new Error(status.message ?? t("The terminal pane is unavailable."));
      }
      const listed = await ipc.terminal.list();
      const listedSession = listed.find(
        (candidate) => candidate.id === session.id,
      );
      if (!listedSession) {
        setError(t("The terminal pane is unavailable."));
        return;
      }
      await refreshSessions(session.id);
      if (listedSession.status === "running") {
        setSupervisorFailureBySession((current) => {
          const next = { ...current };
          delete next[session.id];
          return next;
        });
        setTerminalAttachGenerations((current) => ({
          ...current,
          [session.id]: (current[session.id] ?? 0) + 1,
        }));
      }
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setRetryingSupervisorSessionIds((current) => {
        const next = new Set(current);
        next.delete(session.id);
        return next;
      });
    }
  }

  function clearResumeFailure(paneId: string) {
    if (!(paneId in resumeFailuresRef.current)) return;
    const next = { ...resumeFailuresRef.current };
    delete next[paneId];
    resumeFailuresRef.current = next;
    setResumeFailures(next);
  }

  function noteResumeInput(sessionId: string) {
    const attempt = resumeAttemptsRef.current.get(sessionId);
    if (attempt) attempt.userWroteInput = true;
  }

  function finishResumeAttempt(
    sessionId: string,
    exitCode: number | null,
    exitedAtMs: number,
  ) {
    const attempt = resumeAttemptsRef.current.get(sessionId);
    if (!attempt) return false;
    resumeAttemptsRef.current.delete(sessionId);
    if (
      dismissedSessionIdsRef.current.has(sessionId) ||
      !isTerminalResumeFailure(attempt, { exitCode, exitedAtMs })
    ) {
      return false;
    }
    const exitedAt = new Date(exitedAtMs).toISOString();
    const paneId = attempt.pane.paneId;
    const failedPane: TerminalPaneState = {
      ...attempt.pane,
      sessionId,
      updatedAt: exitedAt,
    };
    const nextPaneSessionIds = bindPaneToSession(
      paneSessionIdsRef.current,
      paneId,
      sessionId,
    );
    paneSessionIdsRef.current = nextPaneSessionIds;
    setPaneSessionIds(nextPaneSessionIds);
    autoResumeClaimedPaneIds.add(paneId);
    restoredPanesRef.current = {
      ...restoredPanesRef.current,
      [paneId]: failedPane,
    };
    setRestoredPanes((current) => ({ ...current, [paneId]: failedPane }));
    const failures = {
      ...resumeFailuresRef.current,
      [paneId]: { exitCode, exitedAt },
    };
    resumeFailuresRef.current = failures;
    setResumeFailures(failures);
    return true;
  }

  function terminalSessionWithResumeFailure(
    session: TerminalSession,
    paneId: string,
  ) {
    const failure = resumeFailuresRef.current[paneId];
    return failure
      ? {
          ...session,
          status: "failed" as const,
          exitCode: failure.exitCode ?? undefined,
          exitedAt: failure.exitedAt,
        }
      : session;
  }

  function trackResumeAttempt(
    pane: TerminalPaneState,
    session: TerminalSession,
    createdByResume: boolean,
  ) {
    const parsedCreatedAt = Date.parse(session.startedAt);
    const createdAtMs = Number.isFinite(parsedCreatedAt)
      ? parsedCreatedAt
      : Date.now();
    resumeAttemptsRef.current.set(session.id, {
      pane,
      createdAtMs,
      createdByResume,
      userWroteInput: false,
    });
    if (session.status !== "running") {
      const parsedExitedAt = session.exitedAt
        ? Date.parse(session.exitedAt)
        : Number.NaN;
      finishResumeAttempt(
        session.id,
        session.exitCode ?? null,
        Number.isFinite(parsedExitedAt) ? parsedExitedAt : Date.now(),
      );
    }
  }

  // The first refresh waits for the restore read: refreshing earlier lists
  // live rows that the restore result then overwrites, and the restored
  // panes ref is only current once that state has committed.
  useEffect(() => {
    if (!claimed || !open || restoreReadyKey !== storageKey) return;
    if (focusSessionId) expectedTerminalSessionIds.add(focusSessionId);
    void refreshSessions(focusSessionId).catch((cause) =>
      setError(errorMessage(cause)),
    );
  }, [claimed, focusSessionId, open, refreshSessions, restoreReadyKey, storageKey]);

  // A notification click asks for one pane. The panel that holds the pane
  // activates its leaf; the other panels ignore the request.
  const activateLeafRef = useRef(activateLeaf);
  activateLeafRef.current = activateLeaf;
  useEffect(
    () =>
      terminalFocusBridge.subscribe((sessionId) => {
        const leafId = paneIdForSession(paneSessionIdsRef.current, sessionId);
        if (!leafId) return false;
        activateLeafRef.current(leafId);
        focusTerminalPaneElement(sessionId);
        return true;
      }),
    [],
  );
  useEffect(() => terminalFocusBridge.retry(), [paneSessionIds]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void ipc.events.listen<{ message: string }>(
      "terminal_supervisor_fallback",
      (payload) => {
        if (!disposed) setSupervisorFallbackMessage(payload.message);
      },
    ).then((listener) => {
      if (disposed) listener();
      else unlisten = listener;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void ipc.events.listen<TerminalSupervisorReconnectingEvent>(
      "terminal_supervisor_reconnecting",
      (payload) => {
        if (disposed) return;
        setSupervisorReconnectBySession((current) => ({
          ...current,
          [payload.id]: {
            attempt: payload.attempt,
            message: payload.message,
          },
        }));
      },
    ).then((listener) => {
      if (disposed) listener();
      else unlisten = listener;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void ipc.events.listen<TerminalSupervisorReconnectedEvent>(
      "terminal_supervisor_reconnected",
      (payload) => {
        if (disposed) return;
        setSupervisorReconnectBySession((current) => {
          if (!(payload.id in current)) return current;
          const next = { ...current };
          delete next[payload.id];
          return next;
        });
        setSupervisorFailureBySession((current) => {
          if (!(payload.id in current)) return current;
          const next = { ...current };
          delete next[payload.id];
          return next;
        });
      },
    ).then((listener) => {
      if (disposed) listener();
      else unlisten = listener;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void ipc.events.listen<TerminalEvent>("terminal_event", (payload) => {
      if (disposed || payload.kind === "data") return;
      setSupervisorReconnectBySession((current) => {
        if (!(payload.sessionId in current)) return current;
        const next = { ...current };
        delete next[payload.sessionId];
        return next;
      });
      setSupervisorFailureBySession((current) => {
        const isSupervisorFailure =
          payload.kind === "error" &&
          isSupervisorTerminalFailure(payload.message);
        if (isSupervisorFailure && payload.message) {
          return { ...current, [payload.sessionId]: payload.message };
        }
        if (!(payload.sessionId in current)) return current;
        const next = { ...current };
        delete next[payload.sessionId];
        return next;
      });
      const resumeFailed = finishResumeAttempt(
        payload.sessionId,
        payload.exitCode ?? null,
        Date.now(),
      );
      setAllRunningCount((count) => Math.max(0, count - 1));
      setClosingSessionIds((current) => {
        if (!current.has(payload.sessionId)) return current;
        const next = new Set(current);
        next.delete(payload.sessionId);
        return next;
      });
      setSessions((current) =>
        current.map((session) =>
          session.id === payload.sessionId
            ? {
                ...session,
                status:
                  resumeFailed || payload.kind === "error"
                    ? "failed"
                    : "exited",
                exitCode: payload.exitCode,
                signal: payload.signal,
                exitedAt: new Date().toISOString(),
              }
            : session,
        ),
      );
      if (payload.message) setError(payload.message);
      void forgetPendingTerminalExit(
        pendingForgetRef.current,
        pendingAgentForgetRef.current,
        payload.sessionId,
        {
          forgetTerminal: (sessionId) =>
            ipc.terminal.forget({ sessionId }),
          forgetAgent: forgetAgentSession,
          refreshAgentSessions: () => agentSessionStore.refreshSessions(),
          refreshTerminalSessions: () => refreshSessions(null),
        },
      ).catch((cause) => setError(errorMessage(cause)));
      // A plain shell that exits with 0 on its own closes its pane (ADR
      // 0010, 9.8). A stop by Halv, a signal or a failure keeps the card.
      if (
        shellExitRulesRef.current &&
        payload.kind === "exit" &&
        payload.exitCode === 0 &&
        !payload.signal &&
        !dismissedSessionIdsRef.current.has(payload.sessionId) &&
        paneIdForSession(paneSessionIdsRef.current, payload.sessionId)
      ) {
        const ended = sessionsRef.current.find(
          (row) => row.id === payload.sessionId,
        );
        if (ended && !ended.agentKind) {
          void closeSessionRef.current(
            { ...ended, status: "exited", exitCode: 0 },
            { prompt: false },
          );
        }
      }
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshSessions]);

  // A clean agent exit continues the pane in a plain shell. The panel that
  // shows the ended session rebinds its pane. The refresh passes no
  // preferred id, so an exit in a background tab keeps the active tab.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let retryTimer: number | undefined;
    const rebind = () => {
      if (disposed) return;
      // A resume transaction blocks refreshes. Try again once it ends.
      if (resumeGuardRef.current.active()) {
        retryTimer = window.setTimeout(rebind, 250);
        return;
      }
      void refreshSessions(null).catch((cause) =>
        setError(errorMessage(cause)),
      );
    };
    void ipc.events.listen<TerminalContinuedEvent>(
      "terminal_continued",
      (payload) => {
        if (disposed) return;
        if (
          !panelOwnsContinuationSource(
            payload.fromSessionId,
            paneSessionIdsRef.current,
            dismissedSessionIdsRef.current,
          )
        ) {
          return;
        }
        window.clearTimeout(retryTimer);
        rebind();
      },
    ).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      window.clearTimeout(retryTimer);
      unlisten?.();
    };
  }, [refreshSessions]);

  function ensureSessionCapacity() {
    if (allRunningCount < MAX_TERMINAL_SESSIONS) return true;
    setError(
      t(
        "Halv can run at most {count} terminal sessions at once. Close a pane before splitting.",
        { count: MAX_TERMINAL_SESSIONS },
      ),
    );
    return false;
  }

  async function openCleanTerminal(): Promise<TerminalSession | null> {
    if (!project) {
      setError(t("Select a project before opening a terminal."));
      return null;
    }
    if (!ensureSessionCapacity()) return null;
    const session = await openPlainTerminalSession(
      (_command, args) => ipc.terminal.open(args as TerminalOpenArgs),
      {
        projectId: project.id,
        workspaceId,
        conversationId: boundConversationId,
        shellTabId,
      },
    );
    expectedTerminalSessionIds.add(session.id);
    return session;
  }

  async function openTerminal(): Promise<TerminalSession | null> {
    const scopeKey = storageKeyRef.current;
    setOpening(true);
    setError(null);
    try {
      const session = await openCleanTerminal();
      if (!session) return null;
      if (storageKeyRef.current !== scopeKey) return null;
      await refreshSessions(session.id);
      return session;
    } catch (cause) {
      setError(errorMessage(cause));
      return null;
    } finally {
      setOpening(false);
    }
  }

  // A plain shell next to a pane: same folder, project, and conversation.
  // A restored pane has no live session, so its saved state is the source.
  async function openShellBeside(
    source: TerminalSession,
  ): Promise<TerminalSession | null> {
    if (!ensureSessionCapacity()) return null;
    const session = await ipc.terminal.openShell({
      sessionId: source.id,
      scope: storageKeyRef.current,
      cols: 100,
      rows: 24,
    });
    expectedTerminalSessionIds.add(session.id);
    return session;
  }

  // The header "+" and the "New shell" command split the active pane to
  // the right. With no pane, the chat panel opens a shell in the project.
  async function openShellFromHeader() {
    if (activeSession) {
      await splitPane(
        activeSession,
        "vertical",
        TERMINAL_SPLIT_TERMINAL_CHOICE_ID,
      );
      return;
    }
    if (!agentEntry) await openTerminal();
  }

  // "Open shell here" on an ended agent pane. The backend binds at most one
  // shell to each ended session.
  async function continueInShell(source: TerminalSession) {
    setError(null);
    try {
      await ipc.terminal.continueShell({ sessionId: source.id });
      await refreshSessions(null);
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  // Creates the replacement session for a saved pane without touching any
  // panel state. Callers apply all state in one place.
  async function createResumedSession(
    pane: TerminalPaneState,
    startFresh: boolean,
  ): Promise<TerminalSession> {
    if (pane.agentKind) {
      const lane = resumeLaneForTerminalPane(
        pane,
        agentSessionsState.laneOptions,
      );
      if (!lane) {
        throw new Error(
          t("Reconnect this terminal lane before you resume the session."),
        );
      }
      const create = (resumeCliSessionId?: string) =>
        agentSessionStore.create({
          agentKind: pane.agentKind!,
          lane,
          cwd: pane.cwd,
          projectId: pane.projectId ?? undefined,
          conversationId: boundConversationId ?? undefined,
          conversationTitle: boundConversationId
            ? boundConversationTitle
            : undefined,
          model: boundConversationId ? boundConversationModel : undefined,
          mode: "terminal_embedded",
          resumeCliSessionId,
          // The resumed pane inherits the saved session's meter history.
          previousSessionId: pane.sessionId,
          cols: pane.cols,
          rows: pane.rows,
          titleState: terminalPaneTitleState(pane),
          permissionMode: conversationApprovalPolicy ?? undefined,
        });
      // A launch error does not prove that the saved transcript is missing.
      // Preserve the session ID so the user can retry the same conversation.
      const createdByResume = !startFresh && Boolean(pane.cliSessionId);
      const created = await create(
        startFresh ? undefined : pane.cliSessionId ?? undefined,
      );
      expectedTerminalSessionIds.add(created.id);
      if (pane.customTitle) {
        await agentSessionStore.rename(created.id, pane.customTitle);
      }
      const listedRows = await ipc.terminal.list();
      const createdTerminal = listedRows.find((row) => row.id === created.id);
      if (!createdTerminal) {
        throw new Error(
          t("The resumed terminal did not appear in the terminal list."),
        );
      }
      trackResumeAttempt(pane, createdTerminal, createdByResume);
      return createdTerminal;
    }
    let session = await ipc.terminal.restoreShell({
      scope: storageKey,
      sessionId: pane.sessionId,
      cols: pane.cols,
      rows: pane.rows,
    });
    if (pane.customTitle) {
      session = await ipc.terminal.rename({
        sessionId: session.id,
        title: pane.customTitle,
      });
    }
    trackResumeAttempt(pane, session, false);
    return session;
  }

  // Relaunch restore treats the saved layout as the source of truth.
  // One commit updates session bindings after every pane resumes.
  async function resumeRestoredPanesAtomically(panes: TerminalPaneState[]) {
    if (panes.length === 0 || resumeGuardRef.current.active()) return;
    const scopeKey = storageKeyRef.current;
    const endResume = resumeGuardRef.current.begin();
    setResumingAll(true);
    setResumingSessionIds((current) => {
      const next = new Set(current);
      for (const pane of panes) next.add(pane.sessionId);
      return next;
    });
    const replacements: Array<{
      pane: TerminalPaneState;
      session: TerminalSession;
    }> = [];
    try {
      let resumablePanes: TerminalPaneState[];
      try {
        const listedRows = await ipc.terminal.list();
        const filtered = filterRestoredPanesForAutoResume(
          panes,
          listedRows,
          agentSessionStore.getSnapshot().entries,
          { supervisorProtocolMismatch: supervisorProtocolMismatchRef.current },
        );
        resumablePanes = filtered.resumablePanes;
        releaseAutoResumePaneClaims(
          autoResumeClaimedPaneIds,
          filtered.droppedPaneIds,
        );
      } catch (cause) {
        if (storageKeyRef.current !== scopeKey) {
          releaseAutoResumePaneClaims(
            autoResumeClaimedPaneIds,
            panes.map((pane) => pane.paneId),
          );
        } else {
          for (const pane of panes) scheduleAutoResumeRetry(pane.paneId);
        }
        setError(errorMessage(cause));
        return;
      }
      for (const pane of resumablePanes) {
        if (storageKeyRef.current !== scopeKey) {
          autoResumeClaimedPaneIds.delete(pane.paneId);
          continue;
        }
        try {
          replacements.push({
            pane,
            session: await createResumedSession(pane, false),
          });
        } catch (cause) {
          // Delayed, capped claim release: the next effect pass retries a
          // transient failure without the infinite create/kill loop.
          scheduleAutoResumeRetry(pane.paneId);
          setError(errorMessage(cause));
        }
      }
      if (storageKeyRef.current !== scopeKey || replacements.length === 0) {
        return;
      }
      const oldIds = new Set(replacements.map(({ pane }) => pane.sessionId));
      const newIds = new Set(
        replacements.map(({ session }) => session.id),
      );
      setRestoredPanes((current) => {
        const next = { ...current };
        for (const { pane } of replacements) {
          if (!resumeFailuresRef.current[pane.paneId]) {
            delete next[pane.paneId];
          }
        }
        return next;
      });
      const nextPaneSessionIds = replacements.reduce(
        (current, { pane, session }) =>
          bindPaneToSession(current, pane.paneId, session.id),
        paneSessionIdsRef.current,
      );
      paneSessionIdsRef.current = nextPaneSessionIds;
      setPaneSessionIds(nextPaneSessionIds);
      setSessions((current) => [
        ...current.filter(
          (row) => !oldIds.has(row.id) && !newIds.has(row.id),
        ),
        ...replacements.map(({ pane, session }) =>
          terminalSessionWithResumeFailure(session, pane.paneId),
        ),
      ]);
      const runningReplacementCount = replacements.filter(
        ({ pane, session }) =>
          session.status === "running" &&
          !resumeFailuresRef.current[pane.paneId],
      ).length;
      if (runningReplacementCount > 0) {
        setAllRunningCount((count) => count + runningReplacementCount);
      }
      onSessionsChangedRef.current(
        await ipc.terminal.list(),
      );
    } finally {
      endResume();
      setResumingAll(false);
      setResumingSessionIds((current) => {
        const next = new Set(current);
        for (const pane of panes) next.delete(pane.sessionId);
        return next;
      });
    }
  }

  async function resumeSession(
    source: TerminalSession,
    startFresh = false,
    paneOverride: TerminalPaneState | null = null,
  ) {
    const paneId =
      paneOverride?.paneId ??
      paneIdForSession(paneSessionIdsRef.current, source.id);
    const pane =
      paneOverride ??
      (paneId ? restoredPanes[paneId] : null) ??
      (paneId ? resumeFallbackPanes[paneId] : null) ??
      null;
    if (
      !paneId ||
      !pane ||
      resumingSessionIds.has(source.id) ||
      resumeGuardRef.current.active() ||
      !ensureSessionCapacity()
    ) {
      return;
    }
    const scopeKey = storageKeyRef.current;
    const endResume = resumeGuardRef.current.begin();
    setResumingSessionIds((current) => new Set(current).add(source.id));
    clearResumeFailure(paneId);
    setError(null);
    try {
      const session = await createResumedSession(pane, startFresh);
      // The conversation changed while the backend created the session. The
      // session stays tagged to its own conversation and reappears on return
      // (its stale saved pane is healed by the CLI-session match); applying
      // state here would save it under the wrong scope.
      if (storageKeyRef.current !== scopeKey) return;
      const resumedPane: TerminalPaneState = {
        ...pane,
        sessionId: session.id,
        cliSessionId: null,
        scrollback: "",
        updatedAt: new Date().toISOString(),
      };
      const resumedCliSessionId = agentSessionStore
        .getSnapshot()
        .entries.find((entry) => entry.session.id === session.id)?.session
        .cliSessionId;
      setRestoredPanes((current) => {
        const next = { ...current };
        if (!resumeFailuresRef.current[paneId]) delete next[paneId];
        return next;
      });
      setResumeFallbackPanes((current) => {
        const next = { ...current };
        delete next[paneId];
        if (pane.agentKind && !resumedCliSessionId) next[paneId] = resumedPane;
        return next;
      });
      const nextPaneSessionIds = bindPaneToSession(
        paneSessionIdsRef.current,
        paneId,
        session.id,
      );
      paneSessionIdsRef.current = nextPaneSessionIds;
      setPaneSessionIds(nextPaneSessionIds);
      setSessions((current) => [
        ...current.filter(
          (candidate) =>
            candidate.id !== source.id && candidate.id !== session.id,
        ),
        terminalSessionWithResumeFailure(session, paneId),
      ]);
      setLayout((current) => {
        if (current.tabs.some((tab) => terminalLayoutHasLeaf(tab.root, paneId))) {
          return current;
        }
        // Recovery from the empty state creates the pane's first leaf.
        const leafIds = current.tabs.flatMap((tab) =>
          collectTerminalLeafIds(tab.root),
        );
        return reconcileTerminalPanelLayout(
          current,
          [...leafIds, paneId],
          paneId,
        );
      });
      if (
        session.status === "running" &&
        !resumeFailuresRef.current[paneId]
      ) {
        setAllRunningCount((count) => count + 1);
      }
      onSessionsChangedRef.current(await ipc.terminal.list());
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      endResume();
      setResumingSessionIds((current) => {
        const next = new Set(current);
        next.delete(source.id);
        return next;
      });
    }
  }

  // The agent entry still knows the folder, lane, and CLI session id after
  // the terminal process is gone, so the dead "unavailable" state can offer
  // resume / start-fresh even when no pane was saved for this scope.
  function agentEntryFallbackPane(entry: AgentSessionEntry): TerminalPaneState {
    const session = entry.session;
    return {
      paneId:
        paneIdForSession(paneSessionIdsRef.current, session.id) ??
        createTerminalPaneId(),
      sessionId: session.id,
      projectId: session.projectId ?? project?.id ?? null,
      conversationId: session.conversationId ?? boundConversationId,
      projectName: project?.name ?? "Terminal",
      cwd: session.cwd,
      executionProfile: "full_access",
      agentKind: session.agentKind,
      laneKind: session.laneKind,
      credentialProfileId: null,
      customTitle: session.customTitle ?? null,
      defaultTitle: session.defaultTitle ?? null,
      generatedTitle: session.generatedTitle ?? null,
      aiVaultTitle: session.aiVaultTitle ?? null,
      generation: session.generation ?? 0,
      cliSessionId: session.cliSessionId ?? null,
      cols: 100,
      rows: 24,
      scrollback: "",
      updatedAt: new Date().toISOString(),
    };
  }

  async function recoverAgentTerminal(startFresh: boolean) {
    if (!agentEntry) return;
    const pane = agentEntryFallbackPane(agentEntry);
    await resumeSession(restoredTerminalSession(pane), startFresh, pane);
  }

  // After an account switch, a queued pane restarts on the active account
  // when it rests. The same CLI session resumes in the same pane.
  const resumeSessionRef = useRef(resumeSession);
  resumeSessionRef.current = resumeSession;
  useAccountRestartRunner(Object.values(paneSessionIds), (sessionId) => {
    const scopeKey = storageKeyRef.current;
    return restartPaneOnActiveAccount(sessionId, {
      find: () => {
        const paneId = paneIdForSession(paneSessionIdsRef.current, sessionId);
        const row = sessionsRef.current.find((session) => session.id === sessionId);
        const entry = agentSessionStore
          .getSnapshot()
          .entries.find((candidate) => candidate.session.id === sessionId);
        if (!paneId || !row || row.status !== "running" || !entry) return null;
        const saved = { ...agentEntryFallbackPane(entry), cols: row.cols, rows: row.rows };
        return { paneId, row, saved, cliSessionId: entry.session.cliSessionId ?? null };
      },
      resumeBusy: () => resumeGuardRef.current.active(),
      stop: () => {
        expectedTerminalSessionIds.delete(sessionId);
        return agentSessionStore.kill(sessionId);
      },
      waitForExit: () => waitForTerminalExit(sessionId, () => ipc.terminal.list()),
      resume: (row, saved) => resumeSessionRef.current(row, false, saved),
      boundSessionId: (paneId) => paneSessionIdsRef.current[paneId] ?? null,
      keepForRetry: (paneId, saved) => {
        if (storageKeyRef.current !== scopeKey) return;
        setRestoredPanes((current) => ({ ...current, [paneId]: saved }));
      },
      forgetOld: async () => {
        dismissedSessionIdsRef.current.add(sessionId);
        await forgetTerminalSession(sessionId, (id) => ipc.terminal.forget({ sessionId: id }));
        await forgetStoppedAgentSession(sessionId, {
          forgetAgent: forgetAgentSession,
          refreshAgentSessions: () => agentSessionStore.refreshSessions(),
        });
      },
      messages: {
        noCliSession: t("Halv cannot resume this session. Restart it yourself to use the new account."),
        resumeFailed: t("Halv could not restart this session. Select Resume to try again."),
      },
    });
  });

  async function resumeAllSessions() {
    if (resumingAll) return;
    const scopeKey = storageKeyRef.current;
    setResumingAll(true);
    const savedSessions = Object.keys(restoredPanes).flatMap((paneId) => {
      const sessionId = paneSessionIds[paneId];
      const session = sessions.find((candidate) => candidate.id === sessionId);
      return session ? [session] : [];
    });
    try {
      for (const session of savedSessions) {
        if (storageKeyRef.current !== scopeKey) break;
        await resumeSession(session);
      }
    } finally {
      setResumingAll(false);
    }
  }

  async function splitPane(
    source: TerminalSession,
    direction: TerminalPaneSplitDirection,
    choiceId: "duplicate" | string = "duplicate",
  ) {
    if (choiceId === "other-project") {
      const firstOther =
        (projects ?? []).find((item) => item.id !== project?.id)?.id ?? null;
      setProjectPicker({
        source,
        direction,
        projectId: firstOther,
        agentKind: (source.agentKind as AgentKind | null) ?? "claude",
      });
      return;
    }
    if (openingLeafIds.has(source.id)) return;
    if (
      choiceId !== TERMINAL_SPLIT_TERMINAL_CHOICE_ID &&
      !ensureSessionCapacity()
    ) {
      return;
    }
    const scopeKey = storageKeyRef.current;
    setOpeningLeafIds((current) => new Set(current).add(source.id));
    setError(null);
    try {
      let session: TerminalSession;
      let splitDirection = direction;
      const terminalSplit = await openTerminalSplitChoice(
        choiceId,
        direction,
        () => openShellBeside(source),
      );
      if (terminalSplit !== undefined) {
        if (!terminalSplit) return;
        session = terminalSplit.session;
        splitDirection = terminalSplit.direction;
        setAllRunningCount((count) => count + 1);
      } else if (choiceId !== "duplicate") {
        let agentKind: AgentKind;
        let cwd = source.cwd;
        let projectId = source.projectId ?? undefined;
        if (choiceId.startsWith("pick:")) {
          const [, pickedKind, pickedProject] = choiceId.split(":");
          const target = (projects ?? []).find(
            (item) => item.id === Number(pickedProject),
          );
          if (!target) {
            throw new Error(t("The chosen project is no longer available."));
          }
          agentKind = pickedKind as AgentKind;
          cwd = target.rootPath;
          projectId = target.id;
        } else {
          agentKind = choiceId as AgentKind;
        }
        const lane = await resolveTerminalSplitAgentLane(
          agentSessionsState.laneOptions,
          agentKind,
          () => agentSessionStore.refreshLaneOptions(),
        );
        const created = await agentSessionStore.create({
          agentKind,
          lane,
          cwd,
          projectId,
          conversationId: boundConversationId ?? undefined,
          conversationTitle: boundConversationId
            ? boundConversationTitle
            : undefined,
          model: boundConversationId ? boundConversationModel : undefined,
          mode: "terminal_embedded",
          cols: 100,
          rows: 24,
          permissionMode: conversationApprovalPolicy ?? undefined,
        });
        expectedTerminalSessionIds.add(created.id);
        const listedRows = await ipc.terminal.list();
        const createdTerminal = listedRows.find((row) => row.id === created.id);
        if (!createdTerminal) {
          throw new Error(t("The selected agent terminal did not start."));
        }
        session = createdTerminal;
        setAllRunningCount(
          listedRows.filter((row) => row.status === "running").length,
        );
      } else if (source.agentKind) {
        const created = await ipc.agent.terminalDuplicate({
            sessionId: source.id,
            cols: 100,
            rows: 24,
          });
        expectedTerminalSessionIds.add(created.id);
        await agentSessionStore.refreshSessions().catch(() => undefined);
        const listedRows = await ipc.terminal.list();
        const createdTerminal = listedRows.find((row) => row.id === created.id);
        if (!createdTerminal) {
          throw new Error(t("The split agent terminal did not start."));
        }
        session = createdTerminal;
        setAllRunningCount(
          listedRows.filter((row) => row.status === "running").length,
        );
      } else {
        session = await ipc.terminal.duplicate({
          sessionId: source.id,
          cols: 100,
          rows: 24,
        });
        expectedTerminalSessionIds.add(session.id);
        setAllRunningCount((count) => count + 1);
      }
      // The conversation changed mid-split; the created session reappears
      // there on return instead of landing in the wrong panel state.
      if (storageKeyRef.current !== scopeKey) return;
      const sourcePaneId = paneIdForSession(
        paneSessionIdsRef.current,
        source.id,
      );
      if (!sourcePaneId) {
        throw new Error(t("The source terminal pane is unavailable."));
      }
      const paneId =
        paneIdForSession(paneSessionIdsRef.current, session.id) ??
        createTerminalPaneId();
      const nextPaneSessionIds = bindPaneToSession(
        paneSessionIdsRef.current,
        paneId,
        session.id,
      );
      paneSessionIdsRef.current = nextPaneSessionIds;
      setPaneSessionIds(nextPaneSessionIds);
      onSessionsChangedRef.current(await ipc.terminal.list());
      setSessions((current) => [
        ...current.filter((row) => row.id !== session.id),
        session,
      ]);
      setLayout((current) => {
        // A racing session-list refresh may already have appended the new
        // session as its own tab; drop those leaves first or the split
        // references the same session twice.
        const tabs = current.tabs.flatMap((candidate) => {
          const root = closeTerminalLeaf(candidate.root, paneId);
          return root ? [{ ...candidate, root }] : [];
        });
        const cleaned = { ...current, tabs };
        const tab = cleaned.tabs.find((candidate) =>
          terminalLayoutHasLeaf(candidate.root, sourcePaneId),
        );
        if (!tab) {
          return reconcileTerminalPanelLayout(
            cleaned,
            [
              ...sessions
                .filter(
                  (row) =>
                    !dismissedSessionIdsRef.current.has(row.id),
                )
                .flatMap((row) => {
                  const candidatePaneId = paneIdForSession(
                    nextPaneSessionIds,
                    row.id,
                  );
                  return candidatePaneId ? [candidatePaneId] : [];
                }),
              paneId,
            ],
            paneId,
          );
        }
        return {
          ...cleaned,
          activeTabId: tab.tabId,
          activeLeafId: paneId,
          tabs: cleaned.tabs.map((candidate) =>
            candidate.tabId === tab.tabId
              ? {
                  ...candidate,
                  root: splitTerminalLeaf(
                    candidate.root,
                    sourcePaneId,
                    paneId,
                    splitDirection,
                  ),
                }
              : candidate,
          ),
        };
      });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setOpeningLeafIds((current) => {
        const next = new Set(current);
        next.delete(source.id);
        return next;
      });
    }
  }

  function cycleTab(offset: -1 | 1) {
    if (layout.tabs.length < 2) return;
    const currentIndex = layout.tabs.findIndex(
      (tab) => tab.tabId === layout.activeTabId,
    );
    const nextIndex =
      (Math.max(0, currentIndex) + offset + layout.tabs.length) %
      layout.tabs.length;
    selectTab(layout.tabs[nextIndex]!.tabId);
  }

  // With the shell tab rules, an idle shell closes without a prompt. The
  // busy probe decides. A shell that it cannot read asks.
  async function shellIdleForClose(session: TerminalSession) {
    if (!shellExitRulesRef.current || session.agentKind || session.pid === 0) {
      return false;
    }
    try {
      const [probe] = await ipc.terminal.foregroundBusy({
        sessionIds: [session.id],
      });
      return probe?.state === "idle";
    } catch {
      return false;
    }
  }

  async function closeSession(
    session: TerminalSession,
    options: { prompt?: boolean } = {},
  ) {
    const paneId = paneIdForSession(paneSessionIdsRef.current, session.id);
    if (!paneId) {
      setError(t("The terminal pane is unavailable."));
      return;
    }
    const agentState = agentSessionStore.getSnapshot();
    const closeSnapshot: TerminalCloseSnapshot = {
      scope: storageKeyRef.current,
      layout: layoutRef.current,
      sessions: sessionsRef.current,
      paneSessionIds: paneSessionIdsRef.current,
      agentEntries: agentState.entries,
      laneOptions: agentState.laneOptions,
      restoredPanes: restoredPanesRef.current,
      resumeFallbackPanes: resumeFallbackPanesRef.current,
      excludedPaneIds: new Set([paneId]),
    };
    if (
      session.status === "running" &&
      options.prompt !== false &&
      !(await shellIdleForClose(session)) &&
      !(await confirmDialog({
        message: t("Stop {title}? The process is still running.", {
          title: terminalTitle(session, sessionTitles, activeAgentEntry),
        }),
      }))
    ) {
      return;
    }
    const wasExpected = expectedTerminalSessionIds.has(session.id);
    const wasAutoResumeClaimed = autoResumeClaimedPaneIds.has(paneId);
    setClosingSessionIds((current) => new Set(current).add(session.id));
    setError(null);
    let waitsForExit = false;
    try {
      dismissedSessionIdsRef.current.add(session.id);
      expectedTerminalSessionIds.delete(session.id);
      autoResumeClaimedPaneIds.add(paneId);
      let deferredError: unknown = null;
      const restoredOnly =
        Boolean(closeSnapshot.restoredPanes[paneId]) && session.pid === 0;
      if (restoredOnly) {
        // The captured write removes detached panes without a backend row.
      } else if (session.status === "running") {
        waitsForExit = true;
        markPendingTerminalForget(
          pendingForgetRef.current,
          pendingAgentForgetRef.current,
          session.id,
          Boolean(session.agentKind),
        );
        if (session.agentKind) {
          const stopped = await agentSessionStore.kill(session.id);
          if (!agentSessionWaitsForExit(stopped.status)) {
            waitsForExit = false;
            clearPendingTerminalForget(
              pendingForgetRef.current,
              pendingAgentForgetRef.current,
              session.id,
            );
            await forgetTerminalSession(
              session.id,
              (sessionId) => ipc.terminal.forget({ sessionId }),
            );
            try {
              await forgetStoppedAgentSession(session.id, {
                forgetAgent: forgetAgentSession,
                refreshAgentSessions: () =>
                  agentSessionStore.refreshSessions(),
              });
            } catch (cause) {
              deferredError = cause;
            }
          } else if (!pendingForgetRef.current.has(session.id)) {
            try {
              await agentSessionStore.refreshSessions();
            } catch (cause) {
              deferredError = cause;
            }
          }
          } else {
            const closeResult = await closeRunningTerminalSession(session.id, {
              closeTerminal: (sessionId) =>
                ipc.terminal.close({ sessionId }),
              forgetTerminal: (sessionId) =>
                ipc.terminal.forget({ sessionId }),
            });
            if (closeResult === "forgotten") {
              waitsForExit = false;
              clearPendingTerminalForget(
                pendingForgetRef.current,
                pendingAgentForgetRef.current,
                session.id,
              );
            }
          }
      } else {
        await forgetTerminalSession(
          session.id,
          (sessionId) => ipc.terminal.forget({ sessionId }),
        );
        if (session.agentKind) {
          try {
            await forgetStoppedAgentSession(session.id, {
              forgetAgent: forgetAgentSession,
              refreshAgentSessions: () => agentSessionStore.refreshSessions(),
            });
          } catch (cause) {
            deferredError = cause;
          }
        }
      }
      if (session.continuedFrom) {
        // The ended agent that this shell replaced goes with the pane.
        try {
          await forgetContinuationSource(session.continuedFrom, {
            forgetTerminal: (sessionId) => ipc.terminal.forget({ sessionId }),
            forgetAgent: forgetAgentSession,
            refreshAgentSessions: () => agentSessionStore.refreshSessions(),
          });
        } catch (cause) {
          deferredError ??= cause;
        }
      }

      try {
        const payload = await writeClosedTerminalRestore(closeSnapshot, {
          writeLayout: writeStoredLayout,
          writeRestore: (value) => ipc.terminal.restoreWrite(value),
        });
        if (storageKeyRef.current === closeSnapshot.scope) {
          lastRestoreWriteRef.current = JSON.stringify(payload);
        }
      } catch (cause) {
        deferredError ??= cause;
      }

      // State cleanup applies only while the captured scope stays active.
      if (storageKeyRef.current !== closeSnapshot.scope) {
        if (deferredError) setError(errorMessage(deferredError));
        return;
      }
      const remainingSessions = sessionsRef.current.filter(
        (row) => row.id !== session.id,
      );
      sessionsRef.current = remainingSessions;
      setSessions(remainingSessions);
      const remainingRestoredPanes = { ...restoredPanesRef.current };
      delete remainingRestoredPanes[paneId];
      restoredPanesRef.current = remainingRestoredPanes;
      setRestoredPanes(remainingRestoredPanes);
      const remainingFallbackPanes = { ...resumeFallbackPanesRef.current };
      delete remainingFallbackPanes[paneId];
      resumeFallbackPanesRef.current = remainingFallbackPanes;
      setResumeFallbackPanes(remainingFallbackPanes);
      const nextPaneSessionIds = { ...paneSessionIdsRef.current };
      delete nextPaneSessionIds[paneId];
      paneSessionIdsRef.current = nextPaneSessionIds;
      setPaneSessionIds(nextPaneSessionIds);
      resumeAttemptsRef.current.delete(session.id);
      clearResumeFailure(paneId);
      const nextLayout = closeTerminalPanelSessions(
        layoutRef.current,
        closeSnapshot.excludedPaneIds,
      );
      layoutRef.current = nextLayout;
      setLayout(nextLayout);
      if (deferredError) setError(errorMessage(deferredError));
      // The last closed pane of a tab closes the tab (ADR 0010, 9.4).
      if (
        shellExitRulesRef.current &&
        allLayoutLeafIds(nextLayout).length === 0
      ) {
        onLastPaneClosedRef.current?.();
      }
    } catch (cause) {
      waitsForExit = false;
      dismissedSessionIdsRef.current.delete(session.id);
      clearPendingTerminalForget(
        pendingForgetRef.current,
        pendingAgentForgetRef.current,
        session.id,
      );
      if (wasExpected) expectedTerminalSessionIds.add(session.id);
      if (!wasAutoResumeClaimed) autoResumeClaimedPaneIds.delete(paneId);
      setError(errorMessage(cause));
    } finally {
      if (!waitsForExit) {
        setClosingSessionIds((current) => {
          const next = new Set(current);
          next.delete(session.id);
          return next;
        });
      }
    }
  }

  closeSessionRef.current = closeSession;

  // "Restart" on the exit card of a failed shell: a fresh shell in the same
  // folder and the same pane. The ended session goes.
  async function restartShell(source: TerminalSession) {
    const paneId = paneIdForSession(paneSessionIdsRef.current, source.id);
    if (!paneId || !ensureSessionCapacity()) return;
    const scopeKey = storageKeyRef.current;
    setError(null);
    try {
      const session = await ipc.terminal.openShell({
        sessionId: source.id,
        scope: scopeKey,
        cols: source.cols,
        rows: source.rows,
      });
      expectedTerminalSessionIds.add(session.id);
      if (storageKeyRef.current !== scopeKey) return;
      dismissedSessionIdsRef.current.add(source.id);
      const nextPaneSessionIds = bindPaneToSession(
        paneSessionIdsRef.current,
        paneId,
        session.id,
      );
      paneSessionIdsRef.current = nextPaneSessionIds;
      setPaneSessionIds(nextPaneSessionIds);
      setSessions((current) => [
        ...current.filter((row) => row.id !== source.id && row.id !== session.id),
        session,
      ]);
      setAllRunningCount((count) => count + 1);
      await ipc.terminal.forget({ sessionId: source.id }).catch(() => undefined);
      onSessionsChangedRef.current(await ipc.terminal.list());
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  function activateLeaf(leafId: string) {
    const sessionId = paneSessionIdsRef.current[leafId];
    if (sessionId) terminalActivityStore.markViewed(sessionId);
    setTerminalFocused(true);
    setLayout((current) => {
      const tab = current.tabs.find((candidate) =>
        terminalLayoutHasLeaf(candidate.root, leafId),
      );
      return tab
        ? { ...current, activeTabId: tab.tabId, activeLeafId: leafId }
        : current;
    });
  }

  function selectTab(tabId: string) {
    const selectedTab = layoutRef.current.tabs.find((tab) => tab.tabId === tabId);
    for (const leafId of selectedTab ? collectTerminalLeafIds(selectedTab.root) : []) {
      const sessionId = paneSessionIdsRef.current[leafId];
      if (sessionId) terminalActivityStore.markViewed(sessionId);
    }
    setLayout((current) => {
      const tab = current.tabs.find((candidate) => candidate.tabId === tabId);
      if (!tab) return current;
      const activeLeafId = terminalLayoutHasLeaf(
        tab.root,
        current.activeLeafId ?? "",
      )
        ? current.activeLeafId
        : collectTerminalLeafIds(tab.root)[0] ?? null;
      return { ...current, activeTabId: tabId, activeLeafId };
    });
  }

  function rearrangePane(
    leafId: string,
    operation: (
      node: TerminalPaneLayoutNode,
      leafId: string,
    ) => TerminalPaneLayoutNode,
  ) {
    if (!activeTab) return;
    setLayout((current) => ({
      ...current,
      tabs: current.tabs.map((tab) =>
        tab.tabId === activeTab.tabId
          ? { ...tab, root: operation(tab.root, leafId) }
          : tab,
      ),
    }));
  }

  function updateRatio(path: TerminalSplitPath, ratio: number) {
    if (!activeTab) return;
    setLayout((current) => ({
      ...current,
      tabs: current.tabs.map((tab) =>
        tab.tabId === activeTab.tabId
          ? { ...tab, root: setTerminalSplitRatio(tab.root, path, ratio) }
          : tab,
      ),
    }));
  }

  // The "New shell" command reaches this panel through the registry.
  const shellRequestRef = useRef<() => void>(NOOP);
  shellRequestRef.current = () => void openShellFromHeader();
  // A shell tab panel answers through the mount registry only. In the
  // older registry, it would share the "" key with the bottom panel.
  useEffect(
    () =>
      claimed && !shellTabId
        ? registerTerminalShellRequest(boundConversationId, () =>
            shellRequestRef.current(),
          )
        : undefined,
    [boundConversationId, claimed, shellTabId],
  );

  // The tab close procedure (ADR 0010): stop every process of the panel,
  // forget its rows, and write the park payload as the final write.
  async function closeAllSessions(options: { park: boolean }) {
    closingGuardRef.current = true;
    const agentState = agentSessionStore.getSnapshot();
    const snapshot = {
      scope: storageKeyRef.current,
      layout: layoutRef.current,
      sessions: sessionsRef.current,
      paneSessionIds: paneSessionIdsRef.current,
      agentEntries: agentState.entries,
      laneOptions: agentState.laneOptions,
      restoredPanes: restoredPanesRef.current,
      resumeFallbackPanes: resumeFallbackPanesRef.current,
    };
    const payload = options.park
      ? parkedTerminalRestorePayload(snapshot)
      : { scope: snapshot.scope, layout: { ...EMPTY_LAYOUT }, panes: [] };
    const boundSessionIds = new Set(Object.values(snapshot.paneSessionIds));
    const live = snapshot.sessions.filter(
      (session) => boundSessionIds.has(session.id) && session.pid !== 0,
    );
    try {
      for (const session of live) {
        dismissedSessionIdsRef.current.add(session.id);
        expectedTerminalSessionIds.delete(session.id);
      }
      for (const paneId of Object.keys(snapshot.paneSessionIds)) {
        autoResumeClaimedPaneIds.add(paneId);
      }
      await Promise.all(
        live.map((session) =>
          stopTerminalSessionForTabClose(session, {
            killAgent: (sessionId) => agentSessionStore.kill(sessionId),
            closeTerminal: (sessionId) => ipc.terminal.close({ sessionId }),
            forgetTerminal: (sessionId) => ipc.terminal.forget({ sessionId }),
            forgetAgent: forgetAgentSession,
            refreshAgentSessions: () => agentSessionStore.refreshSessions(),
          }),
        ),
      );
      writeStoredLayout(payload.scope, payload.layout);
      await ipc.terminal.restoreWrite(payload);
      lastRestoreWriteRef.current = JSON.stringify(payload);
      // A reopen of the conversation resumes its parked panes once.
      releaseAutoResumePaneClaims(
        autoResumeClaimedPaneIds,
        payload.panes.map((pane) => pane.paneId),
      );
    } catch (cause) {
      closingGuardRef.current = false;
      for (const session of live) dismissedSessionIdsRef.current.delete(session.id);
      setError(errorMessage(cause));
      void refreshSessions(null).catch(() => undefined);
      throw cause;
    }
  }

  function closeFacts(): TerminalPanelPaneFact[] {
    const boundSessionIds = new Set(Object.values(paneSessionIdsRef.current));
    return sessionsRef.current
      .filter((session) => boundSessionIds.has(session.id))
      .map((session) => ({
        sessionId: session.id,
        kind: session.agentKind ? ("agent" as const) : ("shell" as const),
        running: session.status === "running" && session.pid !== 0,
        status: session.agentKind
          ? (agentPaneViewFor(terminalActivityStore.getSnapshot(), session.id)
              ?.status ?? null)
          : null,
        shellBusy: null,
      }));
  }

  const handleRef = useRef<{
    closeAll: typeof closeAllSessions;
    closePane: (sessionId: string | null) => Promise<void>;
    focusActive: () => boolean;
    splitActive: (direction: TerminalPaneSplitDirection) => boolean;
  }>({
    closeAll: closeAllSessions,
    closePane: async () => undefined,
    focusActive: () => false,
    splitActive: () => false,
  });
  handleRef.current = {
    closeAll: closeAllSessions,
    closePane: async (sessionId) => {
      const leafId = layoutRef.current.activeLeafId;
      const targetId =
        sessionId ?? (leafId ? paneSessionIdsRef.current[leafId] : null);
      const session = sessionsRef.current.find((row) => row.id === targetId);
      if (session) await closeSession(session);
    },
    // The focus lands in this frame, so the caller can check first that
    // no menu or rename field took the focus meanwhile. False: the pane
    // does not show yet, and the caller tries again.
    focusActive: () => {
      const leafId = layoutRef.current.activeLeafId;
      const sessionId = leafId ? paneSessionIdsRef.current[leafId] : null;
      if (!sessionId) return false;
      const pane = [
        ...document.querySelectorAll<HTMLElement>(
          ".terminal-pane[data-terminal-session-id]",
        ),
      ].find(
        (element) =>
          element.dataset.terminalSessionId === sessionId &&
          element.getClientRects().length > 0 &&
          getComputedStyle(element).visibility !== "hidden",
      );
      const input = pane?.querySelector<HTMLElement>(".xterm-helper-textarea");
      if (!input) return false;
      input.focus();
      return true;
    },
    splitActive: (direction) => {
      const leafId = layoutRef.current.activeLeafId;
      const sessionId = leafId ? paneSessionIdsRef.current[leafId] : null;
      const session = sessionsRef.current.find((row) => row.id === sessionId);
      if (!session) return false;
      void splitPane(session, direction, "duplicate");
      return true;
    },
  };
  useEffect(() => {
    if (!claimScope || !claimed) return undefined;
    terminalPanelRegistry.setHandle(storageKey, claimToken, {
      closeAll: (options) => handleRef.current.closeAll(options),
      focusActivePane: () => handleRef.current.focusActive(),
      openShell: () => shellRequestRef.current(),
      panes: closeFacts,
      closePane: (sessionId) => handleRef.current.closePane(sessionId),
      splitActivePane: (direction) => handleRef.current.splitActive(direction),
    });
    return () => terminalPanelRegistry.setHandle(storageKey, claimToken, null);
    // closeFacts reads refs only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [claimScope, claimToken, claimed, storageKey]);

  if (!open) return null;

  if (!claimed) {
    return (
      <section
        aria-label={t("Terminal panel")}
        className={`terminal-panel${agentEntry || fill ? " terminal-panel-agent" : ""}`}
      >
        <div className="terminal-panel-claim-wait">
          {t("This terminal shows in its tab.")}
        </div>
      </section>
    );
  }

  const displayName = activeSession
    ? terminalTitle(activeSession, sessionTitles, activeAgentEntry)
    : agentEntry
      ? agentSessionTitle(agentEntry.session)
      : boundConversationId
        ? boundConversationTitle
        : t("Terminal");
  const displayCwd = activeSession?.cwd ?? agentEntry?.session.cwd ?? "";
  const savingsMeter = agentEntry ? (
    <SessionSavingsMeter
      compact
      measurementNote={terminalSavingsMeasurementNote(
        agentEntry.session.laneKind,
        agentEntry.session.savingsSources,
        t,
      )}
      unmeasured={sessionSavingsUnmeasured(
        agentEntry.session.laneKind,
        agentEntry.session.savingsSources,
      )}
      ponytailMode={agentEntry.session.ponytailMode}
      turns={agentEntry.turns}
      totals={agentEntry.totals}
      conversationTotals={conversationSavedTotals}
      planBilling={laneIsPlan(agentEntry.session.laneKind)}
      routing={routingSavingsFromTotals(routingReport?.totals)}
    />
  ) : null;

  return (
    <section
      aria-label={
        agentEntry
          ? t("{agent} embedded terminal", {
              agent: agentLabel(agentEntry.session.agentKind),
            })
          : t("Terminal panel")
      }
      className={`terminal-panel${agentEntry || fill ? " terminal-panel-agent" : ""}${fill ? " terminal-panel-fill" : ""}`}
      data-terminal-root
      data-terminal-scope={storageKey}
      onKeyDown={(event) => {
        if (event.defaultPrevented) return;
        if (
          shortcutMatchesEvent(
            event,
            shortcutForAction(
              "workspace.previous_terminal_tab",
              shortcutOverrides,
            ),
          )
        ) {
          event.preventDefault();
          cycleTab(-1);
        } else if (
          shortcutMatchesEvent(
            event,
            shortcutForAction(
              "workspace.next_terminal_tab",
              shortcutOverrides,
            ),
          )
        ) {
          event.preventDefault();
          cycleTab(1);
        }
      }}
    >
      <div className="terminal-frame" data-terminal-focused={terminalFocused}>
        {fill ? null : (
        <header className="terminal-header">
          <div className="terminal-identity">
            <span className="terminal-agent-mark" aria-hidden>
              {activeSession
                ? terminalAgentMark(activeAgentEntry, activeSession.agentKind)
                : terminalAgentMark(agentEntry)}
            </span>
            <div className="terminal-identity-copy">
              <strong>{displayName}</strong>
              <code title={displayCwd}>{middleTruncate(displayCwd, 58)}</code>
            </div>
          </div>
          <div className="terminal-header-actions">
            {Object.keys(restoredPanes).length > 0 ? (
              <button
                className="terminal-kill-button"
                disabled={resumingAll}
                onClick={() => void resumeAllSessions()}
                type="button"
              >
                {resumingAll ? t("Resuming…") : t("Resume all")}
              </button>
            ) : null}
            {savingsMeter}
            {activePaneView ? (
              <span
                className="terminal-status-label"
                data-agent-status={activePaneView.status}
                title={agentPaneTitle(activePaneView, t)}
              >
                <AgentStateGlyph size="sm" view={activePaneView} />
                {agentPaneLabel(activePaneView, t)}
              </span>
            ) : displayedStatus ? (
              <span className="terminal-status-label">
                <span
                  className={`agent-session-status-dot agent-session-status-dot-${displayedStatus}`}
                  aria-hidden
                />
                {surfaceStatusLabel(displayedStatus, t)}
              </span>
            ) : null}

            <button
              aria-label={t("New shell")}
              className="terminal-icon-button terminal-new-shell-button"
              disabled={
                opening ||
                (activeSession
                  ? openingLeafIds.has(activeSession.id)
                  : Boolean(agentEntry) || !project)
              }
              onClick={() => void openShellFromHeader()}
              title={t("Open a shell beside the active terminal pane.")}
              type="button"
            >
              +
            </button>
            {!agentEntry ? (
              <button
                aria-label={t("Close terminal panel")}
                className="terminal-icon-button"
                onClick={onClose}
                type="button"
              >
                ×
              </button>
            ) : null}
          </div>
        </header>
        )}
        {fill && headerSlot && savingsMeter
          ? createPortal(savingsMeter, headerSlot)
          : null}
        {fill && resumeBannerShown && restoredPaneCount >= 2 ? (
          <div className="terminal-resume-banner" role="status">
            <span>{t("{count} saved panes can resume.", { count: restoredPaneCount })}</span>
            <button
              disabled={resumingAll}
              onClick={() => void resumeAllSessions()}
              type="button"
            >
              {resumingAll ? t("Resuming…") : t("Resume all")}
            </button>
          </div>
        ) : null}

        {projectPicker ? (
          <div
            className="terminal-project-picker-overlay"
            onClick={() => setProjectPicker(null)}
            role="presentation"
          >
            <div
              aria-label={t("Open agent in another project")}
              className="terminal-project-picker"
              onClick={(event) => event.stopPropagation()}
              role="dialog"
            >
              <strong>{t("Open agent in another project")}</strong>
              <span className="terminal-project-picker-label">
                {t("Project")}
              </span>
              <div className="terminal-project-picker-list">
                {(projects ?? [])
                  .filter((item) => item.id !== project?.id)
                  .map((item) => (
                    <button
                      data-selected={projectPicker.projectId === item.id}
                      key={item.id}
                      onClick={() =>
                        setProjectPicker({ ...projectPicker, projectId: item.id })
                      }
                      type="button"
                    >
                      <strong>{item.name}</strong>
                      <small>{item.rootPath}</small>
                    </button>
                  ))}
              </div>
              <span className="terminal-project-picker-label">
                {t("Agent")}
              </span>
              <div className="terminal-project-picker-agents">
                {splitAgentChoices.map((choice) => (
                  <button
                    data-ready={choice.ready}
                    data-selected={projectPicker.agentKind === choice.id}
                    key={choice.id}
                    onClick={() =>
                      setProjectPicker({
                        ...projectPicker,
                        agentKind: choice.id as AgentKind,
                      })
                    }
                    type="button"
                  >
                    {choice.label}
                  </button>
                ))}
              </div>
              <div className="terminal-project-picker-actions">
                <button onClick={() => setProjectPicker(null)} type="button">
                  {t("Cancel")}
                </button>
                <button
                  className="terminal-project-picker-confirm"
                  disabled={projectPicker.projectId === null}
                  onClick={() => {
                    const picked = projectPicker;
                    setProjectPicker(null);
                    if (picked.projectId === null) return;
                    void splitPane(
                      picked.source,
                      picked.direction,
                      `pick:${picked.agentKind}:${picked.projectId}`,
                    );
                  }}
                  type="button"
                >
                  {t("Open")}
                </button>
              </div>
            </div>
          </div>
        ) : null}

        {error ? (
          <div className="terminal-error" role="alert">{error}</div>
        ) : null}

        {supervisorFallbackMessage !== null ? (
          <TerminalInlineNotice
            detail={supervisorFallbackMessage}
            dismissLabel={t("Dismiss supervisor fallback notice")}
            message={t(
              "Terminals are running inside Halv because the supervisor could not start. They will not survive an app restart.",
            )}
            onDismiss={() => setSupervisorFallbackMessage(null)}
          />
        ) : null}

        {supervisorReplayAlert !== null ? (
          <TerminalInlineNotice
            detail={supervisorReplayAlert.detail}
            dismissLabel={t("Dismiss supervisor replay notice")}
            message={terminalSupervisorReplayMessage(
              supervisorReplayAlert,
              t,
            )}
            onDismiss={() => setSupervisorReplayAlert(null)}
          />
        ) : null}

        {activeTab ? (
          <TerminalLayoutRenderer
            activeLeafId={layout.activeLeafId}
            agentEntry={agentEntry}
            agentEntries={agentSessionsState.entries}
            activityStatusBySession={activityStatusBySession}
            closingSessionIds={closingSessionIds}
            node={activeTab.root}
            onActivate={activateLeaf}
            onClose={(session) => void closeSession(session)}
            onError={setError}
            onInput={noteResumeInput}
            onOpenShell={(session) => void continueInShell(session)}
            onRestart={
              shellExitRules
                ? (session) => void restartShell(session)
                : undefined
            }
            shellExitRules={shellExitRules}
            onPromptSubmitGate={onPromptSubmitGate}
            onPromptSubmitSignalHandled={onPromptSubmitSignalHandled}
            onSessionAttached={onSessionAttached}
            onSessionAttachError={onSessionAttachError}
            onCycleTab={cycleTab}
            onRatio={updateRatio}
            onResume={(session, startFresh, pane) =>
              void resumeSession(session, startFresh, pane)
            }
            onRetrySupervisor={(session) =>
              void retrySupervisorConnection(session)
            }
            onRearrange={
              activeTab.root.type === "split"
                ? (leafId, operation) => rearrangePane(leafId, operation)
                : null
            }
            onSplit={(session, direction, choiceId) =>
              void splitPane(session, direction, choiceId)
            }
            onTitle={(sessionId, title) =>
              setSessionTitles((current) => ({ ...current, [sessionId]: title }))
            }
            openingLeafIds={openingLeafIds}
            path={[]}
            paneSessionIds={paneSessionIds}
            promptSubmitSignals={promptSubmitSignals}
            restoredPanes={restoredPanes}
            resumeFailures={resumeFailures}
            resumeFallbackPanes={resumeFallbackPanes}
            resumingSessionIds={resumingSessionIds}
            sessions={sessions}
            sessionTitles={sessionTitles}
            supervisorFailureBySession={supervisorFailureBySession}
            supervisorReconnectBySession={supervisorReconnectBySession}
            retryingSupervisorSessionIds={retryingSupervisorSessionIds}
            terminalAttachGenerations={terminalAttachGenerations}
            splitChoices={combinedSplitChoices}
            shortcutOverrides={shortcutOverrides}
            visible={visible}
          />
        ) : startingLabel ||
          (fill && refreshedKey !== storageKey) ||
          (shellTabId && (opening || firstShellPending)) ||
          (agentEntry &&
            ["starting", "running", "waiting_input"].includes(agentEntry.session.status)) ? (
          // A tab that starts its agent, or a tab body that still reads
          // its panes, shows a calm placeholder. The choices of the empty
          // state would flash and then vanish.
          <div
            aria-busy="true"
            className="workspace-tab-body-starting"
            data-terminal-starting={startingLabel || agentEntry ? "true" : undefined}
          >
            {startingLabel || agentEntry ? (
              <>
                <span aria-hidden className="terminal-starting-spinner conversation-spinner" />
                <span>
                  {t("Starting {agent}…", {
                    agent: startingLabel ?? agentLabel(agentEntry!.session.agentKind),
                  })}
                </span>
              </>
            ) : null}
          </div>
        ) : shellTabId ? (
          // A shell tab whose shells ended. Its tab stays (ADR 0010, T5).
          <div className="terminal-empty-state">
            <span className="terminal-empty-mark" aria-hidden>{">_"}</span>
            <strong>{t("No shell runs in this tab")}</strong>
            <p>{t("Open a shell in the folder of this workspace.")}</p>
            <div className="terminal-empty-actions">
              <button
                disabled={!project || opening}
                onClick={() => void openTerminal()}
                type="button"
              >
                {opening ? t("Opening terminal…") : t("Open shell")}
              </button>
            </div>
          </div>
        ) : (
          <div className="terminal-empty-state">
            <span className="terminal-empty-mark" aria-hidden>{">_"}</span>
            <strong>
              {agentEntry
                ? t("Terminal unavailable")
                : t("Open agent terminal")}
            </strong>
            <p>
              {agentEntry
              ? t(
                  "This terminal process ended. Resume the saved session or start fresh in the same folder.",
                )
              : t("Choose Claude, Codex, Kimi, GLM, or Grok for this project.")}
            </p>
            {agentEntry ? (
              <div className="terminal-empty-actions">
                {agentEntry.session.cliSessionId ? (
                  <button
                    disabled={resumingSessionIds.has(agentEntry.session.id)}
                    onClick={() => void recoverAgentTerminal(false)}
                    type="button"
                  >
                    {resumingSessionIds.has(agentEntry.session.id)
                      ? t("Resuming…")
                      : t("Resume")}
                  </button>
                ) : null}
                <button
                  disabled={resumingSessionIds.has(agentEntry.session.id)}
                  onClick={() => void recoverAgentTerminal(true)}
                  type="button"
                >
                  {t("Start fresh")}
                </button>
              </div>
            ) : null}
            {!agentEntry ? (
              <div className="terminal-empty-actions">
                <button
                  disabled={!project || !onOpenAgentTerminal}
                  onClick={onOpenAgentTerminal}
                  type="button"
                >
                  {t("Open agent terminal")}
                </button>
                <button
                  disabled={!project || opening}
                  onClick={() => void openTerminal()}
                  type="button"
                >
                  {project
                    ? opening
                      ? t("Opening terminal…")
                      : t("Open shell in {project}", { project: project.name })
                    : t("Select a project first")}
                </button>
              </div>
            ) : null}
          </div>
        )}
      </div>
    </section>
  );
}

function TerminalInlineNotice({
  actions,
  detail,
  dismissLabel,
  message,
  onDismiss,
}: {
  actions?: ReactNode;
  detail?: string;
  dismissLabel: string;
  message: string;
  onDismiss: () => void;
}) {
  return (
    <div className="terminal-error terminal-inline-notice" role="alert">
      <div className="terminal-notice-message">
        <div>{message}</div>
        {detail ? (
          <div className="mt-0.5 text-[0.6rem] opacity-80">{detail}</div>
        ) : null}
      </div>
      <div className="terminal-notice-controls">
        {actions}
        <button
          aria-label={dismissLabel}
          className="terminal-notice-dismiss flex h-5 w-5 shrink-0 items-center justify-center rounded text-sm hover:bg-danger/10 focus:outline-none focus:ring-2 focus:ring-danger/30"
          onClick={onDismiss}
          type="button"
        >
          ×
        </button>
      </div>
    </div>
  );
}

type TerminalLayoutRendererProps = {
  onRearrange?:
    | ((
        leafId: string,
        operation: (
          node: TerminalPaneLayoutNode,
          leafId: string,
        ) => TerminalPaneLayoutNode,
      ) => void)
    | null;
  activeLeafId: string | null;
  agentEntry: AgentSessionEntry | null;
  agentEntries: AgentSessionEntry[];
  activityStatusBySession: Readonly<Record<string, TerminalActivityStatus>>;
  closingSessionIds: ReadonlySet<string>;
  node: TerminalPaneLayoutNode;
  onActivate: (leafId: string) => void;
  onClose: (session: TerminalSession) => void;
  onError: (message: string) => void;
  onInput: (sessionId: string) => void;
  onOpenShell: (session: TerminalSession) => void;
  /** The shell tab rules: a failed plain shell offers Restart. */
  onRestart?: (session: TerminalSession) => void;
  shellExitRules: boolean;
  onPromptSubmitGate?: () => boolean;
  onPromptSubmitSignalHandled: (sessionId: string, signal: number) => void;
  onSessionAttached: (sessionId: string) => void;
  onSessionAttachError: (sessionId: string, message: string) => void;
  onCycleTab: (offset: -1 | 1) => void;
  onRatio: (path: TerminalSplitPath, ratio: number) => void;
  onResume: (
    session: TerminalSession,
    startFresh: boolean,
    pane: TerminalPaneState,
  ) => void;
  onRetrySupervisor: (session: TerminalSession) => void;
  onSplit: (
    session: TerminalSession,
    direction: TerminalPaneSplitDirection,
    choiceId: "duplicate" | string,
  ) => void;
  onTitle: (sessionId: string, title: string) => void;
  openingLeafIds: ReadonlySet<string>;
  path: TerminalSplitPath;
  paneSessionIds: Readonly<Record<string, string>>;
  promptSubmitSignals: Readonly<Record<string, number>>;
  restoredPanes: Record<string, TerminalPaneState>;
  resumeFailures: Record<string, TerminalResumeFailure>;
  resumeFallbackPanes: Record<string, TerminalPaneState>;
  resumingSessionIds: ReadonlySet<string>;
  sessions: TerminalSession[];
  sessionTitles: Record<string, string>;
  splitChoices: TerminalPaneSplitChoice[];
  shortcutOverrides: KeyboardShortcutOverrides;
  supervisorFailureBySession: Readonly<Record<string, string>>;
  supervisorReconnectBySession: Readonly<
    Record<string, TerminalSupervisorReconnect>
  >;
  retryingSupervisorSessionIds: ReadonlySet<string>;
  terminalAttachGenerations: Readonly<Record<string, number>>;
  visible: boolean;
};

function TerminalLayoutRenderer(props: TerminalLayoutRendererProps) {
  const t = useT();
  const { node } = props;
  if (node.type === "leaf") {
    const paneId = node.leafId;
    const sessionId = props.paneSessionIds[paneId];
    const session = props.sessions.find((row) => row.id === sessionId);
    if (!session) return null;
    const entry =
      props.agentEntries.find((candidate) => candidate.session.id === session.id) ??
      (session.id === props.agentEntry?.session.id ? props.agentEntry : null);
    const displayStatus: SurfaceStatus = effectiveTerminalStatus(
      session.status,
      entry?.session.status,
      props.activityStatusBySession[session.id],
      entry?.session.mode,
    );
    const restoredPane = props.restoredPanes[paneId] ?? null;
    const resumeFallback = props.resumeFallbackPanes[paneId] ?? null;
    const startFreshPane =
      restoredPane?.agentKind && restoredPane.cliSessionId
        ? restoredPane
        : resumeFallback;
    return (
      <TerminalPane
        active={props.activeLeafId === paneId}
        closing={props.closingSessionIds.has(session.id)}
        displayStatus={displayStatus}
        restoredScrollback={restoredPane?.scrollback}
        resumeFailure={props.resumeFailures[paneId] ?? null}
        resuming={props.resumingSessionIds.has(session.id)}
        onActivate={() => props.onActivate(paneId)}
        onAttached={props.onSessionAttached}
        onAttachError={props.onSessionAttachError}
        onClose={() => props.onClose(session)}
        onCycleTab={props.onCycleTab}
        onError={props.onError}
        onInput={props.onInput}
        onOpenShell={
          // An ended live agent pane can continue in a shell. A saved pane
          // or a failed resume offers Resume and Start fresh instead. A
          // supervisor failure offers Retry: the agent can still run there.
          session.agentKind &&
          session.status !== "running" &&
          !restoredPane &&
          !props.resumeFailures[paneId] &&
          !props.supervisorFailureBySession[session.id]
            ? () => props.onOpenShell(session)
            : undefined
        }
        onPromptSubmitGate={props.onPromptSubmitGate}
        onPromptSubmitSignalHandled={props.onPromptSubmitSignalHandled}
        closesOnCleanExit={props.shellExitRules}
        onRestart={
          // A plain shell that ended starts again in the same pane. A saved
          // pane offers Resume instead.
          props.onRestart &&
          !session.agentKind &&
          session.status !== "running" &&
          !restoredPane &&
          !props.resumeFailures[paneId] &&
          !props.supervisorFailureBySession[session.id]
            ? () => props.onRestart!(session)
            : undefined
        }
        onResume={
          restoredPane
            ? () => props.onResume(session, false, restoredPane)
            : undefined
        }
        onRetrySupervisor={
          session.status === "failed" &&
          props.supervisorFailureBySession[session.id]
            ? () => props.onRetrySupervisor(session)
            : undefined
        }
        onStartFresh={
          startFreshPane
            ? () => props.onResume(session, true, startFreshPane)
            : undefined
        }
        onSplit={(direction, choiceId) =>
          props.onSplit(session, direction, choiceId)
        }
        onSwap={
          props.onRearrange
            ? () => props.onRearrange!(paneId, swapTerminalSiblings)
            : undefined
        }
        onRotate={
          props.onRearrange
            ? () => props.onRearrange!(paneId, rotateTerminalSplitAt)
            : undefined
        }
        onTitle={(title) => props.onTitle(session.id, title)}
        session={session}
        promptSubmitSignal={props.promptSubmitSignals[session.id] ?? 0}
        shortcutOverrides={props.shortcutOverrides}
        splitChoices={props.splitChoices}
        splitting={props.openingLeafIds.has(session.id)}
        supervisorFailureMessage={
          props.supervisorFailureBySession[session.id]
        }
        supervisorReconnect={
          props.supervisorReconnectBySession[session.id] ?? null
        }
        retryingSupervisor={props.retryingSupervisorSessionIds.has(session.id)}
        title={terminalTitle(session, props.sessionTitles, entry)}
        visible={props.visible}
        key={`${paneId}:${session.id}:${entry?.session.generation ?? restoredPane?.generation ?? 0}:${props.terminalAttachGenerations[session.id] ?? 0}`}
      />
    );
  }

  const ratio = node.ratio ?? 0.5;
  const firstStyle = { flexBasis: `${ratio * 100}%` } as CSSProperties;
  const secondStyle = { flexBasis: `${(1 - ratio) * 100}%` } as CSSProperties;
  return (
    <div className={`terminal-split terminal-split-${node.direction}`}>
      <div className="terminal-split-child" style={firstStyle}>
        <TerminalLayoutRenderer
          {...props}
          node={node.first}
          path={[...props.path, "first"]}
        />
      </div>
      <div
        aria-label={
          node.direction === "vertical"
            ? t("Resize vertical terminal split")
            : t("Resize horizontal terminal split")
        }
        className="terminal-split-divider"
        onPointerDown={(event) =>
          startDividerDrag(event, node.direction, props.path, props.onRatio)
        }
        role="separator"
      />
      <div className="terminal-split-child" style={secondStyle}>
        <TerminalLayoutRenderer
          {...props}
          node={node.second}
          path={[...props.path, "second"]}
        />
      </div>
    </div>
  );
}

function startDividerDrag(
  event: ReactPointerEvent<HTMLDivElement>,
  direction: TerminalPaneSplitDirection,
  path: TerminalSplitPath,
  onRatio: (path: TerminalSplitPath, ratio: number) => void,
) {
  event.preventDefault();
  const container = event.currentTarget.parentElement;
  if (!container) return;
  const pointerId = event.pointerId;
  event.currentTarget.setPointerCapture(pointerId);
  const move = (moveEvent: PointerEvent) => {
    const rect = container.getBoundingClientRect();
    const size = direction === "vertical" ? rect.width : rect.height;
    if (size <= 0) return;
    const position =
      direction === "vertical"
        ? moveEvent.clientX - rect.left
        : moveEvent.clientY - rect.top;
    const minimumRatio = Math.min(0.5, MIN_PANE_SIZE / size);
    const ratio = Math.min(1 - minimumRatio, Math.max(minimumRatio, position / size));
    onRatio(path, ratio);
  };
  const stop = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", stop);
    window.removeEventListener("pointercancel", stop);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", stop);
  window.addEventListener("pointercancel", stop);
}

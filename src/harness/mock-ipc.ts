import type {
  AgentMemory,
  AppPreferences,
  AppUpdateSnapshot,
  BillingPrices,
  BranchStatus,
  ChatDeltaPayload,
  ChatSendResponse,
  ConversationCompaction,
  ConversationSummary,
  CredentialProfile,
  ImageAttachment,
  MessageSearchResult,
  Project,
  RouteDecision,
  SidecarInstallProgress,
  SidecarStatus,
  StoredMessage,
  TaskRoute,
  RoutingSavingsReport,
  RoutingTotals,
  TerminalPrelude,
  TerminalSession,
  TokenUsage,
  TurnDiffSummary,
  WorkspaceTabKey,
  WorkspaceTabRow,
  WorkspaceTabsOp,
  WorkspaceStatus,
} from "../types";
import type {
  AgentKind,
  AgentSession,
  AgentSessionCreateLaneKind,
  AgentSessionEvent,
} from "../agentSessions";
import type {
  WalletProvider,
  WalletStaleSession,
  WalletState,
} from "../accountWallet";
import {
  halvWalletState,
  orcaClaudeUsageWindows,
  orcaClaudeWindowsFor,
  orcaLimitWalletUsage,
  orcaRestartStaleSessions,
  orcaRestartWalletState,
  orcaWalletState,
  orcaWalletUsage,
} from "./wallet-fixtures.ts";
import {
  setIpcTransport,
  type CommandName,
  type IpcContract,
  type RemoteStatus,
  type Transport,
} from "../ipc.ts";
import { completedTurnCount } from "../compaction.ts";
import { sortConversations } from "../conversationOrdering.ts";
import {
  AGENT_CAPABILITIES,
  AGENT_SESSION_CREDENTIAL_PROFILES,
  AGENT_SESSION_LANE_OPTIONS,
  AGENT_SESSION_PROBE,
  CHAT_MODELS,
  DAILY_TOKEN_ACTIVITY,
  HISTORICAL_AGENT_CONVERSATION_ID,
  HISTORICAL_REASONING,
  HISTORICAL_TOOLS,
  MARKETING_PRO_AGENT_EVENTS,
  MARKETING_PRO_AGENT_SESSION,
  MARKETING_PRO_SESSION_ID,
  MARKETING_PRO_TERMINAL_SCROLLBACK,
  MARKETING_PRO_TERMINAL_SESSION,
  RUNNING_SIDECAR,
  TRANSIENT_PROXY_MESSAGE,
  USAGE_LIMIT_MESSAGE,
  chatGptAuth,
  clone,
  createConversation,
  createFixtureState,
  normalizeApprovalPolicy,
  signedInAccount,
  signedOutAccount,
  syncConversationSummary,
  updateConversationProjectFields,
  type HarnessFixtureState,
} from "./fixtures.ts";
import type { HarnessSelection } from "./states";
import type { AgentStatusEntry } from "../agentPaneStatus.ts";
import { createFixtureWorkspaceTabs } from "./workspace-tabs.ts";
import { tabFacts } from "../workspaceTabs.ts";
import {
  AGENT_STATUS_ENTRIES,
  AGENT_STATUS_EXTRA_TERMINALS,
  prepareAgentStatusHarness,
  showHarnessNotificationCard,
} from "./agent-status.ts";
import {
  createMarketingWorkspaceAgentSessions,
  createMarketingWorkspaceAgentStatuses,
  createMarketingWorkspaceTerminals,
  MARKETING_WORKSPACE_AGENT_EVENTS,
  MARKETING_WORKSPACE_FOLDER,
  MARKETING_WORKSPACE_SCROLLBACK,
} from "./marketing-workspace.ts";
import {
  createGlyphShowcaseStatuses,
  createGlyphShowcaseTerminals,
  createProjectTreeAgentStatuses,
  createAdoptionTerminals,
  createProjectTreeTerminals,
  createSecondDocsPane,
  createSecondDocsPaneStatus,
  createSpikeWorktree,
  PROJECT_TREE_SPIKE_ID,
} from "./project-tree.ts";
import {
  appUpdateHarnessBlockers,
  appUpdateHarnessRelease,
  appUpdateHarnessSnapshot,
} from "./app-update-fixtures.ts";
import {
  MARKETING_PRO_SPLIT_AGENT_EVENTS,
  MARKETING_PRO_SPLIT_AGENT_SESSIONS,
  MARKETING_PRO_SPLIT_CONVERSATION_ID,
  MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
  MARKETING_PRO_SPLIT_SESSION_IDS,
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK,
  MARKETING_PRO_SPLIT_TERMINAL_SESSIONS,
  prepareMarketingProSplitHarness,
} from "./marketing-pro-split.ts";
import {
  MARKETING_CRUX_AGENT_EVENTS,
  MARKETING_CRUX_AGENT_SESSIONS,
  MARKETING_CRUX_TERMINAL_SCROLLBACK,
  MARKETING_CRUX_TERMINAL_SESSIONS,
  MARKETING_PRO_SPLIT_AGENT_EVENTS as MARKETING_SIX_AGENT_EVENTS,
  MARKETING_PRO_SPLIT_AGENT_SESSIONS as MARKETING_SIX_AGENT_SESSIONS,
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK as MARKETING_SIX_TERMINAL_SCROLLBACK,
  MARKETING_PRO_SPLIT_TERMINAL_SESSIONS as MARKETING_SIX_TERMINAL_SESSIONS,
  MARKETING_TERMINAL_CONVERSATION_ID,
  prepareMarketingTerminalSceneHarness,
} from "./marketing-terminal-scenes.ts";
import {
  MARKETING_CLAUDE_USAGE_WINDOWS,
  MARKETING_CODEX_USAGE_WINDOWS,
  isMarketingRealismFixture,
  prepareMarketingRealismHarness,
} from "./marketing-realism.ts";
import { prepareMarketingSavingsHarness } from "./marketing-savings.ts";
import {
  MARKETING_SUBAGENT_CONVERSATION_ID,
  MARKETING_SUBAGENT_CONVERSATION_TITLE,
  MARKETING_SUBAGENT_THREADS,
  prepareMarketingSubagentsHarness,
} from "./marketing-subagents.ts";

type InvokeArgs = Record<string, unknown>;

const TASK_ROUTES: readonly TaskRoute[] = [
  {
    id: "route-1",
    sessionId: "session-laya-1",
    conversationId: null,
    title: "Rename the settings header",
    briefChars: 214,
    complexity: "trivial",
    complexityConfidence: 0.92,
    needsReasoning: 0.08,
    risky: 0.02,
    tier: "cheapest",
    lane: "claude",
    model: "haiku",
    effort: null,
    reasons: ["difficulty"],
    workerLane: "auto",
    status: "passed",
    escalatedFromId: null,
    provider: "jev",
    providerModel: "typesafe/jev-1.13-20260917",
    latencyMs: 142,
    createdAt: "2026-09-21T10:00:00.000Z",
    updatedAt: "2026-09-21T10:00:32.000Z",
  },
  {
    id: "route-2",
    sessionId: "session-laya-1",
    conversationId: null,
    title: "Add the task routing toggle",
    briefChars: 806,
    complexity: "standard",
    complexityConfidence: 0.71,
    needsReasoning: 0.34,
    risky: 0.12,
    tier: "cheap",
    lane: "codex",
    model: "gpt-6-luna",
    effort: "high",
    reasons: ["difficulty", "quota"],
    workerLane: "auto",
    status: "failed",
    escalatedFromId: null,
    provider: "jev",
    providerModel: "typesafe/jev-1.13-20260917",
    latencyMs: 168,
    createdAt: "2026-09-21T10:04:00.000Z",
    updatedAt: "2026-09-21T10:09:11.000Z",
  },
  {
    id: "route-3",
    sessionId: "session-laya-1",
    conversationId: null,
    title: "Add the task routing toggle",
    briefChars: 806,
    complexity: "standard",
    complexityConfidence: 0.71,
    needsReasoning: 0.34,
    risky: 0.12,
    tier: "medium",
    lane: "codex",
    model: "gpt-6-sol",
    effort: "medium",
    reasons: ["escalated", "cap"],
    workerLane: "auto",
    status: "routed",
    escalatedFromId: "route-2",
    provider: "jev",
    providerModel: "typesafe/jev-1.13-20260917",
    latencyMs: 151,
    createdAt: "2026-09-21T10:09:12.000Z",
    updatedAt: "2026-09-21T10:09:12.000Z",
  },
  {
    id: "route-4",
    sessionId: "session-laya-2",
    conversationId: null,
    title: "Migrate the savings schema",
    briefChars: 1421,
    complexity: "hard",
    complexityConfidence: 0.83,
    needsReasoning: 0.77,
    risky: 0.64,
    tier: "strong",
    lane: "claude",
    model: "opus",
    effort: "xhigh",
    reasons: ["risk", "cap"],
    workerLane: "auto",
    status: "escalated",
    escalatedFromId: null,
    provider: "jev",
    providerModel: "typesafe/jev-1.13-20260917",
    latencyMs: 189,
    createdAt: "2026-09-21T10:15:00.000Z",
    updatedAt: "2026-09-21T10:21:40.000Z",
  },
  {
    id: "route-5",
    sessionId: "session-laya-2",
    conversationId: null,
    title: "Review the release checklist",
    briefChars: 512,
    complexity: "hard",
    complexityConfidence: 0.55,
    needsReasoning: 0.81,
    risky: 0.21,
    tier: "max",
    lane: "claude",
    model: null,
    effort: null,
    reasons: ["confidence", "quota"],
    workerLane: "auto",
    status: "passed",
    escalatedFromId: null,
    provider: "jev",
    providerModel: "typesafe/jev-1.13-20260917",
    latencyMs: 174,
    createdAt: "2026-09-21T10:30:00.000Z",
    updatedAt: "2026-09-21T10:44:02.000Z",
  },
];

const workflowRoute = (
  overrides: Partial<TaskRoute> & Pick<TaskRoute, "id" | "title" | "tier">,
): TaskRoute => ({
  sessionId: "session-laya-1",
  conversationId: null,
  briefChars: 180,
  complexity: "hard",
  complexityConfidence: 0.96,
  needsReasoning: 0.88,
  risky: 0.04,
  lane: "claude",
  model: null,
  effort: null,
  reasons: ["difficulty", "reasoning"],
  workerLane: "claude",
  status: "passed",
  escalatedFromId: null,
  provider: "jev",
  providerModel: "typesafe/jev-1.13-20260917",
  latencyMs: 131,
  createdAt: "2026-09-21T10:20:00.000Z",
  updatedAt: "2026-09-21T10:31:00.000Z",
  defaultModel: "claude-opus-5-5",
  defaultEffort: "xhigh",
  defaultSource: "observed",
  closedUnreported: false,
  receipt: null,
  ...overrides,
});

const workflowFields = (
  key: string,
  role: "read" | "implement" | "verify",
  instances = 1,
) => ({
  batchId: "batch-fix-findings",
  name: "fix-findings",
  key,
  role,
  instances,
});

const workflowReceipt = (
  routeId: string,
  outcome: "passed" | "failed" | "escalated",
  models: string[],
  outputTokens: number,
) => ({
  routeId,
  sessionId: "session-laya-1",
  outcome,
  agents: 1,
  failedAgents: 0,
  inputTokens: 2_400,
  outputTokens,
  cacheReadTokens: 1_820_000,
  cacheCreationTokens: 64_000,
  durationMs: 240_000,
  usageSource: "measured" as const,
  reportedAt: "2026-09-21T10:31:00.000Z",
  models,
});

// One routed Workflow batch (ADR 0014): a read stage on Sonnet, an escalated
// implement stage and its retry, a running verify stage, a stage that JEV
// did not answer, and an agent that ignored its route.
const WORKFLOW_TASK_ROUTES: readonly TaskRoute[] = [
  workflowRoute({
    id: "wf-scout",
    title: "Map the parser call sites",
    tier: "cheap",
    model: "sonnet",
    effort: "xhigh",
    reasons: ["difficulty", "reasoning", "role"],
    workflow: workflowFields("scout", "read", 4),
    receipt: workflowReceipt("wf-scout", "passed", ["claude-sonnet-5-5"], 18_200),
  }),
  workflowRoute({
    id: "wf-fix",
    title: "Fix one parser finding",
    tier: "cheap",
    model: "sonnet",
    effort: "high",
    complexity: "standard",
    reasons: ["difficulty"],
    status: "escalated",
    workflow: workflowFields("fix", "implement", 3),
    receipt: workflowReceipt("wf-fix", "escalated", ["claude-sonnet-5-5"], 26_500),
  }),
  workflowRoute({
    id: "wf-fix-retry",
    title: "Fix one parser finding",
    tier: "medium",
    model: "opus",
    effort: "medium",
    complexity: "standard",
    reasons: ["difficulty", "escalated"],
    escalatedFromId: "wf-fix",
    createdAt: "2026-09-21T10:31:00.000Z",
    updatedAt: "2026-09-21T10:38:00.000Z",
    workflow: workflowFields("fix", "implement", 3),
    receipt: workflowReceipt("wf-fix-retry", "passed", ["claude-opus-5-5"], 21_900),
  }),
  workflowRoute({
    id: "wf-check",
    title: "Challenge every fix",
    tier: "medium",
    model: "opus",
    effort: "high",
    status: "routed",
    reasons: ["difficulty", "reasoning", "role"],
    workflow: workflowFields("check", "verify", 2),
  }),
  workflowRoute({
    id: "wf-docs",
    title: "Update the parser notes",
    tier: "max",
    provider: "fallback",
    providerModel: "none",
    reasons: ["fallback"],
    workflow: workflowFields("docs", "implement"),
    receipt: workflowReceipt("wf-docs", "passed", ["claude-opus-5-5"], 4_100),
  }),
  workflowRoute({
    id: "wf-audit",
    title: "List the old parser flags",
    tier: "cheap",
    model: "sonnet",
    effort: "xhigh",
    reasons: ["difficulty", "reasoning", "role"],
    workflow: workflowFields("audit", "read"),
    receipt: workflowReceipt("wf-audit", "passed", ["claude-opus-5-5"], 9_800),
  }),
];
// JEV routing dollars (ADR 0016). `?routing=estimated` gives the savings
// surfaces settled routing chains; without it routing is not estimated.
const ROUTING_WEEK: RoutingTotals = {
  chains: 12,
  unpricedChains: 1,
  lowUsd: 0.84,
  pointUsd: 1.02,
  highUsd: 2.65,
  benchChains: 0,
  unreportedChains: 1,
};
const ROUTING_LIFETIME: RoutingTotals = {
  ...ROUTING_WEEK,
  chains: 31,
  lowUsd: 2.41,
  pointUsd: 2.96,
  highUsd: 7.38,
};
const NO_ROUTING: RoutingTotals = {
  chains: 0,
  unpricedChains: 0,
  lowUsd: 0,
  pointUsd: 0,
  highUsd: 0,
  benchChains: 0,
  unreportedChains: 0,
};
// The routes of one conversation: Haiku gains at the floor, Sonnet gets no
// credit without enough bench pairs, and a failed attempt is a measured loss.
const ROUTING_REPORT: RoutingSavingsReport = {
  totals: {
    chains: 5,
    unpricedChains: 0,
    lowUsd: 0.18,
    pointUsd: 0.31,
    highUsd: 1.42,
    benchChains: 0,
    unreportedChains: 0,
  },
  chains: [],
  routeCosts: [
    { routeId: "route-1", actualUsd: 0.02, models: ["claude-haiku-5-5"] },
    { routeId: "wf-scout", actualUsd: 0.43, models: ["claude-sonnet-5-5"] },
    { routeId: "wf-fix", actualUsd: 0.61, models: ["claude-sonnet-5-5"] },
    { routeId: "wf-fix-retry", actualUsd: 1.12, models: ["claude-opus-5-5"] },
    { routeId: "wf-docs", actualUsd: 0.38, models: ["claude-opus-5-5"] },
    { routeId: "wf-audit", actualUsd: 0.41, models: ["claude-opus-5-5"] },
  ],
};

function routingFixtureOn(): boolean {
  return (
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("routing") === "estimated"
  );
}

function withRouting<T extends { dollarsUsd?: number; routing?: RoutingTotals }>(
  value: T,
  routing: RoutingTotals,
): T {
  if (!routingFixtureOn()) return value;
  return {
    ...value,
    dollarsUsd: (value.dollarsUsd ?? 0) + routing.lowUsd,
    routing,
  };
}

type Callback = (payload: unknown) => unknown;

export type UxAuditHarnessMock = {
  invoke: <T>(command: string, args?: InvokeArgs) => Promise<T>;
  subscribe: <T>(event: string, handler: (payload: T) => void) => () => void;
};

type TauriInternals = {
  invoke: (command: string, args?: InvokeArgs, options?: unknown) => Promise<unknown>;
  transformCallback: (callback?: Callback, once?: boolean) => number;
  unregisterCallback: (id: number) => void;
  runCallback: (id: number, payload: unknown) => unknown;
  callbacks: Map<number, Callback>;
  convertFileSrc: (path: string, protocol?: string) => string;
  metadata: {
    currentWindow: { label: string };
    currentWebview: { label: string; windowLabel: string };
  };
};

type TauriHarnessWindow = Window &
  typeof globalThis & {
    __TAURI_INTERNALS__?: TauriInternals;
  };

type ActiveStream = {
  conversationId: string;
  timers: Set<number>;
  assistantText: string;
  cancelled: boolean;
  snapshotTurnDiffs: TurnDiffSummary[];
  resolve: (response: ChatSendResponse) => void;
  reject: (reason: unknown) => void;
  revertibleFiles: number;
};

const USD_BILLING_PRICES = {
  currency: "usd",
  plans: {
    basic: { month: 200, year: 1_600 },
    unlimited: { month: 1_000, year: 7_900 },
  },
} satisfies BillingPrices;

const STREAM_USAGE: TokenUsage = {
  inputTokens: 3_842,
  outputTokens: 786,
  totalTokens: 4_628,
  cachedTokens: 1_280,
  reasoningTokens: 312,
};

let installedFixtureTransport: FixtureTransport | undefined;

export function emitHarnessEvent(event: string, payload: unknown) {
  if (!installedFixtureTransport) throw new Error("Harness is not installed");
  return installedFixtureTransport.emit(event, payload);
}

export type HarnessTerminalExit = {
  exitCode?: number;
  signal?: string;
  // The last agent screen. A continuation shell shows it as its prelude.
  lastScreen?: string;
  // False simulates a continuation shell that cannot start, for example
  // because the folder was removed.
  continueInShell?: boolean;
};

// Ends a harness terminal the way the backend does: the terminal event
// first, then the continuation shell after a clean agent exit, then the
// agent session event. Returns the continuation shell, if one starts.
export function exitHarnessTerminal(
  sessionId: string,
  exit: HarnessTerminalExit = {},
) {
  if (!installedFixtureTransport) throw new Error("Harness is not installed");
  return installedFixtureTransport.exitTerminal(sessionId, exit);
}

export function installUxAuditHarness(selection: HarnessSelection) {
  const { mock, router, runtime, state } = createHarnessRuntime(selection);
  suppressBrowserNotifications();
  runtime.install();
  installedFixtureTransport = router;
  prepareMarketingRealismHarness(selection.name);
  prepareMarketingProSplitHarness(selection.name);
  prepareMarketingTerminalSceneHarness(selection.name);
  prepareMarketingSubagentsHarness(selection.name);
  prepareMarketingSavingsHarness(selection.name);
  prepareAgentStatusHarness(selection.name);
  router.startScenarioLifecycle();
  document.documentElement.dataset.uxAuditScenario = state.scenario;
  document.documentElement.dataset.harnessState = selection.name;
  return mock;
}

export function createUxAuditHarnessMock(
  selection: HarnessSelection,
): UxAuditHarnessMock {
  const { mock, router } = createHarnessRuntime(selection);
  router.startScenarioLifecycle();
  return mock;
}

function harnessSessionStorage() {
  try {
    return typeof window === "undefined" ? null : window.sessionStorage;
  } catch {
    return null;
  }
}

function createHarnessRuntime(selection: HarnessSelection) {
  const state = createFixtureState(selection.definition.fixture);
  const transport = new FixtureTransport(state);
  const mock: UxAuditHarnessMock = {
    invoke: (command, args = {}) => transport.invokeHarness(command, args),
    subscribe: (event, handler) => transport.subscribe(event, handler),
  };
  return { mock, router: transport, runtime: transport, state };
}

function suppressBrowserNotifications() {
  class AuditNotification {
    static readonly permission: NotificationPermission = "denied";

    static requestPermission() {
      return Promise.resolve("denied" as NotificationPermission);
    }
  }

  Object.defineProperty(window, "Notification", {
    configurable: true,
    value: AuditNotification,
  });
}

export class FixtureTransport implements Transport {
  private readonly callbacks = new Map<number, Callback>();
  private readonly eventListeners = new Map<
    string,
    Set<(payload: unknown) => void>
  >();
  private readonly activeStreams = new Map<string, ActiveStream>();
  private readonly compactions = new Map<string, ConversationCompaction>();
  private credentialProfiles: CredentialProfile[];
  private readonly agentLogins = new Set<AgentKind>();
  private wallet: WalletState = halvWalletState();
  private walletPendingCount = 0;
  // Agent panes that still run an old account (wallet_stale_sessions).
  private walletStale: WalletStaleSession[] = [];
  private agentSessions: AgentSession[] = [];
  private terminalSessions: TerminalSession[] = [];
  private readonly terminalScrollback = new Map<string, string>();
  private readonly terminalPreludes = new Map<string, TerminalPrelude>();
  private readonly terminalContinuations = new Map<string, string>();
  private agentStatuses: AgentStatusEntry[] = [];
  // The ?environments=1 file is unapproved until local_environment_approve.
  private fixtureEnvironmentApproved = false;
  private readonly workspaceTabs = createFixtureWorkspaceTabs(() =>
    tabFacts({
      conversations: this.state.conversations,
      workspaces: [
        ...this.state.projects.map((project) => ({
          id: `primary-${project.id}`,
          projectId: project.id,
          kind: "primary" as const,
        })),
        ...this.state.worktrees,
      ],
    }),
    harnessSessionStorage(),
  );
  private memories: AgentMemory[] = [];
  private historicalOutputHydrationNeeded = true;
  private marketingProAgentEventsDelivered = false;
  private marketingTerminalSceneEventsDelivered = false;
  private automaticInstallTimer: number | null = null;
  private installPromise: Promise<SidecarStatus> | null = null;
  private installResolve: ((status: SidecarStatus) => void) | null = null;
  private installTimers = new Set<number>();
  private anthropicConfigured = false;
  private remote: RemoteStatus = {
    enabled: false,
    relayUrl: "wss://relay.halv.ai",
    relayUrlIsDefault: true,
    connection: "off",
    error: null,
    hostId: null,
    hostName: "Demo MacBook Pro",
    devices: [],
    pairing: null,
    pendingApproval: null,
  };
  private remoteHostCount = 0;
  private readonly state: HarnessFixtureState;
  private appUpdate: AppUpdateSnapshot;
  private preferences: AppPreferences = {
    subagentConcurrencyLimit: null as number | null,
    completionNotifications: "background",
    permissionNotifications: true,
    questionNotifications: true,
    keyboardShortcuts: {} as Record<string, string>,
    personality: "none",
    ponytail: "off",
    claudeSubscriptionProxy: true,
    memoriesEnabled: true,
    analyticsEnabled: true,
    defaultModel: null,
    defaultEffort: null,
    defaultAgentMode: true,
    defaultApprovalPolicy: null,
    defaultFastMode: false,
    taskRouting: true,
    workflowRouting: true,
    claudeUltracode: false,
    workerLane: "auto",
    routingTierAgents: {
      cheapest: { lane: "auto", model: null, effort: null },
      cheap: { lane: "auto", model: null, effort: null },
      medium: { lane: "auto", model: null, effort: null },
      strong: { lane: "auto", model: null, effort: null },
    },
  };

  constructor(state: HarnessFixtureState) {
    this.state = state;
    this.appUpdate = appUpdateHarnessSnapshot(state.fixture.update);
    // ?remote= selects a Settings > Mobile state for the visual harness.
    this.remote = remoteFixtureStatus(
      this.remote,
      typeof window === "undefined"
        ? null
        : new URLSearchParams(window.location.search).get("remote"),
    );
    if (state.fixture.wallet === "orca" || state.fixture.wallet === "orca-limit") {
      this.wallet = orcaWalletState();
    }
    if (state.fixture.wallet === "orca-restart") {
      this.wallet = orcaRestartWalletState();
      this.walletStale = orcaRestartStaleSessions({
        codex: MARKETING_PRO_SPLIT_SESSION_IDS.codex,
        claude: MARKETING_PRO_SPLIT_SESSION_IDS.bypass,
      });
    }
    this.credentialProfiles =
      state.fixture.credentials === "none"
        ? []
        : clone(AGENT_SESSION_CREDENTIAL_PROFILES);
    if (
      state.fixture.credentials === "chatgpt" ||
      state.fixture.credentials === "codex"
    ) {
      this.agentLogins.add("codex");
    }
    if (
      state.fixture.history === "marketing" ||
      state.fixture.history === "marketing-pro"
    ) {
      this.agentLogins.add("claude");
    }
    if (state.fixture.history === "marketing-pro") {
      this.agentSessions = [clone(MARKETING_PRO_AGENT_SESSION)];
      this.terminalSessions = [clone(MARKETING_PRO_TERMINAL_SESSION)];
    }
    if (state.fixture.marketingProSplit) {
      const conversation = state.conversations.find(
        (candidate) => candidate.id === MARKETING_PRO_SPLIT_CONVERSATION_ID,
      );
      if (conversation) {
        conversation.title = MARKETING_PRO_SPLIT_CONVERSATION_TITLE;
        conversation.titleSource = "manual";
        conversation.mode = "agent";
        conversation.agentMode = true;
        conversation.agentBackend = "claude_session";
        conversation.agentSessionModel = "sonnet";
      }
      this.agentSessions = clone(MARKETING_PRO_SPLIT_AGENT_SESSIONS);
      this.terminalSessions = clone(MARKETING_PRO_SPLIT_TERMINAL_SESSIONS);
    }
    if (state.fixture.agentStatus) {
      this.terminalSessions.push(...clone(AGENT_STATUS_EXTRA_TERMINALS));
      this.agentStatuses = clone(AGENT_STATUS_ENTRIES);
    }
    if (state.fixture.worktrees) {
      this.terminalSessions.push(
        ...createProjectTreeTerminals(state.projects, state.worktrees),
      );
      this.agentStatuses.push(...createProjectTreeAgentStatuses());
      // ?spike=clean|dirty adds a worktree that no conversation uses, for
      // the removal specs of the tree.
      const spike =
        typeof window === "undefined"
          ? null
          : new URLSearchParams(window.location.search).get("spike");
      if (
        (spike === "clean" || spike === "dirty") &&
        !state.worktrees.some((item) => item.id === PROJECT_TREE_SPIKE_ID)
      ) {
        state.worktrees.push(createSpikeWorktree(state.projects, spike === "dirty"));
      }
      // ?adoption=1 adds two shells outside every project (ADR 0020): one in
      // a git repository that is no project yet, one in the home folder.
      if (typeof window !== "undefined" && new URLSearchParams(window.location.search).get("adoption") === "1") {
        this.terminalSessions.push(...createAdoptionTerminals());
      }
      // ?panes=1 gives the docs conversation a second agent pane, for the
      // pane child rows of the tree.
      if (typeof window !== "undefined" && new URLSearchParams(window.location.search).get("panes") === "1") {
        this.terminalSessions.push(...createSecondDocsPane(this.terminalSessions));
        this.agentStatuses.push(createSecondDocsPaneStatus());
      }
    }
    // ?nested=1 adds a folder project that holds the two fixture projects,
    // for the folder-scan specs of the tree (ADR 0019).
    if (
      typeof window !== "undefined" &&
      new URLSearchParams(window.location.search).get("nested") === "1" &&
      state.projects.length > 0 &&
      !state.projects.some((project) => project.rootPath === NESTED_REPO_PARENT)
    ) {
      const now = Date.now();
      state.projects.push({
        id: state.nextProject++,
        name: "Projects",
        rootPath: NESTED_REPO_PARENT,
        trustLevel: "trusted",
        approvalPolicy: "ask",
        createdAt: new Date(now - 9_000 * 60_000).toISOString(),
        lastOpenedAt: new Date(now - 300 * 60_000).toISOString(),
      });
    }
    if (state.fixture.glyphShowcase) {
      this.terminalSessions.push(...createGlyphShowcaseTerminals(state.projects));
      this.agentStatuses.push(...createGlyphShowcaseStatuses());
    }
    if (state.fixture.marketingTerminalScene) {
      const scene =
        state.fixture.marketingTerminalScene === "pro-split"
          ? {
              sessions: MARKETING_SIX_AGENT_SESSIONS,
              terminals: MARKETING_SIX_TERMINAL_SESSIONS,
              title: "Parallel release hardening",
            }
          : {
              sessions: MARKETING_CRUX_AGENT_SESSIONS,
              terminals: MARKETING_CRUX_TERMINAL_SESSIONS,
              title: "Map authentication architecture",
            };
      const conversation = state.conversations.find(
        (candidate) => candidate.id === MARKETING_TERMINAL_CONVERSATION_ID,
      );
      if (conversation) {
        conversation.title = scene.title;
        conversation.titleSource = "manual";
        conversation.mode = "agent";
        conversation.agentMode = true;
        conversation.agentBackend = "claude_session";
        conversation.agentSessionModel = "sonnet";
      }
      this.agentSessions = clone(scene.sessions);
      this.terminalSessions = clone(scene.terminals);
    }
    if (state.fixture.marketingSubagents) {
      const conversation = state.conversations.find(
        (candidate) => candidate.id === MARKETING_SUBAGENT_CONVERSATION_ID,
      );
      if (conversation) {
        conversation.title = MARKETING_SUBAGENT_CONVERSATION_TITLE;
        conversation.titleSource = "manual";
        conversation.messageCount = 1;
      }
    }
    if (state.fixture.marketingWorkspace) {
      this.agentLogins.add("claude");
      this.agentSessions = createMarketingWorkspaceAgentSessions();
      this.terminalSessions = createMarketingWorkspaceTerminals();
      this.agentStatuses = createMarketingWorkspaceAgentStatuses();
    }
    if (state.fixture.marketingSubagents || state.fixture.marketingSavings) {
      const badgeConversation = state.conversations.find(
        (conversation) => conversation.id === "audit-agent-refactor",
      );
      this.terminalSessions.push({
        ...clone(MARKETING_PRO_TERMINAL_SESSION),
        id: "marketing-sidebar-saved-terminal",
        title: "Claude 1",
        projectId: badgeConversation?.projectId ?? null,
        projectName: badgeConversation?.projectName ?? "halv",
        cwd:
          badgeConversation?.projectRootPath ?? "/Users/demo/programming/halv",
        conversationId: "audit-agent-refactor",
      });
    }
  }

  install() {
    setIpcTransport(this);
    const harnessWindow = window as TauriHarnessWindow;
    harnessWindow.__TAURI_INTERNALS__ = {
      invoke: (command, args = {}) => this.invokeHarness(command, args),
      transformCallback: (callback, once = false) =>
        this.registerCallback(callback, once),
      unregisterCallback: (id) => this.callbacks.delete(id),
      runCallback: (id, payload) => this.runCallback(id, payload),
      callbacks: this.callbacks,
      convertFileSrc: (path, protocol = "asset") =>
        `${protocol}://localhost/${encodeURIComponent(path)}`,
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" },
      },
    };
  }

  async invoke<K extends CommandName>(
    name: K,
    args: IpcContract[K]["args"],
  ): Promise<IpcContract[K]["result"]> {
    return (await fixtureCommandHandlers[name](
      this,
      (args ?? {}) as InvokeArgs,
    )) as IpcContract[K]["result"];
  }

  async invokeHarness<T>(command: string, args: InvokeArgs): Promise<T> {
    return (await this.routeCommand(command, args)) as T;
  }

  listen<T>(event: string, handler: (payload: T) => void) {
    return Promise.resolve(this.subscribe(event, handler));
  }

  subscribe<T>(event: string, handler: (payload: T) => void) {
    const listeners =
      this.eventListeners.get(event) ?? new Set<(payload: unknown) => void>();
    const listener = handler as (payload: unknown) => void;
    listeners.add(listener);
    this.eventListeners.set(event, listeners);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0) this.eventListeners.delete(event);
    };
  }

  async emit(event: string, payload: unknown) {
    for (const listener of [...(this.eventListeners.get(event) ?? [])]) {
      listener(payload);
    }
  }

  async exitTerminal(sessionId: string, exit: HarnessTerminalExit) {
    const terminal = this.terminalSessions.find(
      (candidate) => candidate.id === sessionId,
    );
    if (!terminal) throw new Error("Terminal session was not found");
    const exitCode = exit.exitCode ?? 0;
    const exitedAt = new Date().toISOString();
    if (exit.lastScreen !== undefined) {
      this.terminalScrollback.set(sessionId, exit.lastScreen);
    }
    terminal.status = "exited";
    terminal.exitCode = exitCode;
    terminal.signal = exit.signal;
    terminal.exitedAt = exitedAt;
    await this.emit("terminal_event", {
      sessionId,
      // Above every sequence that a harness data event uses.
      sequence: 1_000_000,
      kind: "exit",
      exitCode,
      signal: exit.signal,
    });
    const continuation =
      exit.continueInShell !== false &&
      terminal.agentKind &&
      exitCode === 0 &&
      !exit.signal
        ? this.continueTerminalInShell(sessionId)
        : null;
    const agent = this.agentSessions.find(
      (candidate) => candidate.id === sessionId,
    );
    if (agent) {
      agent.status = exitCode === 0 ? "exited" : "failed";
      agent.exitCode = exitCode;
      agent.lastActiveAt = exitedAt;
      agent.generation = (agent.generation ?? 0) + 1;
      await this.emit("agent-session-event", {
        sessionId,
        event: { type: "sessionExited", code: exitCode },
      });
    }
    return continuation ? clone(continuation) : null;
  }

  // The backend gives each ended agent at most one continuation shell.
  private continueTerminalInShell(sourceId: string) {
    const existingId = this.terminalContinuations.get(sourceId);
    const existing = this.terminalSessions.find(
      (candidate) => candidate.id === existingId,
    );
    if (existing) return existing;
    const source = this.terminalSessions.find(
      (candidate) => candidate.id === sourceId,
    );
    if (!source) throw new Error(`terminal session \`${sourceId}\` was not found`);
    if (source.status === "running") {
      throw new Error("the terminal is still running");
    }
    if (!source.agentKind) {
      throw new Error("only agent terminals continue in a shell");
    }
    const session: TerminalSession = {
      ...auditTerminalSession(
        `terminal-${crypto.randomUUID().replace(/-/g, "")}`,
        source.cwd,
        source.projectId ?? null,
        source.projectName,
      ),
      conversationId: source.conversationId ?? undefined,
      shellTabId: source.shellTabId ?? undefined,
      continuedFrom: source.id,
    };
    this.terminalSessions.push(session);
    this.terminalContinuations.set(sourceId, session.id);
    this.terminalPreludes.set(session.id, {
      scrollback: this.terminalScrollback.get(sourceId) ?? "",
      agentKind: source.agentKind,
      exitCode: source.exitCode ?? null,
    });
    void this.emit("terminal_continued", {
      fromSessionId: sourceId,
      session: clone(session),
    });
    return session;
  }

  // A plain shell next to a live session or a saved pane.
  private openShellBeside(sourceId: string, scope: string | null) {
    const live = this.terminalSessions.find(
      (candidate) => candidate.id === sourceId,
    );
    let origin: {
      cwd: string;
      projectId: number | null;
      projectName: string;
      conversationId: string | null;
      shellTabId?: string | null;
    } | null = live
      ? {
          cwd: live.cwd,
          projectId: live.projectId ?? null,
          projectName: live.projectName,
          conversationId: live.conversationId ?? null,
          shellTabId: live.shellTabId ?? null,
        }
      : null;
    if (!origin && scope) {
      const raw = window.localStorage.getItem(
        `ux-audit.terminal-restore:${scope}`,
      );
      const saved = raw
        ? (JSON.parse(raw) as {
            panes: Array<{
              sessionId: string;
              cwd: string;
              projectId: number | null;
              projectName: string;
              conversationId: string | null;
            }>;
          })
        : null;
      const pane = saved?.panes.find((candidate) => candidate.sessionId === sourceId);
      // Like Rust, a saved pane takes its owner from the shell tab scope.
      origin = pane
        ? {
            ...pane,
            shellTabId: pane.conversationId ? null : harnessShellTabIdFromScope(scope),
          }
        : null;
    }
    if (!origin) {
      throw new Error(`terminal session \`${sourceId}\` was not found`);
    }
    const session: TerminalSession = {
      ...auditTerminalSession(
        `terminal-${crypto.randomUUID().replace(/-/g, "")}`,
        origin.cwd,
        origin.projectId,
        origin.projectName,
      ),
      conversationId: origin.conversationId ?? undefined,
      shellTabId: origin.shellTabId ?? undefined,
    };
    this.terminalSessions.push(session);
    return session;
  }

  // Like the Rust gateway, the event never holds the pairing offer.
  private emitRemoteChanged(): RemoteStatus {
    const status = clone(this.remote);
    void this.emit("remote-changed", { ...clone(status), pairing: null });
    return status;
  }

  private registerCallback(callback?: Callback, once = false) {
    let id = crypto.getRandomValues(new Uint32Array(1))[0];
    while (this.callbacks.has(id)) {
      id = crypto.getRandomValues(new Uint32Array(1))[0];
    }
    this.callbacks.set(id, (payload) => {
      if (once) this.callbacks.delete(id);
      return callback?.(payload);
    });
    return id;
  }

  private runCallback(id: number, payload: unknown) {
    return this.callbacks.get(id)?.(payload);
  }

  startScenarioLifecycle() {
    if (this.state.scenario !== "fresh") {
      return;
    }
    this.automaticInstallTimer = window.setTimeout(() => {
      this.automaticInstallTimer = null;
      void this.beginHeadroomInstall();
    }, 2_000);
  }

  routeCommand(command: string, args: InvokeArgs): unknown {
    switch (command) {
      case "account_claim_referral": {
        const code = stringArg(args, "code").trim().toUpperCase();
        const referral = {
          code: "HALVDEMO",
          link: "https://halv.ai/invite/HALVDEMO",
          trialDays: 7,
          invited: 0,
          rewarded: 0,
          claimedCode: code,
        };
        this.state.account.referral = referral;
        if (this.state.account.entitlements) {
          this.state.account.entitlements.referral = referral;
        }
        return { ok: true, trialDays: referral.trialDays };
      }
      case "account_savings_queue_status":
        return { unreported: 0, rejected: 0, lastError: null };
      case "account_publish_receipt":
        return {
          id: "audit-receipt",
          url: "https://halv.ai/r/audit-receipt",
        };
      case "auth_status":
        return clone(this.state.account);
      case "auth_credential_status":
        return clone(this.state.auth);
      case "auth_anthropic_status":
        return {
          configured: this.anthropicConfigured,
          ...(this.anthropicConfigured ? { masked: "sk-ant-...audit" } : {}),
        };
      case "auth_set_anthropic_api_key":
        this.anthropicConfigured = true;
        return { configured: true, masked: "sk-ant-...audit" };
      case "auth_clear_anthropic_api_key":
        this.anthropicConfigured = false;
        return { configured: false };
      case "auth_login_chatgpt":
        if (this.state.fixture.chatGptLogin === "pending") {
          return new Promise<never>(() => undefined);
        }
        if (this.state.fixture.chatGptLogin === "failure") {
          throw new Error("ChatGPT connection was not completed");
        }
        this.state.auth = chatGptAuth(this.state.account.email ?? undefined);
        return clone(this.state.auth);
      case "auth_logout":
      case "auth_clear_api_key":
        this.state.auth = { signedOut: {} };
        return clone(this.state.auth);
      case "auth_set_api_key": {
        const key = stringArg(args, "apiKey");
        this.state.auth = {
          apiKey: { masked: `sk-...${key.slice(-4).padStart(4, "•")}` },
        };
        return clone(this.state.auth);
      }
      case "credential_profile_add": {
        const provider = stringArg(args, "provider");
        const kind = stringArg(args, "kind");
        if (
          provider !== "openai" &&
          provider !== "anthropic" &&
          provider !== "zai" &&
          provider !== "xai" &&
          provider !== "moonshot"
        ) {
          throw new Error("Credential provider is not supported");
        }
        if (kind !== "api_key") {
          throw new Error("OAuth profiles cannot be added yet");
        }
        stringArg(args, "secret");
        const now = new Date().toISOString();
        const profile: CredentialProfile = {
          id: crypto.randomUUID(),
          provider,
          name: stringArg(args, "name").trim(),
          kind,
          priority: 0,
          status: "active",
          lastValidatedAt: now,
          metadataJson: null,
          createdAt: now,
          updatedAt: now,
        };
        this.credentialProfiles.push(profile);
        return clone(profile);
      }
      case "credential_profile_remove": {
        const id = stringArg(args, "id");
        this.requireCredentialProfile(id);
        this.credentialProfiles = this.credentialProfiles.filter(
          (profile) => profile.id !== id,
        );
        return null;
      }
      case "credential_profile_rename": {
        const profile = this.requireCredentialProfile(stringArg(args, "id"));
        profile.name = stringArg(args, "name").trim();
        profile.updatedAt = new Date().toISOString();
        return clone(profile);
      }
      case "credential_profile_set_enabled": {
        const profile = this.requireCredentialProfile(stringArg(args, "id"));
        const enabled = booleanArg(args, "enabled");
        if (profile.status !== "invalid" || enabled) {
          profile.status = enabled ? "active" : "disabled";
          profile.updatedAt = new Date().toISOString();
        }
        return clone(profile);
      }
      case "credential_profile_set_priority": {
        const profile = this.requireCredentialProfile(stringArg(args, "id"));
        profile.priority = numberArg(args, "priority");
        profile.updatedAt = new Date().toISOString();
        return clone(profile);
      }
      case "credential_profile_test": {
        const profile = this.requireCredentialProfile(stringArg(args, "id"));
        const now = new Date().toISOString();
        profile.status = "active";
        profile.lastValidatedAt = now;
        profile.updatedAt = now;
        return clone(profile);
      }
      case "auth_request_code":
        return { ok: true, devCode: "424242" };
      case "auth_email_challenge":
        return "audit-turnstile-token";
      case "auth_oauth_cancel":
      case "app_open_external_url":
      case "app_open_path":
      case "app_reveal_path":
      case "dot_set_view":
      case "dot_reload":
        return null;
      case "auth_verify":
        this.state.account = signedInAccount(
          stringArg(args, "email"),
          this.state.account.freeTurnsUsed,
          this.state.account.firstLaunchAt,
        );
        return clone(this.state.account);
      case "auth_oauth_providers":
        return ["google", "apple"];
      case "auth_oauth_login": {
        const provider = stringArg(args, "provider");
        if (provider !== "google" && provider !== "apple") {
          throw new Error(`[ux-audit] Unsupported account provider: ${provider}`);
        }
        this.state.account = signedInAccount(
          `audit.${provider}@example.com`,
          this.state.account.freeTurnsUsed,
          this.state.account.firstLaunchAt,
        );
        return clone(this.state.account);
      }
      case "auth_refresh_entitlements":
        if (this.state.account.signedIn) {
          this.state.account = {
            ...this.state.account,
            syncedAt: new Date().toISOString(),
            stale: false,
          };
        }
        return clone(this.state.account);
      case "auth_signout":
        this.state.account = signedOutAccount(
          this.state.account.freeTurnsUsed,
          this.state.account.firstLaunchAt,
        );
        return clone(this.state.account);
      case "auth_open_checkout":
        return null;
      case "billing_prices": {
        return clone(USD_BILLING_PRICES);
      }
      case "tray_get_snapshot":
        return { today: null, total: null, today_usd: null, total_usd: null, windows: [], checked_at: null };
      case "tray_action":
      case "tray_resize":
      case "tray_set_labels":
        return null;
      case "billing_lifetime_offer":
        return {active:true,owned:false,currency:"usd",amount:9000,serverNow:Math.floor(Date.now()/1000),startsAt:Math.floor(Date.now()/1000)-3600,endsAt:HARNESS_LIFETIME_END,capacity:100,sold:0,available:100};
      case "billing_checkout":
      case "billing_checkout_browser": {
        const plan = stringArg(args, "plan");
        const interval = stringArg(args, "interval");
        if (plan !== "basic" && plan !== "unlimited" && plan !== "lifetime") {
          throw new Error(`[ux-audit] Unsupported billing plan: ${plan}`);
        }
        if (interval !== "month" && interval !== "year") {
          throw new Error(`[ux-audit] Unsupported billing interval: ${interval}`);
        }
        const activateSubscription = () => {
          const entitlements = this.state.account.entitlements;
          if (!entitlements) return;
          this.state.account = {
            ...this.state.account,
            plan,
            entitlements: {
              ...entitlements,
              canUseApp: true,
              plan,
              message: null,
            },
            syncedAt: new Date().toISOString(),
            stale: false,
          };
        };
        if (command === "billing_checkout") {
          window.setTimeout(() => {
            activateSubscription();
            void this.emit("billing-checkout-event", { outcome: "success" });
          }, 400);
        } else {
          activateSubscription();
        }
        return null;
      }
      case "billing_portal":
        return null;

      case "route_task": {
        const brief = stringArg(args, "brief").trim();
        if (!brief) {
          throw new Error("Task brief is empty.");
        }
        const riskyWords = ["auth", "billing", "payment", "migration", "delete"];
        const lowered = brief.toLowerCase();
        let choice: RouteDecision["complexity"]["choice"] = "standard";
        let confidence = 0.75;
        let risky = 0.05;
        if (riskyWords.some((word) => lowered.includes(word))) {
          choice = "hard";
          confidence = 0.85;
          risky = 0.9;
        } else if (brief.length < 80) {
          choice = "trivial";
          confidence = 0.9;
        }
        const decision: RouteDecision = {
          schema: "task-tier-v1",
          provider: "jev",
          model: "typesafe/jev-1.13-20260917",
          complexity: {
            choice,
            confidence,
            probabilities: {
              trivial: choice === "trivial" ? confidence : 0.1,
              standard: choice === "standard" ? confidence : 0.1,
              hard: choice === "hard" ? confidence : 0.1,
            },
          },
          needsReasoning: choice === "hard" ? 0.8 : 0.2,
          risky,
          usage: { inputTokens: brief.length, outputTokens: 64 },
          latencyMs: 120,
        };
        return decision;
      }

      case "analytics_install_id":
        return "00000000-0000-4000-8000-000000000000";
      case "analytics_record":
      case "analytics_flush":
        return null;

      case "preferences_get":
        return clone(this.preferences);
      case "preferences_set_subagent_concurrency_limit":
        this.preferences.subagentConcurrencyLimit =
          typeof args.limit === "number" ? args.limit : null;
        return clone(this.preferences);
      case "preferences_set_notifications":
        this.preferences = {
          ...this.preferences,
          completionNotifications: args.completionNotifications as AppPreferences["completionNotifications"],
          permissionNotifications: Boolean(args.permissionNotifications),
          questionNotifications: Boolean(args.questionNotifications),
        };
        return clone(this.preferences);
      case "preferences_set_keyboard_shortcuts":
        this.preferences.keyboardShortcuts = clone(
          (args.shortcuts ?? {}) as Record<string, string>,
        );
        return clone(this.preferences);
      case "preferences_set_personality":
        this.preferences.personality =
          args.personality as AppPreferences["personality"];
        return clone(this.preferences);
      case "preferences_set_ponytail":
        this.preferences.ponytail = args.mode as AppPreferences["ponytail"];
        return clone(this.preferences);
      case "preferences_set_claude_subscription_proxy":
        this.preferences.claudeSubscriptionProxy = Boolean(
          args.claudeSubscriptionProxy,
        );
        return clone(this.preferences);
      case "preferences_set_memories_enabled":
        this.preferences.memoriesEnabled = Boolean(args.memoriesEnabled);
        return clone(this.preferences);
      case "preferences_set_task_routing":
        this.preferences.taskRouting = Boolean(args.taskRouting);
        return clone(this.preferences);
      case "preferences_set_worker_lane":
        this.preferences.workerLane =
          args.workerLane as AppPreferences["workerLane"];
        return clone(this.preferences);
      case "preferences_set_workflow_routing":
        this.preferences.workflowRouting = Boolean(args.workflowRouting);
        return clone(this.preferences);
      case "preferences_set_claude_ultracode":
        this.preferences.claudeUltracode = Boolean(args.claudeUltracode);
        return clone(this.preferences);
      case "preferences_set_routing_tier_agents":
        this.preferences.routingTierAgents = clone(
          args.routingTierAgents as AppPreferences["routingTierAgents"],
        );
        this.preferences.workerLane = "auto";
        return clone(this.preferences);
      case "preferences_set_analytics_enabled":
        this.preferences.analyticsEnabled = Boolean(args.analyticsEnabled);
        return clone(this.preferences);
      case "preferences_set_conversation_defaults":
        this.preferences = {
          ...this.preferences,
          defaultModel: stringArg(args, "defaultModel"),
          defaultEffort: stringArg(args, "defaultEffort"),
          defaultAgentMode: booleanArg(args, "defaultAgentMode"),
          defaultApprovalPolicy: normalizeApprovalPolicy(
            args.defaultApprovalPolicy,
          ),
          defaultFastMode: booleanArg(args, "defaultFastMode"),
        };
        return clone(this.preferences);

      case "receipt_write_png": {
        const path = stringArg(args, "path");
        if (!path.toLowerCase().endsWith(".png")) {
          throw new Error("Receipt path must use the .png extension.");
        }
        if (!Array.isArray(args.bytes)) {
          throw new Error("[ux-audit] Expected array argument: bytes");
        }
        return null;
      }

      case "remote_status":
        return clone(this.remote);
      // The fixture gateway is always active: it makes no relay connection.
      case "remote_set_active":
        return clone(this.remote);
      case "remote_set_enabled": {
        const enabled = booleanArg(args, "enabled");
        if (enabled && !this.remote.hostId) {
          this.remoteHostCount += 1;
          this.remote.hostId = `fixtureHost${String(this.remoteHostCount).padStart(11, "0")}`;
        }
        this.remote.enabled = enabled;
        this.remote.connection = enabled ? "connected" : "off";
        this.remote.error = null;
        if (!enabled) {
          this.remote.pairing = null;
          this.remote.pendingApproval = null;
          this.remote.devices = this.remote.devices.map((device) => ({
            ...device,
            connected: false,
          }));
        }
        return this.emitRemoteChanged();
      }
      case "remote_set_relay_url": {
        const relayUrl = nullableStringArg(args, "relayUrl")?.trim() || null;
        if (relayUrl !== null && !isFixtureRelayUrl(relayUrl)) {
          throw new Error(
            "Use a wss:// relay URL. ws:// works only for 127.0.0.1, ::1, and localhost.",
          );
        }
        this.remote.relayUrl = relayUrl?.replace(/\/$/, "") ?? "wss://relay.halv.ai";
        this.remote.relayUrlIsDefault = relayUrl === null;
        return this.emitRemoteChanged();
      }
      case "remote_pairing_start": {
        if (!this.remote.enabled) {
          throw new Error("Turn on Mobile before you pair a phone.");
        }
        this.remote.pairing = {
          uri: "halv://pair?p=fixture-offer",
          qrSvgDataUri: fixtureQrSvgDataUri(),
          expiresAt: Date.now() + 5 * 60_000,
        };
        const offer = this.remote.pairing;
        // A fixture phone scans the offer and asks for approval.
        globalThis.setTimeout(() => {
          if (this.remote.pairing !== offer || this.remote.pendingApproval) return;
          this.remote.pendingApproval = {
            requestId: `fixtureRequest${Date.now()}`,
            name: "Demo iPhone",
            platform: "ios",
            model: "iPhone 17",
            code: "028809",
            expiresAt: Date.now() + 120_000,
          };
          void this.emit("remote-pairing-request", clone(this.remote.pendingApproval));
          void this.emitRemoteChanged();
        }, 1_500);
        void this.emitRemoteChanged();
        return clone(this.remote.pairing);
      }
      case "remote_pairing_cancel":
        this.remote.pairing = null;
        this.remote.pendingApproval = null;
        return this.emitRemoteChanged();
      case "remote_pairing_respond": {
        const requestId = stringArg(args, "requestId");
        const allow = booleanArg(args, "allow");
        const pending = this.remote.pendingApproval;
        if (!pending || pending.requestId !== requestId) {
          throw new Error("This pairing request is no longer waiting.");
        }
        if (allow) {
          const now = Date.now();
          this.remote.devices.unshift({
            id: `fixtureDevice${String(this.remote.devices.length + 1).padStart(9, "0")}`,
            name: pending.name,
            platform: pending.platform,
            model: pending.model,
            pairedAt: now,
            lastSeenAt: now,
            connected: true,
          });
        }
        this.remote.pendingApproval = null;
        this.remote.pairing = null;
        return this.emitRemoteChanged();
      }
      case "remote_devices_list":
        return clone(this.remote.devices);
      case "remote_device_revoke": {
        const deviceId = stringArg(args, "deviceId");
        if (!this.remote.devices.some((device) => device.id === deviceId)) {
          throw new Error("This device is not paired.");
        }
        this.remote.devices = this.remote.devices.filter(
          (device) => device.id !== deviceId,
        );
        return this.emitRemoteChanged();
      }
      case "remote_identity_reset":
        this.remoteHostCount += 1;
        this.remote.hostId = `fixtureHost${String(this.remoteHostCount).padStart(11, "0")}`;
        this.remote.devices = [];
        this.remote.pairing = null;
        this.remote.pendingApproval = null;
        return this.emitRemoteChanged();

      case "diagnostics_snapshot":
        return {
          schemaVersion: 1,
          generatedAt: new Date().toISOString(),
          overallStatus: "healthy",
          app: {
            version: "0.4.4",
            operatingSystem: "macos",
            architecture: "aarch64",
          },
          activation: null,
          localDataCounts: {
            conversations: this.state.conversations.length,
            messages: Object.values(this.state.messages).reduce(
              (total, messages) => total + messages.length,
              0,
            ),
            projects: this.state.projects.length,
            workspaces: 0,
            memories: this.memories.length,
          },
          storageSchemaVersion: 1,
          health: {
            storage: { status: "healthy", summary: "Fixture storage is ready." },
            preferences: {
              status: "healthy",
              summary: "Fixture preferences are ready.",
            },
            headroom: { status: "healthy", summary: "Fixture proxy is ready." },
          },
          privacy: {
            localOnly: true,
            automaticallyUploaded: false,
            included: [],
            excluded: [],
          },
        };
      case "diagnostics_export_support_report":
        return true;

      case "list_custom_prompts":
        return [];

      case "sidecar_status":
        return clone(this.state.sidecar);
      case "sidecar_stats":
        return clone(this.state.sidecarStats);
      case "sidecar_restart":
        this.finishHeadroomInstall();
        return clone(this.state.sidecar);
      case "sidecar_install_headroom":
        return this.beginHeadroomInstall();
      case "sidecar_update_headroom":
        return this.beginHeadroomInstall();

      case "storage_total_savings":
        return clone(
          routingFixtureOn()
            ? {
                ...withRouting(this.state.savingsTotals, ROUTING_LIFETIME),
                routingCalibrationVersion: "2026-10-15.1",
              }
            : this.state.savingsTotals,
        );
      case "storage_savings_today":
        return clone(this.state.savingsToday);
      case "storage_savings_week":
        return clone(withRouting(this.state.savingsWeek, ROUTING_WEEK));
      case "storage_routing_savings":
        return clone(
          routingFixtureOn()
            ? ROUTING_REPORT
            : { totals: NO_ROUTING, chains: [], routeCosts: [] },
        );
      case "storage_daily_token_activity":
        return clone(DAILY_TOKEN_ACTIVITY);
      case "storage_list_conversations":
        return clone(sortConversations(this.state.conversations));
      case "storage_list_credential_profiles":
        return clone(this.credentialProfiles);
      case "storage_list_task_routes": {
        const conversationId = stringArg(args, "conversationId");
        // `?taskRoutes=workflow` adds a routed Workflow batch (ADR 0014).
        const routes =
          new URLSearchParams(window.location.search).get("taskRoutes") ===
          "workflow"
            ? [...TASK_ROUTES, ...WORKFLOW_TASK_ROUTES]
            : TASK_ROUTES;
        // The hero shows routes that went well.
        const shown = this.state.fixture.marketingWorkspace
          ? routes.filter((route) => route.status === "passed" || route.status === "routed")
          : routes;
        return shown.map((route) => clone({ ...route, conversationId }));
      }
      case "storage_list_usage_windows":
      case "storage_list_profile_cooldowns":
        return [];
      case "storage_get_messages": {
        const conversationId = stringArg(args, "conversationId");
        if (conversationId === HISTORICAL_AGENT_CONVERSATION_ID) {
          this.scheduleHistoricalAgentHydration();
        }
        return clone(this.messagesWithSequence(conversationId));
      }
      case "fork_conversation":
        return this.forkConversation(
          stringArg(args, "conversationId"),
          numberArg(args, "uptoSequence"),
        );
      case "storage_get_latest_conversation_compaction":
        return clone(
          this.compactions.get(stringArg(args, "conversationId")) ?? null,
        );
      case "storage_get_conversation_task_plan":
        return null;
      case "storage_delete_conversation":
        this.deleteConversation(stringArg(args, "conversationId"));
        return null;
      case "storage_set_conversation_title": {
        const conversation = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        );
        if (!conversation) throw new Error("Conversation was not found");
        conversation.title = stringArg(args, "title").trim();
        conversation.titleSource = "manual";
        return clone(conversation);
      }
      case "storage_set_conversation_pinned": {
        const conversation = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        );
        if (!conversation) throw new Error("Conversation was not found");
        conversation.pinned = Boolean(args.pinned);
        return clone(conversation);
      }
      case "storage_create_conversation_draft": {
        const draft = stringArg(args, "draft");
        const conversation = createConversation(this.state, {
          id: stringArg(args, "conversationId"),
          title: draft.trim().replace(/\s+/g, " ").slice(0, 80),
          mode: args.mode === "agent" ? "agent" : "chat",
          model: stringArg(args, "model"),
          projectId: nullableNumberArg(args, "projectId"),
          workspaceId: nullableStringArg(args, "workspaceId"),
          agentMode: Boolean(args.agentMode),
          approvalPolicyOverride: normalizeApprovalPolicy(args.approvalPolicy),
          workflowMode: args.workflowMode === "plan" ? "plan" : "default",
          fastMode: Boolean(args.fastMode),
          draft,
          draftOnly: true,
        });
        conversation.credentialProfileId =
          typeof args.credentialProfileId === "string"
            ? args.credentialProfileId
            : null;
        conversation.agentBackend =
          typeof args.agentBackend === "string"
            ? (args.agentBackend as ConversationSummary["agentBackend"])
            : null;
        conversation.agentSessionModel =
          typeof args.agentSessionModel === "string"
            ? args.agentSessionModel
            : null;
        return clone(conversation);
      }
      case "storage_set_conversation_draft": {
        const conversationId = stringArg(args, "conversationId");
        const conversation = this.state.conversations.find(
          (item) => item.id === conversationId,
        );
        if (!conversation) throw new Error("Conversation was not found");
        const draft = stringArg(args, "draft");
        if (
          !draft.trim() &&
          conversation.draftOnly &&
          conversation.messageCount === 0 &&
          conversation.goal == null
        ) {
          this.deleteConversation(conversationId);
          return null;
        }
        conversation.draft = draft.trim() ? draft : "";
        conversation.updatedAt = new Date().toISOString();
        if (conversation.draftOnly && draft.trim()) {
          conversation.title = draft.trim().replace(/\s+/g, " ").slice(0, 80);
        }
        return clone(conversation);
      }
      case "storage_consume_conversation_draft": {
        const conversation = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        );
        if (!conversation) throw new Error("Conversation was not found");
        conversation.draft = "";
        conversation.draftOnly = false;
        conversation.updatedAt = new Date().toISOString();
        return clone(conversation);
      }
      case "storage_set_conversation_archived": {
        const conversation = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        );
        if (!conversation) throw new Error("Conversation was not found");
        conversation.archivedAt = args.archived
          ? Math.floor(Date.now() / 1000)
          : null;
        return clone(conversation);
      }
      case "storage_set_conversation_memories_mode": {
        const conversation = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        );
        if (!conversation) throw new Error("Conversation was not found");
        conversation.memoriesMode =
          args.mode === "on" || args.mode === "off" ? args.mode : null;
        return clone(conversation);
      }
      case "storage_set_conversation_fast_mode": {
        const conversation = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        );
        if (!conversation) throw new Error("Conversation was not found");
        conversation.fastMode = Boolean(args.enabled);
        return clone(conversation);
      }
      case "storage_set_conversation_mode": {
        const conversation = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        );
        if (!conversation) throw new Error("Conversation was not found");
        conversation.mode = args.mode === "agent" ? "agent" : "chat";
        return clone(conversation);
      }
      case "storage_set_conversation_agent_backend": {
        const conversation = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        );
        if (!conversation) throw new Error("Conversation was not found");
        conversation.agentBackend =
          typeof args.agentBackend === "string"
            ? (args.agentBackend as ConversationSummary["agentBackend"])
            : null;
        conversation.agentSessionModel =
          typeof args.agentSessionModel === "string"
            ? args.agentSessionModel
            : null;
        return clone(conversation);
      }
      case "storage_set_conversation_credential_profile": {
        const conversation = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        );
        if (!conversation) throw new Error("Conversation was not found");
        conversation.credentialProfileId =
          typeof args.credentialProfileId === "string"
            ? args.credentialProfileId
            : null;
        return clone(conversation);
      }
      case "storage_rebuild_agent_memories":
        return 0;
      case "storage_list_agent_memories":
        return clone(this.memories);
      case "storage_delete_agent_memory": {
        const id = numberArg(args, "id");
        const before = this.memories.length;
        this.memories = this.memories.filter((memory) => memory.id !== id);
        return this.memories.length !== before;
      }
      case "storage_clear_agent_memories": {
        const cleared = this.memories.length;
        this.memories = [];
        return cleared;
      }
      case "storage_truncate_from":
        return this.truncateConversation(
          stringArg(args, "conversationId"),
          stringArg(args, "messageId"),
        );
      case "storage_set_conversation_goal":
        return this.setConversationGoal(args);
      case "storage_set_conversation_approval_policy":
        return this.setConversationApprovalPolicy(args);
      case "search_messages":
        return this.searchMessages(stringArg(args, "query"));
      case "export_conversation_to_file":
        return true;

      case "chat_list_models":
        return clone(CHAT_MODELS);
      case "chat_agent_capabilities":
        return clone(AGENT_CAPABILITIES);
      case "chat_list_turn_diffs":
        return clone(
          this.state.turnDiffs[stringArg(args, "conversationId")] ?? [],
        );
      case "chat_list_subagents":
        return this.state.fixture.marketingSubagents &&
          stringArg(args, "conversationId") ===
            MARKETING_SUBAGENT_CONVERSATION_ID
          ? clone(MARKETING_SUBAGENT_THREADS)
          : [];
      case "compact_conversation":
        return this.compactConversation(args);
      case "chat_send":
        return this.startChatStream(args, false);
      case "chat_steer":
        return null;
      case "chat_run_shell":
        return this.startShellStream(args);
      case "chat_regenerate":
        return this.startChatStream(args, true);
      case "chat_cancel":
        this.cancelStream(stringArg(args, "conversationId"), false);
        return null;
      case "chat_cancel_and_revert": {
        const revertedFiles = this.cancelStream(
          stringArg(args, "conversationId"),
          true,
        );
        return { revertedFiles };
      }
      case "chat_set_agent_mode":
        return this.setAgentMode(args);
      case "chat_set_workflow_mode":
        return this.setWorkflowMode(args);
      case "chat_interrupt_subagent":
      case "chat_send_subagent_message":
        return null;
      case "chat_interrupt_all_subagents":
        return 0;
      case "chat_tool_decision":
        return null;
      case "chat_approve_last_denied":
        return false;
      case "chat_revert_turn":
        this.revertTurn(
          stringArg(args, "conversationId"),
          stringArg(args, "turnId"),
        );
        return null;

      case "project_list":
        return clone(
          [...this.state.projects].sort(
            (left, right) =>
              Date.parse(right.lastOpenedAt ?? right.createdAt) -
              Date.parse(left.lastOpenedAt ?? left.createdAt),
          ),
        );
      case "project_open":
        return this.openProject(numberArg(args, "id"));
      case "project_add_local":
        return this.addLocalProject(stringArg(args, "path"));
      case "project_clone_github":
        return this.cloneGithubProject(args);
      case "project_remove":
        this.removeProject(numberArg(args, "id"));
        return null;
      case "project_search_files":
        return this.searchProjectFiles(stringArg(args, "query"));
      case "project_git_status":
        return this.projectGitStatus(numberArg(args, "id"));
      case "project_git_root_for_path": {
        const path = stringArg(args, "path");
        return [...this.state.projects]
          .sort((left, right) => right.rootPath.length - left.rootPath.length)
          .find((project) => path.startsWith(project.rootPath))?.rootPath ?? null;
      }
      case "project_adoption_root": {
        // Terminal adoption (ADR 0020). In the fixture, every folder of
        // /Users/auditor/Projects is a git repository. The home folder and
        // other folders are not.
        const path = stringArg(args, "path").replace(/\/+$/, "");
        const known = [...this.state.projects]
          .sort((left, right) => right.rootPath.length - left.rootPath.length)
          .find((project) => path === project.rootPath || path.startsWith(`${project.rootPath}/`));
        if (known) return known.rootPath;
        return /^\/Users\/auditor\/Projects\/[^/]+/.exec(path)?.[0] ?? null;
      }
      case "project_git_repos_under_path": {
        const path = stringArg(args, "path");
        return this.state.projects
          .map((project) => project.rootPath)
          .filter((rootPath) => rootPath.startsWith(path) || path.startsWith(rootPath));
      }
      case "project_scan_nested_repos": {
        const path = stringArg(args, "path").replace(/\/+$/, "");
        const repos =
          NESTED_REPO_FIXTURES.get(path) ??
          (this.state.fixture.marketingWorkspace
            ? marketingWorkspaceNestedRepos(path)
            : undefined);
        return { path, gitRepo: repos === undefined, repos: repos ?? [], truncated: false };
      }
      case "project_set_trust":
        return this.setProjectTrust(args);
      case "project_set_approval_policy":
        return this.setProjectApprovalPolicy(args);
      case "project_create_draft_pr":
        return "Draft pull request created: https://github.com/example/halv/pull/42";
      case "project_commit":
        return `[main abc1234] ${stringArg(args, "message")}`;
      case "project_push":
        return "Branch 'main' set up to track 'origin/main'.";
      case "project_review_snapshot":
      case "project_review_snapshot_for_cwd":
      case "project_review_file_diff":
      case "project_review_apply_action": {
        const requestedCwd =
          typeof args.cwd === "string" ? args.cwd : null;
        const project = [...this.state.projects]
          .sort((left, right) => right.rootPath.length - left.rootPath.length)
          .find((item) =>
            typeof args.projectId === "number"
              ? item.id === args.projectId
              : requestedCwd?.startsWith(item.rootPath),
          );
        const path = typeof args.path === "string" ? args.path : null;
        const fixtureDiff = [
          "diff --git a/src/release.ts b/src/release.ts",
          "--- a/src/release.ts",
          "+++ b/src/release.ts",
          "@@ -8,5 +8,6 @@ export async function verifyRelease() {",
          "   const manifest = await readManifest();",
          "-  const channel = manifest.channel;",
          "+  const channel = manifest.releaseChannel;",
          "+  await verifyChecksums(manifest);",
          "   return channel;",
          " }",
          "diff --git a/src-tauri/src/release.rs b/src-tauri/src/release.rs",
          "--- a/src-tauri/src/release.rs",
          "+++ b/src-tauri/src/release.rs",
          "@@ -31,4 +31,4 @@ pub fn release_ready(manifest: &Manifest) -> bool {",
          "-    manifest.signed",
          "+    manifest.signed && manifest.checksums_verified",
          " }",
          "",
        ].join("\n");
        const diff = path
          ? fixtureDiff.split("diff --git a/src-tauri", 1)[0] ?? fixtureDiff
          : fixtureDiff;
        return {
          id: "audit-review",
          projectId: project?.id ?? 1,
          head: "main",
          generatedAt: new Date().toISOString(),
          cwd:
            requestedCwd ??
            project?.rootPath ??
            "/Users/auditor/Projects/canned-project",
          diff,
          truncated: false,
        };
      }
      case "project_review_open_file":
        return null;

      case "workspace_list": {
        const projectId = numberArg(args, "projectId");
        const project = this.state.projects.find((item) => item.id === projectId);
        return project
          ? [{
              id: `primary-${project.id}`,
              projectId: project.id,
              name: project.name,
              kind: "primary",
              rootPath: project.rootPath,
              managed: false,
              pinned: true,
              state: "available",
              createdAt: project.createdAt,
              lastUsedAt: project.lastOpenedAt ?? project.createdAt,
              dirty: false,
              conversationCount: 0,
              currentBranch: this.isFolderProject(project.rootPath) ? null : "main",
              available: true,
            }, ...clone(this.state.worktrees.filter((item) => item.projectId === project.id)).map(
              // Like storage: every conversation of the worktree counts.
              (workspace) => ({
                ...workspace,
                conversationCount: this.state.conversations.filter(
                  (item) => item.workspaceId === workspace.id,
                ).length,
              }),
            )]
          : [];
      }
      case "workspace_tabs_snapshot":
        return this.workspaceTabs.snapshot();
      case "workspace_tabs_apply":
        return this.workspaceTabs.apply(
          (args as { ops: WorkspaceTabsOp[] }).ops,
        );
      case "workspace_tabs_seed": {
        const seed = args as {
          rows: WorkspaceTabRow[];
          active: Record<string, WorkspaceTabKey | null>;
        };
        return this.workspaceTabs.seed(seed.rows, seed.active);
      }
      case "workspace_overview":
        // The tree has no dirty flag and no conversation count.
        return this.state.projects.flatMap((project) =>
          (this.routeCommand("workspace_list", { projectId: project.id }) as Array<
            Record<string, unknown>
          >).map(({ dirty: _dirty, conversationCount: _count, ...workspace }) => ({
            ...workspace,
            isGitRepo: !this.isFolderProject(project.rootPath),
          })),
        );
      case "workspace_list_base_refs":
        return ["HEAD", "main", "origin/main"];
      case "workspace_current": {
        // Only a fixture worktree answers; the primary keeps today's null.
        const workspaceId = this.state.conversations.find(
          (item) => item.id === stringArg(args, "conversationId"),
        )?.workspaceId;
        return clone(this.state.worktrees.find((item) => item.id === workspaceId) ?? null);
      }
      case "workspace_create_worktree": {
        const projectId = numberArg(args, "projectId");
        const project = this.state.projects.find((item) => item.id === projectId);
        if (this.state.fixture.worktrees && project) {
          // The tree specs: a real fixture worktree, after the gate. A spec
          // can make the call fail (__harnessWorktreeCreateError).
          const branch = nullableStringArg(args, "branch");
          const suffix = crypto.randomUUID().slice(0, 8);
          return harnessWorktreeGate().then(() => {
            const failure = (window as HarnessWorktreeWindow).__harnessWorktreeCreateError;
            if (failure) throw new Error(failure);
            const now = new Date().toISOString();
            const workspace: WorkspaceStatus = {
              id: `audit-wt-${suffix}`,
              projectId,
              name: branch ?? `Worktree ${suffix}`,
              kind: "worktree",
              rootPath: `/Users/auditor/Library/Application Support/Halv/worktrees/${suffix}/halv`,
              gitCommonDir: `${project.rootPath}/.git`,
              baseRef: stringArg(args, "baseRef"),
              baseCommit: "4a487d0",
              branch,
              managed: true,
              pinned: false,
              state: "available",
              createdAt: now,
              lastUsedAt: now,
              dirty: false,
              conversationCount: 0,
              currentBranch: branch,
              available: true,
              isGitRepo: true,
            };
            this.state.worktrees.push(workspace);
            return clone(workspace);
          });
        }
        return {
          id: "audit-worktree",
          projectId,
          name: "halv/audit-worktree",
          kind: "worktree",
          rootPath: `${project?.rootPath ?? "/tmp/project"}/.halv-worktree`,
          managed: true,
          pinned: false,
          state: "available",
          createdAt: new Date().toISOString(),
          lastUsedAt: new Date().toISOString(),
          dirty: false,
          conversationCount: 0,
          currentBranch: "halv/audit-worktree",
          localEnvironmentConfigPath: nullableStringArg(
            args,
            "environmentConfigPath",
          ),
          available: true,
        };
      }
      case "workspace_create_branch": {
        const project = this.state.projects[0];
        const branch = stringArg(args, "branch");
        return {
          id: stringArg(args, "workspaceId"),
          projectId: project?.id ?? 1,
          name: branch,
          kind: "worktree",
          rootPath: `${project?.rootPath ?? "/tmp/project"}/.halv-worktree`,
          branch,
          managed: true,
          pinned: false,
          state: "available",
          createdAt: new Date().toISOString(),
          lastUsedAt: new Date().toISOString(),
          dirty: false,
          conversationCount: 1,
          currentBranch: branch,
          available: true,
        };
      }
      case "workspace_handoff": {
        const project = this.state.projects[0];
        const local = stringArg(args, "target") === "local";
        if (this.state.fixture.worktrees) {
          // Like storage: the conversation moves to the target workspace.
          const conversation = this.state.conversations.find(
            (item) => item.id === stringArg(args, "conversationId"),
          );
          if (conversation && local) {
            conversation.workspaceId = `primary-${project?.id ?? 1}`;
            conversation.projectRootPath = project?.rootPath ?? null;
          }
        }
        return {
          workspace: {
            id: local ? `primary-${project?.id ?? 1}` : "audit-worktree",
            projectId: project?.id ?? 1,
            name: local ? project?.name ?? "Project" : "Worktree audit",
            kind: local ? "primary" : "worktree",
            rootPath: local
              ? project?.rootPath ?? "/tmp/project"
              : `${project?.rootPath ?? "/tmp/project"}/.halv-worktree`,
            managed: !local,
            pinned: local,
            state: "available",
            createdAt: new Date().toISOString(),
            lastUsedAt: new Date().toISOString(),
            dirty: false,
            conversationCount: 1,
            currentBranch: local ? "main" : null,
            available: true,
          },
          warning: null,
        };
      }
      case "workspace_remove": {
        if (this.state.fixture.worktrees) {
          // The refusals of Rust `workspace_remove`, then the removal. A
          // spec can hold the call (__harnessHoldWorktreeOps) or make it
          // fail (__harnessWorkspaceRemoveError).
          const workspaceId = stringArg(args, "workspaceId");
          const workspace = this.state.worktrees.find((item) => item.id === workspaceId);
          if (!workspace) throw new Error("the primary workspace cannot be removed");
          if (workspace.pinned) throw new Error("unpin the workspace before removing it");
          if (this.state.conversations.some((item) => item.workspaceId === workspaceId)) {
            throw new Error("workspace is still attached to a conversation");
          }
          if (workspace.dirty) {
            throw new Error("workspace has uncommitted changes and will not be removed");
          }
          return harnessWorktreeGate().then(() => {
            const failure = (window as HarnessWorktreeWindow).__harnessWorkspaceRemoveError;
            if (failure) throw new Error(failure);
            this.state.worktrees = this.state.worktrees.filter((item) => item.id !== workspaceId);
            return null;
          });
        }
        return null;
      }
      case "workspace_open_in": {
        // Tests read the calls. Nothing opens.
        const global = window as typeof window & {
          __harnessWorkspaceOpenIn?: Array<Record<string, unknown>>;
        };
        (global.__harnessWorkspaceOpenIn ??= []).push({ ...args });
        return null;
      }
      case "workspace_set_pinned": {
        const workspace = this.state.worktrees.find(
          (item) => item.id === stringArg(args, "workspaceId"),
        );
        if (workspace) workspace.pinned = booleanArg(args, "pinned");
        return null;
      }
      case "workspace_bind_conversation":
        return null;

      case "local_environment_list":
        // ?environments=1 gives the project one environment with actions,
        // for the Run button of the strip.
        return this.fixtureEnvironments(numberArg(args, "projectId"));
      case "local_environment_save": {
        const project = this.state.projects.find(
          (item) => item.id === numberArg(args, "projectId"),
        );
        const filename = nullableStringArg(args, "filename") ?? "environment.toml";
        const configPath =
          nullableStringArg(args, "configPath") ??
          `${project?.rootPath ?? "/tmp/project"}/.codex/environments/${filename}`;
        return {
          configPath,
          workspaceRoot: project?.rootPath ?? "/tmp/project",
          inherited: false,
          config: args.config,
          error: null,
          contentSha256: null,
          approved: false,
        };
      }
      case "local_environment_delete":
        return null;
      case "local_environment_approve": {
        const environment = this.fixtureEnvironments(numberArg(args, "projectId"))[0];
        if (
          !environment ||
          stringArg(args, "configPath") !== environment.configPath ||
          stringArg(args, "contentSha256") !== environment.contentSha256
        ) {
          throw new Error("local environment changed after review");
        }
        this.fixtureEnvironmentApproved = true;
        return null;
      }
      case "local_environment_resolve_action": {
        // The fixture keeps the Rust rule: an action command comes only from
        // approved content with the hash that the UI showed.
        const environment = this.fixtureEnvironments(numberArg(args, "projectId"))[0];
        if (
          !environment ||
          stringArg(args, "configPath") !== environment.configPath ||
          stringArg(args, "contentSha256") !== environment.contentSha256
        ) {
          throw new Error("[ux-audit] Local environment actions do not run in fixtures");
        }
        if (!this.fixtureEnvironmentApproved) {
          throw new Error("local environment needs approval before its scripts run");
        }
        const action = environment.config.actions[numberArg(args, "actionIndex")];
        if (!action) throw new Error("local environment action not found");
        return action;
      }

      case "extensions_snapshot":
        return {
          skills: [],
          plugins: [],
          availablePlugins: [],
          mcpServers: [],
          mcpAuth: [],
          cruxActivation: "auto",
          cruxEnabled: true,
          cruxCensus: null,
          cruxIndexStatus: {
            state: "indexed",
            documentCount: 1_284,
          },
        };
      case "skill_read":
        return "# Audit skill\n\nFixture instructions.";
      case "plugins_install_local":
        return {
          id: "audit-plugin",
          name: "Audit plugin",
          version: "1.0.0",
          root: "/tmp/audit-plugin",
          enabled: true,
          installedAt: new Date().toISOString(),
          contributesSkills: true,
          contributesMcp: false,
          unsupportedComponents: [],
        };
      case "skills_set_enabled":
      case "plugins_set_enabled":
      case "plugins_uninstall":
      case "mcp_server_remove":
      case "mcp_server_set_enabled":
      case "mcp_server_set_bearer_token":
      case "mcp_server_clear_bearer_token":
      case "mcp_server_oauth_login":
      case "mcp_server_oauth_logout":
      case "crux_workspace_open":
        return { approvalRequired: false };
      case "crux_approve_indexing":
      case "crux_reindex_now":
      case "crux_set_activation":
        return null;
      case "mcp_server_upsert":
        return { ...(args.server as object), id: "audit-mcp" };
      case "mcp_server_test":
        return {
          serverId: "audit-mcp",
          serverName: "Audit MCP",
          protocolVersion: "2025-11-25",
          tools: [],
        };
      case "automation_list":
        return [];
      case "automation_upsert":
        return {
          ...(args.draft as object),
          id: "audit-automation",
          runs: [],
          nextRunAt: new Date(Date.now() + 60_000).toISOString(),
          createdAt: new Date().toISOString(),
          updatedAt: new Date().toISOString(),
        };
      case "automation_set_enabled":
      case "automation_run_now":
      case "automation_delete":
      case "automation_report_run":
      case "automation_mark_run_read":
      case "automation_mark_all_read":
        return null;
      case "automation_unread_count":
        return 0;

      case "browser_ui_state":
        return auditBrowserState();
      case "browser_ui_navigate":
        return auditBrowserState(stringArg(args, "url"));
      case "browser_ui_select_tab":
      case "browser_ui_close_tab":
      case "browser_ui_history":
      case "browser_ui_click":
      case "browser_ui_type_focused":
      case "browser_ui_preview_style":
        return auditBrowserState();
      case "browser_ui_annotation_target":
        return {
          annotationId: "audit-annotation",
          url: "http://localhost:3000",
          selector: "#audit-button",
          tag: "button",
          text: "Audit button",
          rect: { x: 120, y: 90, width: 160, height: 44 },
        };
      case "browser_settings_get":
        return {
          siteAccess: "always_ask",
          siteRules: [],
          fullCdpAccess: false,
        };
      case "browser_settings_set":
        return args.settings;
      case "browser_clear_data":
        return null;

      case "composer_read_dropped_image": {
        const path = stringArg(args, "path");
        return {
          name: path.split("/").pop() || "fixture.png",
          mediaType: "image/png",
          sizeBytes: 68,
          data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB",
        };
      }

      case "agent_session_probe":
        // ?ghostty=off forces Pro mode onto the embedded terminal so the
        // terminal-first flows are testable in a browser.
        return clone(
          this.state.fixture.history === "marketing-pro" ||
            new URLSearchParams(window.location.search).get("ghostty") === "off"
            ? {
                ...AGENT_SESSION_PROBE,
                ghostty: {
                  found: false,
                  version: null,
                  applescriptReady: false,
                },
              }
            : AGENT_SESSION_PROBE,
        );
      case "agent_claude_usage_windows":
        return {
          // The limit wallet wins over the marketing windows, so the
          // rotation state can pair it with busy agent panes.
          windows: this.state.fixture.wallet === "orca-limit"
            ? orcaClaudeWindowsFor(
                orcaLimitWalletUsage(Date.now()),
                this.wallet.claude.activeId,
              )
            : isMarketingRealismFixture(this.state.fixture)
              ? clone(MARKETING_CLAUDE_USAGE_WINDOWS)
              : this.wallet.store === "orca"
                ? orcaClaudeUsageWindows(Date.now())
                : [],
          unavailableReason: null,
        };
      case "agent_codex_usage_windows":
        return {
          planType: null,
          windows: isMarketingRealismFixture(this.state.fixture)
            ? clone(MARKETING_CODEX_USAGE_WINDOWS)
            : [],
        };
      case "agent_cli_install": {
        const agentKind = stringArg(args, "agentKind");
        return {
          agentKind,
          version: "fixture-1.0.0",
          binary: `/Users/auditor/.local/bin/${agentKind}`,
        };
      }
      case "agent_session_lane_options":
        return clone(
          AGENT_SESSION_LANE_OPTIONS.map((agent) => ({
            ...agent,
            lanes: agent.lanes.map((lane) => {
              const provider = agentLaneProfileProvider(lane.kind);
              return {
                ...lane,
                loggedIn:
                  lane.loggedIn === null
                    ? null
                    : this.agentLogins.has(agent.agentKind),
                credentialProfiles: provider
                  ? this.credentialProfiles.filter(
                      (profile) =>
                        profile.provider === provider &&
                        profile.status === "active",
                    )
                  : [],
              };
            }),
          })),
        );
      case "agent_session_saved_totals":
        // The hero meter shows the live savings of its pane only.
        if (this.state.fixture.marketingWorkspace) return [];
        if (this.state.fixture.marketingTerminalScene === "pro-split") {
          return [
            {
              sessionId: MARKETING_SIX_AGENT_SESSIONS[0].id,
              conversationId: MARKETING_TERMINAL_CONVERSATION_ID,
              savingsTokens: 31_204,
              cacheReadTokens: 18_240,
              dollarsUsd: 0.07,
            },
          ];
        }
        if (this.state.fixture.marketingTerminalScene === "crux") {
          return [
            {
              sessionId: MARKETING_CRUX_AGENT_SESSIONS[0].id,
              conversationId: MARKETING_TERMINAL_CONVERSATION_ID,
              savingsTokens: 44_876,
              cacheReadTokens: 18_240,
              dollarsUsd: 0.11,
            },
          ];
        }
        if (this.state.fixture.marketingProSplit) {
          return [
            {
              sessionId: MARKETING_PRO_SPLIT_AGENT_SESSIONS[0].id,
              conversationId: MARKETING_PRO_SPLIT_CONVERSATION_ID,
              savingsTokens: 44_876,
              cacheReadTokens: 18_240,
              dollarsUsd: 0.11,
            },
          ];
        }
        return [];
      case "agent_session_list":
        if (
          this.state.fixture.marketingWorkspace &&
          !this.marketingTerminalSceneEventsDelivered
        ) {
          this.marketingTerminalSceneEventsDelivered = true;
          return this.deliverAgentEvents(MARKETING_WORKSPACE_AGENT_EVENTS);
        }
        if (
          this.state.fixture.marketingTerminalScene &&
          !this.marketingTerminalSceneEventsDelivered
        ) {
          this.marketingTerminalSceneEventsDelivered = true;
          return this.deliverMarketingTerminalSceneAgentSessions();
        }
        if (
          this.state.fixture.marketingProSplit &&
          !this.marketingProAgentEventsDelivered
        ) {
          this.marketingProAgentEventsDelivered = true;
          return this.deliverMarketingProSplitAgentSessions();
        }
        if (
          this.state.fixture.history === "marketing-pro" &&
          !this.state.fixture.marketingTerminalScene &&
          !this.marketingProAgentEventsDelivered
        ) {
          this.marketingProAgentEventsDelivered = true;
          return this.deliverMarketingProAgentSessions();
        }
        return clone(this.agentSessions);
      case "agent_session_checkpoints":
        return [];
      case "agent_session_checkpoint_preview":
        return {
          checkpointId: stringArg(args, "checkpointId"),
          modified: [],
          created: [],
          deleted: [],
        };
      case "agent_session_checkpoint_restore":
        return {
          checkpointId: stringArg(args, "checkpointId"),
          safetyCheckpointId: "harness-safety-checkpoint",
          modified: [],
          created: [],
          deleted: [],
        };
      case "agent_login_launch": {
        const agentKind = stringArg(args, "agentKind");
        if (this.state.fixture.chatGptLogin === "pending") {
          return new Promise<never>(() => undefined);
        }
        if (this.state.fixture.chatGptLogin === "failure") {
          throw new Error(`${agentKind} connection was not completed`);
        }
        this.agentLogins.add(agentKind as AgentKind);
        const id = `audit-${agentKind}-login`;
        this.terminalSessions = this.terminalSessions.filter(
          (session) => session.id !== id,
        );
        this.terminalSessions.push(
          auditTerminalSession(id, `/tmp/${agentKind}-login`),
        );
        return id;
      }
      case "wallet_list":
        return clone(this.wallet);
      case "wallet_select": {
        const provider = stringArg(args, "provider") as WalletProvider;
        const nextId = nullableStringArg(args, "accountId");
        if (
          nextId !== null &&
          !this.wallet[provider].accounts.some((account) => account.id === nextId)
        ) {
          throw new Error("That account is no longer in the wallet.");
        }
        const commit = () => {
          this.wallet[provider].activeId = nextId;
          const state = clone(this.wallet);
          void this.emit("wallet-changed", state);
          return { state, staleSessions: this.liveStaleSessions() };
        };
        if (this.wallet.store !== "orca") return commit();
        // Orca mode can start Orca first, so a switch takes a moment.
        return new Promise((resolve) => {
          window.setTimeout(() => resolve(commit()), 400);
        });
      }
      case "wallet_add_launch": {
        const provider = stringArg(args, "provider") as WalletProvider;
        this.walletPendingCount += 1;
        const pendingId = `pending${this.walletPendingCount}`;
        const id = `audit-${provider}-wallet-login`;
        this.terminalSessions = this.terminalSessions.filter(
          (session) => session.id !== id,
        );
        this.terminalSessions.push(
          auditTerminalSession(id, `/tmp/${provider}-wallet-login`),
        );
        return { pendingId, terminalId: id };
      }
      case "wallet_add_finish": {
        const provider = stringArg(args, "provider") as WalletProvider;
        const pendingId = stringArg(args, "pendingId");
        this.wallet[provider].accounts.push({
          id: pendingId,
          email: `work${this.wallet[provider].accounts.length + 1}@example.com`,
          organizationName: null,
          plan: provider === "codex" ? "plus" : "max",
          missing: false,
          createdAt: new Date(0).toISOString(),
        });
        this.wallet[provider].activeId = pendingId;
        const state = clone(this.wallet);
        void this.emit("wallet-changed", state);
        return { state, added: true };
      }
      case "wallet_add_cancel":
        return null;
      case "wallet_remove": {
        const provider = stringArg(args, "provider") as WalletProvider;
        const accountId = stringArg(args, "accountId");
        const providerState = this.wallet[provider];
        providerState.accounts = providerState.accounts.filter(
          (account) => account.id !== accountId,
        );
        if (providerState.activeId === accountId) providerState.activeId = null;
        const state = clone(this.wallet);
        void this.emit("wallet-changed", state);
        return state;
      }
      case "wallet_stale_sessions":
        return this.liveStaleSessions();
      case "wallet_keep_sessions": {
        const kept = new Set(args.sessionIds as string[]);
        this.walletStale = this.walletStale.filter(
          (session) => !kept.has(session.sessionId),
        );
        return null;
      }
      case "wallet_usage": {
        if (this.wallet.store !== "orca") return [];
        const wallet = this.wallet;
        const usage =
          this.state.fixture.wallet === "orca-limit"
            ? orcaLimitWalletUsage(Date.now())
            : orcaWalletUsage(Date.now());
        return usage.filter(
          (usage) =>
            usage.accountId !== wallet[usage.provider].activeId &&
            wallet[usage.provider].accounts.some(
              (account) => account.id === usage.accountId,
            ),
        );
      }
      case "agent_terminal_duplicate": {
        const source = this.agentSessions.find(
          (candidate) => candidate.id === stringArg(args, "sessionId"),
        );
        if (!source || source.mode !== "terminal_embedded") {
          throw new Error("only built-in agent terminals can be split");
        }
        const now = new Date().toISOString();
        const session: AgentSession = {
          ...source,
          id: crypto.randomUUID(),
          // The real backend keeps the source's conversation binding and
          // discovers a fresh CLI session id for the duplicate.
          cliSessionId: crypto.randomUUID(),
          pid: 4242,
          status: "running",
          createdAt: now,
          lastActiveAt: now,
          exitCode: null,
        };
        this.agentSessions.unshift(session);
        this.terminalSessions.push({
          ...auditTerminalSession(session.id, session.cwd, session.projectId),
          agentKind: session.agentKind,
          conversationId: session.conversationId ?? undefined,
        });
        return clone(session);
      }
      case "agent_session_create":
      case "agent_terminal_create":
      case "agent_ghostty_launch": {
        const launchHistory = window as typeof window & {
          __harnessTerminalCreateRequests?: Array<string | null>;
        };
        (launchHistory.__harnessTerminalCreateRequests ??= []).push(
          typeof args.resumeCliSessionId === "string" ? args.resumeCliSessionId : null,
        );
        if (new URLSearchParams(window.location.search).has("failAgentLaunch")) {
          throw new Error("Account initialization connection failed");
        }
        const finishAgentCreate = () => {
        const lane = args.lane as { kind?: AgentSessionCreateLaneKind };
        const now = new Date().toISOString();
        const session: AgentSession = {
          id: crypto.randomUUID(),
          agentKind: stringArg(args, "agentKind") as AgentSession["agentKind"],
          laneKind: lane.kind ?? "custom",
          mode:
            command === "agent_terminal_create"
              ? "terminal_embedded"
              : command === "agent_ghostty_launch"
                ? "terminal_ghostty"
                : "gui",
          projectId:
            typeof args.projectId === "number" ? args.projectId : null,
          conversationId:
            typeof args.conversationId === "string"
              ? args.conversationId
              : undefined,
          conversationTitle:
            typeof args.conversationTitle === "string"
              ? args.conversationTitle
              : undefined,
          model: typeof args.model === "string" ? args.model : undefined,
          cliSessionId:
            typeof args.resumeCliSessionId === "string"
              ? args.resumeCliSessionId
              : crypto.randomUUID(),
          cwd: stringArg(args, "cwd"),
          pid: 4242,
          status: "running",
          createdAt: now,
          lastActiveAt: now,
        };
        if (typeof args.initialPrompt === "string") {
          // Tests assert the first message rode along with the spawn, the
          // same way __harnessTerminalWrites records typed delivery.
          const global = window as typeof window & {
            __harnessInitialPrompts?: Array<{
              sessionId: string;
              prompt: string;
            }>;
          };
          (global.__harnessInitialPrompts ??= []).push({
            sessionId: session.id,
            prompt: args.initialPrompt,
          });
        }
        this.agentSessions.unshift(session);
        if (session.mode === "terminal_embedded") {
          this.terminalSessions.push({
            ...auditTerminalSession(session.id, session.cwd, session.projectId),
            agentKind: session.agentKind,
            conversationId: session.conversationId ?? undefined,
          });
        }
        return clone(session);
        };
        // ?slow=1 makes session creation take real-world time so the
        // restore/resume interleavings behave like the real backend. A
        // number above 1 is the delay in milliseconds.
        const slow = new URLSearchParams(window.location.search).get("slow");
        if (slow) {
          const delay = Number(slow) > 1 ? Number(slow) : 700;
          return new Promise((resolve) =>
            setTimeout(() => resolve(finishAgentCreate()), delay),
          );
        }
        return finishAgentCreate();
      }
      case "agent_session_send":
      case "agent_session_respond_permission":
        return null;
      case "agent_session_kill": {
        const session = this.agentSessions.find(
          (candidate) => candidate.id === stringArg(args, "sessionId"),
        );
        if (!session) throw new Error("Agent session was not found");
        session.status = "exited";
        session.exitCode = 0;
        session.lastActiveAt = new Date().toISOString();
        const terminal = this.terminalSessions.find(
          (candidate) => candidate.id === session.id,
        );
        if (terminal) {
          terminal.status = "exited";
          terminal.exitCode = 0;
          terminal.exitedAt = session.lastActiveAt;
          globalThis.setTimeout(() => {
            void this.emit("terminal_event", {
              sessionId: session.id,
              sequence: 1,
              kind: "exit",
              exitCode: 0,
            });
          }, 0);
        }
        return clone(session);
      }
      case "agent_session_forget": {
        const sessionId = stringArg(args, "sessionId");
        const index = this.agentSessions.findIndex(
          (candidate) => candidate.id === sessionId,
        );
        if (index < 0) throw new Error("Agent session was not found");
        if (this.agentSessions[index]!.status === "running") {
          throw new Error(`Agent session \`${sessionId}\` is running`);
        }
        this.agentSessions.splice(index, 1);
        return null;
      }
      case "agent_session_rename": {
        const session = this.agentSessions.find(
          (candidate) => candidate.id === stringArg(args, "sessionId"),
        );
        if (!session) throw new Error("Agent session was not found");
        session.customTitle = nullableStringArg(args, "title");
        session.lastActiveAt = new Date().toISOString();
        return clone(session);
      }

      case "terminal_list":
        return clone(this.terminalSessions);
      case "terminal_supervisor_status":
      case "terminal_supervisor_retry":
      case "terminal_supervisor_restart":
        return {
          available: true,
          socketPath: "/tmp/ux-audit-supervisor.sock",
          pid: 4242,
          sessions: this.terminalSessions.length,
          runningSessions: this.terminalSessions.filter(
            (session) => session.status === "running",
          ).length,
          supervisorVersion: "0.4.15",
          supervisorStale: false,
          appVersion: "0.4.15",
          message: null,
        };
      case "terminal_open": {
        const project = this.state.projects.find(
          (item) => item.id === numberArg(args, "projectId"),
        );
        const conversationId = nullableStringArg(args, "conversationId");
        const shellTabId = nullableStringArg(args, "shellTabId");
        const workspaceId = nullableStringArg(args, "workspaceId");
        // The owner checks of Rust `terminal_open` (ADR 0010).
        if (conversationId && shellTabId) {
          throw new Error("a terminal belongs to a conversation or to a shell tab, not both");
        }
        const shellTab = shellTabId
          ? this.workspaceTabs.snapshot().rows.find((row) => row.key === `shell:${shellTabId}`)
          : null;
        if (shellTabId && !shellTab) {
          throw new Error(`shell tab \`${shellTabId}\` was not found`);
        }
        if (shellTab && workspaceId && shellTab.shellWorkspaceId !== workspaceId) {
          throw new Error(`shell tab \`${shellTabId}\` belongs to another workspace`);
        }
        const root =
          this.state.worktrees.find((item) => item.id === workspaceId)?.rootPath ??
          project?.rootPath ??
          "/tmp/project";
        const session: TerminalSession = {
          ...auditTerminalSession(
            `terminal-${crypto.randomUUID().replace(/-/g, "")}`,
            root,
            project?.id ?? 1,
            project?.name,
          ),
          conversationId: conversationId ?? undefined,
          shellTabId: shellTabId ?? undefined,
        };
        this.terminalSessions.push(session);
        // Tests inspect the owner and the folder of each new shell.
        const global = window as typeof window & {
          __harnessTerminalOpenRequests?: Array<Record<string, unknown>>;
        };
        (global.__harnessTerminalOpenRequests ??= []).push({
          ...args,
          sessionId: session.id,
          cwd: root,
        });
        return clone(session);
      }
      case "terminal_attach": {
        const session = this.terminalSessions.find(
          (candidate) => candidate.id === stringArg(args, "sessionId"),
        );
        if (!session) throw new Error("Terminal session was not found");
        const marketingSplitScrollback =
          MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[session.id];
        if (marketingSplitScrollback) {
          return {
            session: clone(session),
            sequence: 0,
            scrollback: marketingSplitScrollback,
          };
        }
        const marketingSceneScrollback =
          MARKETING_SIX_TERMINAL_SCROLLBACK[session.id] ??
          MARKETING_CRUX_TERMINAL_SCROLLBACK[session.id] ??
          MARKETING_WORKSPACE_SCROLLBACK[session.id];
        if (marketingSceneScrollback) {
          return {
            session: clone(session),
            sequence: 0,
            scrollback: marketingSceneScrollback,
          };
        }
        const prelude = this.terminalPreludes.get(session.id);
        return {
          session: clone(session),
          sequence: 0,
          scrollback:
            this.terminalScrollback.get(session.id) ??
            (session.id === MARKETING_PRO_SESSION_ID
              ? MARKETING_PRO_TERMINAL_SCROLLBACK
              : "$ "),
          ...(prelude ? { prelude: clone(prelude) } : {}),
        };
      }
      case "terminal_rename": {
        const session = this.terminalSessions.find(
          (candidate) => candidate.id === stringArg(args, "sessionId"),
        );
        if (!session) throw new Error("Terminal session was not found");
        session.title =
          typeof args.title === "string" && args.title.trim()
            ? args.title
            : undefined;
        return clone(session);
      }
      case "terminal_set_shell_tab": {
        // The rules of Rust `check_shell_adoption` ("Open in tab").
        const session = this.terminalSessions.find(
          (candidate) => candidate.id === stringArg(args, "sessionId"),
        );
        if (!session) throw new Error("Terminal session was not found");
        const shellTabId = stringArg(args, "shellTabId").trim();
        if (!shellTabId) throw new Error("a shell tab id is required");
        if (session.conversationId) {
          throw new Error("a conversation terminal cannot move to a shell tab");
        }
        // A standalone agent terminal moves too (ADR 0020).
        if (session.status !== "running") {
          throw new Error("only a running shell can move to a shell tab");
        }
        if (session.shellTabId && session.shellTabId !== shellTabId) {
          throw new Error("the shell already belongs to another shell tab");
        }
        if (
          !this.workspaceTabs.snapshot().rows.some((row) => row.key === `shell:${shellTabId}`)
        ) {
          throw new Error(`shell tab \`${shellTabId}\` was not found`);
        }
        session.shellTabId = shellTabId;
        return clone(session);
      }
      case "terminal_duplicate": {
        const source = this.terminalSessions.find(
          (candidate) => candidate.id === stringArg(args, "sessionId"),
        );
        if (!source) throw new Error("Terminal session was not found");
        // Like the backend, a duplicate never continues the source's agent.
        const { continuedFrom: _continuedFrom, ...duplicate } = clone(source);
        const session = { ...duplicate, id: crypto.randomUUID() };
        this.terminalSessions.push(session);
        return clone(session);
      }
      // The restore state lives in localStorage so a page reload behaves
      // like an app relaunch: saved panes survive, live sessions do not.
      case "terminal_restore_read": {
        const raw = window.localStorage.getItem(
          `ux-audit.terminal-restore:${stringArg(args, "scope")}`,
        );
        return raw ? JSON.parse(raw) : null;
      }
      case "terminal_restore_write": {
        const scope = stringArg(args, "scope");
        const key = `ux-audit.terminal-restore:${scope}`;
        const previous = window.localStorage.getItem(key);
        const existing = previous
          ? (JSON.parse(previous) as {
              panes: Array<{ sessionId: string; scrollback: string }>;
            })
          : null;
        const now = new Date().toISOString();
        const panes = (args.panes as Array<Record<string, unknown>>).map(
          (pane) => ({
            ...pane,
            scrollback:
              existing?.panes.find(
                (candidate) => candidate.sessionId === pane.sessionId,
              )?.scrollback ?? "$ restored\r\n",
            updatedAt: now,
          }),
        );
        if (panes.length === 0) {
          window.localStorage.removeItem(key);
          return null;
        }
        window.localStorage.setItem(
          key,
          JSON.stringify({ layout: args.layout, panes, updatedAt: now }),
        );
        return null;
      }
      case "terminal_restore_shell": {
        const raw = window.localStorage.getItem(
          `ux-audit.terminal-restore:${stringArg(args, "scope")}`,
        );
        if (!raw) throw new Error("saved terminal was not found");
        const saved = JSON.parse(raw) as {
          panes: Array<{
            sessionId: string;
            cwd: string;
            projectId: number | null;
            conversationId?: string | null;
          }>;
        };
        const pane = saved.panes.find(
          (candidate) => candidate.sessionId === stringArg(args, "sessionId"),
        );
        if (!pane) throw new Error("saved terminal was not found");
        // Like Rust, the scope of a shell tab panel gives the owner back.
        const session: TerminalSession = {
          ...auditTerminalSession(
            crypto.randomUUID(),
            pane.cwd,
            pane.projectId,
            "Shell",
          ),
          conversationId: pane.conversationId ?? undefined,
          shellTabId: pane.conversationId
            ? undefined
            : (harnessShellTabIdFromScope(stringArg(args, "scope")) ?? undefined),
        };
        this.terminalSessions.push(session);
        return clone(session);
      }
      case "terminal_write": {
        // Tests inspect what reached the PTY.
        const global = window as typeof window & {
          __harnessTerminalWrites?: Array<{ sessionId: string; data: string }>;
        };
        (global.__harnessTerminalWrites ??= []).push({
          sessionId: stringArg(args, "sessionId"),
          data: stringArg(args, "data"),
        });
        return null;
      }
      case "terminal_resize":
      case "terminal_close":
        return null;
      case "terminal_open_shell":
        return clone(
          this.openShellBeside(
            stringArg(args, "sessionId"),
            nullableStringArg(args, "scope"),
          ),
        );
      case "terminal_continue_shell":
        return clone(this.continueTerminalInShell(stringArg(args, "sessionId")));
      case "app_hide_main_window": {
        // Tests count the hide requests of the Cmd+W chain.
        const global = window as typeof window & { __harnessHideMainWindowCalls?: number };
        global.__harnessHideMainWindowCalls = (global.__harnessHideMainWindowCalls ?? 0) + 1;
        return null;
      }
      case "diagnostics_resource_usage": {
        const terminals = this.terminalSessions
          .filter((terminal) => terminal.status === "running")
          .map((terminal, index) => ({
            sessionId: terminal.id,
            bytes: (96 + index * 48) * 1024 * 1024,
          }));
        const appBytes = 412 * 1024 * 1024;
        return {
          appBytes,
          terminals,
          totalBytes: terminals.reduce((sum, terminal) => sum + terminal.bytes, appBytes),
        };
      }
      case "terminal_link_targets":
        // The harness has no file system, so no candidate resolves.
        return ((args.candidates as string[] | undefined) ?? []).map((text) => ({
          text,
          path: null,
          kind: "none",
        }));
      case "terminal_foreground_busy": {
        // Tests set `__harnessShellBusy[sessionId]` to "busy" or "unknown".
        // A running shell is idle by default.
        const busy =
          (window as typeof window & {
            __harnessShellBusy?: Record<string, "busy" | "idle" | "unknown">;
          }).__harnessShellBusy ?? {};
        return ((args.sessionIds as string[] | undefined) ?? []).map((sessionId) => {
          const terminal = this.terminalSessions.find((candidate) => candidate.id === sessionId);
          return {
            sessionId,
            state: !terminal
              ? "unknown"
              : terminal.status !== "running"
                ? "idle"
                : (busy[sessionId] ?? "idle"),
          };
        });
      }
      case "terminal_forget": {
        const sessionId = stringArg(args, "sessionId");
        const index = this.terminalSessions.findIndex(
          (candidate) => candidate.id === sessionId,
        );
        if (index < 0) throw new Error("Terminal session was not found");
        this.terminalSessions.splice(index, 1);
        return null;
      }
      case "agent_status_snapshot":
        return clone(this.agentStatuses);
      case "agent_status_mark_viewed":
        this.readAgentStatus(stringArg(args, "sessionId"));
        return null;
      case "agent_status_set_focus": {
        const sessionId =
          typeof args.sessionId === "string" ? args.sessionId : null;
        // Tests inspect the pane that the app reports as seen.
        (window as typeof window & { __harnessAgentStatusFocus?: string | null })
          .__harnessAgentStatusFocus = sessionId;
        if (sessionId) this.readAgentStatus(sessionId);
        return null;
      }
      case "terminal_notify": {
        // Tests inspect the banners that Rust would show.
        const global = window as typeof window & {
          __harnessTerminalNotifications?: Array<{
            sessionId: string;
            title: string;
            body: string;
          }>;
        };
        const notification = {
          sessionId: stringArg(args, "sessionId"),
          title: stringArg(args, "title"),
          body: stringArg(args, "body"),
        };
        (global.__harnessTerminalNotifications ??= []).push(notification);
        showHarnessNotificationCard(notification);
        return null;
      }

      case "app_update_relaunch":
        return null;
      case "app_update_snapshot":
        return clone(this.appUpdate);
      case "app_update_check": {
        const release = appUpdateHarnessRelease(this.state.fixture.update);
        const checking = this.setAppUpdate({
          deferredVersion: null,
          status: { state: "checking", userInitiated: true },
        });
        window.setTimeout(() => {
          this.setAppUpdate({
            status: release ?? { state: "upToDate", userInitiated: true },
          });
        }, 400);
        return checking;
      }
      case "app_update_download": {
        const { status } = this.appUpdate;
        if (status.state !== "available") return clone(this.appUpdate);
        return this.setAppUpdate({
          status: {
            state: "downloading",
            version: status.version,
            downloadedBytes: 0,
            totalBytes: 176_883_936,
          },
        });
      }
      case "app_update_restart_blockers":
        return appUpdateHarnessBlockers(this.state.fixture.update);
      case "app_update_restart": {
        const { status } = this.appUpdate;
        if (status.state !== "ready") throw new Error("No downloaded Halv update is ready to install.");
        const blockers = appUpdateHarnessBlockers(this.state.fixture.update);
        const busy =
          blockers.chatReplies + blockers.guiAgentSessions + blockers.inProcessTerminals > 0;
        if (!booleanArg(args, "force") && busy) {
          this.setAppUpdate({ blockers });
          return blockers;
        }
        // The harness stops at "installing"; it never restarts the page.
        this.setAppUpdate({
          blockers: null,
          restartWhenIdle: false,
          status: { state: "installing", version: status.version, trigger: "restart" },
        });
        return null;
      }
      case "app_update_restart_when_idle":
        return this.setAppUpdate({
          restartWhenIdle: true,
          blockers: appUpdateHarnessBlockers(this.state.fixture.update),
        });
      case "app_update_cancel_restart":
        return this.setAppUpdate({ restartWhenIdle: false, blockers: null });
      case "app_update_restart_ack":
        return null;
      case "app_update_defer":
        return this.setAppUpdate({ deferredVersion: stringArg(args, "version") });
      case "app_update_set_auto_download":
        return this.setAppUpdate({ autoDownload: booleanArg(args, "enabled") });
      case "app_update_dismiss_post_update":
        return this.setAppUpdate({ postUpdate: null });
      case "plugin:dialog|open":
        return "/Users/auditor/Projects/canned-project";
      case "plugin:process|restart":
        return null;
      case "plugin:app|version":
        return "0.4.4";
      case "plugin:notification|is_permission_granted":
        return true;
      default:
        throw new Error(`[ux-audit] Unhandled invoke command: ${command}`);
    }
  }

  // ?environments=1 gives each project one environment file with actions.
  private fixtureEnvironments(projectId: number) {
    if (new URLSearchParams(window.location.search).get("environments") !== "1") {
      return [];
    }
    const project = this.state.projects.find((item) => item.id === projectId);
    const root = project?.rootPath ?? "/tmp/project";
    return [
      {
        configPath: `${root}/.codex/environments/environment.toml`,
        workspaceRoot: root,
        inherited: false,
        config: {
          version: 1,
          name: "Dev",
          setup: { script: "pnpm install" },
          actions: [
            { name: "Dev server", icon: "run" as const, command: "pnpm dev" },
            { name: "Unit tests", icon: "test" as const, command: "pnpm test:unit" },
          ],
        },
        error: null,
        contentSha256: "0".repeat(64),
        approved: this.fixtureEnvironmentApproved,
      },
    ];
  }

  // Rust sends every update change as an event, so the harness does too.
  private setAppUpdate(change: Partial<AppUpdateSnapshot>) {
    this.appUpdate = { ...this.appUpdate, ...change };
    void this.emit("app_update://snapshot", clone(this.appUpdate));
    return clone(this.appUpdate);
  }

  /** Marks the finished turns of a pane as read, as the Rust store does. */
  // A stopped pane no longer runs the old account.
  private liveStaleSessions() {
    return clone(
      this.walletStale.filter((stale) =>
        this.agentSessions.some(
          (session) =>
            session.id === stale.sessionId && session.status === "running",
        ),
      ),
    );
  }

  private readAgentStatus(sessionId: string) {
    const entry = this.agentStatuses.find(
      (candidate) => candidate.sessionId === sessionId,
    );
    if (!entry?.unread) return;
    entry.unread = false;
    void this.emit("agent-status", clone(entry));
  }

  private scheduleHistoricalAgentHydration() {
    const includeToolOutput = this.historicalOutputHydrationNeeded;
    this.historicalOutputHydrationNeeded = false;
    window.setTimeout(() => {
      void this.emitHistoricalAgentActivity(includeToolOutput);
    }, 80);
  }

  private async emitMarketingProAgentEvents() {
    for (const event of MARKETING_PRO_AGENT_EVENTS) {
      await this.emit("agent-session-event", {
        sessionId: MARKETING_PRO_SESSION_ID,
        event: clone(event),
      });
    }
  }

  private async deliverMarketingProAgentSessions() {
    await this.emitMarketingProAgentEvents();
    return clone(this.agentSessions);
  }

  private async deliverMarketingProSplitAgentSessions() {
    for (const [sessionId, events] of Object.entries(
      MARKETING_PRO_SPLIT_AGENT_EVENTS,
    )) {
      for (const event of events) {
        await this.emit("agent-session-event", {
          sessionId,
          event: clone(event),
        });
      }
    }
    return clone(this.agentSessions);
  }

  private async deliverAgentEvents(events: Record<string, AgentSessionEvent[]>) {
    for (const [sessionId, sessionEvents] of Object.entries(events)) {
      for (const event of sessionEvents) {
        await this.emit("agent-session-event", { sessionId, event: clone(event) });
      }
    }
    return clone(this.agentSessions);
  }

  /** A folder project that is no git repository (ADR 0019). */
  private isFolderProject(path: string) {
    return (
      isNestedRepoFolder(path) ||
      (this.state.fixture.marketingWorkspace === true && path === MARKETING_WORKSPACE_FOLDER)
    );
  }

  private async deliverMarketingTerminalSceneAgentSessions() {
    const events =
      this.state.fixture.marketingTerminalScene === "pro-split"
        ? MARKETING_SIX_AGENT_EVENTS
        : MARKETING_CRUX_AGENT_EVENTS;
    for (const [sessionId, sessionEvents] of Object.entries(events)) {
      for (const event of sessionEvents) {
        await this.emit("agent-session-event", {
          sessionId,
          event: clone(event),
        });
      }
    }
    return clone(this.agentSessions);
  }

  private async emitHistoricalAgentActivity(includeToolOutput: boolean) {
    for (const fixture of HISTORICAL_TOOLS) {
      await this.emit("chat_delta", fixture.call);
      if (includeToolOutput) {
        await this.emit("chat_delta", fixture.output);
      }
      await this.emit("chat_delta", fixture.result);
    }
    await this.emit("chat_delta", {
      conversationId: HISTORICAL_AGENT_CONVERSATION_ID,
      kind: "reasoning",
      text: HISTORICAL_REASONING,
    } satisfies ChatDeltaPayload);
  }

  private async beginHeadroomInstall(): Promise<SidecarStatus> {
    if (this.state.sidecar.state === "running") {
      return clone(this.state.sidecar);
    }
    if (this.installPromise) {
      return this.installPromise;
    }
    if (this.automaticInstallTimer !== null) {
      window.clearTimeout(this.automaticInstallTimer);
      this.automaticInstallTimer = null;
    }
    this.state.sidecar = { state: "installing", port: 8787 };
    this.installPromise = new Promise<SidecarStatus>((resolve) => {
      this.installResolve = resolve;
    });

    const progress: Array<[number, SidecarInstallProgress]> = [
      [0, { stage: "starting", line: "Setting up the proxy." }],
      [650, { stage: "detectingHeadroom", line: "Proxy found; starting it." }],
      [1_300, { stage: "success", line: "Proxy is running on port 8787." }],
    ];
    for (const [delay, payload] of progress) {
      const timer = window.setTimeout(() => {
        this.installTimers.delete(timer);
        void this.emit("sidecar_install_progress", payload);
        if (payload.stage === "success") {
          this.finishHeadroomInstall();
        }
      }, delay);
      this.installTimers.add(timer);
    }
    return this.installPromise;
  }

  private finishHeadroomInstall() {
    for (const timer of this.installTimers) {
      window.clearTimeout(timer);
    }
    this.installTimers.clear();
    this.state.sidecar = clone(RUNNING_SIDECAR);
    this.installResolve?.(clone(this.state.sidecar));
    this.installResolve = null;
    this.installPromise = null;
  }

  private startChatStream(args: InvokeArgs, regenerate: boolean) {
    if (this.state.scenario === "usage-limit") {
      return Promise.reject(USAGE_LIMIT_MESSAGE);
    }
    if (this.state.scenario === "proxy-error") {
      return Promise.reject(TRANSIENT_PROXY_MESSAGE);
    }

    const requestedConversationId = nullableStringArg(args, "conversationId");
    let conversation = requestedConversationId
      ? this.requireConversation(requestedConversationId)
      : null;
    if (!conversation) {
      const projectId = nullableNumberArg(args, "projectId");
      conversation = createConversation(this.state, {
        model: stringArg(args, "model"),
        mode: args.conversationMode === "agent" ? "agent" : "chat",
        projectId,
        workspaceId: nullableStringArg(args, "workspaceId"),
        agentMode: booleanArg(args, "agentMode"),
        approvalPolicyOverride: normalizeApprovalPolicy(args.approvalPolicy),
        fastMode:
          typeof args.fastMode === "boolean" ? args.fastMode : undefined,
        workflowMode:
          nullableStringArg(args, "workflowMode") === "plan" ? "plan" : "default",
      });
    }
    const requestedWorkflowMode = nullableStringArg(args, "workflowMode");
    if (requestedWorkflowMode === "plan" || requestedWorkflowMode === "default") {
      conversation.workflowMode = requestedWorkflowMode;
    }
    if (typeof args.fastMode === "boolean") {
      conversation.fastMode = args.fastMode;
    }
    if (typeof args.credentialProfileId === "string") {
      conversation.credentialProfileId = args.credentialProfileId;
    }
    if (this.activeStreams.has(conversation.id)) {
      return Promise.reject(
        "A chat request is already running for this conversation",
      );
    }

    conversation.model = stringArg(args, "model");
    if (conversation.id === HISTORICAL_AGENT_CONVERSATION_ID) {
      this.historicalOutputHydrationNeeded = true;
    }
    const snapshotTurnDiffs = clone(this.state.turnDiffs[conversation.id] ?? []);
    if (regenerate) {
      const rows = this.state.messages[conversation.id] ?? [];
      while (rows.length > 0 && rows[rows.length - 1]?.role === "assistant") {
        rows.pop();
      }
    } else {
      const text = stringArg(args, "text");
      (this.state.messages[conversation.id] ??= []).push({
        id: `audit-user-${Date.now()}`,
        conversationId: conversation.id,
        role: "user",
        content: text,
        createdAt: new Date().toISOString(),
        model: stringArg(args, "model"),
        inputTokens: null,
        outputTokens: null,
        error: false,
        mentions: stringArrayArg(args, "mentions"),
        attachments: imageAttachmentArrayArg(args, "attachments"),
        tokensSaved: null,
        savingsUsd: null,
      });
    }
    syncConversationSummary(this.state, conversation.id);

    const promise = new Promise<ChatSendResponse>((resolve, reject) => {
      const task: ActiveStream = {
        conversationId: conversation.id,
        timers: new Set(),
        assistantText: "",
        cancelled: false,
        snapshotTurnDiffs,
        resolve,
        reject,
        revertibleFiles: 0,
      };
      this.activeStreams.set(conversation.id, task);
      this.scheduleStream(task);
    });
    return promise;
  }

  private startShellStream(args: InvokeArgs) {
    const requestedConversationId = nullableStringArg(args, "conversationId");
    let conversation = requestedConversationId
      ? this.requireConversation(requestedConversationId)
      : null;
    if (!conversation) {
      const projectId = nullableNumberArg(args, "projectId");
      if (projectId === null) {
        return Promise.reject("Shell mode requires a selected project");
      }
      conversation = createConversation(this.state, {
        model: stringArg(args, "model"),
        projectId,
        agentMode: booleanArg(args, "agentMode"),
        approvalPolicyOverride: normalizeApprovalPolicy(args.approvalPolicy),
        fastMode:
          typeof args.fastMode === "boolean" ? args.fastMode : undefined,
      });
    }
    if (!conversation.projectId || !conversation.projectRootPath) {
      return Promise.reject("Shell mode requires a project-bound conversation");
    }
    if (this.activeStreams.has(conversation.id)) {
      return Promise.reject(
        "A chat request is already running for this conversation",
      );
    }

    const command = stringArg(args, "command").trim();
    if (!command) {
      return Promise.reject("Shell command cannot be empty");
    }
    conversation.model = stringArg(args, "model");
    const userContent = `!${command}`;
    (this.state.messages[conversation.id] ??= []).push({
      id: `audit-shell-user-${Date.now()}`,
      conversationId: conversation.id,
      role: "user",
      content: userContent,
      createdAt: new Date().toISOString(),
      model: stringArg(args, "model"),
      inputTokens: null,
      outputTokens: null,
      error: false,
      mentions: null,
      tokensSaved: null,
      savingsUsd: null,
    });
    syncConversationSummary(this.state, conversation.id);

    const output = `halv shell harness\ncommand: ${command}\nproject: ${conversation.projectRootPath}`;
    const assistantText = `\`\`\`text\nExit code: 0\nWall time: 0.042 seconds\n[Total output lines: 3]\nOutput:\n${output}\n\`\`\``;
    const promise = new Promise<ChatSendResponse>((resolve, reject) => {
      const task: ActiveStream = {
        conversationId: conversation.id,
        timers: new Set(),
        assistantText,
        cancelled: false,
        snapshotTurnDiffs: clone(this.state.turnDiffs[conversation.id] ?? []),
        resolve,
        reject,
        revertibleFiles: 0,
      };
      this.activeStreams.set(conversation.id, task);
      const callId = `direct-shell-${conversation.id}-${Date.now()}`;
      this.schedule(task, 80, () =>
        this.emitChatDelta({
          conversationId: conversation.id,
          kind: "tool_call",
          callId,
          command,
          toolKind: "shell",
          cwd: conversation.projectRootPath ?? "",
          status: "running",
        }),
      );
      this.schedule(task, 160, () =>
        this.emitChatDelta({
          conversationId: conversation.id,
          kind: "tool_output",
          callId,
          tail: `${output}\n`,
        }),
      );
      this.schedule(task, 240, () =>
        this.emitChatDelta({
          conversationId: conversation.id,
          kind: "tool_result",
          callId,
          status: "completed",
          exitCode: 0,
          wallTimeMs: 42,
          truncatedOutput: false,
        }),
      );
      this.schedule(task, 320, async () => {
        (this.state.messages[conversation.id] ??= []).push({
          id: `audit-shell-assistant-${Date.now()}`,
          conversationId: conversation.id,
          role: "assistant",
          content: assistantText,
          createdAt: new Date().toISOString(),
          model: conversation.model,
          inputTokens: null,
          outputTokens: null,
          error: false,
          mentions: null,
          tokensSaved: null,
          savingsUsd: null,
        });
        syncConversationSummary(this.state, conversation.id);
        await this.emitChatDelta({
          conversationId: conversation.id,
          kind: "done",
        });
        this.activeStreams.delete(conversation.id);
        task.resolve({
          conversationId: conversation.id,
          assistantMessage: assistantText,
          skippedMentions: [],
        });
      });
    });
    return promise;
  }

  private scheduleStream(task: ActiveStream) {
    const conversationId = task.conversationId;
    this.schedule(task, 120, () =>
      this.emitChatDelta({
        conversationId,
        kind: "reasoning",
        text: "**Approach:** inspect the live component contract before changing state.\n\n",
      }),
    );
    this.schedule(task, 480, () =>
      this.emitChatDelta({
        conversationId,
        kind: "reasoning",
        text: "- Verify the IPC route\n- Stream a representative result\n",
      }),
    );
    this.schedule(task, 820, () =>
      this.emitTextDelta(task, "## Browser audit response\n\n"),
    );
    this.schedule(task, 1_120, () =>
      this.emitTextDelta(
        task,
        "The real React tree is receiving this reply through the mocked Tauri event channel.\n\n",
      ),
    );
    this.schedule(task, 1_420, () =>
      this.emitTextDelta(
        task,
        "```ts\nconst result = await invoke(\"chat_send\", payload);\n```\n\n",
      ),
    );
    this.schedule(task, 1_690, () =>
      this.emitChatDelta({
        conversationId,
        kind: "tool_call",
        callId: `live-tool-${conversationId}`,
        command: "rg -n \"invoke\\(\" src",
        toolKind: "shell",
        cwd: "/Users/auditor/Projects/halv",
        status: "running",
        justification: "Inventory the frontend IPC surface used by the real UI.",
      }),
    );
    this.schedule(task, 1_980, () =>
      this.emitChatDelta({
        conversationId,
        kind: "tool_output",
        callId: `live-tool-${conversationId}`,
        tail: "src/App.tsx:155: invoke<AccountAuthStatus>(\"auth_status\")\nsrc/App.tsx:2007: invoke<ChatSendResponse>(\"chat_send\")\n",
      }),
    );
    this.schedule(task, 2_220, () =>
      this.emitChatDelta({
        conversationId,
        kind: "tool_result",
        callId: `live-tool-${conversationId}`,
        status: "completed",
        exitCode: 0,
        wallTimeMs: 86,
        truncatedOutput: false,
      }),
    );
    this.schedule(task, 2_440, () => {
      const turnDiff: TurnDiffSummary = {
        turnId: `live-turn-${Date.now()}`,
        cwd: "/Users/auditor/Projects/halv",
        createdAt: new Date().toISOString(),
        revertible: true,
        diff: "diff --git a/src/example.ts b/src/example.ts\n--- a/src/example.ts\n+++ b/src/example.ts\n@@ -1 +1 @@\n-export const audited = false;\n+export const audited = true;",
      };
      (this.state.turnDiffs[conversationId] ??= []).push(turnDiff);
      task.revertibleFiles = 1;
      return this.emitChatDelta({
        conversationId,
        kind: "turn_diff",
        turnId: turnDiff.turnId,
        diff: turnDiff.diff,
        cwd: turnDiff.cwd,
        revertible: turnDiff.revertible,
      });
    });
    this.schedule(task, 2_700, () =>
      this.emitTextDelta(
        task,
        "The streamed Markdown, tool lifecycle, diff summary, and usage footer all use the production rendering path.",
      ),
    );
    this.schedule(task, 3_050, async () => {
      const rows = (this.state.messages[conversationId] ??= []);
      rows.push({
        id: `audit-assistant-${Date.now()}`,
        conversationId,
        role: "assistant",
        content: task.assistantText,
        createdAt: new Date().toISOString(),
        model: this.requireConversation(conversationId).model,
        inputTokens: STREAM_USAGE.inputTokens,
        outputTokens: STREAM_USAGE.outputTokens,
        error: false,
        mentions: null,
        tokensSaved: 5_740,
        savingsUsd: 0.0052,
        usdMethod: "effective_rate",
      });
      syncConversationSummary(this.state, conversationId);
      this.bumpSavingsAfterTurn();
      // goalSignal is intentionally omitted: Rust serializes goalSignal: None this way.
      await this.emitChatDelta({
        conversationId,
        kind: "done",
        usage: STREAM_USAGE,
      });
      this.activeStreams.delete(conversationId);
      task.resolve({
        conversationId,
        assistantMessage: task.assistantText,
        usage: STREAM_USAGE,
        skippedMentions: [],
        serviceTier:
          this.requireConversation(conversationId).fastMode &&
          CHAT_MODELS.some(
            (model) =>
              model.id === this.requireConversation(conversationId).model &&
              model.supportsFastMode,
          )
            ? "fast"
            : "default",
      });
    });
  }

  private schedule(
    task: ActiveStream,
    delay: number,
    action: () => void | Promise<void>,
  ) {
    const timer = window.setTimeout(() => {
      task.timers.delete(timer);
      if (!task.cancelled) {
        void action();
      }
    }, delay);
    task.timers.add(timer);
  }

  private emitTextDelta(task: ActiveStream, text: string) {
    task.assistantText += text;
    return this.emitChatDelta({
      conversationId: task.conversationId,
      kind: "delta",
      text,
    });
  }

  private emitChatDelta(payload: ChatDeltaPayload) {
    return this.emit("chat_delta", payload);
  }

  private cancelStream(conversationId: string, revert: boolean) {
    const task = this.activeStreams.get(conversationId);
    if (!task) {
      return 0;
    }
    task.cancelled = true;
    for (const timer of task.timers) {
      window.clearTimeout(timer);
    }
    task.timers.clear();
    this.activeStreams.delete(conversationId);

    if (revert) {
      this.state.turnDiffs[conversationId] = clone(task.snapshotTurnDiffs);
    }
    (this.state.messages[conversationId] ??= []).push({
      id: `audit-cancelled-${Date.now()}`,
      conversationId,
      role: "assistant",
      content: task.assistantText || "Chat request cancelled",
      createdAt: new Date().toISOString(),
      model: this.requireConversation(conversationId).model,
      inputTokens: null,
      outputTokens: null,
      error: true,
      mentions: null,
      tokensSaved: null,
      savingsUsd: null,
    });
    syncConversationSummary(this.state, conversationId);
    void this.emitChatDelta({
      conversationId,
      kind: "error",
      text: "Chat request cancelled",
    });
    task.reject("Chat request cancelled");
    return revert ? task.revertibleFiles : 0;
  }

  private deleteConversation(conversationId: string) {
    this.state.conversations = this.state.conversations.filter(
      (item) => item.id !== conversationId,
    );
    delete this.state.messages[conversationId];
    delete this.state.turnDiffs[conversationId];
    this.compactions.delete(conversationId);
  }

  private messagesWithSequence(conversationId: string) {
    return (this.state.messages[conversationId] ?? []).map((message, index) => ({
      ...message,
      sequence: message.sequence ?? index + 1,
    }));
  }

  private forkConversation(conversationId: string, uptoSequence: number) {
    const source = this.requireConversation(conversationId);
    const fork = createConversation(this.state, {
      title: `Fork of ${source.title}`,
      model: source.model,
      projectId: source.projectId ?? null,
      agentMode: source.agentMode,
      workflowMode: source.workflowMode,
    });
    fork.goal = source.goal ?? null;
    fork.approvalPolicyOverride = source.approvalPolicyOverride ?? null;

    const sourceToForkSequence = new Map<number, number>();
    const sourceMessages = this.messagesWithSequence(conversationId);
    if (!sourceMessages.some((message) => message.sequence === uptoSequence)) {
      throw new Error("Fork point was not found");
    }
    const copiedMessages = sourceMessages
      .filter((message) => (message.sequence ?? 0) <= uptoSequence)
      .map((message, index): StoredMessage => {
        const newSequence = index + 1;
        if (message.sequence !== undefined) {
          sourceToForkSequence.set(message.sequence, newSequence);
        }
        return {
          ...message,
          id: `${fork.id}-message-${newSequence}`,
          sequence: newSequence,
          conversationId: fork.id,
          attachments: message.attachments?.map((attachment, attachmentIndex) => ({
            ...attachment,
            id: `${fork.id}-message-${newSequence}-attachment-${attachmentIndex + 1}`,
          })),
          tokensSaved: null,
          savingsUsd: null,
        };
      });
    this.state.messages[fork.id] = copiedMessages;
    fork.messageCount = copiedMessages.length;

    const sourceCompaction = this.compactions.get(conversationId);
    if (
      sourceCompaction &&
      sourceCompaction.uptoMessageId <= uptoSequence
    ) {
      const remappedSequence = sourceToForkSequence.get(
        sourceCompaction.uptoMessageId,
      );
      if (remappedSequence !== undefined) {
        this.compactions.set(fork.id, {
          ...sourceCompaction,
          id: Date.now(),
          conversationId: fork.id,
          uptoMessageId: remappedSequence,
        });
      }
    }

    return clone(fork);
  }

  private compactConversation(args: InvokeArgs) {
    const conversationId = stringArg(args, "conversationId");
    if (this.activeStreams.has(conversationId)) {
      throw new Error("Wait for the current turn to finish before compacting context.");
    }
    const messages = this.messagesWithSequence(conversationId);
    if (completedTurnCount(messages) < 2) {
      throw new Error("At least 2 completed turns are required before compacting context.");
    }
    const boundary = [...messages]
      .reverse()
      .find((message) => message.role === "assistant" && !message.error);
    if (boundary?.sequence === undefined) {
      throw new Error("No completed turn is available to compact.");
    }
    const compaction: ConversationCompaction = {
      id: Date.now(),
      conversationId,
      uptoMessageId: boundary.sequence,
      summary: "The audit harness compacted the older conversation context.",
      createdAt: new Date().toISOString(),
    };
    this.compactions.set(conversationId, compaction);
    void this.emit("chat_delta", {
      conversationId,
      kind: "context_compacted",
      compaction,
    } satisfies ChatDeltaPayload);
    return clone(compaction);
  }

  private truncateConversation(conversationId: string, messageId: string) {
    const rows = this.state.messages[conversationId] ?? [];
    const index = rows.findIndex((message) => message.id === messageId);
    if (index < 0) {
      throw {
        code: "messageNotFound",
        message: `Message ${messageId} was not found in conversation ${conversationId}`,
      };
    }
    rows.splice(index);
    syncConversationSummary(this.state, conversationId);
    return clone(this.requireConversation(conversationId));
  }

  private setConversationGoal(args: InvokeArgs) {
    const conversationId = nullableStringArg(args, "conversationId");
    const projectId = nullableNumberArg(args, "projectId");
    const conversation = conversationId
      ? this.requireConversation(conversationId)
      : createConversation(this.state, {
          model: nullableStringArg(args, "model") ?? "gpt-5.6-sol",
          projectId,
          agentMode: false,
          workspaceId: nullableStringArg(args, "workspaceId"),
        });
    conversation.goal = nullableStringArg(args, "goal");
    conversation.updatedAt = new Date().toISOString();
    return clone(conversation);
  }

  private setConversationApprovalPolicy(args: InvokeArgs) {
    const conversation = this.requireConversation(
      stringArg(args, "conversationId"),
    );
    conversation.approvalPolicyOverride = normalizeApprovalPolicy(args.policy);
    conversation.updatedAt = new Date().toISOString();
    return clone(conversation);
  }

  private setAgentMode(args: InvokeArgs) {
    const conversation = this.requireConversation(
      stringArg(args, "conversationId"),
    );
    conversation.agentMode = booleanArg(args, "enabled");
    conversation.updatedAt = new Date().toISOString();
    return clone(conversation);
  }

  private setWorkflowMode(args: InvokeArgs) {
    const conversation = this.requireConversation(
      stringArg(args, "conversationId"),
    );
    const mode = stringArg(args, "mode");
    if (mode !== "default" && mode !== "plan") {
      throw new Error("workflow mode must be `default` or `plan`");
    }
    conversation.workflowMode = mode;
    conversation.updatedAt = new Date().toISOString();
    return clone(conversation);
  }

  private revertTurn(conversationId: string, turnId: string) {
    this.state.turnDiffs[conversationId] = (
      this.state.turnDiffs[conversationId] ?? []
    ).filter((turnDiff) => turnDiff.turnId !== turnId);
  }

  private searchMessages(query: string): MessageSearchResult[] {
    const needle = query.trim().toLowerCase();
    if (!needle) {
      return [];
    }
    const results: MessageSearchResult[] = [];
    for (const conversation of this.state.conversations) {
      for (const message of this.state.messages[conversation.id] ?? []) {
        const index = message.content.toLowerCase().indexOf(needle);
        if (index < 0) {
          continue;
        }
        const start = Math.max(0, index - 32);
        const matchEnd = index + needle.length;
        const end = Math.min(message.content.length, matchEnd + 88);
        const snippet = [
          message.content.slice(start, index),
          "[[HALV_HIT]]",
          message.content.slice(index, matchEnd),
          "[[/HALV_HIT]]",
          message.content.slice(matchEnd, end),
        ]
          .join("")
          .replace(/\s+/g, " ");
        results.push({
          conversationId: conversation.id,
          conversationTitle: conversation.title,
          messageId: message.id,
          role: message.role,
          snippet,
        });
      }
    }
    return results.slice(0, 12);
  }

  private openProject(id: number) {
    const project = this.requireProject(id);
    project.lastOpenedAt = new Date().toISOString();
    return clone(project);
  }

  private addLocalProject(path: string) {
    const trimmed = path.replace(/\/+$/, "");
    const name = trimmed.split("/").filter(Boolean).pop() ?? "Local project";
    const project: Project = {
      id: this.state.nextProject++,
      name,
      rootPath: trimmed,
      trustLevel: null,
      approvalPolicy: "ask",
      createdAt: new Date().toISOString(),
      lastOpenedAt: new Date().toISOString(),
    };
    this.state.projects.unshift(project);
    return clone(project);
  }

  private cloneGithubProject(args: InvokeArgs) {
    const url = stringArg(args, "url");
    const rawName = url.split("/").filter(Boolean).pop() ?? "cloned-project";
    const name = rawName.replace(/\.git$/i, "") || "cloned-project";
    const parent = nullableStringArg(args, "destParent") ?? "/Users/auditor/Projects";
    return this.addLocalProject(`${parent.replace(/\/+$/, "")}/${name}`);
  }

  private removeProject(id: number) {
    this.state.projects = this.state.projects.filter((project) => project.id !== id);
    for (const conversation of this.state.conversations) {
      if (conversation.projectId === id) {
        updateConversationProjectFields(conversation, undefined);
        conversation.agentMode = false;
      }
    }
  }

  private searchProjectFiles(query: string) {
    const files = [
      "ARCHITECTURE.md",
      "src/App.tsx",
      "src/types.ts",
      "src/components/MessageList.tsx",
      "src-tauri/src/chat/mod.rs",
      "src-tauri/src/sidecar/mod.rs",
    ];
    const needle = query.trim().toLowerCase();
    return files
      .filter((path) => !needle || path.toLowerCase().includes(needle))
      .map((path) => ({ path }));
  }

  private projectGitStatus(id: number): BranchStatus {
    this.requireProject(id);
    return id === 1
      ? {
          isGitRepo: true,
          branch: "ux-audit-harness",
          defaultBranch: "main",
          clean: false,
          autoCreatedBranch: null,
          warning: null,
        }
      : {
          isGitRepo: true,
          branch: "main",
          defaultBranch: "main",
          clean: true,
          autoCreatedBranch: null,
          warning: null,
        };
  }

  private setProjectTrust(args: InvokeArgs) {
    const project = this.requireProject(numberArg(args, "id"));
    const level = stringArg(args, "level");
    if (level !== "trusted" && level !== "untrusted") {
      throw new Error(`Unknown trust level: ${level}`);
    }
    project.trustLevel = level;
    for (const conversation of this.state.conversations) {
      if (conversation.projectId === project.id) {
        conversation.projectTrustLevel = level;
      }
    }
    return clone(project);
  }

  private setProjectApprovalPolicy(args: InvokeArgs) {
    const project = this.requireProject(numberArg(args, "id"));
    project.approvalPolicy = normalizeApprovalPolicy(args.policy);
    return clone(project);
  }

  private bumpSavingsAfterTurn() {
    this.state.sidecarStats = {
      ...this.state.sidecarStats,
      tokensSaved: this.state.sidecarStats.tokensSaved + 5_740,
      costSavedUsd: this.state.sidecarStats.costSavedUsd + 0.18,
      requestsTotal: this.state.sidecarStats.requestsTotal + 1,
      sessionTokensSaved: this.state.sidecarStats.sessionTokensSaved + 5_740,
      sessionCostSavedUsd: this.state.sidecarStats.sessionCostSavedUsd + 0.18,
      inputTokens: this.state.sidecarStats.inputTokens + STREAM_USAGE.inputTokens,
      processInputTokens:
        this.state.sidecarStats.processInputTokens + STREAM_USAGE.inputTokens,
      outputTokens: this.state.sidecarStats.outputTokens + STREAM_USAGE.outputTokens,
      proxyCompressionTokensSaved:
        this.state.sidecarStats.proxyCompressionTokensSaved + 5_740,
      compressionTokensSaved:
        this.state.sidecarStats.compressionTokensSaved + 5_740,
    };
    this.state.savingsTotals.tokensSaved += 5_740;
    this.state.savingsTotals.savingsUsd += 0.18;
    this.state.savingsTotals.sessionTokensSaved += 5_740;
    this.state.savingsTotals.sessionSavingsUsd += 0.18;
    this.state.savingsToday.tokensSaved += 5_740;
    this.state.savingsToday.savingsUsd += 0.18;
    this.state.savingsWeek.tokensSaved += 5_740;
    this.state.savingsWeek.savingsUsd += 0.18;
    this.state.savingsTotals.dollarsUsd = (this.state.savingsTotals.dollarsUsd ?? 0) + 0.0052;
    this.state.savingsToday.dollarsUsd = (this.state.savingsToday.dollarsUsd ?? 0) + 0.0052;
    this.state.savingsWeek.dollarsUsd = (this.state.savingsWeek.dollarsUsd ?? 0) + 0.0052;
  }

  private requireConversation(conversationId: string) {
    const conversation = this.state.conversations.find(
      (item) => item.id === conversationId,
    );
    if (!conversation) {
      throw new Error(`Conversation not found: ${conversationId}`);
    }
    return conversation;
  }

  private requireCredentialProfile(id: string) {
    const profile = this.credentialProfiles.find((item) => item.id === id);
    if (!profile) {
      throw new Error("Credential profile was not found");
    }
    return profile;
  }

  private requireProject(id: number) {
    const project = this.state.projects.find((item) => item.id === id);
    if (!project) {
      throw new Error(`Project not found: ${id}`);
    }
    return project;
  }
}

type FixtureCommandHandler = (
  transport: FixtureTransport,
  args: InvokeArgs,
) => unknown;

// The folders of the folder-scan fixtures (ADR 0019) and the git
// repositories that the scan finds below each one. The scan reports any
// other path as a git repository.
const NESTED_REPO_PARENT = "/Users/auditor/Projects";
const NESTED_REPO_FIXTURES = new Map<string, string[]>([
  [NESTED_REPO_PARENT, ["/Users/auditor/Projects/design-system", "/Users/auditor/Projects/halv"]],
  ["/Users/auditor/Clients", ["/Users/auditor/Clients/api", "/Users/auditor/Clients/site"]],
]);

function marketingWorkspaceNestedRepos(path: string) {
  return path === MARKETING_WORKSPACE_FOLDER
    ? [`${MARKETING_WORKSPACE_FOLDER}/app`, `${MARKETING_WORKSPACE_FOLDER}/web`]
    : undefined;
}

function isNestedRepoFolder(path: string) {
  return NESTED_REPO_FIXTURES.has(path);
}

function routeFixtureCommand(name: CommandName): FixtureCommandHandler {
  return (transport, args) => transport.routeCommand(name, args);
}

export const fixtureCommandHandlers: Record<
  CommandName,
  FixtureCommandHandler
> = {
  account_claim_referral: routeFixtureCommand("account_claim_referral"),
  account_publish_receipt: routeFixtureCommand("account_publish_receipt"),
  account_savings_queue_status: routeFixtureCommand("account_savings_queue_status"),
  agent_claude_usage_windows: routeFixtureCommand("agent_claude_usage_windows"),
  agent_cli_install: routeFixtureCommand("agent_cli_install"),
  agent_codex_usage_windows: routeFixtureCommand("agent_codex_usage_windows"),
  agent_ghostty_launch: routeFixtureCommand("agent_ghostty_launch"),
  agent_login_launch: routeFixtureCommand("agent_login_launch"),
  wallet_list: routeFixtureCommand("wallet_list"),
  wallet_select: routeFixtureCommand("wallet_select"),
  wallet_add_launch: routeFixtureCommand("wallet_add_launch"),
  wallet_add_finish: routeFixtureCommand("wallet_add_finish"),
  wallet_add_cancel: routeFixtureCommand("wallet_add_cancel"),
  wallet_remove: routeFixtureCommand("wallet_remove"),
  wallet_usage: routeFixtureCommand("wallet_usage"),
  wallet_stale_sessions: routeFixtureCommand("wallet_stale_sessions"),
  wallet_keep_sessions: routeFixtureCommand("wallet_keep_sessions"),
  agent_session_checkpoint_preview: routeFixtureCommand("agent_session_checkpoint_preview"),
  agent_session_checkpoint_restore: routeFixtureCommand("agent_session_checkpoint_restore"),
  agent_session_checkpoints: routeFixtureCommand("agent_session_checkpoints"),
  agent_session_create: routeFixtureCommand("agent_session_create"),
  agent_session_forget: routeFixtureCommand("agent_session_forget"),
  agent_session_kill: routeFixtureCommand("agent_session_kill"),
  agent_session_lane_options: routeFixtureCommand("agent_session_lane_options"),
  agent_session_list: routeFixtureCommand("agent_session_list"),
  agent_session_probe: routeFixtureCommand("agent_session_probe"),
  agent_session_rename: routeFixtureCommand("agent_session_rename"),
  agent_session_respond_permission: routeFixtureCommand("agent_session_respond_permission"),
  agent_session_saved_totals: routeFixtureCommand("agent_session_saved_totals"),
  agent_session_send: routeFixtureCommand("agent_session_send"),
  agent_terminal_create: routeFixtureCommand("agent_terminal_create"),
  agent_terminal_duplicate: routeFixtureCommand("agent_terminal_duplicate"),
  analytics_flush: routeFixtureCommand("analytics_flush"),
  analytics_install_id: routeFixtureCommand("analytics_install_id"),
  analytics_record: routeFixtureCommand("analytics_record"),
  app_open_external_url: routeFixtureCommand("app_open_external_url"),
  app_open_path: routeFixtureCommand("app_open_path"),
  app_reveal_path: routeFixtureCommand("app_reveal_path"),
  app_hide_main_window: routeFixtureCommand("app_hide_main_window"),
  dot_set_view: routeFixtureCommand("dot_set_view"),
  dot_reload: routeFixtureCommand("dot_reload"),
  app_update_relaunch: routeFixtureCommand("app_update_relaunch"),
  auth_anthropic_status: routeFixtureCommand("auth_anthropic_status"),
  auth_credential_status: routeFixtureCommand("auth_credential_status"),
  auth_email_challenge: routeFixtureCommand("auth_email_challenge"),
  auth_login_chatgpt: routeFixtureCommand("auth_login_chatgpt"),
  auth_logout: routeFixtureCommand("auth_logout"),
  auth_oauth_cancel: routeFixtureCommand("auth_oauth_cancel"),
  auth_oauth_login: routeFixtureCommand("auth_oauth_login"),
  auth_oauth_providers: routeFixtureCommand("auth_oauth_providers"),
  auth_refresh_entitlements: routeFixtureCommand("auth_refresh_entitlements"),
  auth_request_code: routeFixtureCommand("auth_request_code"),
  auth_set_anthropic_api_key: routeFixtureCommand("auth_set_anthropic_api_key"),
  auth_set_api_key: routeFixtureCommand("auth_set_api_key"),
  auth_signout: routeFixtureCommand("auth_signout"),
  auth_status: routeFixtureCommand("auth_status"),
  auth_verify: routeFixtureCommand("auth_verify"),
  automation_delete: routeFixtureCommand("automation_delete"),
  automation_list: routeFixtureCommand("automation_list"),
  automation_mark_all_read: routeFixtureCommand("automation_mark_all_read"),
  automation_mark_run_read: routeFixtureCommand("automation_mark_run_read"),
  automation_report_run: routeFixtureCommand("automation_report_run"),
  automation_run_now: routeFixtureCommand("automation_run_now"),
  automation_set_enabled: routeFixtureCommand("automation_set_enabled"),
  automation_unread_count: routeFixtureCommand("automation_unread_count"),
  automation_upsert: routeFixtureCommand("automation_upsert"),
  billing_checkout: routeFixtureCommand("billing_checkout"),
  billing_checkout_browser: routeFixtureCommand("billing_checkout_browser"),
  billing_prices: routeFixtureCommand("billing_prices"),
  tray_get_snapshot: routeFixtureCommand("tray_get_snapshot"),
  tray_action: routeFixtureCommand("tray_action"),
  tray_resize: routeFixtureCommand("tray_resize"),
  tray_set_labels: routeFixtureCommand("tray_set_labels"),
  billing_lifetime_offer: routeFixtureCommand("billing_lifetime_offer"),
  billing_portal: routeFixtureCommand("billing_portal"),
  browser_clear_data: routeFixtureCommand("browser_clear_data"),
  browser_settings_get: routeFixtureCommand("browser_settings_get"),
  browser_settings_set: routeFixtureCommand("browser_settings_set"),
  browser_ui_annotation_target: routeFixtureCommand("browser_ui_annotation_target"),
  browser_ui_click: routeFixtureCommand("browser_ui_click"),
  browser_ui_close_tab: routeFixtureCommand("browser_ui_close_tab"),
  browser_ui_history: routeFixtureCommand("browser_ui_history"),
  browser_ui_navigate: routeFixtureCommand("browser_ui_navigate"),
  browser_ui_preview_style: routeFixtureCommand("browser_ui_preview_style"),
  browser_ui_select_tab: routeFixtureCommand("browser_ui_select_tab"),
  browser_ui_state: routeFixtureCommand("browser_ui_state"),
  browser_ui_type_focused: routeFixtureCommand("browser_ui_type_focused"),
  chat_agent_capabilities: routeFixtureCommand("chat_agent_capabilities"),
  chat_approve_last_denied: routeFixtureCommand("chat_approve_last_denied"),
  chat_cancel: routeFixtureCommand("chat_cancel"),
  chat_cancel_and_revert: routeFixtureCommand("chat_cancel_and_revert"),
  chat_interrupt_all_subagents: routeFixtureCommand("chat_interrupt_all_subagents"),
  chat_interrupt_subagent: routeFixtureCommand("chat_interrupt_subagent"),
  chat_list_models: routeFixtureCommand("chat_list_models"),
  chat_list_subagents: routeFixtureCommand("chat_list_subagents"),
  chat_list_turn_diffs: routeFixtureCommand("chat_list_turn_diffs"),
  chat_regenerate: routeFixtureCommand("chat_regenerate"),
  chat_revert_turn: routeFixtureCommand("chat_revert_turn"),
  chat_run_shell: routeFixtureCommand("chat_run_shell"),
  chat_send: routeFixtureCommand("chat_send"),
  chat_send_subagent_message: routeFixtureCommand("chat_send_subagent_message"),
  chat_set_agent_mode: routeFixtureCommand("chat_set_agent_mode"),
  chat_set_workflow_mode: routeFixtureCommand("chat_set_workflow_mode"),
  chat_steer: routeFixtureCommand("chat_steer"),
  chat_tool_decision: routeFixtureCommand("chat_tool_decision"),
  compact_conversation: routeFixtureCommand("compact_conversation"),
  composer_read_dropped_image: routeFixtureCommand("composer_read_dropped_image"),
  credential_profile_add: routeFixtureCommand("credential_profile_add"),
  credential_profile_remove: routeFixtureCommand("credential_profile_remove"),
  credential_profile_rename: routeFixtureCommand("credential_profile_rename"),
  credential_profile_set_enabled: routeFixtureCommand("credential_profile_set_enabled"),
  credential_profile_set_priority: routeFixtureCommand("credential_profile_set_priority"),
  credential_profile_test: routeFixtureCommand("credential_profile_test"),
  crux_approve_indexing: routeFixtureCommand("crux_approve_indexing"),
  crux_reindex_now: routeFixtureCommand("crux_reindex_now"),
  crux_set_activation: routeFixtureCommand("crux_set_activation"),
  crux_workspace_open: routeFixtureCommand("crux_workspace_open"),
  diagnostics_export_support_report: routeFixtureCommand("diagnostics_export_support_report"),
  diagnostics_snapshot: routeFixtureCommand("diagnostics_snapshot"),
  export_conversation_to_file: routeFixtureCommand("export_conversation_to_file"),
  extensions_snapshot: routeFixtureCommand("extensions_snapshot"),
  fork_conversation: routeFixtureCommand("fork_conversation"),
  list_custom_prompts: routeFixtureCommand("list_custom_prompts"),
  local_environment_approve: routeFixtureCommand("local_environment_approve"),
  local_environment_delete: routeFixtureCommand("local_environment_delete"),
  local_environment_list: routeFixtureCommand("local_environment_list"),
  local_environment_resolve_action: routeFixtureCommand("local_environment_resolve_action"),
  local_environment_save: routeFixtureCommand("local_environment_save"),
  mcp_server_clear_bearer_token: routeFixtureCommand("mcp_server_clear_bearer_token"),
  mcp_server_oauth_login: routeFixtureCommand("mcp_server_oauth_login"),
  mcp_server_oauth_logout: routeFixtureCommand("mcp_server_oauth_logout"),
  mcp_server_remove: routeFixtureCommand("mcp_server_remove"),
  mcp_server_set_bearer_token: routeFixtureCommand("mcp_server_set_bearer_token"),
  mcp_server_set_enabled: routeFixtureCommand("mcp_server_set_enabled"),
  mcp_server_test: routeFixtureCommand("mcp_server_test"),
  mcp_server_upsert: routeFixtureCommand("mcp_server_upsert"),
  plugins_install_local: routeFixtureCommand("plugins_install_local"),
  plugins_set_enabled: routeFixtureCommand("plugins_set_enabled"),
  plugins_uninstall: routeFixtureCommand("plugins_uninstall"),
  preferences_get: routeFixtureCommand("preferences_get"),
  preferences_set_analytics_enabled: routeFixtureCommand("preferences_set_analytics_enabled"),
  preferences_set_claude_subscription_proxy: routeFixtureCommand("preferences_set_claude_subscription_proxy"),
  preferences_set_conversation_defaults: routeFixtureCommand("preferences_set_conversation_defaults"),
  preferences_set_keyboard_shortcuts: routeFixtureCommand("preferences_set_keyboard_shortcuts"),
  preferences_set_memories_enabled: routeFixtureCommand("preferences_set_memories_enabled"),
  preferences_set_notifications: routeFixtureCommand("preferences_set_notifications"),
  preferences_set_personality: routeFixtureCommand("preferences_set_personality"),
  preferences_set_ponytail: routeFixtureCommand("preferences_set_ponytail"),
  preferences_set_subagent_concurrency_limit: routeFixtureCommand("preferences_set_subagent_concurrency_limit"),
  preferences_set_task_routing: routeFixtureCommand("preferences_set_task_routing"),
  preferences_set_worker_lane: routeFixtureCommand("preferences_set_worker_lane"),
  preferences_set_workflow_routing: routeFixtureCommand("preferences_set_workflow_routing"),
  preferences_set_claude_ultracode: routeFixtureCommand("preferences_set_claude_ultracode"),
  preferences_set_routing_tier_agents: routeFixtureCommand("preferences_set_routing_tier_agents"),
  project_add_local: routeFixtureCommand("project_add_local"),
  project_clone_github: routeFixtureCommand("project_clone_github"),
  project_commit: routeFixtureCommand("project_commit"),
  project_create_draft_pr: routeFixtureCommand("project_create_draft_pr"),
  project_git_repos_under_path: routeFixtureCommand("project_git_repos_under_path"),
  project_git_root_for_path: routeFixtureCommand("project_git_root_for_path"),
  project_adoption_root: routeFixtureCommand("project_adoption_root"),
  project_git_status: routeFixtureCommand("project_git_status"),
  project_list: routeFixtureCommand("project_list"),
  project_open: routeFixtureCommand("project_open"),
  project_push: routeFixtureCommand("project_push"),
  project_remove: routeFixtureCommand("project_remove"),
  project_review_apply_action: routeFixtureCommand("project_review_apply_action"),
  project_review_file_diff: routeFixtureCommand("project_review_file_diff"),
  project_review_snapshot_for_cwd: routeFixtureCommand("project_review_snapshot_for_cwd"),
  project_scan_nested_repos: routeFixtureCommand("project_scan_nested_repos"),
  project_search_files: routeFixtureCommand("project_search_files"),
  project_set_trust: routeFixtureCommand("project_set_trust"),
  receipt_write_png: routeFixtureCommand("receipt_write_png"),
  remote_device_revoke: routeFixtureCommand("remote_device_revoke"),
  remote_devices_list: routeFixtureCommand("remote_devices_list"),
  remote_identity_reset: routeFixtureCommand("remote_identity_reset"),
  remote_pairing_cancel: routeFixtureCommand("remote_pairing_cancel"),
  remote_pairing_respond: routeFixtureCommand("remote_pairing_respond"),
  remote_pairing_start: routeFixtureCommand("remote_pairing_start"),
  remote_set_active: routeFixtureCommand("remote_set_active"),
  remote_set_enabled: routeFixtureCommand("remote_set_enabled"),
  remote_set_relay_url: routeFixtureCommand("remote_set_relay_url"),
  remote_status: routeFixtureCommand("remote_status"),
  route_task: routeFixtureCommand("route_task"),
  search_messages: routeFixtureCommand("search_messages"),
  sidecar_install_headroom: routeFixtureCommand("sidecar_install_headroom"),
  sidecar_restart: routeFixtureCommand("sidecar_restart"),
  sidecar_stats: routeFixtureCommand("sidecar_stats"),
  sidecar_status: routeFixtureCommand("sidecar_status"),
  sidecar_update_headroom: routeFixtureCommand("sidecar_update_headroom"),
  skill_read: routeFixtureCommand("skill_read"),
  skills_set_enabled: routeFixtureCommand("skills_set_enabled"),
  storage_clear_agent_memories: routeFixtureCommand("storage_clear_agent_memories"),
  storage_consume_conversation_draft: routeFixtureCommand("storage_consume_conversation_draft"),
  storage_create_conversation_draft: routeFixtureCommand("storage_create_conversation_draft"),
  storage_daily_token_activity: routeFixtureCommand("storage_daily_token_activity"),
  storage_delete_agent_memory: routeFixtureCommand("storage_delete_agent_memory"),
  storage_delete_conversation: routeFixtureCommand("storage_delete_conversation"),
  storage_get_conversation_task_plan: routeFixtureCommand("storage_get_conversation_task_plan"),
  storage_get_latest_conversation_compaction: routeFixtureCommand("storage_get_latest_conversation_compaction"),
  storage_get_messages: routeFixtureCommand("storage_get_messages"),
  storage_rebuild_agent_memories: routeFixtureCommand("storage_rebuild_agent_memories"),
  storage_list_agent_memories: routeFixtureCommand("storage_list_agent_memories"),
  storage_list_conversations: routeFixtureCommand("storage_list_conversations"),
  storage_list_credential_profiles: routeFixtureCommand("storage_list_credential_profiles"),
  storage_list_task_routes: routeFixtureCommand("storage_list_task_routes"),
  storage_list_usage_windows: routeFixtureCommand("storage_list_usage_windows"),
  storage_routing_savings: routeFixtureCommand("storage_routing_savings"),
  storage_savings_today: routeFixtureCommand("storage_savings_today"),
  storage_savings_week: routeFixtureCommand("storage_savings_week"),
  storage_set_conversation_agent_backend: routeFixtureCommand("storage_set_conversation_agent_backend"),
  storage_set_conversation_approval_policy: routeFixtureCommand("storage_set_conversation_approval_policy"),
  storage_set_conversation_archived: routeFixtureCommand("storage_set_conversation_archived"),
  storage_set_conversation_credential_profile: routeFixtureCommand("storage_set_conversation_credential_profile"),
  storage_set_conversation_draft: routeFixtureCommand("storage_set_conversation_draft"),
  storage_set_conversation_fast_mode: routeFixtureCommand("storage_set_conversation_fast_mode"),
  storage_set_conversation_goal: routeFixtureCommand("storage_set_conversation_goal"),
  storage_set_conversation_memories_mode: routeFixtureCommand("storage_set_conversation_memories_mode"),
  storage_set_conversation_mode: routeFixtureCommand("storage_set_conversation_mode"),
  storage_set_conversation_pinned: routeFixtureCommand("storage_set_conversation_pinned"),
  storage_set_conversation_title: routeFixtureCommand("storage_set_conversation_title"),
  storage_total_savings: routeFixtureCommand("storage_total_savings"),
  storage_truncate_from: routeFixtureCommand("storage_truncate_from"),
  terminal_attach: routeFixtureCommand("terminal_attach"),
  terminal_close: routeFixtureCommand("terminal_close"),
  terminal_duplicate: routeFixtureCommand("terminal_duplicate"),
  terminal_forget: routeFixtureCommand("terminal_forget"),
  terminal_list: routeFixtureCommand("terminal_list"),
  terminal_open: routeFixtureCommand("terminal_open"),
  terminal_rename: routeFixtureCommand("terminal_rename"),
  terminal_set_shell_tab: routeFixtureCommand("terminal_set_shell_tab"),
  terminal_resize: routeFixtureCommand("terminal_resize"),
  terminal_restore_read: routeFixtureCommand("terminal_restore_read"),
  terminal_restore_shell: routeFixtureCommand("terminal_restore_shell"),
  terminal_restore_write: routeFixtureCommand("terminal_restore_write"),
  terminal_supervisor_restart: routeFixtureCommand("terminal_supervisor_restart"),
  terminal_supervisor_retry: routeFixtureCommand("terminal_supervisor_retry"),
  terminal_supervisor_status: routeFixtureCommand("terminal_supervisor_status"),
  terminal_write: routeFixtureCommand("terminal_write"),
  terminal_open_shell: routeFixtureCommand("terminal_open_shell"),
  terminal_continue_shell: routeFixtureCommand("terminal_continue_shell"),
  terminal_foreground_busy: routeFixtureCommand("terminal_foreground_busy"),
  terminal_link_targets: routeFixtureCommand("terminal_link_targets"),
  diagnostics_resource_usage: routeFixtureCommand("diagnostics_resource_usage"),
  workspace_bind_conversation: routeFixtureCommand("workspace_bind_conversation"),
  workspace_create_branch: routeFixtureCommand("workspace_create_branch"),
  workspace_create_worktree: routeFixtureCommand("workspace_create_worktree"),
  workspace_current: routeFixtureCommand("workspace_current"),
  workspace_handoff: routeFixtureCommand("workspace_handoff"),
  workspace_list: routeFixtureCommand("workspace_list"),
  workspace_overview: routeFixtureCommand("workspace_overview"),
  workspace_list_base_refs: routeFixtureCommand("workspace_list_base_refs"),
  workspace_remove: routeFixtureCommand("workspace_remove"),
  workspace_open_in: routeFixtureCommand("workspace_open_in"),
  workspace_set_pinned: routeFixtureCommand("workspace_set_pinned"),
  workspace_tabs_snapshot: routeFixtureCommand("workspace_tabs_snapshot"),
  workspace_tabs_apply: routeFixtureCommand("workspace_tabs_apply"),
  workspace_tabs_seed: routeFixtureCommand("workspace_tabs_seed"),
  app_update_snapshot: routeFixtureCommand("app_update_snapshot"),
  app_update_check: routeFixtureCommand("app_update_check"),
  app_update_download: routeFixtureCommand("app_update_download"),
  app_update_restart_blockers: routeFixtureCommand("app_update_restart_blockers"),
  app_update_restart: routeFixtureCommand("app_update_restart"),
  app_update_restart_when_idle: routeFixtureCommand("app_update_restart_when_idle"),
  app_update_cancel_restart: routeFixtureCommand("app_update_cancel_restart"),
  app_update_restart_ack: routeFixtureCommand("app_update_restart_ack"),
  app_update_defer: routeFixtureCommand("app_update_defer"),
  app_update_set_auto_download: routeFixtureCommand("app_update_set_auto_download"),
  app_update_dismiss_post_update: routeFixtureCommand("app_update_dismiss_post_update"),
  agent_status_snapshot: routeFixtureCommand("agent_status_snapshot"),
  agent_status_mark_viewed: routeFixtureCommand("agent_status_mark_viewed"),
  agent_status_set_focus: routeFixtureCommand("agent_status_set_focus"),
  terminal_notify: routeFixtureCommand("terminal_notify"),
};

// Worktree operations of the tree specs (ADR 0010, S7). While
// __harnessHoldWorktreeOps is true, a create or a remove waits until the
// spec calls __harnessReleaseWorktreeOps().
type HarnessWorktreeWindow = typeof window & {
  __harnessHoldWorktreeOps?: boolean;
  __harnessReleaseWorktreeOps?: () => void;
  __harnessWorkspaceRemoveError?: string;
  __harnessWorktreeCreateError?: string;
};

function harnessWorktreeGate(): Promise<void> {
  const global = window as HarnessWorktreeWindow;
  if (!global.__harnessHoldWorktreeOps) return Promise.resolve();
  return new Promise((resolve) => {
    const previous = global.__harnessReleaseWorktreeOps;
    global.__harnessReleaseWorktreeOps = () => {
      global.__harnessHoldWorktreeOps = false;
      global.__harnessReleaseWorktreeOps = undefined;
      previous?.();
      resolve();
    };
  });
}

function auditTerminalSession(
  id: string,
  cwd: string,
  projectId: number | null = null,
  projectName = "Agent login",
): TerminalSession {
  return {
    id,
    projectId,
    projectName,
    cwd,
    shell: "/bin/zsh",
    pid: 4242,
    executionProfile: "full_access",
    status: "running",
    cols: 100,
    rows: 30,
    startedAt: new Date().toISOString(),
  };
}

// The shell tab of a panel scope, as Rust reads it: the scope can carry
// the panel storage prefix.
function harnessShellTabIdFromScope(scope: string | null) {
  const bare = (scope ?? "").replace(/^halv\.terminal-layout\.v1:/, "");
  return bare.startsWith("shell:") && bare.length > "shell:".length
    ? bare.slice("shell:".length)
    : null;
}

function agentLaneProfileProvider(kind: AgentSessionCreateLaneKind) {
  if (kind === "claudeApiKey") return "anthropic";
  if (kind === "kimiApiKey") return "moonshot";
  if (kind === "zaiGlm") return "zai";
  if (kind === "grokApiKey") return "xai";
  return null;
}

function auditBrowserState(url = "http://localhost:3000") {
  const svg = encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="720"><rect width="100%" height="100%" fill="#f7f5ef"/><text x="64" y="88" font-family="system-ui" font-size="32" fill="#191914">Halv shared browser</text><rect id="audit-button" x="120" y="90" width="160" height="44" rx="10" fill="#69b596"/><text x="155" y="119" font-family="system-ui" font-size="16" fill="#0d1914">Audit button</text></svg>`,
  );
  return {
    tabs: [
      {
        id: "audit-browser-tab",
        title: "Halv browser audit",
        url,
        active: true,
      },
    ],
    activeTabId: "audit-browser-tab",
    title: "Halv browser audit",
    url,
    screenshotDataUrl: `data:image/svg+xml,${svg}`,
    viewportWidth: 1280,
    viewportHeight: 720,
  };
}

const FIXTURE_HOUR_MS = 60 * 60_000;
const FIXTURE_DAY_MS = 24 * FIXTURE_HOUR_MS;

/**
 * Applies a Settings > Mobile preset: "connecting", "connected", "error",
 * "pairing", or "approval". Any other value keeps the gateway off.
 */
function remoteFixtureStatus(base: RemoteStatus, preset: string | null): RemoteStatus {
  const presets = ["connecting", "connected", "error", "pairing", "approval"];
  if (!preset || !presets.includes(preset)) return base;
  const now = Date.now();
  const live = preset !== "connecting" && preset !== "error";
  const status: RemoteStatus = {
    ...base,
    enabled: true,
    connection: preset === "connecting" ? "connecting" : preset === "error" ? "error" : "connected",
    error:
      preset === "error"
        ? "Halv cannot reach the relay. Halv tries again in 12 s."
        : null,
    hostId: "fixtureHost00000000001",
    devices: [
      {
        id: "fixtureDevice000000001",
        name: "Work iPhone",
        platform: "ios",
        model: "iPhone 17 Pro",
        pairedAt: now - 12 * FIXTURE_DAY_MS,
        lastSeenAt: now,
        connected: live,
      },
      {
        id: "fixtureDevice000000002",
        name: "Pixel 10",
        platform: "android",
        model: "Pixel 10",
        pairedAt: now - 41 * FIXTURE_DAY_MS,
        lastSeenAt: now - 3 * FIXTURE_HOUR_MS,
        connected: false,
      },
    ],
  };
  if (preset === "pairing" || preset === "approval") {
    status.pairing = {
      uri: "halv://pair?p=fixture-offer",
      qrSvgDataUri: fixtureQrSvgDataUri(),
      expiresAt: now + 5 * 60_000,
    };
  }
  if (preset === "approval") {
    status.pendingApproval = {
      requestId: "fixtureRequestPreset",
      name: "Demo iPhone",
      platform: "ios",
      model: "iPhone 17",
      code: "028809",
      expiresAt: now + 120_000,
    };
  }
  return status;
}

/**
 * A QR-like SVG for the harness: the three finder patterns, the timing lines,
 * and fixed pseudo-random modules. It encodes nothing.
 */
function fixtureQrSvgDataUri() {
  const size = 29;
  const quiet = 4;
  const dark = new Set<string>();
  const reserved = new Set<string>();
  const finder = (left: number, top: number) => {
    for (let y = -1; y <= 7; y += 1) {
      for (let x = -1; x <= 7; x += 1) {
        const column = left + x;
        const row = top + y;
        if (column < 0 || row < 0 || column >= size || row >= size) continue;
        reserved.add(`${column},${row}`);
        const ring = Math.max(Math.abs(x - 3), Math.abs(y - 3));
        if (ring === 3 || ring <= 1) dark.add(`${column},${row}`);
      }
    }
  };
  finder(0, 0);
  finder(size - 7, 0);
  finder(0, size - 7);
  for (let index = 8; index < size - 8; index += 1) {
    reserved.add(`${index},6`).add(`6,${index}`);
    if (index % 2 === 0) dark.add(`${index},6`).add(`6,${index}`);
  }
  let seed = 0x5eed;
  for (let row = 0; row < size; row += 1) {
    for (let column = 0; column < size; column += 1) {
      seed = (Math.imul(seed, 1_103_515_245) + 12_345) >>> 0;
      if (!reserved.has(`${column},${row}`) && (seed >>> 16) % 100 < 48) {
        dark.add(`${column},${row}`);
      }
    }
  }
  const path = [...dark]
    .map((cell) => {
      const [column, row] = cell.split(",").map(Number);
      return `M${column + quiet} ${row + quiet}h1v1h-1z`;
    })
    .join("");
  const extent = size + 2 * quiet;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 ${extent} ${extent}" shape-rendering="crispEdges"><rect width="${extent}" height="${extent}" fill="#fff"/><path d="${path}" fill="#000"/></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

// The relay URL rule of the remote protocol (section 5.1): wss://, or ws:// for loopback hosts.
function isFixtureRelayUrl(value: string) {
  try {
    const url = new URL(value);
    if (url.username || url.password || url.search || url.hash) return false;
    if (url.protocol === "wss:") return true;
    return (
      url.protocol === "ws:" &&
      ["127.0.0.1", "[::1]", "localhost"].includes(url.hostname)
    );
  } catch {
    return false;
  }
}

function stringArg(args: InvokeArgs, key: string) {
  const value = args[key];
  if (typeof value !== "string") {
    throw new Error(`[ux-audit] Expected string argument: ${key}`);
  }
  return value;
}

function nullableStringArg(args: InvokeArgs, key: string) {
  const value = args[key];
  if (value === null || value === undefined) {
    return null;
  }
  if (typeof value !== "string") {
    throw new Error(`[ux-audit] Expected nullable string argument: ${key}`);
  }
  return value;
}

function numberArg(args: InvokeArgs, key: string) {
  const value = args[key];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`[ux-audit] Expected number argument: ${key}`);
  }
  return value;
}

function nullableNumberArg(args: InvokeArgs, key: string) {
  const value = args[key];
  if (value === null || value === undefined) {
    return null;
  }
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`[ux-audit] Expected nullable number argument: ${key}`);
  }
  return value;
}

function booleanArg(args: InvokeArgs, key: string) {
  const value = args[key];
  if (typeof value !== "boolean") {
    throw new Error(`[ux-audit] Expected boolean argument: ${key}`);
  }
  return value;
}

function stringArrayArg(args: InvokeArgs, key: string) {
  const value = args[key];
  if (value === null || value === undefined) {
    return null;
  }
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    throw new Error(`[ux-audit] Expected string array argument: ${key}`);
  }
  return value as string[];
}

function imageAttachmentArrayArg(
  args: InvokeArgs,
  key: string,
): ImageAttachment[] | undefined {
  const value = args[key];
  if (value === null || value === undefined) {
    return undefined;
  }
  if (
    !Array.isArray(value) ||
    value.some(
      (item) =>
        typeof item !== "object" ||
        item === null ||
        typeof item.id !== "string" ||
        typeof item.name !== "string" ||
        typeof item.mediaType !== "string" ||
        typeof item.sizeBytes !== "number" ||
        typeof item.data !== "string",
    )
  ) {
    throw new Error(`[ux-audit] Expected image attachment array argument: ${key}`);
  }
  return clone(value as ImageAttachment[]);
}

const HARNESS_LIFETIME_END = Math.floor(Date.now() / 1000) + 86400;

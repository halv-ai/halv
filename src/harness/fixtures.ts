import type {
  AccountAuthStatus,
  AgentCapabilities,
  ApprovalPolicy,
  AuthStatus,
  ChatDeltaPayload,
  ChatModel,
  ConversationSummary,
  CredentialProfile,
  DailyTokenActivity,
  Project,
  SavingsToday,
  SavingsTotals,
  SidecarStats,
  SidecarStatus,
  StoredMessage,
  TerminalSession,
  TurnDiffSummary,
  WorkspaceStatus,
} from "../types";
import type {
  AgentSession,
  AgentSessionEvent,
  AgentSessionLaneOptions,
  AgentSessionProbe,
} from "../agentSessions";
import type { HarnessFixtureConfig, HarnessScenario } from "./states";
import {
  MARKETING_PRO_SPLIT_PLAN_WINDOWS,
} from "./marketing-pro-split.ts";
import {
  MARKETING_ALL_TIME_DOLLARS,
  MARKETING_ALL_TIME_TOKENS,
  MARKETING_ALL_TIME_USD,
  MARKETING_RECENT_DOLLARS,
  MARKETING_SESSION_DOLLARS,
  MARKETING_TODAY_DOLLARS,
  MARKETING_WEEK_DOLLARS,
  MARKETING_ENGINE_SAVINGS,
  MARKETING_RECENT_SAVINGS,
  MARKETING_SESSION_TOKENS,
  MARKETING_WEEK_TOKENS,
  isMarketingRealismFixture,
} from "./marketing-realism.ts";
import {
  MARKETING_SUBAGENT_CONVERSATION_ID,
  MARKETING_SUBAGENT_MESSAGES,
} from "./marketing-subagents.ts";
import {
  addGlyphShowcaseConversations,
  addParkedConversation,
  applyProjectTreeConversations,
  createProjectTreeWorktrees,
} from "./project-tree.ts";
import {
  createMarketingWorkspaceConversations,
  createMarketingWorkspaceProjects,
  createMarketingWorkspaceWorktrees,
} from "./marketing-workspace.ts";

export const HISTORICAL_AGENT_CONVERSATION_ID = "audit-agent-refactor";

export const USAGE_LIMIT_MESSAGE =
  "You've hit your ChatGPT plan's usage limit. Resets in about 1h 46m. (HTTP 429 · usage_limit_reached)";

export const PAUSED_RESTARTS_MESSAGE =
  "managed proxy exited (exit status: 1) on port 8787";

export const TRANSIENT_PROXY_MESSAGE =
  "Timed out waiting for proxy response headers";

export const RUNNING_SIDECAR: SidecarStatus = {
  state: "running",
  spawnedByUs: true,
  managedByHalv: true,
  version: "0.42.0-dev+halv.g284912cf4f65",
  pinnedVersion: "0.42.0-dev+halv.g284912cf4f65",
  port: 8787,
  optimizationPaused: false,
};

export const SIDECAR_STATS: SidecarStats = {
  port: 8787,
  tokensSaved: 148_920,
  costSavedUsd: 4.83,
  lifetimeRequests: 1944,
  lifetimeCacheSavingsUsd: 301.0,
  requestsTotal: 342,
  requestsByProvider: { openai: 342 },
  usageByLane: [
    {
      lane: "codex_chatgpt",
      billing: "plan",
      provider: "openai",
      requests: 342,
      inputTokens: 236_880,
      outputTokens: null,
      costUsd: null,
      tokensSaved: 148_920,
    },
  ],
  unattributedUsage: {
    lane: "unknown",
    billing: "unknown",
    provider: "custom",
    requests: 0,
    inputTokens: 0,
    outputTokens: null,
    costUsd: null,
    tokensSaved: 0,
  },
  requestsCached: 91,
  requestsFailed: 3,
  sessionTokensSaved: 18_460,
  sessionCostSavedUsd: 0.71,
  inputTokens: 236_880,
  processInputTokens: 236_880,
  outputTokens: 41_240,
  proxyCompressionTokensSaved: 101_730,
  compressionTokensSaved: 101_730,
  cliFilteringTokensSaved: 47_190,
  rtkTokensSaved: 47_190,
  leanCtxTokensSaved: 0,
  toolSearchTokensSaved: 0,
  estimatedOutputTokensSaved: 2_480,
  mcpTokensSaved: 0,
  liveWsTokensSaved: 101_730,
  cacheSavingsUsd: 1.24,
  costWithHeadroomUsd: 2.91,
  savingsPercent: 53.6,
  planWindows: [
    {
      label: "7d",
      usedPercent: 27,
      resetsAt: Math.floor(Date.now() / 1000) + 4 * 24 * 60 * 60,
    },
  ],
};

export const CHAT_MODELS: ChatModel[] = [
  {
    id: "gpt-5.6-sol",
    label: "gpt-5.6-sol",
    provider: "openai",
    reasoningEfforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
    defaultEffort: "medium",
    supportsFastMode: true,
  },
  {
    id: "gpt-5.6-terra",
    label: "gpt-5.6-terra",
    provider: "openai",
    reasoningEfforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
    defaultEffort: "medium",
    supportsFastMode: true,
  },
  {
    id: "gpt-5.6-luna",
    label: "gpt-5.6-luna",
    provider: "openai",
    reasoningEfforts: ["low", "medium", "high", "xhigh", "max"],
    defaultEffort: "medium",
    supportsFastMode: true,
  },
  {
    id: "gpt-5.5",
    label: "gpt-5.5",
    provider: "openai",
    reasoningEfforts: ["low", "medium", "high", "xhigh"],
    defaultEffort: "medium",
    supportsFastMode: false,
  },
];

export const AGENT_CAPABILITIES: AgentCapabilities = {
  platform: "macos",
  sandboxAvailable: true,
  message:
    "macOS Seatbelt sandbox available. Read-only and workspace-write modes deny network by default.",
};

export const AGENT_SESSION_CREDENTIAL_PROFILES: CredentialProfile[] = [
  {
    id: "audit-anthropic-profile",
    provider: "anthropic",
    name: "Anthropic work key",
    kind: "api_key",
    priority: 0,
    status: "active",
    lastValidatedAt: "2026-08-16T12:00:00.000Z",
    metadataJson: null,
    createdAt: "2026-08-16T12:00:00.000Z",
    updatedAt: "2026-08-16T12:00:00.000Z",
  },
  {
    id: "audit-openai-profile",
    provider: "openai",
    name: "API work key",
    kind: "api_key",
    priority: 0,
    status: "active",
    lastValidatedAt: "2026-08-16T12:00:00.000Z",
    metadataJson: null,
    createdAt: "2026-08-16T12:00:00.000Z",
    updatedAt: "2026-08-16T12:00:00.000Z",
  },
  {
    id: "audit-zai-profile",
    provider: "zai",
    name: "GLM Coding Plan work key",
    kind: "api_key",
    priority: 0,
    status: "active",
    lastValidatedAt: "2026-08-16T12:00:00.000Z",
    metadataJson: null,
    createdAt: "2026-08-16T12:00:00.000Z",
    updatedAt: "2026-08-16T12:00:00.000Z",
  },
  {
    id: "audit-moonshot-profile",
    provider: "moonshot",
    name: "Moonshot work key",
    kind: "api_key",
    priority: 0,
    status: "active",
    lastValidatedAt: "2026-08-16T12:00:00.000Z",
    metadataJson: null,
    createdAt: "2026-08-16T12:00:00.000Z",
    updatedAt: "2026-08-16T12:00:00.000Z",
  },
  {
    id: "audit-xai-profile",
    provider: "xai",
    name: "Grok work key",
    kind: "api_key",
    priority: 0,
    status: "active",
    lastValidatedAt: "2026-08-16T12:00:00.000Z",
    metadataJson: null,
    createdAt: "2026-08-16T12:00:00.000Z",
    updatedAt: "2026-08-16T12:00:00.000Z",
  },
];

export const AGENT_SESSION_LANE_OPTIONS: AgentSessionLaneOptions = [
  {
    agentKind: "claude",
    lanes: [
      { kind: "claudeSubscription", loggedIn: true, credentialProfiles: [] },
      {
        kind: "claudeApiKey",
        loggedIn: null,
        credentialProfiles: [AGENT_SESSION_CREDENTIAL_PROFILES[0]],
      },
      { kind: "custom", loggedIn: null, credentialProfiles: [] },
    ],
  },
  {
    agentKind: "codex",
    lanes: [
      { kind: "codexChatGpt", loggedIn: true, credentialProfiles: [] },
      { kind: "custom", loggedIn: null, credentialProfiles: [] },
    ],
  },
  {
    agentKind: "kimi",
    lanes: [
      { kind: "kimiSubscription", loggedIn: false, credentialProfiles: [] },
      {
        kind: "kimiApiKey",
        loggedIn: null,
        credentialProfiles: [AGENT_SESSION_CREDENTIAL_PROFILES[3]],
      },
      { kind: "custom", loggedIn: null, credentialProfiles: [] },
    ],
  },
  {
    agentKind: "zai",
    lanes: [
      {
        kind: "zaiGlm",
        loggedIn: null,
        credentialProfiles: [AGENT_SESSION_CREDENTIAL_PROFILES[2]],
      },
      { kind: "custom", loggedIn: null, credentialProfiles: [] },
    ],
  },
  {
    agentKind: "grok",
    lanes: [
      {
        kind: "grokApiKey",
        loggedIn: null,
        credentialProfiles: [AGENT_SESSION_CREDENTIAL_PROFILES[4]],
      },
      { kind: "custom", loggedIn: null, credentialProfiles: [] },
    ],
  },
];

export const AGENT_SESSION_PROBE: AgentSessionProbe = {
  claude: { found: true, version: "2.1.0" },
  codex: { found: true, version: "0.147.0" },
  kimi: { found: true, version: "1.2.3" },
  zai: { found: true, version: "2.1.0" },
  grok: { found: true, version: "2.1.0" },
  ghostty: { found: true, version: "1.3.0", applescriptReady: true },
};

export const MARKETING_PRO_SESSION_ID = "marketing-pro-claude";

export const MARKETING_PRO_AGENT_SESSION: AgentSession = {
  id: MARKETING_PRO_SESSION_ID,
  agentKind: "claude",
  laneKind: "claudeSubscription",
  mode: "terminal_embedded",
  projectId: 3,
  conversationId: "audit-marketing",
  conversationTitle: "Release readiness audit",
  model: "sonnet",
  cwd: "/Users/auditor/Projects/halv/web",
  pid: 4242,
  status: "running",
  createdAt: minutesAgo(8),
  lastActiveAt: minutesAgo(1),
};

export const MARKETING_PRO_TERMINAL_SESSION: TerminalSession = {
  id: MARKETING_PRO_SESSION_ID,
  projectId: 3,
  projectName: "halv-web",
  conversationId: MARKETING_PRO_AGENT_SESSION.conversationId,
  cwd: MARKETING_PRO_AGENT_SESSION.cwd,
  shell: "/bin/zsh",
  pid: 4242,
  executionProfile: "full_access",
  agentKind: "claude",
  status: "running",
  cols: 112,
  rows: 32,
  startedAt: MARKETING_PRO_AGENT_SESSION.createdAt,
};

export const MARKETING_PRO_AGENT_EVENTS: AgentSessionEvent[] = [
  {
    type: "sessionInit",
    cliSessionId: "claude-session-marketing",
    model: "Claude Sonnet 5",
  },
  { type: "turnStarted" },
  {
    type: "toolCallStarted",
    name: "Write",
    summaryJson: JSON.stringify({
      file_path: "/Users/auditor/Projects/design-system/tokens.ts",
    }),
  },
  { type: "toolCallCompleted", name: "Write" },
  {
    type: "turnCompleted",
    usage: {
      inputTokens: 4_860,
      outputTokens: 612,
      cacheReadTokens: 12_740,
      cacheCreationTokens: 1_420,
      totalCostUsd: 0.18,
      savingsTokens: 8_920,
      dollarsUsd: 0.02,
      savingsSource: "Headroom",
    },
  },
  { type: "turnStarted" },
];

export const MARKETING_PRO_TERMINAL_SCROLLBACK = [
  "\x1b[1;36mClaude Code\x1b[0m  \x1b[2mSonnet 5 · /Projects/halv/web\x1b[0m",
  "",
  "\x1b[1;36m❯\x1b[0m Review the release checklist and verify the build.",
  "",
  "\x1b[1m●\x1b[0m I’ll inspect the release scripts and run the focused checks.",
  "\x1b[36m● Read\x1b[0m(package.json, RELEASING.md)",
  "\x1b[32m✓\x1b[0m The release configuration is consistent. Running \x1b[36mpnpm build\x1b[0m now.",
  "",
  "\x1b[2m  Building production bundle…\x1b[0m",
].join("\r\n");

// Dollars use the methods of ADR 0015 since the upgrade day. Older
// savings keep their tokens only.
const DOLLARS_SINCE = "2026-08-06T09:00:00Z";

export const SAVINGS_TOTALS: SavingsTotals = {
  tokensSaved: 148_920,
  savingsUsd: 4.83,
  dollarsUsd: 1.86,
  legacyTokensSaved: 96_400,
  dollarsSince: DOLLARS_SINCE,
  sessionDollarsUsd: 0.09,
  pendingDollarsUsd: 0,
  priceVersion: "2026-10-15",
  localTokensSaved: 148_920,
  localSavingsUsd: 4.83,
  localDollarsUsd: 1.86,
  pendingTokensSaved: 0,
  pendingSavingsUsd: 0,
  sessionTokensSaved: 18_460,
  sessionSavingsUsd: 0.71,
  cruxTokensSaved: 12_400,
};

export const SAVINGS_TODAY: SavingsToday = {
  tokensSaved: 31_740,
  savingsUsd: 1.18,
  recentSavings: [1_260, 1_980, 2_340, 2_840, 3_150, 4_270, 5_900, 10_000],
  dollarsUsd: 0.12,
  recentDollars: [0.004, 0.007, 0.006, 0.011, 0.009, 0.016, 0.021, 0.046],
};

export const ZERO_SAVINGS_TOTALS: SavingsTotals = {
  ...SAVINGS_TOTALS,
  tokensSaved: 0,
  savingsUsd: 0,
  dollarsUsd: 0,
  legacyTokensSaved: 0,
  sessionDollarsUsd: 0,
  localTokensSaved: 0,
  localSavingsUsd: 0,
  localDollarsUsd: 0,
  sessionTokensSaved: 0,
};

export const ZERO_SAVINGS_TODAY: SavingsToday = {
  tokensSaved: 0,
  savingsUsd: 0,
  recentSavings: [],
  dollarsUsd: 0,
  recentDollars: [],
};

export const SAVINGS_WEEK: SavingsToday = {
  tokensSaved: 102_480,
  savingsUsd: 3.42,
  recentSavings: SAVINGS_TODAY.recentSavings,
  dollarsUsd: 0.42,
  recentDollars: SAVINGS_TODAY.recentDollars,
};

export const DAILY_TOKEN_ACTIVITY: DailyTokenActivity[] = Array.from(
  { length: 120 },
  (_, index) => {
    const date = new Date(2026, 7, 13, 12, 0, 0, 0);
    date.setDate(date.getDate() - (119 - index));
    const weekdayFactor = date.getDay() === 0 ? 0.38 : date.getDay() === 6 ? 0.56 : 1;
    const rhythm = 0.72 + ((index * 17) % 31) / 50;
    const inputTokens = Math.round((7_200 + index * 58) * weekdayFactor * rhythm);
    const outputTokens = Math.round((1_450 + index * 13) * weekdayFactor * rhythm);
    const tokensSaved = Math.round(inputTokens * (0.24 + (index % 7) * 0.035));
    // The last eight days come after the dollar metric started.
    const dollarsUsd =
      index >= 112 ? Math.round(tokensSaved * 0.000_021 * 10_000) / 10_000 : 0;

    return {
      date: `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`,
      inputTokens,
      outputTokens,
      tokensSaved,
      dollarsUsd,
    };
  },
);

const AGGRESSIVE_SIDECAR_STATS: SidecarStats = {
  ...SIDECAR_STATS,
  tokensSaved: 2_843_700,
  costSavedUsd: 91.42,
  requestsTotal: 4_812,
  requestsByProvider: { openai: 4_520, anthropic: 292 },
  usageByLane: [
    {
      lane: "codex_chatgpt",
      billing: "plan",
      provider: "openai",
      requests: 4_520,
      inputTokens: 3_490_000,
      outputTokens: null,
      costUsd: null,
      tokensSaved: 2_550_000,
    },
    {
      lane: "claude_subscription",
      billing: "plan",
      provider: "claude",
      requests: 292,
      inputTokens: 225_000,
      outputTokens: null,
      costUsd: null,
      tokensSaved: 293_700,
    },
  ],
  requestsCached: 1_284,
  requestsFailed: 7,
  sessionTokensSaved: 86_300,
  sessionCostSavedUsd: 2.78,
  inputTokens: 3_715_000,
  processInputTokens: 3_715_000,
  outputTokens: 682_400,
  proxyCompressionTokensSaved: 2_034_500,
  compressionTokensSaved: 2_034_500,
  cliFilteringTokensSaved: 809_200,
  rtkTokensSaved: 809_200,
  estimatedOutputTokensSaved: 44_800,
  liveWsTokensSaved: 2_034_500,
  cacheSavingsUsd: 18.36,
  costWithHeadroomUsd: 44.17,
  savingsPercent: 67.4,
};

const AGGRESSIVE_SAVINGS_TOTALS: SavingsTotals = {
  tokensSaved: 2_843_700,
  savingsUsd: 91.42,
  dollarsUsd: 27.6,
  legacyTokensSaved: 1_240_000,
  dollarsSince: DOLLARS_SINCE,
  sessionDollarsUsd: 0.86,
  pendingDollarsUsd: 0,
  priceVersion: "2026-10-15",
  localTokensSaved: 2_843_700,
  localSavingsUsd: 91.42,
  localDollarsUsd: 27.6,
  pendingTokensSaved: 0,
  pendingSavingsUsd: 0,
  sessionTokensSaved: 86_300,
  sessionSavingsUsd: 2.78,
  cruxTokensSaved: 184_600,
};

const AGGRESSIVE_SAVINGS_TODAY: SavingsToday = {
  tokensSaved: 612_400,
  savingsUsd: 19.74,
  recentSavings: [
    30_200,
    41_800,
    58_900,
    77_600,
    95_300,
    82_600,
    101_400,
    124_600,
  ],
  dollarsUsd: 3.81,
  recentDollars: [0.19, 0.26, 0.37, 0.48, 0.59, 0.51, 0.63, 0.78],
};

const AGGRESSIVE_SAVINGS_WEEK: SavingsToday = {
  tokensSaved: 1_925_300,
  savingsUsd: 62.08,
  recentSavings: AGGRESSIVE_SAVINGS_TODAY.recentSavings,
  dollarsUsd: 12.34,
  recentDollars: AGGRESSIVE_SAVINGS_TODAY.recentDollars,
};

export type HistoricalToolFixture = {
  call: ChatDeltaPayload;
  output: ChatDeltaPayload;
  result: ChatDeltaPayload;
};

export const HISTORICAL_REASONING = [
  "**Audit plan:** compare the IPC contract with the component call sites.",
  "- Preserve production behavior\n- Exercise error and success states with real Markdown rendering",
].join("\n\n");

export const HISTORICAL_TOOLS: HistoricalToolFixture[] = [
  historicalTool("history-tool-1", "rg -n \"invoke\\(\" src", "src/App.tsx:155: invoke<AccountAuthStatus>(\"auth_status\")\n", 0, 84),
  historicalTool("history-tool-2", "pnpm exec tsc --noEmit", "TypeScript compilation completed successfully.\n", 0, 1_420),
  historicalTool("history-tool-3", "pnpm test -- --runInBand", "ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL Command \"test\" not found\n", 1, 211),
  historicalTool("history-tool-4", "git diff --check", "", 0, 39),
  historicalTool("history-tool-5", "pnpm build", "✓ built in 1.34s\n", 0, 2_104),
  historicalTool("history-tool-6", "git status --short", " M src/App.tsx\n?? src/harness/\n", 0, 47),
];

export type HarnessFixtureState = {
  scenario: HarnessScenario;
  fixture: HarnessFixtureConfig;
  account: AccountAuthStatus;
  auth: AuthStatus;
  sidecar: SidecarStatus;
  sidecarStats: SidecarStats;
  savingsTotals: SavingsTotals;
  savingsToday: SavingsToday;
  savingsWeek: SavingsToday;
  conversations: ConversationSummary[];
  messages: Record<string, StoredMessage[]>;
  projects: Project[];
  // Managed worktrees. Each project also has its primary workspace.
  worktrees: WorkspaceStatus[];
  turnDiffs: Record<string, TurnDiffSummary[]>;
  nextConversation: number;
  nextProject: number;
};

export function createFixtureState(
  fixture: HarnessFixtureConfig,
): HarnessFixtureState {
  const historyEnabled = fixture.history === "full";
  const marketingHistory =
    fixture.history === "marketing" || fixture.history === "marketing-pro";
  const aggressiveSavings = fixture.savings === "aggressive";
  // A new install has no saving yet (ADR 0018): the trial card must not
  // open before the first result.
  const noSavings = fixture.savings === "none";
  const projects =
    fixture.projects === "none"
      ? []
      : fixture.marketingWorkspace
        ? createMarketingWorkspaceProjects()
        : createProjects(marketingHistory);
  if (isMarketingRealismFixture(fixture) && !fixture.marketingWorkspace) {
    const primaryProject = projects.find((project) =>
      marketingHistory ? project.id === 3 : project.id === 1,
    );
    if (primaryProject) {
      primaryProject.name = "halv";
      primaryProject.rootPath = "/Users/demo/programming/halv";
    }
  }
  const sidecarStats = clone(
    aggressiveSavings ? AGGRESSIVE_SIDECAR_STATS : SIDECAR_STATS,
  );
  if (fixture.apiRequests === "none") {
    sidecarStats.requestsByProvider = {};
  }
  const worktrees = fixture.marketingWorkspace
    ? createMarketingWorkspaceWorktrees()
    : fixture.worktrees
      ? createProjectTreeWorktrees(projects)
      : [];
  return {
    scenario: fixture.scenario,
    fixture,
    account: accountFixture(fixture.account, aggressiveSavings),
    auth: credentialFixture(fixture.credentials),
    sidecar: sidecarFixture(fixture.sidecar),
    sidecarStats,
    savingsTotals: clone(
      noSavings
        ? ZERO_SAVINGS_TOTALS
        : aggressiveSavings
          ? AGGRESSIVE_SAVINGS_TOTALS
          : SAVINGS_TOTALS,
    ),
    savingsToday: clone(
      noSavings
        ? ZERO_SAVINGS_TODAY
        : aggressiveSavings
          ? AGGRESSIVE_SAVINGS_TODAY
          : SAVINGS_TODAY,
    ),
    savingsWeek: clone(
      noSavings
        ? ZERO_SAVINGS_TODAY
        : aggressiveSavings
          ? AGGRESSIVE_SAVINGS_WEEK
          : SAVINGS_WEEK,
    ),
    conversations: withFixtureWorktrees(
      historyEnabled || marketingHistory
        ? createConversations(marketingHistory, projects).map((conversation) =>
            fixture.history === "marketing-pro" &&
            conversation.id === "audit-marketing"
              ? { ...conversation, mode: "agent" as const }
              : conversation,
          )
        : [],
      worktrees,
      fixture.parkedConversation === true,
      fixture.glyphShowcase === true,
    ),
    messages:
      historyEnabled || marketingHistory ? createMessages(marketingHistory) : {},
    projects,
    worktrees,
    turnDiffs: historyEnabled ? createTurnDiffs() : {},
    nextConversation: 1,
    nextProject: marketingHistory ? 4 : 3,
    ...(fixture.marketingProSplit
      ? {
          sidecarStats: {
            ...sidecarStats,
            tokensSaved: MARKETING_ALL_TIME_TOKENS,
            costSavedUsd: MARKETING_ALL_TIME_USD,
            lifetimeTokensSaved: MARKETING_ALL_TIME_TOKENS,
            sessionTokensSaved: MARKETING_SESSION_TOKENS,
            sessionCostSavedUsd: 1.44,
            planWindows: MARKETING_PRO_SPLIT_PLAN_WINDOWS,
          },
          savingsTotals: {
            tokensSaved: MARKETING_ALL_TIME_TOKENS,
            savingsUsd: MARKETING_ALL_TIME_USD,
            localTokensSaved: MARKETING_ALL_TIME_TOKENS,
            localSavingsUsd: MARKETING_ALL_TIME_USD,
            pendingTokensSaved: 0,
            pendingSavingsUsd: 0,
            sessionTokensSaved: MARKETING_SESSION_TOKENS,
            sessionSavingsUsd: 1.44,
            cruxTokensSaved: 4_500_000,
            dollarsUsd: MARKETING_ALL_TIME_DOLLARS,
            localDollarsUsd: MARKETING_ALL_TIME_DOLLARS,
            legacyTokensSaved: 223_317,
            dollarsSince: "2026-08-07T09:00:00Z",
            sessionDollarsUsd: MARKETING_SESSION_DOLLARS,
            pendingDollarsUsd: 0,
            priceVersion: "2026-10-15",
          },
          savingsToday: {
            tokensSaved: MARKETING_RECENT_SAVINGS.at(-1) ?? 0,
            savingsUsd: 1.36,
            recentSavings: [...MARKETING_RECENT_SAVINGS],
            dollarsUsd: MARKETING_TODAY_DOLLARS,
            recentDollars: [...MARKETING_RECENT_DOLLARS],
          },
          savingsWeek: {
            tokensSaved: MARKETING_WEEK_TOKENS,
            savingsUsd: MARKETING_ALL_TIME_USD,
            recentSavings: [...MARKETING_RECENT_SAVINGS],
            dollarsUsd: MARKETING_WEEK_DOLLARS,
            recentDollars: [...MARKETING_RECENT_DOLLARS],
          },
        }
      : {}),
    ...(fixture.marketingSubagents
      ? {
          messages: {
            ...createMessages(marketingHistory),
            [MARKETING_SUBAGENT_CONVERSATION_ID]: clone(
              MARKETING_SUBAGENT_MESSAGES,
            ),
          },
        }
      : {}),
    ...(fixture.marketingSavings
      ? {
          sidecarStats: {
            ...sidecarStats,
            tokensSaved: MARKETING_ALL_TIME_TOKENS,
            costSavedUsd: MARKETING_ALL_TIME_USD,
            lifetimeTokensSaved: MARKETING_ALL_TIME_TOKENS,
            sessionTokensSaved: MARKETING_SESSION_TOKENS,
            sessionCostSavedUsd: 1.44,
            rtkTokensSaved: MARKETING_ENGINE_SAVINGS.rtk,
            leanCtxTokensSaved: MARKETING_ENGINE_SAVINGS.leanContext,
            toolSearchTokensSaved: MARKETING_ENGINE_SAVINGS.toolSearch,
            compressionTokensSaved: MARKETING_ENGINE_SAVINGS.compression,
            planWindows: [],
          },
          savingsTotals: {
            tokensSaved: MARKETING_ALL_TIME_TOKENS,
            savingsUsd: MARKETING_ALL_TIME_USD,
            localTokensSaved: MARKETING_ALL_TIME_TOKENS,
            localSavingsUsd: MARKETING_ALL_TIME_USD,
            pendingTokensSaved: 0,
            pendingSavingsUsd: 0,
            sessionTokensSaved: MARKETING_SESSION_TOKENS,
            sessionSavingsUsd: 1.44,
            cruxTokensSaved: MARKETING_ENGINE_SAVINGS.crux,
            dollarsUsd: MARKETING_ALL_TIME_DOLLARS,
            localDollarsUsd: MARKETING_ALL_TIME_DOLLARS,
            legacyTokensSaved: 223_317,
            dollarsSince: "2026-08-07T09:00:00Z",
            sessionDollarsUsd: MARKETING_SESSION_DOLLARS,
            pendingDollarsUsd: 0,
            priceVersion: "2026-10-15",
          },
          savingsToday: {
            tokensSaved: 4_537_378,
            savingsUsd: 144.9,
            recentSavings: [...MARKETING_RECENT_SAVINGS],
            dollarsUsd: 10.2,
            recentDollars: [...MARKETING_RECENT_DOLLARS],
          },
          savingsWeek: {
            tokensSaved: MARKETING_WEEK_TOKENS,
            savingsUsd: MARKETING_ALL_TIME_USD,
            recentSavings: [...MARKETING_RECENT_SAVINGS],
            dollarsUsd: MARKETING_WEEK_DOLLARS,
            recentDollars: [...MARKETING_RECENT_DOLLARS],
          },
        }
      : {}),
    ...(isMarketingRealismFixture(fixture)
      ? {
          sidecarStats: {
            ...sidecarStats,
            tokensSaved: MARKETING_ALL_TIME_TOKENS,
            costSavedUsd: MARKETING_ALL_TIME_USD,
            lifetimeTokensSaved: MARKETING_ALL_TIME_TOKENS,
            sessionTokensSaved: MARKETING_SESSION_TOKENS,
            sessionCostSavedUsd: 1.44,
            rtkTokensSaved: MARKETING_ENGINE_SAVINGS.rtk,
            leanCtxTokensSaved: MARKETING_ENGINE_SAVINGS.leanContext,
            toolSearchTokensSaved: MARKETING_ENGINE_SAVINGS.toolSearch,
            compressionTokensSaved: MARKETING_ENGINE_SAVINGS.compression,
            planWindows: [],
          },
          savingsTotals: {
            tokensSaved: MARKETING_ALL_TIME_TOKENS,
            savingsUsd: MARKETING_ALL_TIME_USD,
            localTokensSaved: MARKETING_ALL_TIME_TOKENS,
            localSavingsUsd: MARKETING_ALL_TIME_USD,
            pendingTokensSaved: 0,
            pendingSavingsUsd: 0,
            sessionTokensSaved: MARKETING_SESSION_TOKENS,
            sessionSavingsUsd: 1.44,
            cruxTokensSaved: MARKETING_ENGINE_SAVINGS.crux,
            dollarsUsd: MARKETING_ALL_TIME_DOLLARS,
            localDollarsUsd: MARKETING_ALL_TIME_DOLLARS,
            legacyTokensSaved: 223_317,
            dollarsSince: "2026-08-07T09:00:00Z",
            sessionDollarsUsd: MARKETING_SESSION_DOLLARS,
            pendingDollarsUsd: 0,
            priceVersion: "2026-10-15",
          },
          savingsToday: {
            tokensSaved: MARKETING_RECENT_SAVINGS.at(-1) ?? 0,
            savingsUsd: 1.36,
            recentSavings: [...MARKETING_RECENT_SAVINGS],
            dollarsUsd: MARKETING_TODAY_DOLLARS,
            recentDollars: [...MARKETING_RECENT_DOLLARS],
          },
          savingsWeek: {
            tokensSaved: MARKETING_WEEK_TOKENS,
            savingsUsd: MARKETING_ALL_TIME_USD,
            recentSavings: [...MARKETING_RECENT_SAVINGS],
            dollarsUsd: MARKETING_WEEK_DOLLARS,
            recentDollars: [...MARKETING_RECENT_DOLLARS],
          },
        }
      : {}),
    ...(fixture.marketingWorkspace
      ? {
          conversations: createMarketingWorkspaceConversations(projects),
          messages: {},
          turnDiffs: {},
          nextProject: projects.length + 1,
        }
      : {}),
  };
}

function accountFixture(
  fixture: HarnessFixtureConfig["account"],
  aggressiveSavings: boolean,
) {
  if (fixture === "paywall") {
    return unsubscribedAccount();
  }
  if (fixture === "signed-out") {
    return signedOutAccount(5);
  }

  const account = signedInAccount();
  if (!aggressiveSavings || !account.entitlements) {
    return account;
  }
  return {
    ...account,
    plan: "unlimited" as const,
    entitlements: {
      ...account.entitlements,
      plan: "unlimited" as const,
      savings: {
        ...account.entitlements.savings,
        tokensSaved: AGGRESSIVE_SAVINGS_TOTALS.tokensSaved,
        capTokens: null,
        allTimeTokensSaved: AGGRESSIVE_SAVINGS_TOTALS.tokensSaved,
        allTimeSavingsUsd: AGGRESSIVE_SAVINGS_TOTALS.savingsUsd,
      },
    },
  };
}

function credentialFixture(
  fixture: HarnessFixtureConfig["credentials"],
): AuthStatus {
  switch (fixture) {
    case "chatgpt":
      return chatGptAuth();
    case "codex":
      return chatGptAuth("audit.codex@example.com", "codex-cli");
    case "api-key":
      return { apiKey: { masked: "sk-...audit" } };
    case "none":
      return { signedOut: {} };
  }
}

function sidecarFixture(fixture: HarnessFixtureConfig["sidecar"]): SidecarStatus {
  switch (fixture) {
    case "running":
      return { ...RUNNING_SIDECAR };
    case "missing":
      return {
        state: "notInstalled",
        installHint:
          "This Halv build is missing its bundled proxy. Reinstall Halv with an official platform installer.",
        port: 8787,
      };
    case "error":
      return { state: "error", message: PAUSED_RESTARTS_MESSAGE, port: 8787 };
    case "off":
      return { state: "off", port: 8787 };
  }
}

export function signedInAccount(
  email = "audit.basic@example.com",
  freeTurnsUsed = 0,
  firstLaunchAt = "2026-08-01T12:00:00.000Z",
): AccountAuthStatus {
  return {
    signedIn: true,
    email,
    plan: "basic",
    serverUrl: "https://api.halv.ai",
    entitlements: {
      beta: false,
      billingRequired: true,
      canUseApp: true,
      plan: "basic",
      message: null,
      checkoutUrl: null,
      billingPortalUrl: "https://halv.ai/account/billing",
      savings: {
        period: new Date().toISOString().slice(0, 7),
        tokensSaved: 148_920,
        capTokens: 500_000,
        exhausted: false,
        allTimeTokensSaved: SAVINGS_TOTALS.tokensSaved,
        allTimeSavingsUsd: SAVINGS_TOTALS.savingsUsd,
      },
    },
    syncedAt: new Date().toISOString(),
    firstLaunchAt,
    freeTurnsUsed,
    stale: false,
  };
}

export function unsubscribedAccount(
  email = "audit.trial@example.com",
  freeTurnsUsed = 5,
  firstLaunchAt = "2026-08-01T12:00:00.000Z",
): AccountAuthStatus {
  return {
    signedIn: true,
    email,
    plan: "beta_free",
    serverUrl: "https://api.halv.ai",
    entitlements: {
      beta: false,
      billingRequired: true,
      canUseApp: false,
      plan: "beta_free",
      message: "Choose a plan to keep using Halv.",
      checkoutUrl: "https://halv.ai",
      billingPortalUrl: "https://halv.ai/account/billing",
      savings: {
        period: new Date().toISOString().slice(0, 7),
        tokensSaved: 0,
        capTokens: null,
        exhausted: false,
      },
    },
    syncedAt: new Date().toISOString(),
    firstLaunchAt,
    freeTurnsUsed,
    stale: false,
  };
}

export function signedOutAccount(
  freeTurnsUsed = 0,
  firstLaunchAt = "2026-08-01T12:00:00.000Z",
): AccountAuthStatus {
  return {
    signedIn: false,
    email: null,
    plan: null,
    serverUrl: "https://api.halv.ai",
    entitlements: null,
    syncedAt: null,
    firstLaunchAt,
    freeTurnsUsed,
    stale: false,
  };
}

export function chatGptAuth(
  email = "audit.pro@example.com",
  source: "app" | "codex-cli" = "app",
): AuthStatus {
  return {
    chatgpt: {
      source,
      email,
      planType: "pro",
      accountId: "acct_ux_audit_pro",
      accessTokenValid: true,
    },
  };
}

export function createConversation(
  state: HarnessFixtureState,
  options: {
    id?: string;
    model?: string;
    mode?: ConversationSummary["mode"];
    projectId?: number | null;
    title?: string;
    titleSource?: ConversationSummary["titleSource"];
    agentMode?: boolean;
    approvalPolicyOverride?: ConversationSummary["approvalPolicyOverride"];
    workflowMode?: "default" | "plan";
    fastMode?: boolean;
    draft?: string;
    draftOnly?: boolean;
    workspaceId?: string | null;
  } = {},
): ConversationSummary {
  const id = options.id ?? `audit-live-${state.nextConversation++}`;
  const project = state.projects.find((item) => item.id === options.projectId);
  // Like storage: a known worktree of the project, else the primary one.
  const worktree = state.worktrees.find(
    (item) => item.id === options.workspaceId && item.projectId === project?.id,
  );
  const now = new Date().toISOString();
  const conversation: ConversationSummary = {
    id,
    title: options.title ?? "New audit conversation",
    titleSource: options.titleSource ?? "default",
    mode: options.mode ?? "chat",
    model: options.model ?? "gpt-5.6-sol",
    createdAt: now,
    updatedAt: now,
    pinned: false,
    draft: options.draft ?? "",
    draftOnly: options.draftOnly ?? false,
    archivedAt: null,
    memoriesMode: null,
    fastMode: options.fastMode ?? false,
    goal: null,
    agentMode: options.agentMode ?? false,
    workflowMode: options.workflowMode ?? "default",
    credentialProfileId: null,
    agentBackend: null,
    agentSessionModel: null,
    projectId: project?.id ?? null,
    projectName: project?.name ?? null,
    projectRootPath: worktree?.rootPath ?? project?.rootPath ?? null,
    projectTrustLevel: project?.trustLevel ?? null,
    approvalPolicyOverride: options.approvalPolicyOverride ?? null,
    messageCount: 0,
    workspaceId: worktree?.id ?? (project ? `primary-${project.id}` : null),
  };
  state.conversations.unshift(conversation);
  state.messages[id] = [];
  state.turnDiffs[id] = [];
  return conversation;
}

export function syncConversationSummary(
  state: HarnessFixtureState,
  conversationId: string,
) {
  const conversation = state.conversations.find((item) => item.id === conversationId);
  if (!conversation) {
    return;
  }
  const rows = state.messages[conversationId] ?? [];
  conversation.messageCount = rows.length;
  if (rows.length > 0) {
    conversation.draft = "";
    conversation.draftOnly = false;
  }
  conversation.updatedAt =
    rows[rows.length - 1]?.createdAt ?? new Date().toISOString();
  const firstUser = rows.find((message) => message.role === "user")?.content.trim();
  if (conversation.title === "New audit conversation" && firstUser) {
    conversation.title = firstUser.length > 46 ? `${firstUser.slice(0, 43)}...` : firstUser;
  }
}

function withFixtureWorktrees(
  conversations: ConversationSummary[],
  worktrees: WorkspaceStatus[],
  parkedConversation = false,
  glyphShowcase = false,
) {
  if (worktrees.length > 0) applyProjectTreeConversations(conversations, worktrees);
  if (parkedConversation) addParkedConversation(conversations);
  if (glyphShowcase) addGlyphShowcaseConversations(conversations);
  return conversations;
}

export function updateConversationProjectFields(
  conversation: ConversationSummary,
  project: Project | undefined,
) {
  conversation.workspaceId = project ? `primary-${project.id}` : null;
  conversation.projectId = project?.id ?? null;
  conversation.projectName = project?.name ?? null;
  conversation.projectRootPath = project?.rootPath ?? null;
  conversation.projectTrustLevel = project?.trustLevel ?? null;
}

function createProjects(marketing = false): Project[] {
  const projects: Project[] = [
    {
      id: 1,
      name: "Halv Desktop",
      rootPath: "/Users/auditor/Projects/halv",
      trustLevel: "trusted",
      approvalPolicy: "ask",
      createdAt: minutesAgo(8_000),
      lastOpenedAt: minutesAgo(4),
    },
    {
      id: 2,
      name: "Design System",
      rootPath: "/Users/auditor/Projects/design-system",
      trustLevel: "untrusted",
      approvalPolicy: "read_only",
      createdAt: minutesAgo(4_000),
      lastOpenedAt: minutesAgo(190),
    },
  ];
  if (marketing) {
    projects.push({
      id: 3,
      name: "halv-web",
      rootPath: "/Users/auditor/Projects/halv/web",
      trustLevel: "trusted",
      approvalPolicy: "ask",
      createdAt: minutesAgo(3_200),
      lastOpenedAt: minutesAgo(2),
    });
  }
  return projects;
}

function createConversations(
  marketing: boolean,
  projects: Project[],
): ConversationSummary[] {
  if (marketing) {
    const marketingConversation = conversation(
      "audit-marketing",
      "Vite multi-page design system",
      "sonnet",
      4,
      false,
      3,
      2,
      projects,
    );
    marketingConversation.agentBackend = "claude_session";
    marketingConversation.agentSessionModel = "sonnet";
    return [
      marketingConversation,
      conversation(
        "audit-agent-refactor",
        "Audit agent refactor",
        "gpt-5.6-sol",
        92,
        true,
        1,
        2,
        projects,
      ),
      conversation(
        "audit-release",
        "Release checklist",
        "gpt-5.6-terra",
        340,
        false,
        2,
        2,
        projects,
      ),
    ];
  }
  return [
    conversation(HISTORICAL_AGENT_CONVERSATION_ID, "Audit agent refactor", "gpt-5.6-sol", 7, true, 1, 2, projects),
    conversation("audit-markdown", "Streaming markdown polish", "gpt-5.6-terra", 24, false, null, 2, projects),
    conversation("audit-proxy", "Proxy restart investigation", "gpt-5.6-luna", 58, true, 1, 2, projects),
    conversation("audit-usage", "ChatGPT usage limit", "gpt-5.5", 180, false, null, 2, projects),
    conversation("audit-onboarding", "Onboarding copy ideas", "gpt-5.6-terra", 420, false, null, 2, projects),
    conversation("audit-release", "Release checklist", "gpt-5.6-sol", 1_440, true, 2, 2, projects),
    conversation("audit-api", "Model capability notes", "gpt-5.5", 2_880, false, null, 2, projects),
  ];
}

function conversation(
  id: string,
  title: string,
  model: string,
  ageMinutes: number,
  agentMode: boolean,
  projectId: number | null,
  messageCount: number,
  projects: Project[],
): ConversationSummary {
  const project = projects.find((item) => item.id === projectId);
  return {
    id,
    title,
    titleSource: "generated",
    mode: "chat",
    model,
    createdAt: minutesAgo(ageMinutes + 160),
    updatedAt: minutesAgo(ageMinutes),
    pinned: false,
    draft: "",
    draftOnly: false,
    archivedAt: null,
    memoriesMode: null,
    fastMode: false,
    goal: id === HISTORICAL_AGENT_CONVERSATION_ID ? "Ship a browser UX audit harness" : null,
    agentMode,
    workflowMode: "default",
    credentialProfileId: null,
    agentBackend: null,
    agentSessionModel: null,
    projectId,
    projectName: project?.name ?? null,
    projectRootPath: project?.rootPath ?? null,
    projectTrustLevel: project?.trustLevel ?? null,
    approvalPolicyOverride: agentMode ? "ask" : null,
    messageCount,
    workspaceId: projectId != null ? `primary-${projectId}` : null,
  };
}

function createMessages(marketing: boolean): Record<string, StoredMessage[]> {
  if (marketing) {
    return {
      "audit-marketing": [
        message(
          "audit-marketing",
          "marketing-user-1",
          "user",
          "How should I structure the Vite config for a multi-page app with a shared design system?",
          6,
        ),
        message(
          "audit-marketing",
          "marketing-assistant-1",
          "assistant",
          [
            "Use explicit HTML entries and keep shared code behind one alias:",
            "",
            "```ts",
            "import { defineConfig } from \"vite\";",
            "import { resolve } from \"node:path\";",
            "",
            "export default defineConfig({",
            "  resolve: { alias: { \"@design-system\": resolve(__dirname, \"src/design-system\") } },",
            "  build: { rollupOptions: { input: {",
            "    app: resolve(__dirname, \"index.html\"),",
            "    admin: resolve(__dirname, \"admin.html\"),",
            "    docs: resolve(__dirname, \"docs.html\"),",
            "  } } },",
            "});",
            "```",
            "",
            "- Shared chunks stay stable because Rollup extracts modules imported by multiple pages.",
            "- The `@design-system` alias gives every entry the same component and token imports.",
            "- The dev server serves each HTML file directly, with normal HMR across shared modules.",
          ].join("\n"),
          4,
          {
            model: "sonnet",
            inputTokens: 2_410,
            outputTokens: 768,
            tokensSaved: 3_830,
            savingsUsd: 0.12,
          },
        ),
      ],
    };
  }
  return {
    [HISTORICAL_AGENT_CONVERSATION_ID]: [
      message(HISTORICAL_AGENT_CONVERSATION_ID, "agent-user-1", "user", "Audit the real React UI and keep the production entry untouched.", 16, { mentions: ["src/App.tsx", "src/types.ts"] }),
      message(
        HISTORICAL_AGENT_CONVERSATION_ID,
        "agent-assistant-1",
        "assistant",
        [
          "## Audit harness ready",
          "",
          "The UI now runs against a typed browser IPC router. The important invariant is:",
          "",
          "```ts",
          "installUxAuditHarness();",
          "await import(\"../main\");",
          "```",
          "",
          "| Surface | Result |",
          "| --- | --- |",
          "| React tree | Real `App` |",
          "| Rust IPC | Deterministic fixture router |",
          "| Events | Tauri-compatible listen/emit path |",
          "",
          "> Production startup remains unchanged.",
        ].join("\n"),
        12,
        { inputTokens: 5_420, outputTokens: 1_184, tokensSaved: 8_930, savingsUsd: 0.29 },
      ),
    ],
    "audit-markdown": [
      message("audit-markdown", "markdown-user-1", "user", "Show me a concise Vite multi-page entry example.", 31),
      message(
        "audit-markdown",
        "markdown-assistant-1",
        "assistant",
        "Use a root HTML entry in development:\n\n```html\n<script type=\"module\" src=\"/src/harness/entry.ts\"></script>\n```\n\n- Vite serves it directly.\n- The default production build still starts from `index.html`.",
        24,
        { inputTokens: 1_920, outputTokens: 410, tokensSaved: 2_870, savingsUsd: 0.08 },
      ),
    ],
    "audit-proxy": [
      message("audit-proxy", "proxy-user-1", "user", "Why did the managed proxy stop restarting?", 62),
      message("audit-proxy", "proxy-assistant-1", "assistant", "The restart budget was exhausted inside its rolling safety window. Use **Restart** once the underlying process issue is resolved.", 58, { inputTokens: 2_110, outputTokens: 322, tokensSaved: 1_740, savingsUsd: 0.05 }),
    ],
    "audit-usage": [
      message("audit-usage", "usage-user-1", "user", "Continue the long repository audit.", 182),
      message("audit-usage", "usage-error-1", "assistant", USAGE_LIMIT_MESSAGE, 180, { error: true }),
    ],
    "audit-onboarding": [
      message("audit-onboarding", "onboarding-user-1", "user", "What should a first-run checklist prioritize?", 425),
      message("audit-onboarding", "onboarding-assistant-1", "assistant", "Prioritize live prerequisites in order: **the proxy**, **the provider connection**, an optional project, then the first prompt.", 420, { inputTokens: 940, outputTokens: 188, tokensSaved: 1_120, savingsUsd: 0.03 }),
    ],
    "audit-release": [
      message("audit-release", "release-user-1", "user", "Prepare the release verification checklist.", 1_448),
      message("audit-release", "release-assistant-1", "assistant", "1. Build the frontend.\n2. Verify the updater reports no pending release.\n3. Inspect the dirty tree.\n4. Leave publishing to the release owner.", 1_440, { inputTokens: 3_410, outputTokens: 564, tokensSaved: 4_980, savingsUsd: 0.14 }),
    ],
    "audit-api": [
      message("audit-api", "api-user-1", "user", "Which reasoning levels does the picker use?", 2_886),
      message("audit-api", "api-assistant-1", "assistant", "The picker reads each model's `reasoningEfforts` and chooses its `defaultEffort`; it does not assume one global capability list.", 2_880, { inputTokens: 1_208, outputTokens: 246, tokensSaved: 980, savingsUsd: 0.03 }),
    ],
  };
}

function createTurnDiffs(): Record<string, TurnDiffSummary[]> {
  return {
    [HISTORICAL_AGENT_CONVERSATION_ID]: [
      {
        turnId: "audit-turn-entry",
        cwd: "/Users/auditor/Projects/halv",
        createdAt: minutesAgo(13),
        revertible: true,
        diff: [
          "diff --git a/ux-audit.html b/ux-audit.html",
          "new file mode 100644",
          "--- /dev/null",
          "+++ b/ux-audit.html",
          "@@ -0,0 +1,3 @@",
          "+<!doctype html>",
          "+<div id=\"root\"></div>",
          "+<script type=\"module\" src=\"/src/harness/entry.ts\"></script>",
        ].join("\n"),
      },
      {
        turnId: "audit-turn-router",
        cwd: "/Users/auditor/Projects/halv",
        createdAt: minutesAgo(12),
        revertible: false,
        diff: "diff --git a/src/harness/mock-ipc.ts b/src/harness/mock-ipc.ts\nnew file mode 100644\n--- /dev/null\n+++ b/src/harness/mock-ipc.ts\n@@ -0,0 +1,2 @@\n+export function installUxAuditHarness() {\n+  // Tauri-compatible browser router\n+}",
      },
    ],
  };
}

function historicalTool(
  callId: string,
  command: string,
  tail: string,
  exitCode: number,
  wallTimeMs: number,
): HistoricalToolFixture {
  return {
    call: {
      conversationId: HISTORICAL_AGENT_CONVERSATION_ID,
      kind: "tool_call",
      callId,
      command,
      toolKind: "shell",
      cwd: "/Users/auditor/Projects/halv",
      status: "running",
    },
    output: {
      conversationId: HISTORICAL_AGENT_CONVERSATION_ID,
      kind: "tool_output",
      callId,
      tail,
    },
    result: {
      conversationId: HISTORICAL_AGENT_CONVERSATION_ID,
      kind: "tool_result",
      callId,
      status: exitCode === 0 ? "completed" : "failed",
      exitCode,
      wallTimeMs,
      truncatedOutput: false,
    },
  };
}

function message(
  conversationId: string,
  id: string,
  role: StoredMessage["role"],
  content: string,
  ageMinutes: number,
  options: Partial<StoredMessage> = {},
): StoredMessage {
  return {
    id,
    conversationId,
    role,
    content,
    createdAt: minutesAgo(ageMinutes),
    model: role === "assistant" ? "gpt-5.6-sol" : null,
    inputTokens: null,
    outputTokens: null,
    error: false,
    mentions: null,
    tokensSaved: null,
    savingsUsd: null,
    ...options,
  };
}

function minutesAgo(minutes: number) {
  return new Date(Date.now() - minutes * 60_000).toISOString();
}

export function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

export function normalizeApprovalPolicy(value: unknown): ApprovalPolicy | null {
  return value === "read_only" ||
    value === "ask" ||
    value === "on_request" ||
    value === "on_failure" ||
    value === "never" ||
    value === "skip_all"
    ? value
    : null;
}

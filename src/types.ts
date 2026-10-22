export type AuthStatus =
  | { signedOut: Record<string, never> }
  | {
      chatgpt: {
        source: "app" | "codex-cli";
        email?: string;
        planType?: string;
        accountId: string;
        accessTokenValid: boolean;
      };
    }
  | { apiKey: { masked: string } }
  | { anthropicApiKey: { source: "api-key"; masked: string } };

export type HalvPlan = "beta_free" | "basic" | "unlimited" | "lifetime";

export type BillingCurrency = "usd" | "brl";

export type PaymentIssue = "past_due" | "unpaid";

export type BillingPlanPrices = {
  month: number;
  year: number;
};

export type BillingPrices = {
  currency: BillingCurrency;
  plans: {
    basic: BillingPlanPrices;
    unlimited: BillingPlanPrices;
  };
};

export type SavingsEntitlement = {
  period: string;
  tokensSaved: number;
  // Old servers still send the Basic plan cap. The app no longer shows it.
  capTokens: number | null;
  exhausted: boolean;
  allTimeTokensSaved?: number;
  allTimeSavingsUsd?: number;
  /** Dollars saved with the methods of ADR 0015, from servers that know them. */
  allTimeV2Usd?: number;
  periodV2Usd?: number;
  /** UNIX seconds of the first saving with a dollar method. */
  v2Since?: number;
  allTimeRoutingNetUsd?: number;
};

export type Referral = {
  code: string;
  link: string;
  trialDays: number;
  invited: number;
  rewarded: number;
  claimedCode: string | null;
};

export type Entitlements = {
  beta: boolean;
  billingRequired: boolean;
  canUseApp: boolean;
  offlineGraceHours?: number;
  plan: HalvPlan;
  message: string | null;
  checkoutUrl: string | null;
  billingPortalUrl: string | null;
  billingPortalAvailable?: boolean | null;
  paymentIssue?: PaymentIssue | null;
  trialConfirmed?: boolean;
  paymentConfirmed?: boolean;
  /** Unix seconds when the current free trial ends (ADR 0018). */
  trialEnd?: number | null;
  savings: SavingsEntitlement;
  referral?: Referral | null;
};

/** Why the user can or cannot use Halv Cloud (ADR 0021). */
export type CloudAccessReason =
  | "signed_out"
  | "no_plan"
  | "payment_issue"
  | "trial"
  | "paid"
  | "beta";

/** Halv Cloud access: JEV task routing, the savings MCP, rotation, sync. */
export type CloudAccess = { granted: boolean; reason: CloudAccessReason };

export type AccountAuthStatus = {
  signedIn: boolean;
  email: string | null;
  plan: HalvPlan | null;
  serverUrl: string;
  entitlements: Entitlements | null;
  referral?: Referral | null;
  syncedAt: string | null;
  firstLaunchAt: string;
  freeTurnsUsed: number;
  stale?: boolean;
};

export type AnthropicCredentialStatus = {
  configured: boolean;
  masked?: string;
};

export type SidecarStatus =
  | {
      state: "running";
      spawnedByUs: boolean;
      managedByHalv: boolean;
      version: string | null;
      pinnedVersion: string;
      port: number;
      optimizationPaused: boolean;
    }
  | {
      state: "installing";
      port: number;
    }
  | {
      state: "starting";
      port: number;
    }
  | {
      state: "notInstalled";
      installHint: string;
      port: number;
    }
  | {
      state: "error";
      message: string;
      port: number;
    };

export type SidecarInstallStage =
  | "starting"
  | "resolvingUv"
  | "downloadingUv"
  | "verifyingUv"
  | "installingWithUv"
  | "installingWithPipx"
  | "detectingHeadroom"
  | "success"
  | "error";

export type SidecarInstallProgress = {
  stage: SidecarInstallStage;
  line: string;
};

export type SidecarInstallError = {
  code: string;
  message: string;
  details: string[];
  manualHint: string;
};

export type LaneUsage = {
  lane: string;
  billing: "plan" | "api" | "unknown";
  provider: string;
  requests: number;
  inputTokens: number;
  outputTokens: number | null;
  costUsd: number | null;
  tokensSaved: number;
};

export type SidecarStats = {
  port: number;
  tokensSaved: number;
  costSavedUsd: number;
  requestsTotal: number;
  lifetimeRequests: number;
  lifetimeCacheSavingsUsd: number;
  lifetimeTokensSaved?: number | null;
  requestsByProvider: Record<string, number>;
  usageByLane: LaneUsage[];
  unattributedUsage: LaneUsage;
  requestsCached: number;
  requestsFailed: number;
  sessionTokensSaved: number;
  sessionCostSavedUsd: number;
  inputTokens: number;
  processInputTokens: number;
  outputTokens: number;
  proxyCompressionTokensSaved: number;
  compressionTokensSaved: number;
  cliFilteringTokensSaved: number;
  rtkTokensSaved: number;
  leanCtxTokensSaved: number;
  toolSearchTokensSaved: number;
  estimatedOutputTokensSaved: number;
  mcpTokensSaved: number;
  liveWsTokensSaved: number;
  cacheSavingsUsd: number;
  costWithHeadroomUsd: number;
  savingsPercent?: number;
  planWindows?: PlanWindowStats[];
};

export type PlanWindowStats = {
  label: string;
  usedPercent: number;
  resetsAt: number | null;
};

export type StatusState = {
  account: AccountAuthStatus | null;
  auth: AuthStatus | null;
  anthropic: AnthropicCredentialStatus | null;
  sidecar: SidecarStatus | null;
  sidecarStats: SidecarStats | null;
  error: string | null;
};

/** The answer to the usage events question. `unset` sends nothing (ADR 0021). */
export type AnalyticsConsent = "unset" | "on" | "off";

/** The list version of usage events that an "on" answer covers (ADR 0022). */
export type AnalyticsConsentVersion = 1 | 2;

/** The newest list version that Halv asked about. `0` means never asked. */
export type AnalyticsConsentAsked = 0 | 1 | 2;

/** The download id of the site link that led to this install (ADR 0021). */
export type AnalyticsAttribution = { downloadId: string | null };

export type AppPreferences = {
  subagentConcurrencyLimit: number | null;
  completionNotifications: "never" | "background" | "always";
  permissionNotifications: boolean;
  questionNotifications: boolean;
  keyboardShortcuts: Record<string, string>;
  personality: PersonalityPreference;
  ponytail: PonytailMode;
  claudeSubscriptionProxy: boolean;
  memoriesEnabled: boolean;
  /** Halv sends usage events only for `on` (ADR 0021). */
  analyticsConsent: AnalyticsConsent;
  /**
   * The list version that the `on` answer covers (ADR 0022). Version 1
   * sends the 13 journey events only.
   */
  analyticsConsentVersion: AnalyticsConsentVersion;
  /** The newest list version that Halv asked about (ADR 0022). */
  analyticsConsentAsked: AnalyticsConsentAsked;
  /** Always `analyticsConsent === "on"`. Use `analyticsConsent` in new code. */
  analyticsEnabled: boolean;
  defaultModel: string | null;
  defaultEffort: string | null;
  defaultAgentMode: boolean;
  defaultApprovalPolicy: ApprovalPolicy | null;
  defaultFastMode: boolean;
  taskRouting: boolean;
  workerLane: "auto" | "claude" | "codex";
  routingTierAgents: RoutingTierAgents;
  /** JEV routes the agents of a Claude Code Workflow script (ADR 0014). */
  workflowRouting: boolean;
  /** New Claude panes start with ultracode on. */
  claudeUltracode: boolean;
};

export type RoutingAgent = AppPreferences["workerLane"];
export type RoutingTier = "cheapest" | "cheap" | "medium" | "strong";
export type RoutingTierAgent = {
  lane: RoutingAgent;
  model: string | null;
  effort: string | null;
};
export type RoutingTierAgents = Record<RoutingTier, RoutingTierAgent>;

export type PersonalityPreference = "none" | "friendly" | "pragmatic";
export type PonytailMode = "off" | "lite" | "full" | "ultra";

export type DiagnosticStatus = "healthy" | "attention" | "unavailable";

export type DiagnosticComponent = {
  status: DiagnosticStatus;
  summary: string;
};

export type SupportDiagnostics = {
  schemaVersion: number;
  generatedAt: string;
  overallStatus: DiagnosticStatus;
  app: {
    version: string;
    operatingSystem: string;
    architecture: string;
  };
  activation: {
    firstActivatedAt: string;
    lastActivatedAt: string;
    launchCount: number;
  } | null;
  localDataCounts: {
    conversations: number;
    messages: number;
    projects: number;
    workspaces: number;
    memories: number;
  } | null;
  storageSchemaVersion: number | null;
  health: {
    storage: DiagnosticComponent;
    preferences: DiagnosticComponent;
    headroom: DiagnosticComponent;
  };
  privacy: {
    localOnly: boolean;
    automaticallyUploaded: boolean;
    included: string[];
    excluded: string[];
  };
};

export interface CredentialProfile {
  id: string;
  provider: string;
  name: string;
  kind: string;
  priority: number;
  status: "active" | "disabled" | "invalid";
  lastValidatedAt: string | null;
  metadataJson: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface UsageWindow {
  id: number | null;
  provider: string;
  credentialProfileId: string | null;
  model: string | null;
  windowKind: string;
  limitValue: number | null;
  remaining: number | null;
  used: number | null;
  resetsAt: string | null;
  retryAfterSeconds: number | null;
  source: "headers" | "error" | "none";
  confidence: "known" | "estimated" | "unknown";
  capturedAt: string;
}

export interface ProfileCooldown {
  provider: string;
  credentialProfileId: string | null;
  reason: string;
  nextAttemptAt: string | null;
  lastError: string | null;
  recordedAt: string;
}

export type AgentBackend =
  | "claude_session"
  | "codex_session"
  | "kimi_session"
  | "zai_session"
  | "grok_session";

export type ConversationSummary = {
  id: string;
  title: string;
  titleSource: "default" | "generated" | "manual";
  mode: "chat" | "agent";
  model: string;
  createdAt: string;
  updatedAt: string;
  pinned: boolean;
  draft: string;
  draftOnly: boolean;
  archivedAt: number | null;
  memoriesMode: "off" | "on" | null;
  fastMode: boolean;
  goal?: string | null;
  agentMode: boolean;
  workflowMode: WorkflowMode;
  credentialProfileId: string | null;
  agentBackend: AgentBackend | null;
  agentSessionModel: string | null;
  projectId?: number | null;
  projectName?: string | null;
  projectRootPath?: string | null;
  projectTrustLevel?: "trusted" | "untrusted" | null;
  approvalPolicyOverride?: ApprovalPolicy | null;
  messageCount: number;
  /** The bound workspace. Null for a chat without a project. */
  workspaceId?: string | null;
};

export type AgentMemory = {
  id: number;
  projectId: number | null;
  projectName: string | null;
  content: string;
  sourceConversationId: string | null;
  createdAt: number;
  updatedAt: number;
};

export type WorkflowMode = "default" | "plan";

export type ApprovalPolicy =
  | "read_only"
  | "ask"
  | "on_request"
  | "on_failure"
  | "never"
  | "skip_all";

export type Project = {
  id: number;
  name: string;
  rootPath: string;
  trustLevel?: "trusted" | "untrusted" | null;
  approvalPolicy?: ApprovalPolicy | null;
  createdAt: string;
  lastOpenedAt?: string | null;
};

/** The git repositories below a folder project (ADR 0019). */
export type NestedRepoScan = {
  /** The canonical folder that the scan started at. */
  path: string;
  /** The folder is a git repository or is inside one. Then `repos` is empty. */
  gitRepo: boolean;
  /** The repository roots: breadth first, in name order per folder. */
  repos: string[];
  /** The scan stopped at a limit before the end of the walk. */
  truncated: boolean;
};

export type WorkspaceStatus = {
  id: string;
  projectId: number;
  name: string;
  kind: "primary" | "worktree";
  rootPath: string;
  gitCommonDir?: string | null;
  baseRef?: string | null;
  baseCommit?: string | null;
  branch?: string | null;
  localEnvironmentConfigPath?: string | null;
  managed: boolean;
  pinned: boolean;
  state: "available" | "missing" | "removing";
  createdAt: string;
  lastUsedAt: string;
  dirty: boolean;
  conversationCount: number;
  currentBranch?: string | null;
  available: boolean;
  isGitRepo?: boolean;
};

/** A workspace in the Projects tree. It has no dirty flag and no count. */
export type WorkspaceOverview = Omit<WorkspaceStatus, "dirty" | "conversationCount">;

/** A tab key is the owner key and the panel scope (ADR 0010). */
export type WorkspaceTabKey = `conversation:${string}` | `shell:${string}`;

/** One row of `workspace_tabs`. Times are unix milliseconds. */
export type WorkspaceTabRow = {
  key: WorkspaceTabKey;
  conversationId: string | null;
  shellWorkspaceId: string | null;
  /** Shell tabs only. */
  customTitle: string | null;
  /** Shell tabs only: "Terminal N", fixed at creation. */
  defaultTitle: string | null;
  position: number;
  createdAt: number;
  activatedAt: number | null;
};

/** One operation of a tab batch. Rust applies one batch in one transaction. */
export type WorkspaceTabsOp =
  | { type: "insertTab"; row: WorkspaceTabRow }
  | { type: "deleteTab"; key: WorkspaceTabKey }
  | { type: "touchTab"; key: WorkspaceTabKey; activatedAt: number }
  | { type: "setPositions"; positions: Array<[WorkspaceTabKey, number]> }
  | { type: "setTitle"; key: WorkspaceTabKey; title: string | null }
  | { type: "setActive"; workspaceId: string; key: WorkspaceTabKey | null };

/** `rev` counts the committed batches of the running app. */
export type WorkspaceTabsSnapshot = {
  rev: number;
  rows: WorkspaceTabRow[];
  activeByWorkspace: Record<string, WorkspaceTabKey | null>;
  seeded: boolean;
};

export type WorkspaceHandoffResult = {
  workspace: WorkspaceStatus;
  warning?: string | null;
};

export type LocalEnvironmentPlatformScript = {
  script: string;
};

export type LocalEnvironmentLifecycle = {
  script: string;
  darwin?: LocalEnvironmentPlatformScript | null;
  linux?: LocalEnvironmentPlatformScript | null;
  win32?: LocalEnvironmentPlatformScript | null;
};

export type LocalEnvironmentAction = {
  name: string;
  icon: "tool" | "run" | "debug" | "test";
  command: string;
  platform?: "darwin" | "linux" | "win32" | null;
};

export type LocalEnvironmentConfig = {
  version: 1;
  name: string;
  setup: LocalEnvironmentLifecycle;
  cleanup?: LocalEnvironmentLifecycle | null;
  actions: LocalEnvironmentAction[];
};

export type LocalEnvironmentDescriptor = {
  configPath: string;
  workspaceRoot: string;
  inherited: boolean;
  config?: LocalEnvironmentConfig | null;
  error?: string | null;
  // SHA-256 of the file content. An approval is valid only for this content.
  contentSha256?: string | null;
  approved?: boolean;
};

export type BrowserTab = {
  id: string;
  title: string;
  url: string;
  active: boolean;
};

export type BrowserUiState = {
  tabs: BrowserTab[];
  activeTabId: string;
  title: string;
  url: string;
  screenshotDataUrl: string;
  viewportWidth: number;
  viewportHeight: number;
};

export type BrowserRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type BrowserAnnotationTarget = {
  annotationId: string;
  url: string;
  selector: string;
  tag: string;
  text: string;
  rect: BrowserRect;
};

export type BrowserStylePreview = {
  fontSize?: number | null;
  color?: string | null;
  padding?: number | null;
  margin?: number | null;
};

export type BrowserSiteRule = {
  hostname: string;
  decision: "allow" | "block";
};

export type BrowserSettings = {
  siteAccess: "always_ask" | "auto_approve" | "always_allow";
  siteRules: BrowserSiteRule[];
  fullCdpAccess: boolean;
};

export type ProjectReviewSnapshot = {
  id: string;
  projectId: number;
  head?: string | null;
  generatedAt: string;
  cwd: string;
  diff: string;
  truncated: boolean;
};

export type ReviewLineSegment = {
  text: string;
  changed: boolean;
};

export type ReviewLine = {
  id: string;
  kind: "context" | "add" | "delete" | "meta";
  oldLine?: number;
  newLine?: number;
  text: string;
  segments?: ReviewLineSegment[];
};

export type ReviewHunk = {
  id: string;
  header: string;
  oldStart: number;
  newStart: number;
  lines: ReviewLine[];
};

export type ReviewFile = {
  id: string;
  path: string;
  oldPath?: string;
  status: "modified" | "added" | "deleted" | "renamed" | "binary";
  additions: number;
  deletions: number;
  binary: boolean;
  hunks: ReviewHunk[];
  raw: string;
};

export type SkillDescriptor = {
  id: string;
  name: string;
  description: string;
  source: string;
  scope: "global" | "project" | "plugin";
  root: string;
  enabled: boolean;
  fingerprint: string;
  displayName: string;
  shortDescription: string;
  allowImplicitInvocation: boolean;
  defaultPrompt?: string | null;
  dependencyTools: string[];
  pluginId?: string | null;
};

export type PluginDescriptor = {
  id: string;
  name: string;
  version: string;
  root: string;
  enabled: boolean;
  installedAt: string;
  contributesSkills: boolean;
  contributesMcp: boolean;
  unsupportedComponents: string[];
  description: string;
  displayName: string;
  developerName?: string | null;
  capabilities: string[];
  skillCount: number;
  mcpServerCount: number;
};

export type PluginCandidate = {
  id: string;
  name: string;
  displayName: string;
  version: string;
  description: string;
  path: string;
  capabilities: string[];
  installed: boolean;
};

export type McpAuthMode = "none" | "bearer" | "oauth";
export type CruxActivation = "auto" | "always_on" | "always_off";

export type CruxProjectCensus = {
  languages: Record<string, { files: number; lines: number }>;
};

export type CruxIndexStatus = {
  state:
    | "indexed"
    | "indexing"
    | "waiting_for_index"
    | "approval_required"
    | "off";
  documentCount?: number | null;
};

export type CruxWorkspaceOpenResult = {
  approvalRequired: boolean;
};

export type McpServerConfig = {
  id: string;
  name: string;
  transport: "stdio" | "streamable_http";
  command?: string | null;
  args: string[];
  cwd?: string | null;
  env: Record<string, string>;
  envVars: string[];
  url?: string | null;
  headers: Record<string, string>;
  envHttpHeaders: Record<string, string>;
  bearerTokenEnvVar?: string | null;
  authMode: McpAuthMode;
  oauthScopes: string[];
  oauthClientId?: string | null;
  enabled: boolean;
  required: boolean;
  startupTimeoutMs: number;
  toolTimeoutMs: number;
  enabledTools: string[];
  disabledTools: string[];
  approvalMode: "auto" | "prompt" | "writes" | "approve";
  toolApprovalModes: Record<
    string,
    "auto" | "prompt" | "writes" | "approve"
  >;
  trustToolAnnotations: boolean;
  source: "user" | "plugin" | "built_in";
  pluginId?: string | null;
  pluginName?: string | null;
};

export type McpAuthStatus = {
  serverId: string;
  mode: McpAuthMode;
  state: string;
  loginSupported: boolean;
  message: string;
};

export type ExtensionsSnapshot = {
  skills: SkillDescriptor[];
  plugins: PluginDescriptor[];
  availablePlugins: PluginCandidate[];
  mcpServers: McpServerConfig[];
  mcpAuth: McpAuthStatus[];
  cruxActivation: CruxActivation;
  cruxEnabled: boolean;
  cruxCensus?: CruxProjectCensus | null;
  cruxIndexStatus: CruxIndexStatus;
};

export type McpToolDescriptor = {
  serverId: string;
  serverName: string;
  name: string;
  wireName: string;
  description: string;
  inputSchema: Record<string, unknown>;
  approvalMode: string;
};

export type McpToolAnnotations = {
  title?: string | null;
  readOnlyHint: boolean;
  destructiveHint: boolean;
  idempotentHint: boolean;
  openWorldHint: boolean;
};

export type McpToolMetadata = {
  name: string;
  annotations: McpToolAnnotations;
  annotationsTrusted: boolean;
  classifiedAsWrite: boolean;
  configuredApprovalMode: string;
  effectiveApprovalMode: string;
};

export type McpConnectionResult = {
  serverId: string;
  serverName: string;
  protocolVersion: string;
  serverCapabilities: Record<string, unknown>;
  serverInstructions?: string | null;
  serverInfo?: Record<string, unknown> | null;
  tools: McpToolDescriptor[];
  toolMetadata: McpToolMetadata[];
};

export type AutomationTarget = "local" | "cloud";
export type AutomationWorkspaceStrategy = "current" | "new_worktree";
export type AutomationConversationMode = "new" | "existing";

export type AutomationRun = {
  id: string;
  startedAt: string;
  finishedAt?: string | null;
  status: string;
  error?: string | null;
  conversationId?: string | null;
  cloudTaskId?: string | null;
  workspaceId?: string | null;
  unread: boolean;
};

export type Automation = {
  id: string;
  name: string;
  prompt: string;
  projectId: number;
  workspaceId?: string | null;
  model: string;
  effort: string;
  target: AutomationTarget;
  repositoryUrl?: string | null;
  baseRef?: string | null;
  cadenceMinutes: number;
  rrule?: string | null;
  timezone: string;
  workspaceStrategy: AutomationWorkspaceStrategy;
  conversationMode: AutomationConversationMode;
  conversationId?: string | null;
  enabled: boolean;
  nextRunAt: string;
  lastRunAt?: string | null;
  lastStatus?: string | null;
  lastError?: string | null;
  cloudTaskId?: string | null;
  runs: AutomationRun[];
  createdAt: string;
  updatedAt: string;
};

export type AutomationDraft = Omit<
  Automation,
  | "id"
  | "nextRunAt"
  | "lastRunAt"
  | "lastStatus"
  | "lastError"
  | "cloudTaskId"
  | "runs"
  | "createdAt"
  | "updatedAt"
> & { id?: string | null };

/**
 * The JEV routing dollars of a period (ADR 0016). The period's dollar total
 * already holds `lowUsd`: the headline uses the low estimate.
 */
export type RoutingTotals = {
  /** Settled chains with a price. */
  chains: number;
  /** Settled chains without measured usage. They add nothing. */
  unpricedChains: number;
  lowUsd: number;
  /** The point estimate. Unreported chains have none. */
  pointUsd: number;
  highUsd: number;
  /** Chains whose factor k came from enough bench pairs. */
  benchChains: number;
  unreportedChains: number;
};

/** The settled routing dollars of one chain, for the task board. */
export type ChainSavings = {
  chainRootId: string;
  outcome: "passed" | "failed" | "unreported";
  usage: "measured" | "no_worker" | "none";
  defaultModel: string | null;
  actualUsd: number;
  netLowUsd: number;
  netPointUsd: number | null;
  netHighUsd: number;
  kSource: "exact" | "floor" | "thin" | "bench" | "none";
};

/** The measured cost of one route. */
export type RouteCost = {
  routeId: string;
  actualUsd: number;
  models: string[];
};

export type RoutingSavingsReport = {
  totals: RoutingTotals;
  chains: ChainSavings[];
  routeCosts: RouteCost[];
};

export type SavingsTotalsPayload = {
  tokensSaved: number;
  savingsUsd: number;
  /** Dollars saved with the methods of ADR 0015 (the headline). */
  dollarsUsd?: number;
  /** Tokens saved before the dollar metric started. */
  legacyTokensSaved?: number;
  /** When this install started the dollar metric (ISO date and time). */
  dollarsSince?: string | null;
  sessionDollarsUsd?: number;
  pendingDollarsUsd?: number;
  /** The version (date) of the price table. */
  priceVersion?: string;
  pendingTokensSaved: number;
  pendingSavingsUsd: number;
  sessionTokensSaved: number;
  sessionSavingsUsd: number;
  sessionPricingSource?: SavingsPricingSource | null;
  cruxTokensSaved: number;
  cruxIncludedInTotal?: boolean;
  rtkTokensSaved?: number;
  rtkIncludedInTotal?: boolean;
  pricingSource?: SavingsPricingSource | null;
  /** The routing part of `dollarsUsd` (ADR 0016). */
  routing?: RoutingTotals;
  /** The version of the bench calibration of the routing factor k. */
  routingCalibrationVersion?: string;
};

export type SavingsTotals = SavingsTotalsPayload & {
  localTokensSaved: number;
  localSavingsUsd: number;
  localDollarsUsd: number;
};

export type SavingsToday = {
  tokensSaved: number;
  savingsUsd: number;
  recentSavings: number[];
  /** Dollars saved with the methods of ADR 0015. */
  dollarsUsd?: number;
  /** The dollars of each event in `recentSavings`. */
  recentDollars?: number[];
  cruxTokensSaved?: number;
  cruxIncludedInTotal?: boolean;
  rtkTokensSaved?: number;
  rtkIncludedInTotal?: boolean;
  pricingSource?: SavingsPricingSource | null;
  /** The routing part of `dollarsUsd` (ADR 0016). */
  routing?: RoutingTotals;
};

export type SavingsPricingSource = "headroom" | "model_table" | "fallback";

export type SavingsMeasurement = {
  state: "measured" | "estimated" | "unmeasured";
  tokens: number | null;
  usd: number | null;
  composition?: {
    headroom?: number;
    rtk?: number;
    crux?: number;
    cacheRead?: number;
  };
  pricingSource?: SavingsPricingSource;
};

export type DailyTokenActivity = {
  date: string;
  inputTokens: number;
  outputTokens: number;
  tokensSaved: number;
  dollarsUsd?: number;
};

export const NEW_THREAD = "__new-thread__";

export type ImageAttachment = {
  id: string;
  name: string;
  mediaType: "image/png" | "image/jpeg" | "image/webp";
  sizeBytes: number;
  data: string;
};

export type QueuedMessage = {
  id: string;
  text: string;
  conversationId: string | null;
  origin?: "user" | "goal";
  mentions?: string[];
  attachments?: ImageAttachment[];
  pinnedProjectId?: number | null;
  pinnedWorkspaceId?: string | null;
};

export type StoredMessage = {
  id: string;
  sequence?: number;
  conversationId: string;
  role: "user" | "assistant" | "developer" | "system";
  content: string;
  createdAt: string;
  model?: string | null;
  inputTokens?: number | null;
  outputTokens?: number | null;
  error: boolean;
  mentions?: string[] | null;
  attachments?: ImageAttachment[];
  tokensSaved?: number | null;
  savingsUsd?: number | null;
  pricingSource?: SavingsPricingSource | null;
  cruxTokensSaved?: number | null;
  cruxIncludedInTotal?: boolean;
  rtkTokensSaved?: number | null;
  rtkIncludedInTotal?: boolean;
  cachedTokens?: number | null;
  savingsSource?: "measured" | "usage" | null;
  /** The dollar method of ADR 0015; absent for savings before it. */
  usdMethod?: string | null;
};

export type ConversationCompaction = {
  id: number;
  conversationId: string;
  uptoMessageId: number;
  summary: string;
  createdAt: string;
};

export type MessageSearchResult = {
  conversationId: string;
  conversationTitle: string;
  messageId: string;
  role: StoredMessage["role"];
  snippet: string;
};

export type ConversationExportFormat = "markdown" | "json";

export type ThreadNotice = {
  id: string;
  text: string;
  createdAt: string;
  copyable?: boolean;
};

export type TokenUsage = {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  cachedTokens: number;
  reasoningTokens: number;
};

export type RouteTier = "trivial" | "standard" | "hard";

export type RouteDecision = {
  schema: "task-tier-v1";
  provider: string;
  model: string;
  complexity: {
    choice: RouteTier;
    confidence: number;
    probabilities: Record<string, number>;
  };
  needsReasoning: number;
  risky: number;
  usage: { inputTokens: number | null; outputTokens: number | null };
  latencyMs: number;
};

export type TaskRouteStatus = "routed" | "passed" | "failed" | "escalated";

export type TaskRoute = {
  id: string;
  sessionId: string;
  conversationId: string | null;
  title: string;
  briefChars: number;
  complexity: RouteTier;
  complexityConfidence: number;
  needsReasoning: number;
  risky: number;
  tier: "cheapest" | "cheap" | "medium" | "strong" | "max";
  lane: "claude" | "codex";
  model: string | null;
  effort: string | null;
  reasons: string[];
  workerLane: "auto" | "claude" | "codex";
  status: TaskRouteStatus;
  escalatedFromId: string | null;
  provider: string;
  providerModel: string;
  latencyMs: number;
  createdAt: string;
  updatedAt: string;
  /** The model that the worker would run without JEV. */
  defaultModel?: string | null;
  defaultEffort?: string | null;
  defaultSource?: "observed" | "coordinator" | "hint" | "none" | null;
  /** The session ended before a report. */
  closedUnreported?: boolean;
  /** The workflow fields of a `route_tasks` route. */
  workflow?: TaskRouteWorkflow | null;
  receipt?: TaskRouteReceipt | null;
};

export type WorkflowRole = "read" | "implement" | "verify";

export type TaskRouteWorkflow = {
  batchId: string;
  name: string | null;
  key: string;
  role: WorkflowRole;
  instances: number;
};

export type TaskRouteReceipt = {
  routeId: string;
  sessionId: string;
  outcome: "passed" | "failed" | "escalated";
  agents: number;
  failedAgents: number;
  inputTokens: number | null;
  outputTokens: number | null;
  cacheReadTokens: number | null;
  cacheCreationTokens: number | null;
  durationMs: number | null;
  usageSource: "measured" | "reported" | "none";
  reportedAt: string;
  /** The exact models that the measured agents ran on. */
  models: string[];
};

export type TaskPlanItem = {
  step: string;
  status: "pending" | "in_progress" | "completed";
};

export type TaskPlan = {
  conversationId?: string;
  explanation?: string;
  plan: TaskPlanItem[];
  updatedAt?: string;
};

export type SubagentMessage = {
  id: string;
  agentId: string;
  role: "user" | "assistant" | "error" | "system";
  content: string;
  generation: number;
  createdAt: string;
};

export type SubagentThread = {
  agentId: string;
  conversationId: string;
  turnId: string;
  taskName: string;
  task: string;
  status: "pending" | "running" | "completed" | "errored" | "interrupted";
  result?: string | null;
  error?: string | null;
  generation: number;
  createdAt: string;
  updatedAt: string;
  messages: SubagentMessage[];
};

export type ChatDeltaPayload = {
  conversationId: string;
  kind:
    | "delta"
    | "reasoning"
    | "done"
    | "error"
    | "tool_call"
    | "tool_output"
    | "tool_result"
    | "turn_diff"
    | "branch_status"
    | "conversation_updated"
    | "context_compacted"
    | "task_list";
  text?: string;
  usage?: TokenUsage;
  goalSignal?: "done" | "continue";
  callId?: string;
  command?: string;
  toolKind?:
    | "shell"
    | "apply_patch"
    | "mcp"
    | "mcp_elicitation"
    | "subagent"
    | "browser"
    | "web";
  cwd?: string;
  status?: string;
  justification?: string;
  tail?: string;
  exitCode?: number;
  wallTimeMs?: number;
  truncatedOutput?: boolean;
  rtkRoute?: "rewritten" | "tracked";
  fileChanges?: PatchFileChange[];
  diff?: string;
  turnId?: string;
  revertible?: boolean;
  branch?: string;
  warning?: string;
  elicitationMode?: "form" | "url";
  elicitationSchema?: McpElicitationSchema;
  elicitationUrl?: string;
  approvalPolicy?: ApprovalPolicy;
  explanation?: string;
  plan?: TaskPlanItem[];
  compaction?: ConversationCompaction;
};

export type TurnDiffSummary = {
  turnId: string;
  diff: string;
  cwd: string;
  createdAt: string;
  revertible: boolean;
};

export type TerminalSession = {
  id: string;
  title?: string | null;
  projectId?: number | null;
  conversationId?: string | null;
  projectName: string;
  cwd: string;
  shell: string;
  pid: number;
  executionProfile: "full_access";
  agentKind?:
    | "antigravity"
    | "claude"
    | "codex"
    | "gemini"
    | "kimi"
    | "zai"
    | "grok"
    | null;
  status: "running" | "exited" | "failed";
  cols: number;
  rows: number;
  startedAt: string;
  exitedAt?: string;
  exitCode?: number;
  signal?: string;
  // The ended agent session that this shell continues in the same pane.
  continuedFrom?: string | null;
  // The shell tab that owns this session (ADR 0010): the uuid of the tab
  // key `shell:<uuid>`. A session with a conversation has no shell tab.
  shellTabId?: string | null;
};

/** The foreground state of one shell, for the close prompt of a tab.
 *  The prompt counts "unknown" as busy. */
export type TerminalForegroundBusy = {
  sessionId: string;
  state: "busy" | "idle" | "unknown";
};

/** Resident memory of Halv and of each running terminal, for the status bar. */
export type ResourceUsageSnapshot = {
  appBytes: number;
  terminals: Array<{ sessionId: string; bytes: number }>;
  totalBytes: number;
};

/** The savings report queue, for Settings > System. */
export type SavingsQueueStatus = {
  unreported: number;
  rejected: number;
  lastError: string | null;
};

export type TerminalSupervisorStatus = {
  available: boolean;
  socketPath: string;
  pid?: number;
  sessions: number;
  runningSessions: number;
  supervisorVersion: string | null;
  supervisorStale: boolean;
  appVersion: string;
  message: string | null;
  // A supervisor with another protocol holds the socket. Its agents can
  // still run, so the panel must not resume them (ADR 0008).
  protocolMismatch?: boolean;
};

export type TerminalSupervisorReconnect = {
  attempt: number;
  message: string;
};

export type TerminalSupervisorReconnectingEvent =
  TerminalSupervisorReconnect & {
    id: string;
  };

export type TerminalSupervisorReconnectedEvent = {
  id: string;
  attempt: number;
};

export type TerminalAttach = {
  session: TerminalSession;
  sequence: number;
  scrollback: string;
  // The last screen of the ended agent that a continuation shell replaces.
  prelude?: TerminalPrelude | null;
};

export type TerminalPrelude = {
  scrollback: string;
  agentKind?: TerminalSession["agentKind"];
  exitCode?: number | null;
};

export type TerminalContinuedEvent = {
  fromSessionId: string;
  session: TerminalSession;
};

export type TerminalEvent = {
  sessionId: string;
  sequence: number;
  kind: "data" | "exit" | "error";
  data?: string;
  replay?: boolean;
  exitCode?: number;
  signal?: string;
  message?: string;
};

export type ChatModel = {
  id: string;
  label: string;
  reasoningEfforts: string[];
  defaultEffort: string;
  provider?: "openai" | "anthropic" | "zai" | "moonshot" | "xai" | "codex_cli" | "claude_cli";
  supportsFastMode: boolean;
  agentBackend?: AgentBackend;
  agentSessionModel?: string;
  modelGroup?: string;
  modelBadge?: string;
};

export type ChatSendResponse = {
  conversationId: string;
  assistantMessage: string;
  usage?: TokenUsage;
  skippedMentions?: string[];
  serviceTier?: string;
  fastModeFallback?: boolean;
};

export type UiMessage = StoredMessage & {
  streaming?: boolean;
  usage?: TokenUsage;
};

export type AgentCapabilities = {
  platform: string;
  sandboxAvailable: boolean;
  message: string;
};

export type ToolCallUi = {
  callId: string;
  toolKind:
    | "shell"
    | "apply_patch"
    | "mcp"
    | "mcp_elicitation"
    | "subagent"
    | "browser"
    | "web";
  command: string;
  cwd: string;
  status: string;
  justification?: string;
  tail: string;
  exitCode?: number;
  wallTimeMs?: number;
  truncatedOutput?: boolean;
  rtkRoute?: "rewritten" | "tracked";
  fileChanges?: PatchFileChange[];
  warning?: string;
  elicitationMode?: "form" | "url";
  elicitationSchema?: McpElicitationSchema;
  elicitationUrl?: string;
};

export type McpElicitationValue = string | number | boolean | string[];

export type McpElicitationField = {
  type: "string" | "number" | "integer" | "boolean" | "array";
  title?: string;
  description?: string;
  format?: string;
  default?: McpElicitationValue;
  enum?: McpElicitationValue[];
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
  items?: {
    type: "string";
    enum?: string[];
  };
};

export type McpElicitationSchema = {
  type: "object";
  properties: Record<string, McpElicitationField>;
  required?: string[];
};

export type PatchFileChange = {
  path: string;
  changeType: "add" | "delete" | "update" | "move_source" | "move_target";
  requiresApproval: boolean;
  approved: boolean;
};

export type BranchStatus = {
  isGitRepo: boolean;
  branch?: string | null;
  defaultBranch?: string | null;
  clean?: boolean | null;
  autoCreatedBranch?: string | null;
  warning?: string | null;
};

export type LifetimeOfferStatus = {
  active: boolean; owned: boolean; currency: "usd"; amount: number;
  serverNow: number; startsAt: number | null; endsAt: number | null;
  capacity: number; sold: number; available: number;
};

// App updates. Rust owns this state and sends it on app_update://snapshot.
// The shape is the contract in docs/adr/0012-app-updates.md.
export type AppUpdateOperation = "check" | "download" | "install" | "relaunch";

export type AppUpdateStatus =
  | { state: "idle" }
  | { state: "checking"; userInitiated: boolean }
  | { state: "upToDate"; userInitiated: boolean }
  | { state: "available"; version: string; notes: string | null; releaseUrl: string }
  | {
      state: "downloading";
      version: string;
      downloadedBytes: number;
      totalBytes: number | null;
    }
  | { state: "ready"; version: string; notes: string | null; releaseUrl: string }
  | { state: "installing"; version: string; trigger: "restart" | "quit" }
  | { state: "restarting"; version: string }
  | {
      state: "error";
      operation: AppUpdateOperation;
      message: string;
      version?: string;
      retryable: boolean;
      userInitiated: boolean;
    };

export type AppUpdateRestartBlockers = {
  chatReplies: number;
  guiAgentSessions: number;
  inProcessTerminals: number;
};

// "replace": the macOS app bundle or the AppImage. "installer": Windows.
// "elevated": a Linux deb or rpm, or a macOS app that only an administrator
// can change. "readOnly": Halv runs from a location that it cannot change,
// such as a disk image. "unsupported": a build that checks only on request.
export type AppUpdateInstallMode =
  | "replace"
  | "installer"
  | "elevated"
  | "readOnly"
  | "unsupported";

export type AppUpdatePostUpdate = {
  fromVersion: string;
  toVersion: string;
  reconnectedSessions: number;
  releaseUrl: string;
};

export type AppUpdateSnapshot = {
  status: AppUpdateStatus;
  currentVersion: string;
  autoDownload: boolean;
  installMode: AppUpdateInstallMode;
  deferredVersion: string | null;
  restartWhenIdle: boolean;
  blockers: AppUpdateRestartBlockers | null;
  postUpdate: AppUpdatePostUpdate | null;
  lastCheckedAt: string | null;
  // The supervisor keeps the terminals and agents through a restart.
  sessionsSurviveRestart: boolean;
};

export type AppUpdateBeforeRestart = { requestId: number };

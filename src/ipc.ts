import type { TrayAction, TraySnapshot } from "./tray/model";
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import type {
  AccountAuthStatus,
  AgentBackend,
  AgentCapabilities,
  AgentMemory,
  AnalyticsAttribution,
  AnalyticsConsent,
  AnthropicCredentialStatus,
  AppPreferences,
  AppUpdateRestartBlockers,
  AppUpdateSnapshot,
  ApprovalPolicy,
  AuthStatus,
  Automation,
  AutomationDraft,
  BranchStatus,
  BrowserAnnotationTarget,
  BrowserSettings,
  BrowserStylePreview,
  BrowserUiState,
  BillingCurrency,
  BillingPrices,
  LifetimeOfferStatus,
  ChatModel,
  ChatSendResponse,
  ConversationCompaction,
  ConversationExportFormat,
  ConversationSummary,
  CredentialProfile,
  CruxActivation,
  CruxWorkspaceOpenResult,
  DailyTokenActivity,
  ExtensionsSnapshot,
  ImageAttachment,
  LocalEnvironmentAction,
  LocalEnvironmentConfig,
  LocalEnvironmentDescriptor,
  McpConnectionResult,
  McpElicitationValue,
  McpServerConfig,
  MessageSearchResult,
  NestedRepoScan,
  PersonalityPreference,
  PluginDescriptor,
  PonytailMode,
  Project,
  ProjectReviewSnapshot,
  RouteDecision,
  SavingsToday,
  SavingsTotalsPayload,
  SidecarStats,
  SidecarStatus,
  StoredMessage,
  SubagentThread,
  SupportDiagnostics,
  TaskPlan,
  TaskRoute,
  RoutingSavingsReport,
  TerminalAttach,
  TerminalForegroundBusy,
  ResourceUsageSnapshot,
  TerminalSession,
  SavingsQueueStatus,
  TerminalSupervisorStatus,
  TurnDiffSummary,
  UsageWindow,
  WorkflowMode,
  WorkspaceHandoffResult,
  WorkspaceOverview,
  WorkspaceStatus,
  WorkspaceTabKey,
  WorkspaceTabRow,
  WorkspaceTabsOp,
  WorkspaceTabsSnapshot,
} from "./types";
import type {
  AgentKind,
  AgentSession,
  AgentSessionCheckpoint,
  AgentSessionCheckpointPreview,
  AgentSessionCheckpointRestoreResult,
  AgentSessionCreateInput,
  AgentSessionLaneOptions,
  AgentSessionProbe,
  AgentSessionSavedTotals,
  AgentSessionTitleState,
} from "./agentSessions";
import type { AgentCliInstallOutcome, AgentCliKind } from "./useAgentCliInstall";
import type {
  WalletAccountArgs,
  WalletAccountUsage,
  WalletAddFinishResult,
  WalletPendingAdd,
  WalletPendingArgs,
  WalletProviderArgs,
  WalletKeepSessionsArgs,
  WalletSelectArgs,
  WalletSelectResult,
  WalletStaleSession,
  WalletState,
} from "./accountWallet";
import type {
  ClaudeUsageWindowsResult,
  CodexUsageSummary,
} from "./providerPlanWindows";
import type { ImageAttachmentData } from "./attachments";
import type { CustomPrompt } from "./customPrompts";
import type { AnalyticsEvent } from "./analytics";
import type {
  TerminalPaneStateInput,
  TerminalPanelState,
} from "./terminalRestore";
import type { TerminalPanelLayoutSnapshot } from "./terminalLayout";
import type { AgentStatusEntry } from "./agentPaneStatus";

export type AgentCliInstallArgs = { agentKind: AgentCliKind };
export type AgentLoginLaunchArgs = { agentKind: AgentKind };
export type AgentSessionIdArgs = { sessionId: string };
export type AgentSessionCheckpointIdArgs = { checkpointId: string };
export type AgentSessionCreateArgs = Omit<AgentSessionCreateInput, "mode">;
export type AgentSessionRenameArgs = { sessionId: string; title: string | null };
export type AgentSessionPermissionArgs = {
  sessionId: string;
  requestId: string;
  approve: boolean;
};
export type AgentSessionSendArgs = { sessionId: string; text: string };
export type AgentTerminalCreateArgs = Omit<AgentSessionCreateInput, "mode">;
export type AgentTerminalDuplicateArgs = {
  sessionId: string;
  cols: number;
  rows: number;
};
export type AgentGhosttyLaunchArgs = {
  agentKind: AgentKind;
  lane: AgentSessionCreateInput["lane"];
  cwd: string;
  projectId?: number;
  conversationId?: string;
  conversationTitle?: string;
  model?: string;
  effort?: string;
  resumeCliSessionId?: string;
  titleState?: AgentSessionTitleState;
  permissionMode?: string;
};

export type OpenExternalUrlArgs = { url: string };
// The path must be absolute and must exist. The backend reveals a launchable
// file in the file manager and never runs it.
export type AppPathArgs = { path: string };
export type ApiKeyArgs = { apiKey: string };
export type OAuthLoginArgs = { provider: string };
export type RequestCodeArgs = { email: string; challengeToken?: string | null };
export type VerifyCodeArgs = { email: string; code: string };
export type RequestCodeResult = {
  ok: boolean;
  devCode?: string | null;
  /** The server sent no code and asks for a Turnstile challenge. */
  challengeRequired?: boolean;
};

export type AccountClaimReferralArgs = { code: string };
export type AccountClaimReferralResult = { ok: boolean; trialDays: number };
export type AccountPublishReceiptArgs = {
  payload: {
    periodStart: string;
    periodEnd: string;
    tokensSaved: number;
    savingsUsd: number;
    percentSaved: number | null;
    agents: { label: string; tokensSaved: number }[];
    pngBase64: string;
  };
};
export type AccountPublishReceiptResult = { id: string; url: string };
export type RouteTaskArgs = { brief: string };

export type AnalyticsRecordArgs = { event: AnalyticsEvent; category?: import("./analytics").AnalyticsFailure };

export type AutomationIdArgs = { automationId: string };
export type AutomationRunArgs = { automationId: string; runId: string };
export type AutomationReportRunArgs = {
  automationId: string;
  status: "completed" | "failed";
  error: string | null;
  conversationId: string | null;
  workspaceId: string | null;
};
export type AutomationSetEnabledArgs = {
  automationId: string;
  enabled: boolean;
};
export type AutomationUpsertArgs = { draft: AutomationDraft };

export type BillingPlan = "basic" | "unlimited" | "lifetime";
export type BillingInterval = "month" | "year";
export type BillingCheckoutArgs = {
  plan: BillingPlan;
  interval: BillingInterval;
};

export type BrowserPointArgs = {
  x: number;
  y: number;
  surfaceWidth: number;
  surfaceHeight: number;
};
export type BrowserNavigateArgs = { url: string; newTab: boolean };
export type BrowserHistoryArgs = { direction: "back" | "forward" | "reload" };
export type BrowserTabArgs = { targetId: string };
export type BrowserPreviewStyleArgs = {
  annotationId: string;
  style: BrowserStylePreview;
};
export type BrowserSettingsSetArgs = { settings: BrowserSettings };
export type BrowserTypeFocusedArgs = { text: string; submit: boolean };

export type ConversationIdArgs = { conversationId: string };
export type ChatRegenerateArgs = ConversationIdArgs & {
  model: string;
  effort: string;
  agentSessionEffort?: string;
};
export type ChatRunShellArgs = {
  conversationId: string | null;
  command: string;
  model: string;
  projectId: number | null;
  workspaceId: string | null;
  agentMode: boolean;
  approvalPolicy: ApprovalPolicy | null;
  fastMode: boolean;
};
export type ChatSendArgs = {
  conversationId: string | null;
  text: string;
  model: string;
  effort: string;
  agentSessionEffort?: string;
  fastMode?: boolean;
  projectId: number | null;
  workspaceId: string | null;
  agentMode: boolean;
  approvalPolicy?: ApprovalPolicy | null;
  workflowMode: WorkflowMode;
  credentialProfileId?: string;
  userInitiated: boolean;
  mentions?: string[];
  attachments?: ImageAttachment[];
  agentBackend?: AgentBackend | null;
  conversationMode?: ConversationSummary["mode"];
};
export type ChatSubagentArgs = ConversationIdArgs & { agentId: string };
export type ChatSubagentMessageArgs = ChatSubagentArgs & { message: string };
export type ChatSetAgentModeArgs = ConversationIdArgs & { enabled: boolean };
export type ChatSetWorkflowModeArgs = ConversationIdArgs & { mode: WorkflowMode };
export type ChatSteerArgs = ConversationIdArgs & {
  text: string;
  mentions?: string[];
};
export type ChatToolDecision =
  | "approved"
  | "approvedForSession"
  | "denied"
  | { approvedPaths: { paths: string[]; remember: boolean } }
  | {
      mcpElicitation: {
        action: "accept" | "decline";
        content: Record<string, McpElicitationValue> | null;
      };
    };
export type ChatToolDecisionArgs = {
  callId: string;
  decision: ChatToolDecision;
};
export type ChatRevertTurnArgs = ConversationIdArgs & { turnId: string };
export type TurnRevertResult = { revertedFiles: number };

export type CompactConversationArgs = ConversationIdArgs & {
  model: string;
  effort: string;
};
export type ReadDroppedImageArgs = { path: string };
export type CredentialProfileAddArgs = {
  provider: string;
  name: string;
  kind: "api_key";
  secret: string;
};
export type CredentialProfileIdArgs = { id: string };
export type CredentialProfileRenameArgs = CredentialProfileIdArgs & {
  name: string;
};
export type CredentialProfileEnabledArgs = CredentialProfileIdArgs & {
  enabled: boolean;
};
export type CredentialProfilePriorityArgs = CredentialProfileIdArgs & {
  priority: number;
};

export type ProjectWorkspaceArgs = {
  projectId: number | null;
  workspaceId: string | null;
};
export type RequiredProjectWorkspaceArgs = {
  projectId: number;
  workspaceId: string | null;
};
export type CruxActivationArgs = ProjectWorkspaceArgs & {
  activation: CruxActivation;
};

export type ConversationExportArgs = ConversationIdArgs & {
  format: ConversationExportFormat;
};
export type ConversationForkArgs = ConversationIdArgs & {
  uptoSequence: number;
};
export type CustomPromptsArgs = { projectRoot: string | null };
export type LocalEnvironmentProjectArgs = { projectId: number };
export type LocalEnvironmentDeleteArgs = LocalEnvironmentProjectArgs & {
  configPath: string;
};
export type LocalEnvironmentSaveArgs = LocalEnvironmentProjectArgs & {
  configPath: string | null;
  filename: string | null;
  config: LocalEnvironmentConfig;
};
export type LocalEnvironmentApproveArgs = LocalEnvironmentProjectArgs & {
  configPath: string;
  contentSha256: string;
};
export type LocalEnvironmentResolveActionArgs = LocalEnvironmentApproveArgs & {
  actionIndex: number;
};

export type McpServerIdArgs = { serverId: string };
export type McpBearerTokenArgs = McpServerIdArgs & { token: string };
export type McpServerEnabledArgs = McpServerIdArgs & {
  enabled: boolean;
  projectId?: number | null;
  workspaceId?: string | null;
};
export type McpServerTestArgs = McpServerIdArgs & ProjectWorkspaceArgs;
export type McpServerUpsertArgs = { server: McpServerConfig };
export type PluginPathArgs = { path: string };
export type PluginIdArgs = { pluginId: string };
export type PluginEnabledArgs = PluginIdArgs & { enabled: boolean };
export type SkillReadArgs = {
  skillId: string;
  projectId: number | null;
  workspaceId: string | null;
};
export type SkillEnabledArgs = { skillId: string; enabled: boolean };

export type ConversationDefaultsArgs = {
  defaultModel: string | null;
  defaultEffort: string | null;
  defaultAgentMode: boolean;
  defaultApprovalPolicy: ApprovalPolicy | null;
  defaultFastMode: boolean;
};
export type KeyboardShortcutsArgs = {
  shortcuts: Record<string, string>;
};
export type AnalyticsEnabledArgs = { analyticsEnabled: boolean };
/** `unset` is not an answer, so the command accepts only `on` and `off`. */
export type AnalyticsConsentArgs = { consent: Exclude<AnalyticsConsent, "unset"> };
export type MemoriesEnabledArgs = { memoriesEnabled: boolean };
export type TaskRoutingArgs = { taskRouting: boolean };
export type WorkflowRoutingArgs = { workflowRouting: boolean };
export type ClaudeUltracodeArgs = { claudeUltracode: boolean };
export type WorkerLaneArgs = { workerLane: AppPreferences["workerLane"] };
export type RoutingTierAgentsArgs = {
  routingTierAgents: AppPreferences["routingTierAgents"];
};
export type StorageListTaskRoutesArgs = { conversationId: string };
export type StorageRoutingSavingsArgs = {
  sessionId?: string | null;
  conversationId?: string | null;
};
export type NotificationsArgs = Pick<
  AppPreferences,
  | "completionNotifications"
  | "permissionNotifications"
  | "questionNotifications"
>;
export type PersonalityArgs = { personality: PersonalityPreference };
export type PonytailArgs = { mode: PonytailMode };
export type ClaudeSubscriptionProxyArgs = {
  claudeSubscriptionProxy: boolean;
};
export type SubagentConcurrencyArgs = { limit: number | null };

// The desktop remote gateway (ADR 0009). Events: "remote-changed" (RemoteStatus)
// and "remote-pairing-request" (RemotePairingRequest).
export type RemoteConnection = "off" | "connecting" | "connected" | "error";
export type RemoteDevice = {
  id: string;
  name: string;
  platform: string;
  model: string | null;
  pairedAt: number;
  lastSeenAt: number | null;
  connected: boolean;
};
export type RemotePairing = {
  uri: string;
  qrSvgDataUri: string;
  expiresAt: number;
};
export type RemotePairingRequest = {
  requestId: string;
  name: string;
  platform: string;
  model: string | null;
  // The 6-digit confirmation code with leading zeros (protocol section 7.3.1).
  code: string;
  expiresAt: number;
};
export type RemoteStatus = {
  enabled: boolean;
  relayUrl: string;
  relayUrlIsDefault: boolean;
  connection: RemoteConnection;
  error: string | null;
  hostId: string | null;
  hostName: string;
  devices: RemoteDevice[];
  pairing: RemotePairing | null;
  pendingApproval: RemotePairingRequest | null;
};
export type RemoteSetActiveArgs = { active: boolean };
export type RemoteSetEnabledArgs = { enabled: boolean };
export type RemoteSetRelayUrlArgs = { relayUrl: string | null };
export type RemotePairingRespondArgs = { requestId: string; allow: boolean };
export type RemoteDeviceRevokeArgs = { deviceId: string };

export type ProjectPathArgs = { path: string };
export type ProjectIdArgs = { id: number };
export type ProjectCloneArgs = {
  url: string;
  destParent: string | null;
};
export type ProjectWorkspaceIdArgs = ProjectIdArgs & {
  workspaceId: string | null;
};
export type ProjectCommitArgs = ProjectWorkspaceIdArgs & { message: string };
export type ProjectReviewScope =
  | "working_tree"
  | "staged"
  | "unstaged"
  | "last_turn"
  | "branch"
  | "commit";
export type ProjectReviewAction = "stage" | "unstage" | "revert";
export type ProjectReviewBaseArgs = {
  cwd: string;
  scope: ProjectReviewScope;
  reference: string | null;
};
export type ProjectReviewSnapshotForCwdArgs = ProjectReviewBaseArgs & {
  conversationId: string | null;
};
export type ProjectReviewFileDiffArgs = ProjectReviewBaseArgs & {
  path: string;
  contextLines: number;
};
export type ProjectReviewApplyActionArgs = ProjectReviewBaseArgs & {
  projectId: number;
  workspaceId: string | null;
  snapshotId: string;
  path: string;
  hunkIndex: number;
  action: ProjectReviewAction;
};
export type ProjectSearchFilesArgs = RequiredProjectWorkspaceArgs & {
  query: string;
};
export type ProjectFileSearchResult = { path: string };
export type ProjectTrustArgs = ProjectIdArgs & {
  level: "trusted" | "untrusted";
};
export type SearchMessagesArgs = { query: string };

export type ReceiptWritePngArgs = { path: string; bytes: number[] };

export type StorageDailyActivityArgs = { sinceIso: string };
export type AgentMemoryIdArgs = { id: number };
export type StorageCreateDraftArgs = {
  conversationId: string | null;
  draft: string;
  model: string;
  projectId: number | null;
  workspaceId: string | null;
  mode: ConversationSummary["mode"];
  agentMode: boolean;
  workflowMode: WorkflowMode;
  credentialProfileId: string | null;
  agentBackend: AgentBackend | null;
  agentSessionModel: string | null;
  approvalPolicy: ApprovalPolicy | null;
  fastMode: boolean;
};
export type StorageSetDraftArgs = ConversationIdArgs & { draft: string };
export type StorageSetAgentBackendArgs = ConversationIdArgs & {
  agentBackend: AgentBackend | null;
  agentSessionModel: string | null;
};
export type StorageSetApprovalPolicyArgs = ConversationIdArgs & {
  policy: ApprovalPolicy | null;
};
export type StorageSetArchivedArgs = ConversationIdArgs & {
  archived: boolean;
};
export type StorageSetCredentialProfileArgs = ConversationIdArgs & {
  credentialProfileId: string | null;
};
export type StorageSetFastModeArgs = ConversationIdArgs & { enabled: boolean };
export type StorageSetGoalArgs = {
  conversationId: string | null;
  goal: string | null;
  model: string | null;
  projectId: number | null;
  workspaceId?: string | null;
};
export type StorageSetMemoriesModeArgs = ConversationIdArgs & {
  mode: "off" | "on" | null;
};
export type StorageSetModeArgs = ConversationIdArgs & {
  mode: ConversationSummary["mode"];
};
export type StorageSetPinnedArgs = ConversationIdArgs & { pinned: boolean };
export type StorageSetTitleArgs = ConversationIdArgs & { title: string };
export type StorageTruncateArgs = ConversationIdArgs & { messageId: string };

export type TerminalAttachArgs = { sessionId: string };
export type TerminalOpenArgs = {
  projectId: number;
  workspaceId: string | null;
  conversationId?: string | null;
  /** The shell tab that owns the new shell. Never together with a
   *  conversation. */
  shellTabId?: string | null;
  cols: number;
  rows: number;
  executionProfile: "full_access";
};
export type TerminalRenameArgs = TerminalAttachArgs & { title: string | null };
/** "Open in tab": the shell tab that adopts an unowned plain shell. */
export type TerminalSetShellTabArgs = TerminalAttachArgs & { shellTabId: string };
export type TerminalResizeArgs = TerminalAttachArgs & {
  cols: number;
  rows: number;
};
export type TerminalRestoreReadArgs = { scope: string };
export type TerminalRestoreShellArgs = TerminalAttachArgs & {
  scope: string;
  cols: number;
  rows: number;
};
export type TerminalRestoreWriteArgs = {
  scope: string;
  layout: TerminalPanelLayoutSnapshot;
  panes: TerminalPaneStateInput[];
};
export type TerminalRestartArgs = { killSessions: boolean };
export type TerminalWriteArgs = TerminalAttachArgs & {
  data: string;
  // A pane numbers its writes from 1. The backend restores their order and
  // drops a repeated number.
  writer?: string;
  sequence?: number;
};
export type TerminalForegroundBusyArgs = { sessionIds: string[] };
// Each candidate is the path text of one link, without ":line:col". The
// backend reads at most 64 candidates of at most 1024 characters each.
export type TerminalLinkTargetsArgs = { sessionId: string; candidates: string[] };
// One result for each candidate, in the same order. `path` is the resolved
// absolute path when `kind` is not "none".
export type TerminalLinkTarget = {
  text: string;
  path: string | null;
  kind: "file" | "dir" | "none";
};
export type TerminalOpenShellArgs = TerminalAttachArgs & {
  scope: string | null;
  cols: number;
  rows: number;
};

export type WorkspaceProjectArgs = { projectId: number };
export type WorkspaceBindArgs = ConversationIdArgs & { workspaceId: string };
export type WorkspaceCreateBranchArgs = {
  workspaceId: string;
  branch: string;
};
export type WorkspaceCreateWorktreeArgs = WorkspaceProjectArgs & {
  baseRef: string;
  branch: string | null;
  environmentConfigPath: string | null;
};
export type WorkspaceCurrentArgs = ConversationIdArgs;
export type WorkspaceHandoffArgs = ConversationIdArgs & {
  target: string;
  environmentConfigPath: string | null;
};
export type WorkspaceIdArgs = { workspaceId: string };
export type WorkspaceSetPinnedArgs = WorkspaceIdArgs & { pinned: boolean };
/** "Open in Finder" and "Open in editor" of the Projects tree. */
export type WorkspaceOpenInArgs = WorkspaceIdArgs & { target: "fileManager" | "editor" };
export type WorkspaceTabsApplyArgs = { ops: WorkspaceTabsOp[] };
export type WorkspaceTabsSeedArgs = {
  rows: WorkspaceTabRow[];
  active: Record<string, WorkspaceTabKey | null>;
};

export type AppUpdateCheckArgs = { userInitiated: boolean };
export type AppUpdateRestartArgs = { force: boolean };
export type AppUpdateRestartAckArgs = { requestId: number };
export type AppUpdateDeferArgs = { version: string };
export type AppUpdateAutoDownloadArgs = { enabled: boolean };
export type AgentStatusFocusArgs = { sessionId: string | null };
export type TerminalNotifyArgs = TerminalAttachArgs & {
  title: string;
  body: string;
};

export interface IpcContract {
  account_claim_referral: { args: AccountClaimReferralArgs; result: AccountClaimReferralResult };
  account_publish_receipt: { args: AccountPublishReceiptArgs; result: AccountPublishReceiptResult };
  account_savings_queue_status: { args: undefined; result: SavingsQueueStatus };
  agent_claude_usage_windows: { args: undefined; result: ClaudeUsageWindowsResult };
  agent_cli_install: { args: AgentCliInstallArgs; result: AgentCliInstallOutcome };
  agent_codex_usage_windows: { args: undefined; result: CodexUsageSummary };
  agent_ghostty_launch: { args: AgentGhosttyLaunchArgs; result: AgentSession };
  agent_login_launch: { args: AgentLoginLaunchArgs; result: string };
  wallet_list: { args: undefined; result: WalletState };
  wallet_select: { args: WalletSelectArgs; result: WalletSelectResult };
  wallet_add_launch: { args: WalletProviderArgs; result: WalletPendingAdd };
  wallet_add_finish: { args: WalletPendingArgs; result: WalletAddFinishResult };
  wallet_add_cancel: { args: WalletPendingArgs; result: void };
  wallet_remove: { args: WalletAccountArgs; result: WalletState };
  wallet_usage: { args: undefined; result: WalletAccountUsage[] };
  wallet_stale_sessions: { args: undefined; result: WalletStaleSession[] };
  wallet_keep_sessions: { args: WalletKeepSessionsArgs; result: void };
  agent_session_checkpoint_preview: { args: AgentSessionCheckpointIdArgs; result: AgentSessionCheckpointPreview };
  agent_session_checkpoint_restore: { args: AgentSessionCheckpointIdArgs; result: AgentSessionCheckpointRestoreResult };
  agent_session_checkpoints: { args: AgentSessionIdArgs; result: AgentSessionCheckpoint[] };
  agent_session_create: { args: AgentSessionCreateArgs; result: AgentSession };
  agent_session_forget: { args: AgentSessionIdArgs; result: void };
  agent_session_kill: { args: AgentSessionIdArgs; result: AgentSession };
  agent_session_lane_options: { args: undefined; result: AgentSessionLaneOptions };
  agent_session_list: { args: undefined; result: AgentSession[] };
  agent_session_probe: { args: undefined; result: AgentSessionProbe };
  agent_session_rename: { args: AgentSessionRenameArgs; result: AgentSession };
  agent_session_respond_permission: { args: AgentSessionPermissionArgs; result: void };
  agent_session_saved_totals: { args: undefined; result: AgentSessionSavedTotals[] };
  agent_session_send: { args: AgentSessionSendArgs; result: void };
  agent_terminal_create: { args: AgentTerminalCreateArgs; result: AgentSession };
  agent_terminal_duplicate: { args: AgentTerminalDuplicateArgs; result: AgentSession };
  analytics_attribution_get: { args: undefined; result: AnalyticsAttribution };
  analytics_flush: { args: undefined; result: void };
  analytics_install_id: { args: undefined; result: string };
  analytics_record: { args: AnalyticsRecordArgs; result: void };
  app_open_external_url: { args: OpenExternalUrlArgs; result: void };
  app_open_path: { args: AppPathArgs; result: void };
  app_reveal_path: { args: AppPathArgs; result: void };
  app_hide_main_window: { args: undefined; result: void };
  dot_set_view: { args: { visible: boolean; x: number; y: number; width: number; height: number }; result: void };
  dot_reload: { args: undefined; result: void };
  app_update_relaunch: { args: undefined; result: void };
  auth_anthropic_status: { args: undefined; result: AnthropicCredentialStatus };
  auth_credential_status: { args: undefined; result: AuthStatus };
  auth_email_challenge: { args: undefined; result: string };
  auth_login_chatgpt: { args: undefined; result: AuthStatus };
  auth_logout: { args: undefined; result: AuthStatus };
  auth_oauth_cancel: { args: undefined; result: void };
  auth_oauth_login: { args: OAuthLoginArgs; result: AccountAuthStatus };
  auth_oauth_providers: { args: undefined; result: string[] };
  auth_refresh_entitlements: { args: undefined; result: AccountAuthStatus };
  auth_request_code: { args: RequestCodeArgs; result: RequestCodeResult };
  auth_set_anthropic_api_key: { args: ApiKeyArgs; result: AnthropicCredentialStatus };
  auth_set_api_key: { args: ApiKeyArgs; result: AuthStatus };
  auth_signout: { args: undefined; result: AccountAuthStatus };
  auth_status: { args: undefined; result: AccountAuthStatus };
  auth_verify: { args: VerifyCodeArgs; result: AccountAuthStatus };
  automation_delete: { args: AutomationIdArgs; result: void };
  automation_list: { args: undefined; result: Automation[] };
  automation_mark_all_read: { args: undefined; result: void };
  automation_mark_run_read: { args: AutomationRunArgs; result: void };
  automation_report_run: { args: AutomationReportRunArgs; result: void };
  automation_run_now: { args: AutomationIdArgs; result: void };
  automation_set_enabled: { args: AutomationSetEnabledArgs; result: void };
  automation_unread_count: { args: undefined; result: number };
  automation_upsert: { args: AutomationUpsertArgs; result: void };
  billing_checkout: { args: BillingCheckoutArgs; result: void };
  billing_checkout_browser: { args: BillingCheckoutArgs; result: void };
  billing_lifetime_offer: { args: undefined; result: LifetimeOfferStatus };
  billing_prices: { args: undefined; result: BillingPrices };
  billing_portal: { args: undefined; result: void };
  browser_clear_data: { args: undefined; result: void };
  browser_settings_get: { args: undefined; result: BrowserSettings };
  browser_settings_set: { args: BrowserSettingsSetArgs; result: BrowserSettings };
  browser_ui_annotation_target: { args: BrowserPointArgs; result: BrowserAnnotationTarget };
  browser_ui_click: { args: BrowserPointArgs; result: BrowserUiState };
  browser_ui_close_tab: { args: BrowserTabArgs; result: BrowserUiState };
  browser_ui_history: { args: BrowserHistoryArgs; result: BrowserUiState };
  browser_ui_navigate: { args: BrowserNavigateArgs; result: BrowserUiState };
  browser_ui_preview_style: { args: BrowserPreviewStyleArgs; result: BrowserUiState };
  browser_ui_select_tab: { args: BrowserTabArgs; result: BrowserUiState };
  browser_ui_state: { args: undefined; result: BrowserUiState };
  browser_ui_type_focused: { args: BrowserTypeFocusedArgs; result: BrowserUiState };
  chat_agent_capabilities: { args: undefined; result: AgentCapabilities };
  chat_approve_last_denied: { args: ConversationIdArgs; result: boolean };
  chat_cancel: { args: ConversationIdArgs; result: void };
  chat_cancel_and_revert: { args: ConversationIdArgs; result: TurnRevertResult };
  chat_interrupt_all_subagents: { args: ConversationIdArgs; result: void };
  chat_interrupt_subagent: { args: ChatSubagentArgs; result: void };
  chat_list_models: { args: undefined; result: ChatModel[] };
  chat_list_subagents: { args: ConversationIdArgs; result: SubagentThread[] };
  chat_list_turn_diffs: { args: ConversationIdArgs; result: TurnDiffSummary[] };
  chat_regenerate: { args: ChatRegenerateArgs; result: ChatSendResponse };
  chat_revert_turn: { args: ChatRevertTurnArgs; result: TurnRevertResult };
  chat_run_shell: { args: ChatRunShellArgs; result: ChatSendResponse };
  chat_send: { args: ChatSendArgs; result: ChatSendResponse };
  chat_send_subagent_message: { args: ChatSubagentMessageArgs; result: void };
  chat_set_agent_mode: { args: ChatSetAgentModeArgs; result: ConversationSummary };
  chat_set_workflow_mode: { args: ChatSetWorkflowModeArgs; result: ConversationSummary };
  chat_steer: { args: ChatSteerArgs; result: void };
  chat_tool_decision: { args: ChatToolDecisionArgs; result: void };
  compact_conversation: { args: CompactConversationArgs; result: ConversationCompaction };
  composer_read_dropped_image: { args: ReadDroppedImageArgs; result: ImageAttachmentData };
  credential_profile_add: { args: CredentialProfileAddArgs; result: CredentialProfile };
  credential_profile_remove: { args: CredentialProfileIdArgs; result: void };
  credential_profile_rename: { args: CredentialProfileRenameArgs; result: CredentialProfile };
  credential_profile_set_enabled: { args: CredentialProfileEnabledArgs; result: CredentialProfile };
  credential_profile_set_priority: { args: CredentialProfilePriorityArgs; result: CredentialProfile };
  credential_profile_test: { args: CredentialProfileIdArgs; result: CredentialProfile };
  crux_approve_indexing: { args: RequiredProjectWorkspaceArgs; result: void };
  crux_reindex_now: { args: RequiredProjectWorkspaceArgs; result: void };
  crux_set_activation: { args: CruxActivationArgs; result: void };
  crux_workspace_open: { args: RequiredProjectWorkspaceArgs; result: CruxWorkspaceOpenResult };
  diagnostics_export_support_report: { args: undefined; result: boolean };
  diagnostics_snapshot: { args: undefined; result: SupportDiagnostics };
  export_conversation_to_file: { args: ConversationExportArgs; result: boolean };
  extensions_snapshot: { args: ProjectWorkspaceArgs; result: ExtensionsSnapshot };
  fork_conversation: { args: ConversationForkArgs; result: ConversationSummary };
  list_custom_prompts: { args: CustomPromptsArgs; result: CustomPrompt[] };
  local_environment_approve: { args: LocalEnvironmentApproveArgs; result: void };
  local_environment_delete: { args: LocalEnvironmentDeleteArgs; result: void };
  local_environment_list: { args: LocalEnvironmentProjectArgs; result: LocalEnvironmentDescriptor[] };
  local_environment_resolve_action: { args: LocalEnvironmentResolveActionArgs; result: LocalEnvironmentAction };
  local_environment_save: { args: LocalEnvironmentSaveArgs; result: LocalEnvironmentDescriptor };
  mcp_server_clear_bearer_token: { args: McpServerIdArgs; result: void };
  mcp_server_oauth_login: { args: McpServerIdArgs; result: void };
  mcp_server_oauth_logout: { args: McpServerIdArgs; result: void };
  mcp_server_remove: { args: McpServerIdArgs; result: void };
  mcp_server_set_bearer_token: { args: McpBearerTokenArgs; result: void };
  mcp_server_set_enabled: { args: McpServerEnabledArgs; result: void };
  mcp_server_test: { args: McpServerTestArgs; result: McpConnectionResult };
  mcp_server_upsert: { args: McpServerUpsertArgs; result: McpServerConfig };
  plugins_install_local: { args: PluginPathArgs; result: PluginDescriptor };
  plugins_set_enabled: { args: PluginEnabledArgs; result: void };
  plugins_uninstall: { args: PluginIdArgs; result: void };
  tray_get_snapshot: { args: undefined; result: TraySnapshot };
  tray_action: { args: { action: TrayAction }; result: void };
  tray_resize: { args: { height: number }; result: void };
  tray_set_labels: { args: { labels: Record<string, string> }; result: void };
  preferences_get: { args: undefined; result: AppPreferences };
  preferences_set_analytics_enabled: { args: AnalyticsEnabledArgs; result: AppPreferences };
  preferences_set_analytics_consent: { args: AnalyticsConsentArgs; result: AppPreferences };
  preferences_set_claude_subscription_proxy: { args: ClaudeSubscriptionProxyArgs; result: AppPreferences };
  preferences_set_conversation_defaults: { args: ConversationDefaultsArgs; result: AppPreferences };
  preferences_set_keyboard_shortcuts: { args: KeyboardShortcutsArgs; result: AppPreferences };
  preferences_set_memories_enabled: { args: MemoriesEnabledArgs; result: AppPreferences };
  preferences_set_notifications: { args: NotificationsArgs; result: AppPreferences };
  preferences_set_personality: { args: PersonalityArgs; result: AppPreferences };
  preferences_set_ponytail: { args: PonytailArgs; result: AppPreferences };
  preferences_set_subagent_concurrency_limit: { args: SubagentConcurrencyArgs; result: AppPreferences };
  preferences_set_task_routing: { args: TaskRoutingArgs; result: AppPreferences };
  preferences_set_worker_lane: { args: WorkerLaneArgs; result: AppPreferences };
  preferences_set_workflow_routing: { args: WorkflowRoutingArgs; result: AppPreferences };
  preferences_set_claude_ultracode: { args: ClaudeUltracodeArgs; result: AppPreferences };
  preferences_set_routing_tier_agents: { args: RoutingTierAgentsArgs; result: AppPreferences };
  project_add_local: { args: ProjectPathArgs; result: Project };
  project_adoption_root: { args: ProjectPathArgs; result: string | null };
  project_clone_github: { args: ProjectCloneArgs; result: Project };
  project_commit: { args: ProjectCommitArgs; result: string };
  project_create_draft_pr: { args: ProjectWorkspaceIdArgs; result: string };
  project_git_repos_under_path: { args: ProjectPathArgs; result: string[] };
  project_git_root_for_path: { args: ProjectPathArgs; result: string | null };
  project_git_status: { args: ProjectWorkspaceIdArgs; result: BranchStatus };
  project_list: { args: undefined; result: Project[] };
  project_open: { args: ProjectIdArgs; result: Project };
  project_push: { args: ProjectWorkspaceIdArgs; result: string };
  project_remove: { args: ProjectIdArgs; result: void };
  project_review_apply_action: { args: ProjectReviewApplyActionArgs; result: ProjectReviewSnapshot };
  project_review_file_diff: { args: ProjectReviewFileDiffArgs; result: ProjectReviewSnapshot };
  project_review_snapshot_for_cwd: { args: ProjectReviewSnapshotForCwdArgs; result: ProjectReviewSnapshot };
  project_scan_nested_repos: { args: ProjectPathArgs; result: NestedRepoScan };
  project_search_files: { args: ProjectSearchFilesArgs; result: ProjectFileSearchResult[] };
  project_set_trust: { args: ProjectTrustArgs; result: Project };
  receipt_write_png: { args: ReceiptWritePngArgs; result: void };
  remote_device_revoke: { args: RemoteDeviceRevokeArgs; result: RemoteStatus };
  remote_devices_list: { args: undefined; result: RemoteDevice[] };
  remote_identity_reset: { args: undefined; result: RemoteStatus };
  remote_pairing_cancel: { args: undefined; result: RemoteStatus };
  remote_pairing_respond: { args: RemotePairingRespondArgs; result: RemoteStatus };
  remote_pairing_start: { args: undefined; result: RemotePairing };
  remote_set_active: { args: RemoteSetActiveArgs; result: RemoteStatus };
  remote_set_enabled: { args: RemoteSetEnabledArgs; result: RemoteStatus };
  remote_set_relay_url: { args: RemoteSetRelayUrlArgs; result: RemoteStatus };
  remote_status: { args: undefined; result: RemoteStatus };
  route_task: { args: RouteTaskArgs; result: RouteDecision };
  search_messages: { args: SearchMessagesArgs; result: MessageSearchResult[] };
  sidecar_install_headroom: { args: undefined; result: SidecarStatus };
  sidecar_restart: { args: undefined; result: SidecarStatus };
  sidecar_stats: { args: undefined; result: SidecarStats };
  sidecar_status: { args: undefined; result: SidecarStatus };
  sidecar_update_headroom: { args: undefined; result: SidecarStatus };
  skill_read: { args: SkillReadArgs; result: string };
  skills_set_enabled: { args: SkillEnabledArgs; result: void };
  storage_clear_agent_memories: { args: undefined; result: number };
  storage_consume_conversation_draft: { args: ConversationIdArgs; result: ConversationSummary };
  storage_create_conversation_draft: { args: StorageCreateDraftArgs; result: ConversationSummary };
  storage_daily_token_activity: { args: StorageDailyActivityArgs; result: DailyTokenActivity[] };
  storage_delete_agent_memory: { args: AgentMemoryIdArgs; result: boolean };
  storage_delete_conversation: { args: ConversationIdArgs; result: void };
  storage_get_conversation_task_plan: { args: ConversationIdArgs; result: TaskPlan | null };
  storage_get_latest_conversation_compaction: { args: ConversationIdArgs; result: ConversationCompaction | null };
  storage_get_messages: { args: ConversationIdArgs; result: StoredMessage[] };
  storage_rebuild_agent_memories: { args: undefined; result: number };
  storage_list_agent_memories: { args: undefined; result: AgentMemory[] };
  storage_list_conversations: { args: undefined; result: ConversationSummary[] };
  storage_list_credential_profiles: { args: undefined; result: CredentialProfile[] };
  storage_list_task_routes: { args: StorageListTaskRoutesArgs; result: TaskRoute[] };
  storage_list_usage_windows: { args: undefined; result: UsageWindow[] };
  storage_routing_savings: { args: StorageRoutingSavingsArgs; result: RoutingSavingsReport };
  storage_savings_today: { args: undefined; result: SavingsToday };
  storage_savings_week: { args: undefined; result: SavingsToday };
  storage_set_conversation_agent_backend: { args: StorageSetAgentBackendArgs; result: ConversationSummary };
  storage_set_conversation_approval_policy: { args: StorageSetApprovalPolicyArgs; result: ConversationSummary };
  storage_set_conversation_archived: { args: StorageSetArchivedArgs; result: ConversationSummary };
  storage_set_conversation_credential_profile: { args: StorageSetCredentialProfileArgs; result: ConversationSummary };
  storage_set_conversation_draft: { args: StorageSetDraftArgs; result: ConversationSummary | null };
  storage_set_conversation_fast_mode: { args: StorageSetFastModeArgs; result: ConversationSummary };
  storage_set_conversation_goal: { args: StorageSetGoalArgs; result: ConversationSummary };
  storage_set_conversation_memories_mode: { args: StorageSetMemoriesModeArgs; result: ConversationSummary };
  storage_set_conversation_mode: { args: StorageSetModeArgs; result: ConversationSummary };
  storage_set_conversation_pinned: { args: StorageSetPinnedArgs; result: ConversationSummary };
  storage_set_conversation_title: { args: StorageSetTitleArgs; result: ConversationSummary };
  storage_total_savings: { args: undefined; result: SavingsTotalsPayload };
  storage_truncate_from: { args: StorageTruncateArgs; result: ConversationSummary };
  terminal_attach: { args: TerminalAttachArgs; result: TerminalAttach };
  terminal_close: { args: TerminalAttachArgs; result: void };
  terminal_duplicate: { args: AgentTerminalDuplicateArgs; result: TerminalSession };
  terminal_forget: { args: TerminalAttachArgs; result: void };
  terminal_list: { args: undefined; result: TerminalSession[] };
  terminal_open: { args: TerminalOpenArgs; result: TerminalSession };
  terminal_rename: { args: TerminalRenameArgs; result: TerminalSession };
  terminal_set_shell_tab: { args: TerminalSetShellTabArgs; result: TerminalSession };
  terminal_resize: { args: TerminalResizeArgs; result: void };
  terminal_restore_read: { args: TerminalRestoreReadArgs; result: TerminalPanelState | null };
  terminal_restore_shell: { args: TerminalRestoreShellArgs; result: TerminalSession };
  terminal_restore_write: { args: TerminalRestoreWriteArgs; result: void };
  terminal_supervisor_restart: { args: TerminalRestartArgs; result: TerminalSupervisorStatus };
  terminal_supervisor_retry: { args: undefined; result: TerminalSupervisorStatus };
  terminal_supervisor_status: { args: undefined; result: TerminalSupervisorStatus };
  terminal_write: { args: TerminalWriteArgs; result: void };
  terminal_open_shell: { args: TerminalOpenShellArgs; result: TerminalSession };
  terminal_continue_shell: { args: TerminalAttachArgs; result: TerminalSession };
  terminal_foreground_busy: { args: TerminalForegroundBusyArgs; result: TerminalForegroundBusy[] };
  terminal_link_targets: { args: TerminalLinkTargetsArgs; result: TerminalLinkTarget[] };
  diagnostics_resource_usage: { args: undefined; result: ResourceUsageSnapshot };
  workspace_bind_conversation: { args: WorkspaceBindArgs; result: WorkspaceStatus };
  workspace_create_branch: { args: WorkspaceCreateBranchArgs; result: WorkspaceStatus };
  workspace_create_worktree: { args: WorkspaceCreateWorktreeArgs; result: WorkspaceStatus };
  workspace_current: { args: WorkspaceCurrentArgs; result: WorkspaceStatus | null };
  workspace_handoff: { args: WorkspaceHandoffArgs; result: WorkspaceHandoffResult };
  workspace_list: { args: WorkspaceProjectArgs; result: WorkspaceStatus[] };
  workspace_overview: { args: undefined; result: WorkspaceOverview[] };
  workspace_list_base_refs: { args: WorkspaceProjectArgs; result: string[] };
  workspace_remove: { args: WorkspaceIdArgs; result: void };
  workspace_open_in: { args: WorkspaceOpenInArgs; result: void };
  workspace_set_pinned: { args: WorkspaceSetPinnedArgs; result: void };
  workspace_tabs_snapshot: { args: undefined; result: WorkspaceTabsSnapshot };
  workspace_tabs_apply: { args: WorkspaceTabsApplyArgs; result: WorkspaceTabsSnapshot };
  workspace_tabs_seed: { args: WorkspaceTabsSeedArgs; result: WorkspaceTabsSnapshot };
  app_update_snapshot: { args: undefined; result: AppUpdateSnapshot };
  app_update_check: { args: AppUpdateCheckArgs; result: AppUpdateSnapshot };
  app_update_download: { args: undefined; result: AppUpdateSnapshot };
  app_update_restart_blockers: { args: undefined; result: AppUpdateRestartBlockers };
  app_update_restart: { args: AppUpdateRestartArgs; result: AppUpdateRestartBlockers | null };
  app_update_restart_when_idle: { args: undefined; result: AppUpdateSnapshot };
  app_update_cancel_restart: { args: undefined; result: AppUpdateSnapshot };
  app_update_restart_ack: { args: AppUpdateRestartAckArgs; result: void };
  app_update_defer: { args: AppUpdateDeferArgs; result: AppUpdateSnapshot };
  app_update_set_auto_download: { args: AppUpdateAutoDownloadArgs; result: AppUpdateSnapshot };
  app_update_dismiss_post_update: { args: undefined; result: AppUpdateSnapshot };
  agent_status_snapshot: { args: undefined; result: AgentStatusEntry[] };
  agent_status_mark_viewed: { args: AgentSessionIdArgs; result: void };
  agent_status_set_focus: { args: AgentStatusFocusArgs; result: void };
  terminal_notify: { args: TerminalNotifyArgs; result: void };
}

export type CommandName = keyof IpcContract;
export type UnlistenFn = () => void;

export interface Transport {
  invoke<K extends CommandName>(
    name: K,
    args: IpcContract[K]["args"],
  ): Promise<IpcContract[K]["result"]>;
  listen<T>(
    event: string,
    handler: (payload: T) => void,
  ): Promise<() => void>;
}

type CommandPrefix<K extends string> = K extends `${infer Prefix}_${string}`
  ? Prefix
  : never;
type CommandSuffix<K extends string> = K extends `${string}_${infer Suffix}`
  ? Suffix
  : never;
type CamelCase<S extends string> = S extends `${infer Head}_${infer Tail}`
  ? `${Head}${Capitalize<CamelCase<Tail>>}`
  : S;
type CommandMethod<K extends CommandName> = IpcContract[K]["args"] extends undefined
  ? () => Promise<IpcContract[K]["result"]>
  : (args: IpcContract[K]["args"]) => Promise<IpcContract[K]["result"]>;
type CommandGroup<P extends CommandPrefix<CommandName>> = {
  [K in Extract<CommandName, `${P}_${string}`> as CamelCase<
    CommandSuffix<K>
  >]: CommandMethod<K>;
};

export type Ipc = {
  [P in CommandPrefix<CommandName>]: CommandGroup<P>;
} & {
  events: {
    listen<T>(event: string, handler: (payload: T) => void): Promise<() => void>;
  };
};

export const commandNames = [
  "tray_get_snapshot",
  "tray_action",
  "tray_resize",
  "tray_set_labels",
  "account_claim_referral",
  "account_publish_receipt",
  "account_savings_queue_status",
  "agent_claude_usage_windows",
  "agent_cli_install",
  "agent_codex_usage_windows",
  "agent_ghostty_launch",
  "agent_login_launch",
  "wallet_list",
  "wallet_select",
  "wallet_add_launch",
  "wallet_add_finish",
  "wallet_add_cancel",
  "wallet_remove",
  "wallet_usage",
  "wallet_stale_sessions",
  "wallet_keep_sessions",
  "agent_session_checkpoint_preview",
  "agent_session_checkpoint_restore",
  "agent_session_checkpoints",
  "agent_session_create",
  "agent_session_forget",
  "agent_session_kill",
  "agent_session_lane_options",
  "agent_session_list",
  "agent_session_probe",
  "agent_session_rename",
  "agent_session_respond_permission",
  "agent_session_saved_totals",
  "agent_session_send",
  "agent_terminal_create",
  "agent_terminal_duplicate",
  "analytics_attribution_get",
  "analytics_flush",
  "analytics_install_id",
  "analytics_record",
  "app_open_external_url",
  "app_open_path",
  "app_reveal_path",
  "app_hide_main_window",
  "dot_set_view",
  "dot_reload",
  "app_update_relaunch",
  "auth_anthropic_status",
  "auth_credential_status",
  "auth_email_challenge",
  "auth_login_chatgpt",
  "auth_logout",
  "auth_oauth_cancel",
  "auth_oauth_login",
  "auth_oauth_providers",
  "auth_refresh_entitlements",
  "auth_request_code",
  "auth_set_anthropic_api_key",
  "auth_set_api_key",
  "auth_signout",
  "auth_status",
  "auth_verify",
  "automation_delete",
  "automation_list",
  "automation_mark_all_read",
  "automation_mark_run_read",
  "automation_report_run",
  "automation_run_now",
  "automation_set_enabled",
  "automation_unread_count",
  "automation_upsert",
  "billing_checkout",
  "billing_checkout_browser",
  "billing_prices",
  "billing_lifetime_offer",
  "billing_portal",
  "browser_clear_data",
  "browser_settings_get",
  "browser_settings_set",
  "browser_ui_annotation_target",
  "browser_ui_click",
  "browser_ui_close_tab",
  "browser_ui_history",
  "browser_ui_navigate",
  "browser_ui_preview_style",
  "browser_ui_select_tab",
  "browser_ui_state",
  "browser_ui_type_focused",
  "chat_agent_capabilities",
  "chat_approve_last_denied",
  "chat_cancel",
  "chat_cancel_and_revert",
  "chat_interrupt_all_subagents",
  "chat_interrupt_subagent",
  "chat_list_models",
  "chat_list_subagents",
  "chat_list_turn_diffs",
  "chat_regenerate",
  "chat_revert_turn",
  "chat_run_shell",
  "chat_send",
  "chat_send_subagent_message",
  "chat_set_agent_mode",
  "chat_set_workflow_mode",
  "chat_steer",
  "chat_tool_decision",
  "compact_conversation",
  "composer_read_dropped_image",
  "credential_profile_add",
  "credential_profile_remove",
  "credential_profile_rename",
  "credential_profile_set_enabled",
  "credential_profile_set_priority",
  "credential_profile_test",
  "crux_approve_indexing",
  "crux_reindex_now",
  "crux_set_activation",
  "crux_workspace_open",
  "diagnostics_export_support_report",
  "diagnostics_snapshot",
  "export_conversation_to_file",
  "extensions_snapshot",
  "fork_conversation",
  "list_custom_prompts",
  "local_environment_approve",
  "local_environment_delete",
  "local_environment_list",
  "local_environment_resolve_action",
  "local_environment_save",
  "mcp_server_clear_bearer_token",
  "mcp_server_oauth_login",
  "mcp_server_oauth_logout",
  "mcp_server_remove",
  "mcp_server_set_bearer_token",
  "mcp_server_set_enabled",
  "mcp_server_test",
  "mcp_server_upsert",
  "plugins_install_local",
  "plugins_set_enabled",
  "plugins_uninstall",
  "preferences_get",
  "preferences_set_analytics_enabled",
  "preferences_set_analytics_consent",
  "preferences_set_claude_subscription_proxy",
  "preferences_set_conversation_defaults",
  "preferences_set_keyboard_shortcuts",
  "preferences_set_memories_enabled",
  "preferences_set_notifications",
  "preferences_set_personality",
  "preferences_set_ponytail",
  "preferences_set_subagent_concurrency_limit",
  "preferences_set_task_routing",
  "preferences_set_worker_lane",
  "preferences_set_workflow_routing",
  "preferences_set_claude_ultracode",
  "preferences_set_routing_tier_agents",
  "project_add_local",
  "project_adoption_root",
  "project_clone_github",
  "project_commit",
  "project_create_draft_pr",
  "project_git_repos_under_path",
  "project_git_root_for_path",
  "project_git_status",
  "project_list",
  "project_open",
  "project_push",
  "project_remove",
  "project_review_apply_action",
  "project_review_file_diff",
  "project_review_snapshot_for_cwd",
  "project_scan_nested_repos",
  "project_search_files",
  "project_set_trust",
  "receipt_write_png",
  "remote_device_revoke",
  "remote_devices_list",
  "remote_identity_reset",
  "remote_pairing_cancel",
  "remote_pairing_respond",
  "remote_pairing_start",
  "remote_set_active",
  "remote_set_enabled",
  "remote_set_relay_url",
  "remote_status",
  "route_task",
  "search_messages",
  "sidecar_install_headroom",
  "sidecar_restart",
  "sidecar_stats",
  "sidecar_status",
  "sidecar_update_headroom",
  "skill_read",
  "skills_set_enabled",
  "storage_clear_agent_memories",
  "storage_consume_conversation_draft",
  "storage_create_conversation_draft",
  "storage_daily_token_activity",
  "storage_delete_agent_memory",
  "storage_delete_conversation",
  "storage_get_conversation_task_plan",
  "storage_get_latest_conversation_compaction",
  "storage_get_messages",
  "storage_list_agent_memories",
  "storage_rebuild_agent_memories",
  "storage_list_conversations",
  "storage_list_credential_profiles",
  "storage_list_task_routes",
  "storage_list_usage_windows",
  "storage_routing_savings",
  "storage_savings_today",
  "storage_savings_week",
  "storage_set_conversation_agent_backend",
  "storage_set_conversation_approval_policy",
  "storage_set_conversation_archived",
  "storage_set_conversation_credential_profile",
  "storage_set_conversation_draft",
  "storage_set_conversation_fast_mode",
  "storage_set_conversation_goal",
  "storage_set_conversation_memories_mode",
  "storage_set_conversation_mode",
  "storage_set_conversation_pinned",
  "storage_set_conversation_title",
  "storage_total_savings",
  "storage_truncate_from",
  "terminal_attach",
  "terminal_close",
  "terminal_duplicate",
  "terminal_forget",
  "terminal_list",
  "terminal_open",
  "terminal_rename",
  "terminal_set_shell_tab",
  "terminal_resize",
  "terminal_restore_read",
  "terminal_restore_shell",
  "terminal_restore_write",
  "terminal_supervisor_restart",
  "terminal_supervisor_retry",
  "terminal_supervisor_status",
  "terminal_write",
  "terminal_open_shell",
  "terminal_continue_shell",
  "terminal_foreground_busy",
  "terminal_link_targets",
  "diagnostics_resource_usage",
  "workspace_bind_conversation",
  "workspace_create_branch",
  "workspace_create_worktree",
  "workspace_current",
  "workspace_handoff",
  "workspace_list",
  "workspace_overview",
  "workspace_list_base_refs",
  "workspace_remove",
  "workspace_open_in",
  "workspace_set_pinned",
  "workspace_tabs_snapshot",
  "workspace_tabs_apply",
  "workspace_tabs_seed",
  "app_update_snapshot",
  "app_update_check",
  "app_update_download",
  "app_update_restart_blockers",
  "app_update_restart",
  "app_update_restart_when_idle",
  "app_update_cancel_restart",
  "app_update_restart_ack",
  "app_update_defer",
  "app_update_set_auto_download",
  "app_update_dismiss_post_update",
  "agent_status_snapshot",
  "agent_status_mark_viewed",
  "agent_status_set_focus",
  "terminal_notify",
] as const satisfies readonly CommandName[];

type MissingCommandName = Exclude<CommandName, (typeof commandNames)[number]>;
const commandNamesAreComplete: MissingCommandName extends never ? true : never =
  true;
void commandNamesAreComplete;

function camelCase(value: string) {
  return value.replace(/_([a-z])/g, (_, letter: string) =>
    letter.toUpperCase(),
  );
}

export function createIpc(transport: Transport): Ipc {
  const groups: Record<string, Record<string, unknown>> = {};
  for (const name of commandNames) {
    const separator = name.indexOf("_");
    const prefix = name.slice(0, separator);
    const method = camelCase(name.slice(separator + 1));
    const group = (groups[prefix] ??= {});
    group[method] = (args: IpcContract[typeof name]["args"]) =>
      transport.invoke(name, args);
  }
  return Object.assign(groups, {
    events: {
      listen: <T>(event: string, handler: (payload: T) => void) =>
        transport.listen(event, handler),
    },
  }) as Ipc;
}

export const tauriTransport: Transport = {
  invoke: (name, args) => tauriInvoke(name, args),
  listen: <T>(event: string, handler: (payload: T) => void) =>
    tauriListen<T>(event, ({ payload }) => handler(payload)),
};

export let ipc = createIpc(tauriTransport);

export function setIpcTransport(transport: Transport) {
  ipc = createIpc(transport);
}

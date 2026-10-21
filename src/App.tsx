import { DotPanel } from "./components/DotPanel";
import { stopTerminalSessionForTabClose } from "./terminalClose";
import { terminalDraftBridge, TerminalDraftError } from "./terminalDraft";
import {
  ConsentAskBanner,
  useAnonymousUsagePreference,
} from "./components/AnonymousUsagePreference";
import { ipc, type UnlistenFn } from "./ipc.ts";
import { openExternalLink } from "./externalLinks.ts";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { ReactNode, SetStateAction } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/plugin-dialog";
import { exit } from "@tauri-apps/plugin-process";
import { AccountGate } from "./components/AccountGate";
import { AgentSessionProModeToggle } from "./components/AgentSessionProModeToggle";
import { AgentSessionsPanel } from "./components/AgentSessionsPanel";
import { AgentSessionView } from "./components/AgentSessionView";
import {
  agentSessionAgentLabel,
  agentSessionModeDecision,
  agentSessionModeForProMode,
  agentSessionProModeStore,
  agentSessionStore,
  agentSupportsInitialPrompt,
  currentTurnSavingsTokens,
  agentSessionTitle,
  forgetAgentSession,
  listAgentSessionLaneOptions,
  probeForGhostty,
  probeAgentSessions,
  restartLaneForAgentSession,
  resolvedAgentSessionModelForConversation,
  type AgentKind,
  type AgentSessionCreateLane,
  type AgentSessionEventPayload,
  type AgentSessionLaneOptions,
  type AgentSessionProbe,
} from "./agentSessions";
import {
  agentPaneViewFor,
  terminalActivityStore,
} from "./terminalActivityStore";
import {
  agentPaneNotificationText,
  unreportedAgentPaneView,
  type AgentStatusEntry,
} from "./agentPaneStatus";
import { createAgentStatusNotifier } from "./agentStatusNotifications";
import { terminalFocusBridge } from "./terminalFocusBridge";
import { sanitizeTerminalPaste } from "./terminalPaste";
import {
  resolveTerminalSplitAgentLane,
  terminalSplitAgentChoice,
} from "./terminalSplit";
import { requestTerminalShell } from "./terminalShellRequest";
import {
  effectiveTerminalStatus,
  guidedTerminalTurnCompleted,
  type TerminalActivityStatus,
} from "./terminalActivity";
import {
  agentBackendForModel,
  composerModelsWithAgentSessions,
  firstRunComposerModel,
  isSessionBackend,
  requestModelForChatModel,
  visibleComposerModels,
} from "./claudeComposerModels";
import {
  agentKindForBackend,
  agentSessionForConversationMode,
  conversationDisplayTitle,
  conversationNamingTitle,
  conversationProModeSwitch,
  conversationTerminalStatus,
  conversationUsesAgentSurface,
  modeForComposerDraft,
  modeForConversationSelection,
  modeForNewConversation,
  type ConversationMode,
} from "./conversationMode";
import { appUpdateStore } from "./appUpdateStore";
import {
  analyticsEventsWithConsent,
  flushAnalytics,
  recordAnalyticsEvent,
  type AnalyticsState,
} from "./analytics";
import { AutomationsPanel } from "./components/AutomationsPanel";
import { BrowserPanel } from "./components/BrowserPanel";
import { ChatComposer } from "./components/ChatComposer";
import { CommandPaletteDialog } from "./components/CommandPaletteDialog";
import { AppDialogHost } from "./components/AppDialogHost";
import { ConnectGate } from "./components/ConnectGate";
import { copyText } from "./clipboard";
import { ExtensionsPanel } from "./components/ExtensionsPanel";
import { LocalEnvironmentsPanel } from "./components/LocalEnvironmentsPanel";
import { KeyboardShortcutsPanel } from "./components/KeyboardShortcutsPanel";
import { MessageList } from "./components/MessageList";
import { ReasoningSummary } from "./components/ReasoningSummary";
import { SubagentOverviewStrip } from "./components/SubagentOverviewStrip";
import {
  FirstRunCard,
  GuidedTaskStatus,
  PreparedFirstTask,
} from "./components/FirstRunCard";
import { WelcomeScreen } from "./components/WelcomeScreen";
import { HalvCloudCard } from "./components/HalvCloudCard";
import { TrialGoalCard } from "./components/TrialGoalCard";
import { notify, notifyTerminalPane } from "./notifications";
import { confirmDialog, promptDialog } from "./dialogs";
import { offerCruxIndexApproval } from "./cruxIndexApproval";
import { mergePersistedMessagesWithStream } from "./messageStream";
import { completedTurnCount } from "./compaction";
import {
  conversationHandoffNotice,
  conversationSendRoute,
} from "./conversationTurnRouting";
import { clearConversationUnviewed } from "./conversationUnviewed";
import {
  createConversationTurnStore,
  isTerminalDelta,
  type PendingNewTurn,
  type TurnState,
} from "./conversationTurnStore";
import { sortConversations } from "./conversationOrdering";
import { nextPinnedConversation } from "./pinnedConversations";
import {
  approvalPolicyForMode,
  chatModeFor,
  safeGuidedTourChatMode,
  effectivePolicy,
  type ChatMode,
} from "./modes";
import { Paywall } from "./components/Paywall";
import { SettingsPanel } from "./components/SettingsPanel";
import { MobileHeaderButton, MobileRemoteHost } from "./components/MobileConnect";
import { mobileConnectStore } from "./mobileConnect";
import { TerminalPanel } from "./components/TerminalPanel";
import { DiffRail } from "./components/DiffRail";
import { DiffRailErrorBoundary } from "./components/DiffRailErrorBoundary";
import { TaskBoard } from "./components/TaskBoard";
import {
  diffRailStore,
  type DiffRailPaneDescriptor,
} from "./diffRail";
import {
  ThreadSidebar,
  type SidebarTerminalSession,
  type ThreadSidebarProjectTree,
} from "./components/ThreadSidebar";
import { SavingsPanel } from "./components/SavingsPanel";
import { StatusBar } from "./components/StatusBar";
import type { SettingsConnectionsRequest } from "./components/SettingsConnections";
import type { WalletProvider } from "./accountWallet";
import { useWalletSnapshot } from "./components/AccountWallet";
import { TaskFindBar } from "./components/TaskFindBar";
import { WorkspaceSwitcher } from "./components/WorkspaceSwitcher";
import {
  commandDescriptionLabel,
  commandErrorLabel,
  commandRegistryWithCustomPrompts,
  commandUnavailableReasonLabel,
  type CommandHandlerContext,
  type CommandRegistryEntry,
} from "./commands";
import type { CustomPrompt } from "./customPrompts";
import { formatCompactTokens, hasProviderCredentials } from "./format";
import {
  CONNECT_GATE_DISMISSED_STORAGE_KEY,
  connectGateConnections,
  nextConnectGateState,
  type ConnectGateConnections,
  type ConnectGateState,
} from "./connectGate";
import { planProviderConnections } from "./providerPlanWindows";
import {
  appLooksFresh as resolveAppLooksFresh,
  firstRunVariant,
  shouldShowFirstRunCard,
} from "./firstRun";
import {
  latestUserForkSequence as resolveLatestUserForkSequence,
  userMessageForkSequence,
} from "./forkConversation";
import { accountStatusStore, gateDecision } from "./gate";
import { cloudAccess } from "./halvCloud.ts";
import {
  lastTurnSavingsPercent,
  readFirstResultSeen,
  trialGoalProgress,
  valueDelivered,
  writeFirstResultSeen,
} from "./activation";
import { mergeAllTimeSavings } from "./savingsTotals";
import {
  OPEN_SAVINGS_METHOD_EVENT,
  routingSavingsFromTotals,
} from "./savingsMethod";
import {
  firstTaskPrompt,
  firstTaskFollowUp,
  type FirstTaskGoal,
  conversationIdToRestore,
  onboardingReplayEnabled,
  onboardingPayoffCopy,
  readWelcomeSeen,
  readCloudInfoSeen,
  readConsentBannerShown,
  readOnboardingPreference,
  shouldShowConsentBanner,
  shouldShowWelcomeScreen,
  writeCloudInfoSeen,
  writeConsentBannerShown,
  writeWelcomeSeen,
  writeOnboardingPreference,
  type OnboardingPreference,
} from "./onboarding";
import {
  cloudPitchVisible,
  readCloudPitchDismissed,
  readCloudPitchShown,
  writeCloudPitchDismissed,
  writeCloudPitchShown,
} from "./trialPrompt";
import {
  pinnedProjectIdForQueuedInput,
  requestProjectIdForInput,
  resolveInputRoute,
} from "./shell";
import {
  formatShortcut,
  shortcutForAction,
  shortcutMatchesEvent,
  type KeyboardShortcutOverrides,
} from "./shortcuts";
import {
  IDLE_TERMINAL_OPENING,
  reduceTerminalOpening,
  terminalOpeningNotice,
  type TerminalOpeningEvent,
  type TerminalOpeningState,
} from "./terminalOpening";
import { featureFlagStore } from "./featureFlags";
import {
  agentLaunchCwd,
  effectiveWorkspaceId,
  primaryWorkspaceId,
  readLastActiveConversationId,
  resolveLastActiveConversationId,
  terminalPanelStorageKey,
  writeLastActiveConversationId,
} from "./conversationWorkspace";
import {
  INITIAL_NAVIGATION,
  legacyNavigation,
  looseNavigation,
  navigationConversationId,
  navigationTabKey,
  readStoredNavigation,
  sameNavigation,
  workspaceNavigation,
  writeStoredNavigation,
  type Navigation,
  type OpenConversationOptions,
} from "./appNavigation";
import { workspaceTabsStore } from "./workspaceTabsStore";
import {
  bulkCloseConfirm,
  closeSuccessor,
  conversationHome,
  conversationTabKey,
  homeWorkspaceId,
  parseTabKey,
  plusMenuItems,
  shellTabKey,
  shellTabTitle,
  stripView,
  tabCloseNeedsConfirm,
  tabCloseReasons,
  tabFacts,
  tabState,
  type ClosePaneFact,
  type TabKey,
  type TabRow,
} from "./workspaceTabs";
import {
  conversationBodyKind,
  liveConversationIds as liveConversationIdsOf,
  liveShellTabIds as liveShellTabIdsOf,
  mountedTabKeys,
} from "./workspaceTabBodies";
import { terminalPanelRegistry, type TerminalPanelPaneFact } from "./terminalPanelRegistry";
import {
  closeChainStep,
  useWorkspaceTabKeys,
  type CloseChainFocus,
} from "./useWorkspaceTabKeys";
import { WorkspaceTabStrip, type WorkspaceStripTab } from "./components/WorkspaceTabStrip";
import { moveIndexBefore } from "./workspaceTabStrip";
import {
  cycledWorkspaceId,
  hasClosedTab,
  pushClosedTab,
  recentTabOrder,
  takeClosedTab,
  type ClosedTabEntry,
} from "./workspaceTabHistory";
import { RecentTabSwitcher, type RecentTabItem } from "./components/RecentTabSwitcher";
import type { WorkspaceTabMenuAction } from "./components/WorkspaceTabContextMenu";
import { WorkspaceRunButton } from "./components/WorkspaceRunButton";
import { WorkspaceLauncher } from "./components/WorkspaceLauncher";
import { launcherItems, workspaceLauncherShown } from "./workspaceLauncher";
import {
  adoptionPlan,
  adoptionWorkspace,
  readAdoptionMemory,
  withAdopted,
  workspaceForAdoptionRoot,
  writeAdoptionMemory,
  type AdoptionMemory,
  type AdoptionStep,
} from "./terminalAdoption";
import { tabbedShellSessionIds } from "./sidebarTerminals";
import { pathIsInside, projectAvatar } from "./projectTree";
import { treeOperationsStore } from "./projectTreeOperations";
import {
  importNestedRepos,
  scanFolderProjectsOnce,
  type FolderScanDeps,
} from "./folderProjects";
import { projectTreeStateStore, withProjectInGroup } from "./projectTreeState";
import { readFeatureFlags } from "./featureFlags";
import {
  readLocalEnvironmentSelection,
  resolveLocalEnvironmentSelection,
  runWithLocalEnvironmentApproval,
  writeLocalEnvironmentSelection,
} from "./localEnvironments";
import {
  readThemePreference,
  resolveThemePreference,
  THEME_STORAGE_KEY,
  type ResolvedTheme,
  type ThemePreference,
} from "./theme";
import { getLocaleDefinition } from "./i18n/config";
import {
  readLanguagePreference,
  readSystemLanguageTags,
  resolveLanguagePreference,
  writeLanguagePreference,
  type LanguagePreference,
} from "./i18n/language";
import { formatMoney, setActiveLocale, useT, type Translate } from "./i18n/runtime";
import { mergeStatus } from "./statusState";
import {
  readAgentAccounts,
  readFocusRefresh,
  readModelInputs,
  readStatusState,
  type AgentAccountsRead,
  type ModelInputsRead,
} from "./focusRefresh";
import {
  NEW_THREAD,
  type AgentBackend,
  type Automation,
  type AgentCapabilities,
  type AccountAuthStatus,
  type ApprovalPolicy,
  type AppPreferences,
  type AnthropicCredentialStatus,
  type AuthStatus,
  type BranchStatus,
  type ChatDeltaPayload,
  type ChatModel,
  type ChatSendResponse,
  type ConversationExportFormat,
  type ConversationSummary,
  type CredentialProfile,
  type DailyTokenActivity,
  type ExtensionsSnapshot,
  type ImageAttachment,
  type LocalEnvironmentAction,
  type LocalEnvironmentDescriptor,
  type McpElicitationValue,
  type McpConnectionResult,
  type PersonalityPreference,
  type PonytailMode,
  type Project,
  type QueuedMessage,
  type SavingsToday,
  type SavingsTotals,
  type SavingsTotalsPayload,
  type SidecarStats,
  type SidecarStatus,
  type StatusState,
  type StoredMessage,
  type SubagentThread,
  type TerminalAttach,
  type TerminalEvent,
  type TerminalSession,
  type ThreadNotice,
  type ToolCallUi,
  type UiMessage,
  type WorkflowMode,
  type WorkspaceHandoffResult,
  type WorkspaceOverview,
  type WorkspaceStatus,
} from "./types";

const ONBOARDING_REPLAY_ENABLED = onboardingReplayEnabled();

type GuidedTaskPhase = "idle" | "prepared" | "waiting" | "success";

const defaultModels: ChatModel[] = [
  {
    id: "gpt-5.5",
    label: "gpt-5.5",
    reasoningEfforts: ["low", "medium", "high", "xhigh"],
    defaultEffort: "medium",
    provider: "openai",
    supportsFastMode: false,
  },
];

const GOAL_KICKOFF_TEXT = "Start working toward the goal.";
const APPROVED_RETRY_TEXT =
  "The previously denied action is now approved — retry it.";
const APP_MENU_CHECK_FOR_UPDATES_EVENT = "halv://menu/check-for-updates";
const TRAY_NEW_CHAT_EVENT = "halv://tray/new-chat";
const GOAL_CONTINUE_TEXT =
  "Continue working toward the standing goal. Do not stop until it is fully accomplished.";
const WORKSPACE_BUSY_ERROR_TEXT =
  "workspace is busy in another conversation";
const EMPTY_NOTICES: ThreadNotice[] = [];
const PROJECT_SELECTION_STORAGE_KEY = "halv.selectedProject";
const UI_FONT_SIZE_STORAGE_KEY = "halv.uiFontSize";
const DEFAULT_UI_FONT_SIZE = 16;
const MIN_UI_FONT_SIZE = 13;
const MAX_UI_FONT_SIZE = 20;

type SendMessageOptions = {
  guidedTask?: boolean;
  origin?: "user" | "goal";
  queueIfPending?: boolean;
  clearComposerDraft?: boolean;
  mentions?: string[];
  attachments?: ImageAttachment[];
  pinnedProjectId?: number | null;
  pinnedWorkspaceId?: string | null;
};

type WorkspaceBusyRetryRequest = {
  conversationId: string;
  projectId: number;
  text: string;
  options: SendMessageOptions;
};

type ProjectSelectionPreference = number | null | undefined;

function readUiFontSize() {
  try {
    const value = Number(window.localStorage.getItem(UI_FONT_SIZE_STORAGE_KEY));
    return Number.isFinite(value)
      ? Math.min(MAX_UI_FONT_SIZE, Math.max(MIN_UI_FONT_SIZE, value))
      : DEFAULT_UI_FONT_SIZE;
  } catch {
    return DEFAULT_UI_FONT_SIZE;
  }
}

function applyUiFontSize(value: number) {
  const next = Math.min(MAX_UI_FONT_SIZE, Math.max(MIN_UI_FONT_SIZE, value));
  document.documentElement.style.fontSize = `${next}px`;
  try {
    window.localStorage.setItem(UI_FONT_SIZE_STORAGE_KEY, String(next));
  } catch {
    // UI scaling still applies for this session when storage is unavailable.
  }
  return next;
}

function currentActionPlatform(): LocalEnvironmentAction["platform"] {
  const platform = navigator.platform.toLowerCase();
  if (platform.includes("mac")) return "darwin";
  if (platform.includes("win")) return "win32";
  return "linux";
}

function readProjectSelectionPreference(): ProjectSelectionPreference {
  try {
    const stored = window.localStorage.getItem(PROJECT_SELECTION_STORAGE_KEY);
    if (stored === null) {
      return undefined;
    }
    if (stored === "none") {
      return null;
    }
    const projectId = Number(stored);
    return Number.isInteger(projectId) && projectId > 0 ? projectId : undefined;
  } catch {
    return undefined;
  }
}

function writeProjectSelectionPreference(projectId: number | null) {
  try {
    window.localStorage.setItem(
      PROJECT_SELECTION_STORAGE_KEY,
      projectId === null ? "none" : String(projectId),
    );
  } catch {
    // Project selection persistence is optional.
  }
}

type ConversationRequestSettings = {
  model: string;
  effort: string;
  agentBackend?: import("./types").AgentBackend | null;
  agentSessionModel?: string | null;
};

type PendingConversationNavigation = {
  conversationId: string;
  generation: number;
};

type TurnRevertResult = {
  revertedFiles: number;
};

type StorageTruncateError = {
  code: "messageNotFound" | "storage";
  message: string;
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function closeParentDetails(element: HTMLElement) {
  element.closest("details")?.removeAttribute("open");
}

function storageTruncateErrorMessage(error: unknown) {
  return isStorageTruncateError(error) ? error.message : errorMessage(error);
}

function isStorageTruncateError(error: unknown): error is StorageTruncateError {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof (error as StorageTruncateError).message === "string"
  );
}

function isMissingEditMessageError(error: unknown) {
  if (isStorageTruncateError(error)) {
    return error.code === "messageNotFound";
  }
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  return (
    message.includes("Query returned no rows") ||
    message.includes("was not found in conversation")
  );
}

function findEditRetryMessage(message: UiMessage, rows: StoredMessage[]) {
  const exact = rows.find(
    (row) => row.conversationId === message.conversationId && row.id === message.id,
  );
  if (exact) {
    return exact;
  }

  const messageCreatedAt = Date.parse(message.createdAt);
  if (!Number.isFinite(messageCreatedAt)) {
    return null;
  }
  const candidates = rows.filter(
    (row) =>
      row.conversationId === message.conversationId &&
      row.role === "user" &&
      row.content === message.content &&
      (row.model ?? null) === (message.model ?? null) &&
      Math.abs(Date.parse(row.createdAt) - messageCreatedAt) <= 60_000,
  );
  return candidates.length === 1 ? candidates[0] : null;
}

function mergeStatusState(current: StatusState, next: StatusState): StatusState {
  return mergeStatus(current, next);
}

async function readSavingsTotals(): Promise<SavingsTotals> {
  const [local, account] = await Promise.all([
    ipc.storage.totalSavings(),
    ipc.auth.status().catch(() => null),
  ]);
  return mergeAllTimeSavings(local, account);
}

async function readSavingsToday(): Promise<SavingsToday> {
  return ipc.storage.savingsToday();
}

async function readSavingsWeek(): Promise<SavingsToday> {
  return ipc.storage.savingsWeek();
}

async function readDailyTokenActivity(
  sinceIso: string,
): Promise<DailyTokenActivity[]> {
  return ipc.storage.dailyTokenActivity({
    sinceIso,
  });
}

// A brand-new agent conversation has no live session and no saved terminals, so
// it must show the task composer instead of an empty terminal surface. Saved
// terminals live in localStorage keyed by conversation id.
function conversationHasSavedTerminals(
  conversationId: string | null,
): boolean {
  if (!conversationId || typeof window === "undefined") return false;
  try {
    const raw = window.localStorage.getItem(
      terminalPanelStorageKey({
        conversationId,
        workspaceId: null,
        projectId: null,
      }),
    );
    if (!raw) return false;
    // The stored blob is a layout snapshot; its tabs list one entry per saved
    // terminal.
    const parsed = JSON.parse(raw) as { tabs?: unknown } | null;
    return Boolean(
      parsed &&
        Array.isArray(parsed.tabs) &&
        parsed.tabs.length > 0,
    );
  } catch {
    return false;
  }
}

// The seed of the workspace tabs reads the saved conversation panels. A
// layout in localStorage names its conversation. The conversation update
// time stands for the save time.
function savedConversationPanelScopes(conversations: ConversationSummary[]) {
  return conversations.flatMap((conversation) =>
    conversationHasSavedTerminals(conversation.id)
      ? [{ conversationId: conversation.id, updatedAt: Date.parse(conversation.updatedAt) || 0 }]
      : [],
  );
}

function localDateKey() {
  const date = new Date();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

function tokenActivitySinceIso() {
  const date = new Date();
  date.setHours(12, 0, 0, 0);
  date.setDate(date.getDate() - date.getDay() - 53 * 7);
  date.setHours(0, 0, 0, 0);
  return date.toISOString();
}

function isInteractiveElementFocused() {
  const active = document.activeElement;
  if (!(active instanceof HTMLElement)) {
    return false;
  }
  if (active.isContentEditable) {
    return true;
  }

  return [
    "a",
    "button",
    "input",
    "option",
    "select",
    "summary",
    "textarea",
  ].includes(active.tagName.toLowerCase());
}

function elementIntersectsViewport(element: Element) {
  const rect = element.getBoundingClientRect();
  return (
    rect.bottom > 0 &&
    rect.right > 0 &&
    rect.top < window.innerHeight &&
    rect.left < window.innerWidth
  );
}

function removeTrailingAssistantMessages(messages: UiMessage[]) {
  const next = [...messages];
  while (next.length > 0 && next[next.length - 1]?.role === "assistant") {
    next.pop();
  }
  return next;
}

function removePendingApprovalByCallId(
  approvals: Record<string, ToolCallUi>,
  callId: string,
) {
  let changed = false;
  const next: Record<string, ToolCallUi> = {};
  for (const [conversationId, approval] of Object.entries(approvals)) {
    if (approval.callId === callId) {
      changed = true;
    } else {
      next[conversationId] = approval;
    }
  }
  return changed ? next : approvals;
}

function truncateNotificationText(text: string, maxLength = 80) {
  const trimmed = text.trim();
  if (trimmed.length <= maxLength) {
    return trimmed;
  }
  return `${trimmed.slice(0, maxLength - 3)}...`;
}

function upsertConversation(
  conversations: ConversationSummary[],
  conversation: ConversationSummary,
) {
  const withoutCurrent = conversations.filter((item) => item.id !== conversation.id);
  return sortConversations([conversation, ...withoutCurrent]);
}

type ConversationDefaults = {
  model: string;
  effort: string;
  agentMode: boolean;
  approvalPolicy: ApprovalPolicy | null;
  fastMode: boolean;
};

function resolveConversationDefaults(
  preferences: Partial<AppPreferences> | null,
  models: ChatModel[],
  firstRunConnections: ConnectGateConnections,
  firstRunAuth: AuthStatus | null = null,
): ConversationDefaults | null {
  const model = firstRunComposerModel(
    models,
    preferences?.defaultModel,
    firstRunConnections,
    firstRunAuth,
  );
  if (!model) return null;
  const effort = model.reasoningEfforts.includes(preferences?.defaultEffort ?? "")
    ? preferences!.defaultEffort!
    : model.defaultEffort;
  return {
    model: model.id,
    effort,
    agentMode: preferences?.defaultAgentMode ?? true,
    approvalPolicy: preferences?.defaultApprovalPolicy ?? null,
    fastMode: preferences?.defaultFastMode ?? false,
  };
}

function browserStorage() {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

// With workspace tabs, the app starts on the workspace of the last session.
// Its strip shows the right tab as soon as the tab facts load.
function initialNavigation(): Navigation {
  if (!readFeatureFlags().workspaceTabs) return INITIAL_NAVIGATION;
  return readStoredNavigation(browserStorage()) ?? INITIAL_NAVIGATION;
}

function agentModeForNewDraft() {
  return (
    modeForNewConversation(agentSessionProModeStore.getSnapshot()) === "agent"
  );
}

function emptyStateForStatus(
  status: StatusState,
  provider: "openai" | "anthropic" | "session",
  t: Translate,
) {
  if (provider === "anthropic" && !status.anthropic?.configured) {
    return {
      title: t("Claude is not connected"),
      description: t("Set an Anthropic API key in Settings to use this Claude model."),
      showSuggestions: false,
      showRestart: false,
    };
  }
  if (
    provider === "openai" &&
    status.auth &&
    ("signedOut" in status.auth || "anthropicApiKey" in status.auth)
  ) {
    return {
      title: t("Not connected"),
      description: t("Connect ChatGPT or set an API key to start chatting."),
      showConnectChatGpt: true,
      showSuggestions: false,
      showRestart: false,
    };
  }

  if (status.sidecar?.state === "notInstalled") {
    return {
      title: t("Proxy is not installed"),
      description: t("Open Settings for the install command, then restart the proxy."),
      showSuggestions: false,
      showRestart: false,
    };
  }

  if (status.sidecar?.state === "error") {
    return {
      title: t("Proxy not running"),
      description: t("Halv couldn't start its local proxy. Restart it to try again."),
      showSuggestions: false,
      showRestart: true,
    };
  }

  if (status.sidecar?.state === "installing") {
    return {
      title: t("Proxy is installing"),
      description:
        t("Halv is setting up its proxy. This happens automatically on first launch."),
      showSuggestions: false,
      showRestart: false,
    };
  }

  if (status.sidecar?.state === "starting") {
    return {
      title: t("Proxy is starting"),
      description:
        t("The proxy is warming up. You can queue a message once it is ready."),
      showSuggestions: false,
      showRestart: false,
    };
  }

  return {
    title: t("Start a conversation"),
    description: t("Ask anything, or pin a standing goal with /goal"),
    showSuggestions: true,
    showRestart: false,
  };
}

function applyDocumentTheme(
  preference: ThemePreference,
  resolvedTheme: ResolvedTheme,
) {
  const root = document.documentElement;
  root.classList.toggle("dark", resolvedTheme === "dark");
  root.dataset.themePreference = preference;
  root.dataset.theme = resolvedTheme;
}

function publishDiffRailPanes(panes: DiffRailPaneDescriptor[]) {
  diffRailStore.publishPanes(panes);
}

// The active pane of the visible terminal panel drives the diff rail and
// tells the agent status which pane the user looks at.
function reportActiveTerminalPane(sessionId: string | null) {
  diffRailStore.setActivePane(sessionId);
  terminalActivityStore.setVisiblePane(sessionId);
}

// The IPC calls of the folder scan (ADR 0019).
function folderScanDeps(): FolderScanDeps {
  return {
    scan: (path) => ipc.project.scanNestedRepos({ path }),
    addLocal: (path) => ipc.project.addLocal({ path }),
    updateState: projectTreeStateStore.update,
    newGroupId: () => window.crypto.randomUUID(),
  };
}

function App() {
  const t = useT();
  // One-shot listener effects read the live translate function here.
  const tRef = useRef(t);
  tRef.current = t;
  const [status, setStatus] = useState<StatusState>({
    account: null,
    auth: null,
    anthropic: null,
    sidecar: null,
    sidecarStats: null,
    error: null,
  });
  const [sidecarRestartPending, setSidecarRestartPending] = useState(false);
  const [loginPending, setLoginPending] = useState(false);
  const [accountFlowOpen, setAccountFlowOpen] = useState<
    "account" | "paywall" | null
  >(null);
  const analyticsPreviousStateRef = useRef<AnalyticsState | null>(null);
  const [gateNowMs, setGateNowMs] = useState(() => Date.now());
  const [connectGateState, setConnectGateState] =
    useState<ConnectGateState>("resolving");
  const [connectGateDismissed, setConnectGateDismissed] = useState(() => {
    try {
      return window.localStorage.getItem(CONNECT_GATE_DISMISSED_STORAGE_KEY) === "1";
    } catch {
      return false;
    }
  });
  const [connectGateLaneOptions, setConnectGateLaneOptions] =
    useState<AgentSessionLaneOptions>([]);
  const [connectGateProbe, setConnectGateProbe] =
    useState<AgentSessionProbe | null>(null);
  const [connectGateAccountsResolved, setConnectGateAccountsResolved] =
    useState(false);
  const connectGateRefreshGenerationRef = useRef(0);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsMemoriesFocusSignal, setSettingsMemoriesFocusSignal] =
    useState(0);
  const [settingsSavingsMethodFocusSignal, setSettingsSavingsMethodFocusSignal] =
    useState(0);
  const [settingsPrivacyFocusSignal, setSettingsPrivacyFocusSignal] =
    useState(0);
  const [
    settingsCredentialProfilesFocusSignal,
    setSettingsCredentialProfilesFocusSignal,
  ] = useState(0);
  const [settingsConnectionsRequest, setSettingsConnectionsRequest] =
    useState<SettingsConnectionsRequest | null>(null);
  const [keyboardShortcutsOpen, setKeyboardShortcutsOpen] = useState(false);
  const [shortcutOverrides, setShortcutOverrides] =
    useState<KeyboardShortcutOverrides>({});
  const [browserSettingsRequested, setBrowserSettingsRequested] =
    useState(false);
  const [extensionsOpen, setExtensionsOpen] = useState(false);
  const [localEnvironmentsOpen, setLocalEnvironmentsOpen] = useState(false);
  const [automationsOpen, setAutomationsOpen] = useState(false);
  const [agentSessionsOpen, setAgentSessionsOpen] = useState(false);
  const [automationUnreadCount, setAutomationUnreadCount] = useState(0);
  const [legacyTerminalOpen, setLegacyTerminalOpen] = useState(false);
  // With workspace tabs, each conversation (and each new-task composer)
  // keeps its own bottom panel state. A panel opened for one conversation
  // never stays half open over the next one.
  const [openBottomPanels, setOpenBottomPanels] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const bottomPanelKeyRef = useRef("composer");
  const setTerminalOpen = useCallback((next: SetStateAction<boolean>) => {
    if (!featureFlagStore.getSnapshot().workspaceTabs) {
      setLegacyTerminalOpen(next);
      return;
    }
    const key = bottomPanelKeyRef.current;
    setOpenBottomPanels((current) => {
      const open = typeof next === "function" ? next(current.has(key)) : next;
      if (open === current.has(key)) return current;
      const updated = new Set(current);
      if (open) updated.add(key);
      else updated.delete(key);
      return updated;
    });
  }, []);
  const [terminalFocusSessionId, setTerminalFocusSessionId] = useState<
    string | null
  >(null);
  const [terminalSessions, setTerminalSessions] = useState<TerminalSession[]>([]);
  const terminalSessionsRef = useRef(terminalSessions);
  terminalSessionsRef.current = terminalSessions;
  const terminalActivitySnapshot = useSyncExternalStore(
    terminalActivityStore.subscribe,
    terminalActivityStore.getSnapshot,
  );
  const terminalActivityBySession = terminalActivitySnapshot.statuses;
  const [terminalPromptSubmitSignals, setTerminalPromptSubmitSignals] =
    useState<Record<string, number>>({});
  const signalTerminalPromptSubmitted = useCallback((sessionId: string) => {
    terminalActivityStore.trackerForSession(sessionId).notePromptSubmitted();
    setTerminalPromptSubmitSignals((current) => ({
      ...current,
      [sessionId]: (current[sessionId] ?? 0) + 1,
    }));
  }, []);
  const acknowledgeTerminalPromptSubmitSignal = useCallback(
    (sessionId: string, signal: number) => {
      setTerminalPromptSubmitSignals((current) => {
        if (current[sessionId] !== signal) return current;
        const next = { ...current };
        delete next[sessionId];
        return next;
      });
    },
    [],
  );
  const handleTerminalActivityChange = useCallback(
    (sessionId: string, status: TerminalActivityStatus | null) => {
      if (
        status === "responding" &&
        guidedTerminalTrackingArmedRef.current &&
        guidedTerminalSessionIdRef.current === sessionId
      ) {
        guidedTerminalSawSustainedOutputRef.current = true;
      }
    },
    [],
  );
  useEffect(
    () => terminalActivityStore.subscribeStatus(handleTerminalActivityChange),
    [handleTerminalActivityChange],
  );
  useEffect(
    () =>
      terminalActivityStore.subscribePanes((sessionId, view, previous) => {
        // A turn that rests or waits can have changed files.
        if (previous?.status === "working" && view?.status !== "working") {
          diffRailStore.markStaleBySession(sessionId);
        }
      }),
    [],
  );
  useEffect(() => {
    // Rust reads the agent signals. Listen before the snapshot read, so no
    // change can fall between the two.
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    const received = new Set<string>();
    terminalActivityStore.setAgentStatusBridge({
      markViewed: (sessionId) => {
        void ipc.agent.statusMarkViewed({ sessionId }).catch(() => undefined);
      },
      setFocus: (sessionId) => {
        void ipc.agent.statusSetFocus({ sessionId }).catch(() => undefined);
      },
    });
    void ipc.events
      .listen<AgentStatusEntry>("agent-status", (entry) => {
        if (disposed) return;
        received.add(entry.sessionId);
        terminalActivityStore.noteAgentStatus(entry);
      })
      .then(async (cleanup) => {
        if (disposed) {
          cleanup();
          return;
        }
        unlisten = cleanup;
        const entries = await ipc.agent.statusSnapshot();
        if (!disposed) terminalActivityStore.hydrateAgentStatuses(entries, received);
      })
      .catch((error) => console.error("Agent status subscription failed", error));
    const updateWindowFocus = () =>
      terminalActivityStore.setWindowFocused(
        document.hasFocus() && document.visibilityState === "visible",
      );
    updateWindowFocus();
    window.addEventListener("focus", updateWindowFocus);
    window.addEventListener("blur", updateWindowFocus);
    document.addEventListener("visibilitychange", updateWindowFocus);
    return () => {
      disposed = true;
      unlisten?.();
      terminalActivityStore.setAgentStatusBridge(null);
      window.removeEventListener("focus", updateWindowFocus);
      window.removeEventListener("blur", updateWindowFocus);
      document.removeEventListener("visibilitychange", updateWindowFocus);
    };
  }, []);
  const trackedTerminalSessionIdsRef = useRef(new Set<string>());
  useEffect(() => {
    const sessionIds = new Set(terminalSessions.map((session) => session.id));
    for (const id of trackedTerminalSessionIdsRef.current) {
      if (!sessionIds.has(id)) terminalActivityStore.forget(id);
    }
    for (const session of terminalSessions) {
      if (session.agentKind && session.status === "running") {
        terminalActivityStore.trackerForSession(session.id);
      } else if (session.status !== "running") {
        terminalActivityStore.forget(session.id);
      }
    }
    trackedTerminalSessionIdsRef.current = sessionIds;
  }, [terminalSessions]);
  const [workspaceView, setWorkspaceView] = useState<"chat" | "browser">(
    "chat",
  );
  const [dotOpen, setDotOpen] = useState(false);
  const [diffRailFocusSignal, setDiffRailFocusSignal] = useState(0);
  const [themePreference, setThemePreferenceState] =
    useState<ThemePreference>(readThemePreference);
  const [languagePreference, setLanguagePreferenceState] =
    useState<LanguagePreference>(readLanguagePreference);
  const [resolvedTheme, setResolvedTheme] = useState<ResolvedTheme>(() =>
    resolveThemePreference(
      readThemePreference(),
      window.matchMedia?.("(prefers-color-scheme: dark)")?.matches ?? true,
    ),
  );
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() =>
    window.matchMedia("(max-width: 767px)").matches,
  );
  const [savingsDrawerOpen, setSavingsDrawerOpen] = useState(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const headerMenuRef = useRef<HTMLDetailsElement>(null);
  const [onboardingPreference, setOnboardingPreference] =
    useState<OnboardingPreference>(readOnboardingPreference);
  const [welcomeSeen, setWelcomeSeen] = useState(readWelcomeSeen);
  // The Halv Cloud pitch after the first result (ADR 0021). Its dismissal
  // is permanent.
  const [cloudPitchDismissed, setCloudPitchDismissed] = useState(() =>
    ONBOARDING_REPLAY_ENABLED ? false : readCloudPitchDismissed(),
  );
  // The pitch card and the consent banner each show on one launch only.
  // App reads the flags once at launch, so a card that shows in this
  // launch stays until the user answers or the launch ends.
  const [cloudPitchShownOnEarlierLaunch] = useState(() =>
    ONBOARDING_REPLAY_ENABLED ? false : readCloudPitchShown(),
  );
  const [consentBannerShownOnEarlierLaunch] = useState(() =>
    ONBOARDING_REPLAY_ENABLED ? false : readConsentBannerShown(),
  );
  // The Halv Cloud information block shows once: on the welcome screen, or
  // in the first-run card when the welcome screen did not show.
  const [cloudInfoSeenOnEarlierLaunch] = useState(() =>
    ONBOARDING_REPLAY_ENABLED ? false : readCloudInfoSeen(),
  );
  const [welcomeShownThisLaunch, setWelcomeShownThisLaunch] = useState(false);
  const [guidedTaskPhase, setGuidedTaskPhase] =
    useState<GuidedTaskPhase>("idle");
  const [guidedTaskCancelled, setGuidedTaskCancelled] = useState(false);
  const [guidedTaskRunMode, setGuidedTaskRunMode] = useState<
    "chat" | "terminal" | null
  >(null);
  const guidedTerminalConversationIdRef = useRef<string | null>(null);
  const guidedTerminalSessionIdRef = useRef<string | null>(null);
  const guidedTerminalSawActiveRef = useRef(false);
  const guidedTerminalSawSustainedOutputRef = useRef(false);
  const guidedTerminalTrackingArmedRef = useRef(false);
  const firstRunProjectPickPendingRef = useRef(false);
  const [firstRunProjectPending, setFirstRunProjectPending] = useState(false);
  const [firstTaskGoal, setFirstTaskGoal] = useState<FirstTaskGoal>("quick-win");
  const guidedTaskPromptRef = useRef("");
  const guidedChatConversationIdRef = useRef<string | null>(null);
  const preparedTaskModeChangePendingRef = useRef<boolean | null>(null);
  const [customPrompts, setCustomPrompts] = useState<CustomPrompt[]>([]);
  const customPromptRequestRef = useRef(0);
  const [searchFocusSignal, setSearchFocusSignal] = useState(0);
  const [composerFocusSignal, setComposerFocusSignal] = useState(0);
  const [dictationSignal, setDictationSignal] = useState(0);
  const [taskFindOpen, setTaskFindOpen] = useState(false);
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [initialDataLoaded, setInitialDataLoaded] = useState(false);
  // The first completed task on any lane (ADR 0018). It persists, so a
  // relaunch keeps the trial card instead of a launch wall.
  const [firstResultSeen, setFirstResultSeen] = useState(() =>
    ONBOARDING_REPLAY_ENABLED ? false : readFirstResultSeen(),
  );
  const firstResultSeenRef = useRef(firstResultSeen);
  const [projects, setProjects] = useState<Project[]>([]);
  // The Projects tree shows its empty state only after the first load.
  const [projectsLoaded, setProjectsLoaded] = useState(false);
  const projectSelectionPreferenceRef = useRef<ProjectSelectionPreference>(
    readProjectSelectionPreference(),
  );
  const [selectedProjectId, setSelectedProjectId] = useState<number | null>(() => {
    const preference = projectSelectionPreferenceRef.current;
    return typeof preference === "number" ? preference : null;
  });
  // The tray listener registers once and reads the current pick here.
  const selectedProjectIdRef = useRef(selectedProjectId);
  useEffect(() => {
    selectedProjectIdRef.current = selectedProjectId;
  }, [selectedProjectId]);
  const [projectSwitcherOpen, setProjectSwitcherOpen] = useState(false);
  const [projectModal, setProjectModal] = useState<
    "add-folder" | "clone-github" | null
  >(null);
  const [projectPathDraft, setProjectPathDraft] = useState("");
  const [cloneUrlDraft, setCloneUrlDraft] = useState("");
  const [cloneDestParentDraft, setCloneDestParentDraft] = useState("~/programming");
  const [projectBusy, setProjectBusy] = useState(false);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [workspaces, setWorkspaces] = useState<WorkspaceStatus[]>([]);
  // The Projects tree lists the workspaces of every project (ADR 0010).
  const featureFlags = useSyncExternalStore(
    featureFlagStore.subscribe,
    featureFlagStore.getSnapshot,
  );
  const [workspaceOverview, setWorkspaceOverview] = useState<WorkspaceOverview[]>([]);
  const [workspaceBaseRefs, setWorkspaceBaseRefs] = useState<string[]>(["HEAD"]);
  const [selectedWorktreeBaseRef, setSelectedWorktreeBaseRef] = useState("HEAD");
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState<string | null>(null);
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  const [workspaceError, setWorkspaceError] = useState<string | null>(null);
  const [workspaceNotice, setWorkspaceNotice] = useState<string | null>(null);
  const [localEnvironments, setLocalEnvironments] = useState<
    LocalEnvironmentDescriptor[]
  >([]);
  const [selectedEnvironmentConfigPath, setSelectedEnvironmentConfigPath] =
    useState<string | null>(null);
  const [environmentActionBusy, setEnvironmentActionBusy] = useState(false);
  const [trustPrompt, setTrustPrompt] = useState<Project | null>(null);
  const [trustBusy, setTrustBusy] = useState(false);
  const [branchStatus, setBranchStatus] = useState<BranchStatus | null>(null);
  const [prBusy, setPrBusy] = useState(false);
  const [prMessage, setPrMessage] = useState<string | null>(null);
  const [savingsTotals, setSavingsTotals] = useState<SavingsTotals | null>(null);
  const [todaySavings, setTodaySavings] = useState<SavingsToday | null>(null);
  const [weeklySavings, setWeeklySavings] = useState<SavingsToday | null>(null);
  const [dailyTokenActivity, setDailyTokenActivity] = useState<
    DailyTokenActivity[]
  >([]);
  const [agentCapabilities, setAgentCapabilities] =
    useState<AgentCapabilities | null>(null);
  const [draftAgentMode, setDraftAgentMode] = useState(agentModeForNewDraft);
  // "Text conversation" of the launcher or the "+" menu (ADR 0020): the
  // composer starts a chat conversation, also in Pro mode. It holds only
  // while the draft composes.
  const [textDraft, setTextDraft] = useState(false);
  const textDraftRef = useRef(false);
  const [draftApprovalPolicy, setDraftApprovalPolicy] =
    useState<ApprovalPolicy | null>(null);
  const [draftWorkflowMode, setDraftWorkflowMode] =
    useState<WorkflowMode>("default");
  const [subagentsByConversation, setSubagentsByConversation] = useState<
    Record<string, SubagentThread[]>
  >({});
  const [noticesByConversation, setNoticesByConversation] = useState<
    Record<string, ThreadNotice[]>
  >({});
  const [terminalOpeningByConversation, setTerminalOpeningByConversation] =
    useState<Record<string, TerminalOpeningState>>({});
  const [approvalDecisionsPending, setApprovalDecisionsPending] = useState<
    Set<string>
  >(() => new Set());
  // navigate() is the one writer. The current conversation is derived from
  // navigation and, with workspace tabs, from the active tab of the strip.
  const workspaceTabsSnapshot = useSyncExternalStore(
    workspaceTabsStore.subscribe,
    workspaceTabsStore.getSnapshot,
  );
  const [navigation, setNavigation] = useState<Navigation>(initialNavigation);
  const navigationRef = useRef(navigation);
  // The restore keeps a choice that the user made before the facts loaded.
  const startNavigationRef = useRef(navigation);
  const currentConversationId = navigationConversationId(
    navigation,
    workspaceTabsSnapshot,
  );
  // Each conversation, and each new-task composer, has its own bottom panel
  // state with workspace tabs.
  const bottomPanelKey = currentConversationId ?? `composer:${
    navigation.kind === "workspace" ? navigation.workspaceId : "none"
  }`;
  bottomPanelKeyRef.current = bottomPanelKey;
  const terminalOpen = featureFlags.workspaceTabs
    ? openBottomPanels.has(bottomPanelKey)
    : legacyTerminalOpen;
  // The conversation that `messages` belongs to. A tab switch shows a
  // loading state until it matches, never the previous messages.
  const [messagesConversationId, setMessagesConversationId] = useState<
    string | null
  >(null);
  const messagesConversationIdRef = useRef<string | null>(null);
  function markMessagesConversation(conversationId: string | null) {
    messagesConversationIdRef.current = conversationId;
    setMessagesConversationId(conversationId);
  }
  // True once the first conversation list arrived. The tab facts are
  // incomplete before it.
  const initialConversationsLoadedRef = useRef(false);
  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [models, setModels] = useState<ChatModel[]>(defaultModels);
  const [credentialProfiles, setCredentialProfiles] = useState<
    CredentialProfile[]
  >([]);
  const [credentialProfilesLoaded, setCredentialProfilesLoaded] =
    useState(false);
  const refreshConnectGateAccounts = useCallback(async (
    read: Promise<AgentAccountsRead> = readAgentAccounts(),
  ) => {
    const generation = connectGateRefreshGenerationRef.current + 1;
    connectGateRefreshGenerationRef.current = generation;
    const { laneOptions, probe, profiles } = await read;
    if (connectGateRefreshGenerationRef.current !== generation) {
      return;
    }
    setConnectGateLaneOptions(
      laneOptions.status === "fulfilled" ? laneOptions.value : [],
    );
    if (laneOptions.status === "fulfilled") {
      // Terminal launches read the store copy. Keep it in step after re-login.
      agentSessionStore.applyLaneOptions(laneOptions.value);
    }
    setConnectGateProbe(probe.status === "fulfilled" ? probe.value : null);
    if (profiles.status === "fulfilled") {
      setCredentialProfiles(profiles.value);
    }
    setCredentialProfilesLoaded(true);
    setConnectGateAccountsResolved(true);
  }, []);
  // A wallet switch changes which login each CLI lane reports.
  const walletVersion = useWalletSnapshot().version;
  const walletVersionSeenRef = useRef(walletVersion);
  useEffect(() => {
    if (walletVersionSeenRef.current === walletVersion) return;
    walletVersionSeenRef.current = walletVersion;
    void refreshConnectGateAccounts();
  }, [refreshConnectGateAccounts, walletVersion]);
  const [draftCredentialProfileId, setDraftCredentialProfileId] = useState<
    string | null
  >(null);
  const [selectedModel, setSelectedModelState] = useState(defaultModels[0].id);
  const [selectedEffort, setSelectedEffortState] = useState(
    defaultModels[0].defaultEffort,
  );
  const [draftFastMode, setDraftFastMode] = useState(false);
  const [draft, setDraftState] = useState("");
  const [draftAttachments, setDraftAttachmentsState] = useState<
    ImageAttachment[]
  >([]);
  const [composerMentionSeed, setComposerMentionSeed] = useState<{
    signal: number;
    mentions: string[];
  }>({ signal: 0, mentions: [] });
  const [queuedMessages, setQueuedMessages] = useState<QueuedMessage[]>([]);
  const [chatError, setChatError] = useState<string | null>(null);
  const workspaceBusyRetryRef = useRef<WorkspaceBusyRetryRequest | null>(null);
  const [transcriptScrollSignal, setTranscriptScrollSignal] = useState(0);

  useEffect(() => {
    if (!savingsDrawerOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSavingsDrawerOpen(false);
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [savingsDrawerOpen]);

  useEffect(() => {
    if (currentConversationId) {
      writeLastActiveConversationId(currentConversationId, window.localStorage);
    }
  }, [currentConversationId]);
  const conversationsRef = useRef<ConversationSummary[]>([]);
  const currentConversationIdRef = useRef<string | null>(null);
  const navigationGenerationRef = useRef(0);
  const pendingConversationNavigationRef =
    useRef<PendingConversationNavigation | null>(null);
  // The conversation whose messages load now.
  const pendingConversationLoadRef = useRef<string | null>(null);
  const sendMessageRef = useRef<
    (
      text: string,
      targetConversationId?: string | null,
      options?: SendMessageOptions,
    ) => Promise<void>
  >(() => Promise.resolve());
  const startNewConversationForProjectRef = useRef<
    (projectId: number | null) => void
  >(() => undefined);
  const selectedModelRef = useRef(defaultModels[0].id);
  const selectedEffortRef = useRef(defaultModels[0].defaultEffort);
  const modelsRef = useRef(defaultModels);
  const conversationRequestSettingsRef = useRef(
    new Map<string, ConversationRequestSettings>(),
  );
  const conversationDefaultsRef = useRef<ConversationDefaults>({
    model: defaultModels[0].id,
    effort: defaultModels[0].defaultEffort,
    agentMode: true,
    approvalPolicy: null,
    fastMode: false,
  });
  const hasExplicitDefaultModelRef = useRef(false);
  const conversationDefaultsSaveRef = useRef<Promise<void>>(Promise.resolve());
  const lastServiceTierByConversationRef = useRef(new Map<string, string>());
  const selectedWorkspaceIdRef = useRef<string | null>(null);
  // A tree click can name a workspace of another project. That project's
  // workspace list loads later, so the choice waits here.
  const pendingWorkspaceSelectionRef = useRef<{
    projectId: number;
    workspaceId: string;
  } | null>(null);
  const queuedRef = useRef<QueuedMessage[]>([]);
  const deletedConversationIdsRef = useRef(new Set<string>());
  const lastTodayDateRef = useRef(localDateKey());
  const lastNotifiedAtRef = useRef(0);
  const approvalDecisionsPendingRef = useRef(new Set<string>());
  const sidecarRestartPendingRef = useRef(false);
  const draftRef = useRef("");
  const draftsByConversationRef = useRef(new Map<string, string>());
  const draftSaveTimersRef = useRef(new Map<string, number>());
  const draftSavePromisesRef = useRef(
    new Map<string, Promise<ConversationSummary | null>>(),
  );
  const draftAttachmentsRef = useRef<ImageAttachment[]>([]);
  const draftAttachmentsByConversationRef = useRef(
    new Map<string, ImageAttachment[]>(),
  );
  const conversationTurnStoreRef = useRef<ReturnType<
    typeof createConversationTurnStore
  > | null>(null);
  if (!conversationTurnStoreRef.current) {
    conversationTurnStoreRef.current = createConversationTurnStore({
      subscribe: (handler) =>
        ipc.events.listen<ChatDeltaPayload>("chat_delta", handler),
      context: () => ({
        currentConversationId: currentConversationIdRef.current,
        deletedConversationIds: deletedConversationIdsRef.current,
        navigationGeneration: navigationGenerationRef.current,
        chatRequestFailedText: tRef.current("Chat request failed"),
        now: new Date().toISOString(),
      }),
      effects: {
        setMessages,
        openConversation: (conversationId) => {
          void openConversation(conversationId, { load: false });
        },
        setChatError,
        setBranchStatus,
        setConversationRunning,
        loadConversations() {
          void loadConversations().catch((error) =>
            console.error("chat_delta: conversation refresh failed", error),
          );
        },
        refreshStatus() {
          void refreshStatus();
        },
        refreshSavingsTotals() {
          void refreshSavingsTotals();
        },
        loadSubagents(conversationId) {
          void loadSubagents(conversationId).catch(() => undefined);
        },
        loadProjects() {
          void loadProjects().catch((error) =>
            console.error("chat_delta: project refresh failed", error),
          );
        },
        refreshAgentSessions() {
          void agentSessionStore.refreshSessions().catch(() => undefined);
        },
        notifyTerminal,
        notifyApproval,
        handleGoalSignal,
        migrateConversationTurnState(
          fromConversationId,
          toConversationId,
          pendingNewTurn,
        ) {
          pendingNewTurn.conversationId = toConversationId;
          applyConversationTurnStateMigration(
            fromConversationId,
            toConversationId,
          );
        },
        switchComposerDraft,
        requestChatCancel(conversationId) {
          void requestChatCancel(
            conversationId,
            conversationTurnState(conversationId)?.pendingNewTurn ?? null,
          );
        },
        requestChatCancelAndRevert(conversationId) {
          void requestChatCancelAndRevert(
            conversationId,
            conversationTurnState(conversationId)?.pendingNewTurn ?? null,
          );
        },
        dispatchQueuedNewThreadMessage,
      },
    });
  }
  const conversationTurnStore = conversationTurnStoreRef.current;
  const conversationTurnSnapshot = useSyncExternalStore(
    conversationTurnStore.subscribe,
    conversationTurnStore.getSnapshot,
  );
  const {
    compactionsByConversation,
    conversationTurnStates,
    pendingApprovals,
    reasoningByConversation,
    runningConversationIds,
    taskPlansByConversation,
    toolCallsByConversation,
    turnDiffsByConversation,
    unviewedConversationIds,
  } = conversationTurnSnapshot;
  const {
    setCompactionsByConversation,
    setPendingApprovals,
    setReasoningByConversation,
    setTaskPlansByConversation,
    setToolCallsByConversation,
    setTurnDiffsByConversation,
    setUnviewedConversationIds,
  } = conversationTurnStore;

  function conversationTurnKey(conversationId: string | null) {
    return conversationId ?? NEW_THREAD;
  }

  function setConversationRunning(conversationId: string | null, running: boolean) {
    conversationTurnStore.setConversationRunning(conversationId, running);
  }

  function conversationTurnState(conversationId: string | null) {
    return conversationTurnStore.getConversationTurnState(conversationId);
  }

  function updateConversationTurnState(
    conversationId: string | null,
    patch: Partial<TurnState>,
  ) {
    return conversationTurnStore.updateConversationTurnState(
      conversationId,
      patch,
    );
  }

  function applyConversationTurnStateMigration(
    fromConversationId: string | null,
    toConversationId: string,
  ) {
    const fromKey = conversationTurnKey(fromConversationId);
    const state = conversationTurnState(toConversationId);
    if (!state) {
      return null;
    }
    const settings = conversationRequestSettingsRef.current.get(fromKey);
    if (settings) {
      conversationRequestSettingsRef.current.delete(fromKey);
      conversationRequestSettingsRef.current.set(toConversationId, settings);
    }
    // Presentation state keyed by the draft turn moves with it. Left behind,
    // the "Opening terminal…" card and its notices leak into the next fresh
    // draft as if they belonged to the new chat.
    moveTerminalOpening(fromKey, toConversationId);
    setNoticesByConversation((current) => {
      const pendingNotices = current[fromKey];
      if (!pendingNotices || pendingNotices.length === 0 || fromKey === toConversationId) {
        return current;
      }
      const next = { ...current };
      delete next[fromKey];
      next[toConversationId] = [
        ...(current[toConversationId] ?? []),
        ...pendingNotices,
      ];
      return next;
    });
    return state;
  }

  function migrateConversationTurnState(
    fromConversationId: string | null,
    toConversationId: string,
    expectedPendingNewTurn?: PendingNewTurn,
  ) {
    const fromKey = conversationTurnKey(fromConversationId);
    if (fromKey === toConversationId) {
      return conversationTurnState(toConversationId) ?? null;
    }
    const sourceState = conversationTurnState(fromConversationId);
    if (
      !sourceState ||
      (expectedPendingNewTurn &&
        sourceState.pendingNewTurn !== expectedPendingNewTurn)
    ) {
      return null;
    }
    const state = conversationTurnStore.migrateConversationTurnState(
      fromConversationId,
      toConversationId,
      currentConversationIdRef.current,
    );
    if (!state) {
      return null;
    }
    return applyConversationTurnStateMigration(
      fromConversationId,
      toConversationId,
    );
  }

  function runningTurnKeyForView(conversationId: string | null) {
    return conversationTurnStore.runningTurnKeyForView(conversationId);
  }

  function resolvePendingNewTurnConversation(
    pendingNewTurn: PendingNewTurn,
    resolvedConversationId: string,
  ) {
    const adoptedConversationId = pendingNewTurn.conversationId;
    if (adoptedConversationId === null) {
      pendingNewTurn.conversationId = resolvedConversationId;
      migrateConversationTurnState(null, resolvedConversationId);
      return;
    }
    if (adoptedConversationId === resolvedConversationId) {
      return;
    }

    const migrated = migrateConversationTurnState(
      adoptedConversationId,
      resolvedConversationId,
      pendingNewTurn,
    );
    pendingNewTurn.conversationId = resolvedConversationId;
    conversationTurnStore.deleteStreamBuffer(adoptedConversationId);
    if (
      !conversationTurnStore
        .getSnapshot()
        .streamBuffersByConversation.has(resolvedConversationId)
    ) {
      conversationTurnStore.setStreamBuffer(resolvedConversationId, "");
    }
    console.error("Corrected a mismatched new-thread conversation route", {
      adoptedConversationId,
      resolvedConversationId,
      turnStateMoved: migrated !== null,
    });
  }

  function clearConversationTurnState(conversationId: string | null) {
    conversationTurnStore.clearConversationTurnState(conversationId);
  }

  function removeDraftConversation(conversationId: string) {
    const timer = draftSaveTimersRef.current.get(conversationId);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      draftSaveTimersRef.current.delete(conversationId);
    }
    const next = conversationsRef.current.filter(
      (conversation) => conversation.id !== conversationId,
    );
    conversationsRef.current = next;
    setConversations(next);
    if (pendingTabSummariesRef.current.delete(conversationId)) syncTabFacts();
    // A foreign key cascade removed its tab row in Rust.
    if (featureFlagStore.getSnapshot().workspaceTabs) workspaceTabsStore.refresh();
    draftsByConversationRef.current.delete(conversationId);
    draftAttachmentsByConversationRef.current.delete(conversationId);
    conversationRequestSettingsRef.current.delete(conversationId);
    if (currentConversationIdRef.current === conversationId) {
      startComposing();
      setMessages([]);
      setDraftAgentMode(agentModeForNewDraft());
    }
  }

  function applyDraftSaveResult(
    conversationId: string,
    summary: ConversationSummary | null,
  ) {
    if (summary) {
      const next = upsertConversation(conversationsRef.current, summary);
      conversationsRef.current = next;
      setConversations(next);
      return;
    }
    removeDraftConversation(conversationId);
  }

  function trackDraftSave(
    conversationId: string,
    operation: Promise<ConversationSummary | null>,
  ) {
    draftSavePromisesRef.current.set(conversationId, operation);
    void operation
      .then((summary) => applyDraftSaveResult(conversationId, summary))
      .catch((error) => setChatError(errorMessage(error)))
      .finally(() => {
        if (draftSavePromisesRef.current.get(conversationId) === operation) {
          draftSavePromisesRef.current.delete(conversationId);
        }
      });
    return operation;
  }

  function persistConversationDraft(conversationId: string, value: string) {
    const previous = draftSavePromisesRef.current.get(conversationId);
    const operation = (previous ?? Promise.resolve(null)).then(() =>
      ipc.storage.setConversationDraft({
        conversationId,
        draft: value,
      }),
    );
    return trackDraftSave(conversationId, operation);
  }

  function scheduleConversationDraftSave(
    conversationId: string,
    value: string,
  ) {
    const existingTimer = draftSaveTimersRef.current.get(conversationId);
    if (existingTimer !== undefined) {
      window.clearTimeout(existingTimer);
    }
    const timer = window.setTimeout(() => {
      draftSaveTimersRef.current.delete(conversationId);
      void persistConversationDraft(conversationId, value);
    }, 300);
    draftSaveTimersRef.current.set(conversationId, timer);
  }

  function createConversationDraft(value: string) {
    const conversationId = window.crypto.randomUUID();
    const projectId = selectedProjectIdRef.current;
    const hasProject = projectId !== null;
    const mode = modeForComposerDraft(proMode && hasProject, textDraftRef.current);
    const model = selectedModelRef.current;
    const modelInfo = modelsRef.current.find((item) => item.id === model);
    const agentBackend = agentBackendForModel(modelInfo, mode === "agent");
    const agentSessionModel = agentBackend
      ? requestModelForChatModel(modelInfo, model)
      : null;

    const draftWorkspaceId = featureFlagStore.getSnapshot().workspaceTabs
      ? composingWorkspaceId(projectId)
      : selectedWorkspaceIdRef.current;
    if (featureFlagStore.getSnapshot().workspaceTabs) {
      // The draft tab stays while the row is created. Typing goes to the
      // new conversation at once.
      composerDraftOwnerRef.current = conversationId;
      markMessagesConversation(conversationId);
      navigate(
        draftWorkspaceId
          ? workspaceNavigation(draftWorkspaceId, true, conversationId)
          : looseNavigation(conversationId),
      );
    } else {
      void openConversation(conversationId, { load: false });
    }
    draftsByConversationRef.current.set(conversationId, value);
    conversationRequestSettingsRef.current.set(conversationId, {
      model,
      effort: selectedEffortRef.current,
      agentBackend,
      agentSessionModel,
    });

    const operation = ipc.storage.createConversationDraft({
        conversationId,
        draft: value,
        model,
        projectId,
        workspaceId: draftWorkspaceId,
        mode,
        agentMode: draftAgentMode && hasProject,
        workflowMode: draftWorkflowMode,
        credentialProfileId: draftCredentialProfileId,
        agentBackend,
        agentSessionModel,
        approvalPolicy: draftApprovalPolicy,
        fastMode: draftFastMode,
      }).catch((error) => {
      removeDraftConversation(conversationId);
      throw error;
    });
    trackDraftSave(conversationId, operation);
    if (featureFlagStore.getSnapshot().workspaceTabs) {
      // The row exists now: the draft tab becomes the conversation tab in
      // place. A user who left the draft gets the tab in the background.
      void operation
        .then((summary) => {
          if (!summary) return;
          const stillDraft = currentConversationIdRef.current === conversationId;
          void openConversation(conversationId, {
            summary,
            load: false,
            activate: stillDraft,
          });
        })
        .catch(() => undefined);
    }
  }

  async function flushConversationDraft(conversationId: string) {
    const timer = draftSaveTimersRef.current.get(conversationId);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      draftSaveTimersRef.current.delete(conversationId);
      return persistConversationDraft(
        conversationId,
        draftsByConversationRef.current.get(conversationId) ?? "",
      );
    }
    const pending = draftSavePromisesRef.current.get(conversationId);
    if (pending) {
      return pending;
    }
    return (
      conversationsRef.current.find(
        (conversation) => conversation.id === conversationId,
      ) ?? null
    );
  }

  function clearComposerDraftLocally(conversationId: string | null) {
    if (conversationId) {
      draftsByConversationRef.current.set(conversationId, "");
      draftAttachmentsByConversationRef.current.set(conversationId, []);
    }
    if (currentConversationIdRef.current !== conversationId) {
      return;
    }
    draftRef.current = "";
    draftAttachmentsRef.current = [];
    setDraftState("");
    setDraftAttachmentsState([]);
  }

  function restoreComposerDraftLocally(
    conversationId: string,
    value: string,
    attachments: ImageAttachment[],
  ) {
    draftsByConversationRef.current.set(conversationId, value);
    draftAttachmentsByConversationRef.current.set(conversationId, attachments);
    if (currentConversationIdRef.current !== conversationId) {
      return;
    }
    draftRef.current = value;
    draftAttachmentsRef.current = attachments;
    setDraftState(value);
    setDraftAttachmentsState(attachments);
  }

  function setDraft(next: SetStateAction<string>) {
    const resolved =
      typeof next === "function" ? next(draftRef.current) : next;
    draftRef.current = resolved;
    const conversationId = currentConversationIdRef.current;
    if (conversationId) {
      draftsByConversationRef.current.set(conversationId, resolved);
      scheduleConversationDraftSave(conversationId, resolved);
    } else if (resolved.trim()) {
      createConversationDraft(resolved);
    }
    setDraftState(resolved);
  }

  function setDraftAttachments(next: SetStateAction<ImageAttachment[]>) {
    const resolved =
      typeof next === "function" ? next(draftAttachmentsRef.current) : next;
    draftAttachmentsRef.current = resolved;
    const conversationId = currentConversationIdRef.current;
    if (conversationId) {
      draftAttachmentsByConversationRef.current.set(conversationId, resolved);
    }
    setDraftAttachmentsState(resolved);
  }

  // The conversation whose draft the composer holds. With workspace tabs
  // the current conversation can change before the composer switches.
  const composerDraftOwnerRef = useRef<string | null>(null);
  function switchComposerDraft(
    conversationId: string | null,
    carryNewConversationDraft = false,
  ) {
    const previousConversationId = featureFlagStore.getSnapshot().workspaceTabs
      ? composerDraftOwnerRef.current
      : currentConversationIdRef.current;
    composerDraftOwnerRef.current = conversationId;
    if (previousConversationId === conversationId) {
      return;
    }
    if (previousConversationId) {
      draftsByConversationRef.current.set(
        previousConversationId,
        draftRef.current,
      );
      draftAttachmentsByConversationRef.current.set(
        previousConversationId,
        draftAttachmentsRef.current,
      );
    }
    const nextDraft = conversationId
      ? previousConversationId === null && carryNewConversationDraft
        ? draftRef.current
        : (draftsByConversationRef.current.get(conversationId) ??
          conversationsRef.current.find(
            (conversation) => conversation.id === conversationId,
          )?.draft ??
          "")
      : "";
    const nextAttachments = conversationId
      ? previousConversationId === null && carryNewConversationDraft
        ? draftAttachmentsRef.current
        : (draftAttachmentsByConversationRef.current.get(conversationId) ?? [])
      : [];
    if (conversationId && carryNewConversationDraft) {
      draftsByConversationRef.current.set(conversationId, nextDraft);
      draftAttachmentsByConversationRef.current.set(
        conversationId,
        nextAttachments,
      );
    }
    if (conversationId && !draftsByConversationRef.current.has(conversationId)) {
      draftsByConversationRef.current.set(conversationId, nextDraft);
    }
    draftRef.current = nextDraft;
    draftAttachmentsRef.current = nextAttachments;
    setDraftState(nextDraft);
    setDraftAttachmentsState(nextAttachments);
  }

  function beginNavigation() {
    navigationGenerationRef.current += 1;
    return navigationGenerationRef.current;
  }

  function navigationIsCurrent(generation: number) {
    return navigationGenerationRef.current === generation;
  }

  // The one writer of navigation. The ref changes at once, so code that runs
  // before the next render reads the new conversation.
  function navigate(next: Navigation) {
    navigationRef.current = next;
    currentConversationIdRef.current = navigationConversationId(
      next,
      workspaceTabsStore.getSnapshot(),
    );
    setNavigation((current) => (sameNavigation(current, next) ? current : next));
    writeStoredNavigation(next, browserStorage());
  }

  // The one entry point that shows a conversation. It loads the messages and
  // shows the conversation in the same render as its messages. With
  // workspace tabs, it opens the tab of the conversation (an explicit
  // intent) and shows it at once. The messages load behind it.
  async function openConversation(
    conversationId: string,
    options: OpenConversationOptions = {},
  ) {
    if (featureFlagStore.getSnapshot().workspaceTabs) {
      return openConversationTab(conversationId, options);
    }
    const show = () => navigate(legacyNavigation(conversationId));
    if (options.load === false) {
      show();
      return null;
    }
    return loadMessages(conversationId, {
      navigationGeneration: options.navigationGeneration,
      onApplied: show,
    });
  }

  // The tab facts of the store. A conversation that the caller just created
  // can be missing from the list, so its summary stays until the list has it.
  const pendingTabSummariesRef = useRef(new Map<string, ConversationSummary>());
  const workspaceOverviewRef = useRef<WorkspaceOverview[] | null>(null);
  function currentTabFacts(extra?: ConversationSummary) {
    if (extra) pendingTabSummariesRef.current.set(extra.id, extra);
    const listed = conversationsRef.current;
    const listedIds = new Set(listed.map((conversation) => conversation.id));
    for (const id of [...pendingTabSummariesRef.current.keys()]) {
      if (listedIds.has(id) && id !== extra?.id) pendingTabSummariesRef.current.delete(id);
    }
    const merged = [
      ...listed.filter((conversation) => conversation.id !== extra?.id),
      ...[...pendingTabSummariesRef.current.values()].filter(
        (conversation) => !listedIds.has(conversation.id) || conversation.id === extra?.id,
      ),
    ];
    return tabFacts({
      conversations: initialConversationsLoadedRef.current ? merged : null,
      workspaces: workspaceOverviewRef.current,
    });
  }

  function syncTabFacts(extra?: ConversationSummary) {
    workspaceTabsStore.setFacts(currentTabFacts(extra));
  }

  // The workspace for a new task: the strip on screen, else the selected
  // workspace, else the primary workspace of the selected project.
  function composingWorkspaceId(projectId = selectedProjectIdRef.current) {
    const nav = navigationRef.current;
    if (nav.kind === "workspace" && nav.workspaceId) {
      const facts = workspaceTabsStore.getSnapshot().facts;
      const workspace = facts.workspaces.get(nav.workspaceId);
      if (!workspace || workspace.projectId === projectId) return nav.workspaceId;
    }
    if (projectId === null) return null;
    const selected = selectedWorkspaceIdRef.current;
    const facts = workspaceTabsStore.getSnapshot().facts;
    if (selected && facts.workspaces.get(selected)?.projectId === projectId) return selected;
    return `primary-${projectId}`;
  }

  // The new-task composer: no conversation is current. With workspace tabs
  // this is the draft tab of a strip, or the loose view without a project.
  function startComposing(projectId: number | null = selectedProjectIdRef.current) {
    if (!featureFlagStore.getSnapshot().workspaceTabs) {
      navigate(legacyNavigation(null));
      return;
    }
    const workspaceId = composingWorkspaceId(projectId);
    markMessagesConversation(null);
    navigate(workspaceId ? workspaceNavigation(workspaceId, true) : looseNavigation(null));
  }

  // The scope of the conversation panel in the mount registry.
  function conversationPanelScope(conversationId: string) {
    return terminalPanelStorageKey({ conversationId, workspaceId: null, projectId: null });
  }

  // The scope of a tab body panel in the mount registry.
  function tabPanelScope(key: TabKey) {
    const parsed = parseTabKey(key);
    return parsed?.kind === "shell"
      ? terminalPanelStorageKey({
          conversationId: null,
          shellTabId: parsed.id,
          workspaceId: null,
          projectId: null,
        })
      : conversationPanelScope(parsed?.id ?? "");
  }

  // The tab whose body shows now: the visible tab of the strip, else the
  // conversation of the loose view.
  function shownTabKey(): TabKey | null {
    const visible = navigationTabKey(navigationRef.current, workspaceTabsStore.getSnapshot());
    if (visible) return visible;
    const conversationId = currentConversationIdRef.current;
    return conversationId ? conversationTabKey(conversationId) : null;
  }

  function shellTabsEnabled() {
    const flags = featureFlagStore.getSnapshot();
    return flags.workspaceTabs && flags.shellTabs;
  }

  // Focus goes where the user expects after a tab change: the active pane
  // of a terminal tab, else the composer. The panel can mount a few frames
  // later, so the request retries for a short time.
  const tabFocusRequestRef = useRef(0);
  function focusVisibleTab() {
    const request = ++tabFocusRequestRef.current;
    let attempts = 0;
    const attempt = () => {
      if (request !== tabFocusRequestRef.current) return;
      // A menu, a dialog or a tab rename that the user opened since the
      // request keeps the focus.
      if (
        document.activeElement?.closest(
          '[role="menu"], [role="dialog"], [data-tab-rename-input="true"]',
        )
      ) {
        return;
      }
      const key = shownTabKey();
      const handle = key ? terminalPanelRegistry.request(tabPanelScope(key)) : null;
      const body = key
        ? document.querySelector<HTMLElement>(
            `[data-workspace-tab-body="${CSS.escape(key)}"]`,
          )
        : null;
      if (body && body.dataset.active === "true") {
        if (handle?.focusActivePane()) return;
      } else if (!body) {
        setComposerFocusSignal((current) => current + 1);
        return;
      }
      attempts += 1;
      if (attempts < 90) window.requestAnimationFrame(attempt);
    };
    window.requestAnimationFrame(attempt);
  }

  // A workspace row or chip: show the strip and its active tab. It writes
  // no tab row.
  function showWorkspace(workspaceId: string) {
    const snapshot = workspaceTabsStore.getSnapshot();
    const view = stripView(snapshot.tabs, snapshot.facts, workspaceId);
    const parsed = parseTabKey(view.activeKey);
    const target = parsed?.kind === "conversation" ? parsed.id : null;
    setDotOpen(false);
    switchComposerDraft(target);
    navigate(workspaceNavigation(workspaceId));
    if (!target) {
      markMessagesConversation(null);
      setMessages([]);
    } else if (messagesConversationIdRef.current !== target) {
      void loadMessages(target, { navigationGeneration: beginNavigation() });
    }
    focusVisibleTab();
  }

  // A strip tab click, a number chord or a cycle: show that tab.
  function activateWorkspaceTab(key: TabKey) {
    const parsed = parseTabKey(key);
    if (parsed?.kind === "shell") {
      if (!shellTabsEnabled()) return;
      for (const terminal of terminalSessionsRef.current) {
        if (terminal.shellTabId === parsed.id) terminalActivityStore.markViewed(terminal.id);
      }
      const snapshot = workspaceTabsStore.getSnapshot();
      const row = snapshot.tabs.rows[key];
      const home = row ? homeWorkspaceId(row, snapshot.facts) : null;
      if (!home) return;
      workspaceTabsStore.activate(key);
      showShellTab(home);
      return;
    }
    if (parsed?.kind !== "conversation") return;
    for (const terminal of terminalSessionsRef.current) {
      if (terminal.conversationId === parsed.id) terminalActivityStore.markViewed(terminal.id);
    }
    void openConversation(parsed.id);
  }

  // Shows the active tab of a strip after a shell tab became active. A
  // shell tab has no conversation, so the chat surface and the composer
  // hide.
  function showShellTab(workspaceId: string) {
    setDotOpen(false);
    switchComposerDraft(null);
    markMessagesConversation(null);
    setMessages([]);
    navigate(workspaceNavigation(workspaceId));
    focusVisibleTab();
  }

  // The first shell of a new shell tab, and the command that a Run action
  // types into it. The panel opens the shell when it mounts.
  const firstShellRequestsRef = useRef(new Map<TabKey, { input: string | null }>());

  // Cmd+T, Ctrl+Shift+T and "+" > New Terminal: a shell tab after the
  // visible tab. A Run action names the tab and types its command.
  function openShellTab(
    options: { title?: string | null; input?: string | null } = {},
  ): TabKey | null {
    if (!shellTabsEnabled()) return null;
    const nav = navigationRef.current;
    if (nav.kind !== "workspace") return null;
    const key = shellTabKey(window.crypto.randomUUID());
    const view = visibleStrip();
    firstShellRequestsRef.current.set(key, { input: options.input ?? null });
    const outcome = workspaceTabsStore.open({
      key,
      shellWorkspaceId: nav.workspaceId,
      afterKey: nav.composing ? null : (view?.activeKey ?? null),
      title: options.title ?? null,
    });
    if (!outcome.queued) {
      firstShellRequestsRef.current.delete(key);
      setTabCloseError(t("The terminal tab could not open."));
      return null;
    }
    setTabCloseError(null);
    showShellTab(nav.workspaceId);
    return key;
  }

  // Moves a terminal without an owner into a new shell tab of a workspace
  // (ADR 0010 "Open in tab", ADR 0020 adoption). The tab never becomes
  // active here. It exists only after Rust accepted the owner, so its panel
  // lists the terminal. A failure removes the empty tab and throws.
  async function moveTerminalIntoShellTab(sessionId: string, workspaceId: string) {
    const terminal = terminalSessionsRef.current.find((item) => item.id === sessionId);
    if (!terminal) throw new Error(t("The terminal tab could not open."));
    const shellTabId = window.crypto.randomUUID();
    const key = shellTabKey(shellTabId);
    const outcome = workspaceTabsStore.open({
      key,
      shellWorkspaceId: workspaceId,
      title: terminal.title?.trim() || null,
      activate: false,
    });
    if (!outcome.queued) throw new Error(t("The terminal tab could not open."));
    await workspaceTabsStore.whenIdle();
    try {
      if (!workspaceTabsStore.getSnapshot().tabs.rows[key]) {
        throw new Error(t("The terminal tab could not open."));
      }
      await ipc.terminal.setShellTab({ sessionId, shellTabId });
    } catch (error) {
      if (workspaceTabsStore.getSnapshot().tabs.rows[key]) workspaceTabsStore.close(key);
      throw error;
    }
    await refreshTerminalSessions().catch(() => undefined);
    return key;
  }

  // "Open in tab" of a terminal row in the Chats area (ADR 0020): the tab
  // goes to the workspace that holds the terminal folder, else to the
  // strip on screen, and it shows at once.
  async function openShellInTab(sessionId: string) {
    if (!shellTabsEnabled()) return;
    // An adoption step of this terminal runs now. Its tab shows when it
    // ends, and no second move starts.
    const running = adoptionStepsRef.current.get(sessionId);
    if (running) {
      const key = await running;
      const row = key ? workspaceTabsStore.getSnapshot().tabs.rows[key] : null;
      if (key && row?.shellWorkspaceId) {
        setTabCloseError(null);
        workspaceTabsStore.activate(key);
        showShellTab(row.shellWorkspaceId);
        return;
      }
    }
    const terminal = terminalSessionsRef.current.find((item) => item.id === sessionId);
    if (!terminal) return;
    const nav = navigationRef.current;
    const workspaceId =
      adoptionWorkspace(terminal, workspaceOverviewRef.current ?? [])?.id ??
      (nav.kind === "workspace" ? nav.workspaceId : null);
    if (!workspaceId) {
      setTabCloseError(t("Open a project before you open a terminal tab."));
      return;
    }
    // A manual move counts as the adoption of the terminal.
    rememberAdoption({ terminals: [sessionId] });
    let key: TabKey;
    try {
      key = await moveTerminalIntoShellTab(sessionId, workspaceId);
    } catch (error) {
      setTabCloseError(errorMessage(error));
      return;
    }
    setTabCloseError(null);
    workspaceTabsStore.activate(key);
    showShellTab(workspaceId);
  }

  // Terminal adoption (ADR 0020). Each live terminal and each live agent
  // conversation without a tab gets one, at most once per viewer. The
  // decisions live in terminalAdoption.ts. A new tab never becomes active.
  const adoptionMemoryRef = useRef<AdoptionMemory | null>(null);
  function rememberAdoption(ids: { terminals?: string[]; conversations?: string[] }) {
    const current = (adoptionMemoryRef.current ??= readAdoptionMemory(browserStorage()));
    const next = withAdopted(current, ids);
    if (next === current) return;
    adoptionMemoryRef.current = next;
    writeAdoptionMemory(next, browserStorage());
  }
  const adoptionPendingRef = useRef(new Set<string>());
  // Terminals whose folder has no adoptable git root. They stay in the
  // Chats area until the next start.
  const adoptionUnresolvableRef = useRef(new Set<string>());
  // The adoptable git roots that Rust gave in this run. A known root skips
  // a second git call.
  const adoptionRootsRef = useRef(new Map<string, string>());
  // The steps that run now, by terminal or conversation id. A manual
  // "Open in tab" waits for the step of its terminal.
  const adoptionStepsRef = useRef(new Map<string, Promise<TabKey | null>>());
  const adoptionRunningRef = useRef(false);
  const adoptionAgainRef = useRef(false);
  const adoptionInputRef = useRef<{
    terminals: TerminalSession[];
    workspaces: WorkspaceOverview[];
    shellTabs: boolean;
    conversationHomes: Map<string, string | null>;
    liveConversationIds: ReadonlySet<string>;
    tabbedConversationIds: Set<string>;
    paused: boolean;
  } | null>(null);

  // The visible workspace while its strip has no tab. Its first tab would
  // replace the launcher or the composer, so adoption waits there.
  function adoptionWaitWorkspaceId() {
    const nav = navigationRef.current;
    if (nav.kind !== "workspace" || nav.composing) return null;
    return (visibleStrip()?.keys.length ?? 0) === 0 ? nav.workspaceId : null;
  }

  async function runTerminalAdoption() {
    if (adoptionRunningRef.current) {
      adoptionAgainRef.current = true;
      return;
    }
    adoptionRunningRef.current = true;
    try {
      do {
        adoptionAgainRef.current = false;
        const input = adoptionInputRef.current;
        if (!input || input.paused) break;
        const steps = adoptionPlan({
          ...input,
          memory: (adoptionMemoryRef.current ??= readAdoptionMemory(browserStorage())),
          pending: adoptionPendingRef.current,
          unresolvable: adoptionUnresolvableRef.current,
          resolvedRoots: adoptionRootsRef.current,
          waitWorkspaceId: adoptionWaitWorkspaceId(),
        });
        for (const step of steps) {
          if (!adoptionInputRef.current || adoptionInputRef.current.paused) break;
          const id = step.kind === "conversation" ? step.conversationId : step.sessionId;
          const run = runAdoptionStep(step);
          adoptionStepsRef.current.set(id, run);
          try {
            await run;
          } finally {
            adoptionStepsRef.current.delete(id);
          }
        }
      } while (adoptionAgainRef.current);
    } finally {
      adoptionRunningRef.current = false;
    }
  }

  // Runs one adoption step. It gives the key of the new shell tab, if any.
  async function runAdoptionStep(step: AdoptionStep): Promise<TabKey | null> {
    const id = step.kind === "conversation" ? step.conversationId : step.sessionId;
    const pending = adoptionPendingRef.current;
    // The plan can be older than a manual "Open in tab" of the same id.
    const memory = (adoptionMemoryRef.current ??= readAdoptionMemory(browserStorage()));
    const handled = step.kind === "conversation" ? memory.conversations : memory.terminals;
    if (handled.includes(id)) return null;
    pending.add(id);
    try {
      if (step.kind === "conversation") {
        if (step.workspaceId === adoptionWaitWorkspaceId()) return null;
        rememberAdoption({ conversations: [id] });
        workspaceTabsStore.open({ key: conversationTabKey(id), activate: false });
        await workspaceTabsStore.whenIdle();
        return null;
      }
      let workspaceId = step.kind === "shell" ? step.workspaceId : null;
      if (step.kind === "resolve") {
        // Rust refuses the home root, the app data and the temporary
        // folders. Without a root, the terminal stays in the Chats area.
        const root =
          adoptionRootsRef.current.get(id) ??
          (await ipc.project.adoptionRoot({ path: step.cwd }).catch(() => null));
        if (!root) {
          adoptionUnresolvableRef.current.add(id);
          return null;
        }
        adoptionRootsRef.current.set(id, root);
        workspaceId =
          workspaceForAdoptionRoot(root, workspaceOverviewRef.current ?? [])?.id ?? null;
        if (!workspaceId) {
          // The repository becomes a project. The selection stays.
          rememberAdoption({ terminals: [id] });
          const project = await ipc.project.addLocal({ path: root });
          await loadProjects();
          await refreshWorkspaceOverview();
          workspaceId = primaryWorkspaceId(project.id);
        }
      }
      if (!workspaceId || workspaceId === adoptionWaitWorkspaceId()) return null;
      rememberAdoption({ terminals: [id] });
      return await moveTerminalIntoShellTab(id, workspaceId);
    } catch (error) {
      adoptionUnresolvableRef.current.add(id);
      console.warn("Terminal adoption failed", error);
      return null;
    } finally {
      pending.delete(id);
    }
  }

  // The first shell of a new shell tab runs. A Run action types its command.
  function handleFirstShellOpened(key: TabKey, session: TerminalSession) {
    const request = firstShellRequestsRef.current.get(key);
    firstShellRequestsRef.current.delete(key);
    if (request?.input) {
      void ipc.terminal
        .write({ sessionId: session.id, data: request.input })
        .catch((error) => setTabCloseError(errorMessage(error)));
    }
    void refreshTerminalSessions().catch(() => undefined);
  }

  // The live titles (OSC) of the active pane of each shell tab.
  const [shellLiveTitles, setShellLiveTitles] = useState<Record<string, string>>({});
  function noteShellLiveTitle(key: TabKey, title: string | null) {
    setShellLiveTitles((current) => {
      if ((current[key] ?? null) === title) return current;
      const next = { ...current };
      if (title) next[key] = title;
      else delete next[key];
      return next;
    });
  }

  // The display title of a shell tab row.
  function shellTabDisplayTitle(row: Pick<TabRow, "key" | "customTitle" | "defaultTitle">) {
    const title = shellTabTitle(row, shellLiveTitles[row.key]);
    return "title" in title
      ? title.title
      : t("Terminal {number}", { number: title.terminalNumber });
  }

  function visibleStrip() {
    const nav = navigationRef.current;
    if (nav.kind !== "workspace") return null;
    const snapshot = workspaceTabsStore.getSnapshot();
    return stripView(snapshot.tabs, snapshot.facts, nav.workspaceId);
  }

  function selectWorkspaceTabByNumber(digit: number) {
    const view = visibleStrip();
    if (!view || view.keys.length === 0) return;
    const key = digit === 9 ? view.keys[view.keys.length - 1] : view.keys[digit - 1];
    if (key) activateWorkspaceTab(key);
  }

  function cycleWorkspaceTab(offset: -1 | 1) {
    const view = visibleStrip();
    if (!view || view.keys.length === 0) return;
    const nav = navigationRef.current;
    const composing = nav.kind === "workspace" && nav.composing;
    const index = composing || !view.activeKey ? -1 : view.keys.indexOf(view.activeKey);
    const from = index < 0 ? (offset > 0 ? -1 : view.keys.length) : index;
    const key = view.keys[(from + offset + view.keys.length) % view.keys.length];
    if (key && (key !== view.activeKey || composing)) activateWorkspaceTab(key);
  }

  // Tabs whose close procedure runs. The strip dims them, and their bodies
  // stay mounted until their processes stopped (ADR 0010, T9).
  const [closingTabKeys, setClosingTabKeys] = useState<ReadonlySet<TabKey>>(
    () => new Set(),
  );
  const [tabCloseError, setTabCloseError] = useState<string | null>(null);

  // Stops the processes of an owner that has no mounted panel: the shells
  // of a hidden chat tab or shell tab, or every session before a delete.
  async function stopOwnedSessions(
    owns: (terminal: TerminalSession) => boolean,
  ) {
    const terminals = await ipc.terminal.list();
    const owned = terminals.filter(
      (terminal) => owns(terminal) && terminal.status === "running",
    );
    await Promise.all(
      owned.map((terminal) =>
        stopTerminalSessionForTabClose(terminal, {
          killAgent: (sessionId) => agentSessionStore.kill(sessionId),
          closeTerminal: (sessionId) => ipc.terminal.close({ sessionId }),
          forgetTerminal: (sessionId) => ipc.terminal.forget({ sessionId }),
          forgetAgent: forgetAgentSession,
          refreshAgentSessions: () => agentSessionStore.refreshSessions(),
        }),
      ),
    );
    if (owned.length > 0) void refreshTerminalSessions().catch(() => undefined);
  }

  function stopConversationSessions(conversationId: string) {
    return stopOwnedSessions((terminal) => terminal.conversationId === conversationId);
  }

  // The busy probe answers for each running shell. A shell that it cannot
  // read stays unknown, and the prompt counts it as busy.
  async function withShellBusy(panes: TerminalPanelPaneFact[]): Promise<ClosePaneFact[]> {
    const shellIds = panes
      .filter((pane) => pane.kind === "shell" && pane.running)
      .map((pane) => pane.sessionId);
    if (shellIds.length === 0) return panes;
    const states = await ipc.terminal
      .foregroundBusy({ sessionIds: shellIds })
      .catch(() => []);
    const stateOf = new Map(states.map((probe) => [probe.sessionId, probe.state]));
    return panes.map((pane) => {
      if (pane.kind !== "shell") return pane;
      const state = stateOf.get(pane.sessionId);
      return {
        ...pane,
        shellBusy: state === "busy" ? true : state === "idle" ? false : null,
      };
    });
  }

  // The close procedure of one tab (ADR 0010, section 9.3): at most one
  // prompt, then every process stops, each agent parks its resume point,
  // and only then the tab leaves the strip. The next tab is the most
  // recent one, else the right neighbor, else the left one.
  // The sessions that a tab owns, and its panes for the close prompt. A
  // mounted panel reports its panes. A tab without a panel lists its
  // running sessions.
  function tabCloseFacts(key: TabKey) {
    const parsed = parseTabKey(key);
    const owns = (terminal: TerminalSession) =>
      parsed?.kind === "conversation"
        ? terminal.conversationId === parsed.id
        : !terminal.conversationId && terminal.shellTabId === parsed?.id;
    const handle = terminalPanelRegistry.request(tabPanelScope(key));
    const listed: TerminalPanelPaneFact[] = handle
      ? handle.panes()
      : terminalSessionsRef.current
          .filter((terminal) => owns(terminal) && terminal.status === "running")
          .map((terminal) => ({
            sessionId: terminal.id,
            kind: terminal.agentKind ? "agent" : "shell",
            running: true,
            status: terminal.agentKind
              ? (agentPaneViewFor(terminalActivityStore.getSnapshot(), terminal.id)?.status ??
                null)
              : null,
            shellBusy: null,
          }));
    return { owns, handle, listed };
  }

  // The lines of the close dialog: one line for each reason.
  function tabCloseReasonLines(reasons: { busyAgents: number; busyShells: number }) {
    return [
      reasons.busyAgents === 1
        ? t("1 agent is working.")
        : reasons.busyAgents > 1
          ? t("{count} agents are working.", { count: reasons.busyAgents })
          : null,
      reasons.busyShells > 0 ? t("A command is running.") : null,
    ].filter(Boolean);
  }

  async function closeWorkspaceTab(
    key: TabKey,
    options: { prompt?: boolean } = {},
  ): Promise<boolean> {
    const parsed = parseTabKey(key);
    if (!parsed || closingTabKeysRef.current.has(key)) return false;
    const { owns, handle, listed } = tabCloseFacts(key);
    const panes = options.prompt === false ? listed : await withShellBusy(listed);
    if (options.prompt !== false && tabCloseNeedsConfirm(panes)) {
      const reasons = tabCloseReasons(panes);
      const conversation =
        parsed.kind === "conversation"
          ? conversationsRef.current.find((item) => item.id === parsed.id)
          : undefined;
      const shellRow =
        parsed.kind === "shell" ? workspaceTabsStore.getSnapshot().tabs.rows[key] : undefined;
      const title = conversation
        ? conversationDisplayTitle(
            conversation,
            conversationNamingTitle(sidebarTerminalsRef.current, conversation.id),
          )
        : shellRow
          ? shellTabDisplayTitle(shellRow)
          : t("this tab");
      const lines = tabCloseReasonLines(reasons);
      const confirmed = await confirmDialog({
        title: t("Close {title}?", { title }),
        message: lines.join(" "),
        confirmLabel: t("Close tab"),
        danger: true,
      });
      if (!confirmed) return false;
    }
    closingTabKeysRef.current = new Set(closingTabKeysRef.current).add(key);
    setClosingTabKeys(closingTabKeysRef.current);
    setTabCloseError(null);
    try {
      // A shell tab has no agent to park.
      if (handle) await handle.closeAll({ park: parsed.kind === "conversation" });
      else await stopOwnedSessions(owns);
    } catch (error) {
      setTabCloseError(t("The tab could not close: {error}", { error: errorMessage(error) }));
      return false;
    } finally {
      const next = new Set(closingTabKeysRef.current);
      next.delete(key);
      closingTabKeysRef.current = next;
      setClosingTabKeys(next);
    }
    // The sidebar and the tree read these lists. They must not keep a
    // stopped process as running.
    void refreshTerminalSessions().catch(() => undefined);
    void agentSessionStore.refreshSessions().catch(() => undefined);
    const snapshot = workspaceTabsStore.getSnapshot();
    const row = snapshot.tabs.rows[key];
    const home = row ? homeWorkspaceId(row, snapshot.facts) : null;
    const view = home ? stripView(snapshot.tabs, snapshot.facts, home) : null;
    const wasVisible = shownTabKey() === key;
    const successor = view && view.activeKey === key ? closeSuccessor(view, key) : null;
    // Shift+Cmd+T can open the tab again (memory only, 20 entries).
    if (home && row) {
      const index = Math.max(0, view?.keys.indexOf(key) ?? 0);
      rememberClosedTab(
        parsed.kind === "conversation"
          ? { kind: "conversation", key, conversationId: parsed.id, workspaceId: home, index }
          : { kind: "shell", key, workspaceId: home, title: row.customTitle, index },
      );
    }
    workspaceTabsStore.close(key);
    firstShellRequestsRef.current.delete(key);
    // A closed tab does not come back by itself (ADR 0020).
    if (parsed.kind === "conversation") rememberAdoption({ conversations: [parsed.id] });
    if (wasVisible) {
      const parsedSuccessor = parseTabKey(successor);
      if (parsedSuccessor?.kind === "conversation") {
        void openConversation(parsedSuccessor.id);
      } else if (parsedSuccessor?.kind === "shell" && home) {
        showShellTab(home);
      } else if (home) {
        switchComposerDraft(null);
        markMessagesConversation(null);
        setMessages([]);
        // An empty strip shows the launcher in Pro mode (ADR 0020), else
        // the composer.
        navigate(workspaceNavigation(home, !agentSessionProModeStore.getSnapshot()));
        focusVisibleTab();
      }
    }
    return true;
  }
  const closingTabKeysRef = useRef<ReadonlySet<TabKey>>(new Set());
  const [headerMeterSlot, setHeaderMeterSlot] = useState<HTMLElement | null>(null);

  // "Close other tabs" and the side closes of the tab menu: one prompt for
  // the whole set, only when a pane is busy. The kept tab shows first, so
  // the closes do not move the view through other tabs.
  async function closeWorkspaceTabSet(keys: TabKey[], keep: TabKey) {
    const targets = keys.filter((key) => !closingTabKeysRef.current.has(key));
    if (targets.length === 0) return;
    const facts = await Promise.all(
      targets.map((key) => withShellBusy(tabCloseFacts(key).listed)),
    );
    if (bulkCloseConfirm(facts).confirm) {
      const reasons = facts.map(tabCloseReasons).reduce(
        (total, item) => ({
          busyAgents: total.busyAgents + item.busyAgents,
          busyShells: total.busyShells + item.busyShells,
        }),
        { busyAgents: 0, busyShells: 0 },
      );
      const confirmed = await confirmDialog({
        title: t("Close {count} tabs?", { count: targets.length }),
        message: tabCloseReasonLines(reasons).join(" "),
        confirmLabel: t("Close tabs"),
        danger: true,
      });
      if (!confirmed) return;
    }
    const shown = shownTabKey();
    if (shown && targets.includes(shown)) activateWorkspaceTab(keep);
    for (const key of targets) await closeWorkspaceTab(key, { prompt: false });
  }

  // The closed-tab stack of Shift+Cmd+T (memory only).
  const [closedTabs, setClosedTabs] = useState<ClosedTabEntry[]>([]);
  const closedTabsRef = useRef<ClosedTabEntry[]>([]);
  function rememberClosedTab(entry: ClosedTabEntry) {
    closedTabsRef.current = pushClosedTab(closedTabsRef.current, entry);
    setClosedTabs(closedTabsRef.current);
  }

  function canReopenClosedTab(entry: ClosedTabEntry) {
    const snapshot = workspaceTabsStore.getSnapshot();
    if (snapshot.tabs.rows[entry.key] || !snapshot.facts.complete) return false;
    if (entry.kind === "shell") {
      return shellTabsEnabled() && snapshot.facts.workspaces.has(entry.workspaceId);
    }
    const conversation = snapshot.facts.conversations.get(entry.conversationId);
    return Boolean(
      conversation &&
        conversationHome(conversation, snapshot.facts.workspaces) === entry.workspaceId,
    );
  }

  // Shift+Cmd+T and "+" > Reopen closed tab: the newest closed tab of the
  // visible workspace comes back at its old place. A conversation resumes
  // its parked panes. A shell tab gets a new shell.
  function reopenClosedTab() {
    const nav = navigationRef.current;
    if (nav.kind !== "workspace") return;
    const taken = takeClosedTab(closedTabsRef.current, nav.workspaceId, canReopenClosedTab);
    closedTabsRef.current = taken.stack;
    setClosedTabs(taken.stack);
    const entry = taken.entry;
    if (!entry) return;
    let reopenedKey: TabKey | null = null;
    if (entry.kind === "conversation") {
      void openConversation(entry.conversationId);
      reopenedKey = entry.key;
    } else {
      reopenedKey = openShellTab({ title: entry.title });
    }
    if (reopenedKey) workspaceTabsStore.move(reopenedKey, entry.index);
  }

  // Cmd+R and the tab menu: the inline rename of the visible tab.
  const [tabRenameRequest, setTabRenameRequest] = useState<{
    key: string;
    nonce: number;
  } | null>(null);
  function requestVisibleTabRename() {
    const key = navigationTabKey(navigationRef.current, workspaceTabsStore.getSnapshot());
    if (!key) return;
    setTabRenameRequest((current) => ({ key, nonce: (current?.nonce ?? 0) + 1 }));
  }

  // A rename from the strip. A shell tab keeps its own title; an empty
  // title restores the default. A conversation tab renames the
  // conversation; an empty title changes nothing.
  function renameWorkspaceTab(key: TabKey, title: string) {
    const parsed = parseTabKey(key);
    if (parsed?.kind === "shell") {
      if (shellTabsEnabled()) workspaceTabsStore.renameShell(key, title || null);
      return;
    }
    if (parsed?.kind !== "conversation" || !title.trim()) return;
    void ipc.storage
      .setConversationTitle({ conversationId: parsed.id, title: title.trim() })
      .then((summary) => {
        upsertConversationSummary(summary);
        return loadConversations();
      })
      .catch((error) => setTabCloseError(errorMessage(error)));
  }

  // "Split terminal right" and "Split terminal down" of the tab menu. A
  // hidden tab shows first; its panel can mount a few frames later.
  function splitWorkspaceTab(key: TabKey, direction: "vertical" | "horizontal") {
    if (shownTabKey() !== key) activateWorkspaceTab(key);
    let attempts = 0;
    const attempt = () => {
      const handle = terminalPanelRegistry.request(tabPanelScope(key));
      if (handle?.splitActivePane(direction)) return;
      attempts += 1;
      if (attempts < 180) window.requestAnimationFrame(attempt);
    };
    attempt();
  }

  // "Copy path" of the tab menu: the root of the workspace of the tab.
  function copyWorkspaceTabPath(key: TabKey) {
    const snapshot = workspaceTabsStore.getSnapshot();
    const row = snapshot.tabs.rows[key];
    const home = row ? homeWorkspaceId(row, snapshot.facts) : null;
    const path = workspaceOverviewRef.current?.find((item) => item.id === home)?.rootPath;
    if (!path) return;
    void copyText(path).then((copied) => {
      if (!copied) setTabCloseError(t("The path could not be copied."));
    });
  }

  function handleTabMenu(
    key: TabKey,
    action: Exclude<WorkspaceTabMenuAction, "rename">,
    shownKeys: readonly TabKey[],
  ) {
    const index = shownKeys.indexOf(key);
    if (action === "close") void closeWorkspaceTab(key);
    else if (action === "close-others") {
      void closeWorkspaceTabSet(shownKeys.filter((item) => item !== key), key);
    } else if (action === "close-right") {
      void closeWorkspaceTabSet(shownKeys.slice(index + 1), key);
    } else if (action === "close-left") {
      void closeWorkspaceTabSet(shownKeys.slice(0, Math.max(0, index)), key);
    } else if (action === "split-right") splitWorkspaceTab(key, "vertical");
    else if (action === "split-down") splitWorkspaceTab(key, "horizontal");
    else if (action === "copy-path") copyWorkspaceTabPath(key);
  }

  // Where Cmd+W starts: the focused pane of a tab body or of the bottom
  // panel, else no pane.
  function closeChainFocus(): CloseChainFocus {
    const active = document.activeElement;
    const sessionId = active?.closest<HTMLElement>("[data-terminal-session-id]")?.dataset
      .terminalSessionId;
    const scope = active?.closest<HTMLElement>("[data-terminal-scope]")?.dataset.terminalScope;
    if (!sessionId || !scope) return { kind: "elsewhere" };
    const paneCount = terminalPanelRegistry.request(scope)?.panes().length ?? 1;
    const tabKey = active?.closest<HTMLElement>("[data-workspace-tab-body]")?.dataset
      .workspaceTabBody;
    return tabKey
      ? { kind: "tab-pane", tabKey, sessionId, paneCount }
      : { kind: "bottom-pane", scope, sessionId, paneCount };
  }

  // Cmd+W and Ctrl+Shift+W (ADR 0010, section 9.4): the focused pane, else
  // the tab. With nothing to close, the window hides, as before.
  async function runCloseChain() {
    const nav = navigationRef.current;
    const visibleKey = navigationTabKey(nav, workspaceTabsStore.getSnapshot());
    const view = visibleStrip();
    const step = closeChainStep({
      shellTabs: shellTabsEnabled(),
      focus: closeChainFocus(),
      visibleTab: visibleKey
        ? {
            key: visibleKey,
            paneCount: terminalPanelRegistry.request(tabPanelScope(visibleKey))?.panes().length ?? 0,
          }
        : null,
      draftWithTabs: nav.kind === "workspace" && nav.composing && (view?.keys.length ?? 0) > 0,
    });
    if (step.type === "hide-window") {
      await ipc.app.hideMainWindow().catch((error) => console.error("Hide window failed", error));
    } else if (step.type === "close-tab") {
      await closeWorkspaceTab(step.tabKey as TabKey);
    } else if (step.type === "leave-draft") {
      if (view?.activeKey) activateWorkspaceTab(view.activeKey);
    } else if (step.scope === "tab") {
      await terminalPanelRegistry
        .request(tabPanelScope(step.tabKey as TabKey))
        ?.closePane(step.sessionId);
    } else {
      const handle = terminalPanelRegistry.request(step.panelScope);
      await handle?.closePane(step.sessionId);
      // The last pane of the bottom panel hides the panel.
      if (step.last && handle && handle.panes().length === 0) setTerminalOpen(false);
    }
  }

  // "+" > agent and Option+Cmd+T: a new agent conversation in this
  // workspace. The CLI starts at once in the workspace root, with no
  // initial prompt (Orca).
  async function startAgentTab(agentKind: AgentKind) {
    const nav = navigationRef.current;
    const facts = workspaceTabsStore.getSnapshot().facts;
    const workspaceId = nav.kind === "workspace" ? nav.workspaceId : composingWorkspaceId();
    const workspace = workspaceId ? facts.workspaces.get(workspaceId) : undefined;
    const overview = workspaceOverviewRef.current?.find((item) => item.id === workspaceId);
    const project = projects.find((item) => item.id === workspace?.projectId);
    if (!workspaceId || !workspace || !project) {
      setTabCloseError(t("Open a project before you start an agent."));
      return;
    }
    const backend: AgentBackend | null =
      agentKind === "claude"
        ? "claude_session"
        : agentKind === "codex"
          ? "codex_session"
          : agentKind === "kimi"
            ? "kimi_session"
            : agentKind === "zai"
              ? "zai_session"
              : agentKind === "grok"
                ? "grok_session"
                : null;
    const modelInfo =
      (backend ? modelsRef.current.find((model) => model.agentBackend === backend) : undefined) ??
      modelsRef.current.find((model) => model.id === selectedModelRef.current);
    const model = modelInfo?.id ?? selectedModelRef.current;
    const agentModel = requestModelForChatModel(modelInfo, model);
    setTabCloseError(null);
    let conversationId: string | null = null;
    try {
      const snapshot = agentSessionStore.getSnapshot();
      const lane = await resolveTerminalSplitAgentLane(snapshot.laneOptions, agentKind, () =>
        agentSessionStore.refreshLaneOptions(),
      );
      let summary = await ipc.storage.setConversationGoal({
        conversationId: null,
        goal: null,
        model,
        projectId: project.id,
        workspaceId,
      });
      summary = await ipc.storage.setConversationMode({ conversationId: summary.id, mode: "agent" });
      if (backend) {
        summary = await ipc.storage.setConversationAgentBackend({
          conversationId: summary.id,
          agentBackend: backend,
          agentSessionModel: agentModel,
        });
      }
      conversationId = summary.id;
      upsertConversationSummary(summary);
      updateTerminalOpening(summary.id, { type: "start" });
      await openConversation(summary.id, { summary, load: false });
      setMessages([]);
      const created = await agentSessionStore.create({
        agentKind,
        lane,
        cwd: overview?.rootPath ?? agentLaunchCwd({ conversation: summary, project }),
        projectId: project.id,
        conversationId: summary.id,
        conversationTitle: summary.title,
        model: backend ? agentModel : undefined,
        mode: "terminal_embedded",
        cols: 100,
        rows: 24,
        permissionMode: approvalPolicyForConversation(summary),
      });
      setTerminalFocusSessionId(created.id);
      void watchTerminalOpening(created.id, summary.id);
      terminalFocusBridge.request(created.id);
      void loadConversations().catch(() => undefined);
    } catch (error) {
      const message = errorMessage(error);
      if (conversationId) updateTerminalOpening(conversationId, { type: "fail", message });
      setTabCloseError(message);
    }
  }

  // A pending open waits for the tab facts (app start) or for the list to
  // learn a conversation that another path created.
  const pendingTabOpenRef = useRef<string | null>(null);

  function openConversationTab(
    conversationId: string,
    options: OpenConversationOptions,
  ) {
    if (options.summary) syncTabFacts(options.summary);
    const facts = workspaceTabsStore.getSnapshot().facts;
    const fact = facts.conversations.get(conversationId);
    let next: Navigation;
    if (!facts.complete || !fact) {
      if (options.activate === false) return Promise.resolve(null);
      // Show the conversation now. Its tab opens when the facts know it.
      pendingTabOpenRef.current = conversationId;
      next = looseNavigation(conversationId);
      if (facts.complete) {
        void loadConversations().catch(() => undefined);
      }
    } else {
      const home = conversationHome(fact, facts.workspaces);
      if (home) {
        workspaceTabsStore.open({
          key: conversationTabKey(conversationId),
          activate: options.activate !== false,
        });
      }
      if (options.activate === false) return Promise.resolve(null);
      pendingTabOpenRef.current = null;
      next = home ? workspaceNavigation(home) : looseNavigation(conversationId);
    }
    const previousConversationId = currentConversationIdRef.current;
    setDotOpen(false);
    switchComposerDraft(conversationId);
    navigate(next);
    if (options.load === false) {
      markMessagesConversation(conversationId);
      return Promise.resolve(null);
    }
    // A user open (a row, a hit, a tab) puts the focus in the tab.
    focusVisibleTab();
    const navigationGeneration = options.navigationGeneration ?? beginNavigation();
    if (
      previousConversationId === conversationId &&
      messagesConversationIdRef.current === conversationId
    ) {
      return Promise.resolve(null);
    }
    return loadMessages(conversationId, { navigationGeneration });
  }

  function conversationTargetForNewInput() {
    const pendingNavigation = pendingConversationNavigationRef.current;
    return pendingNavigation && navigationIsCurrent(pendingNavigation.generation)
      ? pendingNavigation.conversationId
      : currentConversationIdRef.current;
  }

  async function requestChatCancel(
    conversationId: string,
    pendingNewTurn: PendingNewTurn | null,
  ) {
    const cancelNavigationGeneration =
      pendingNewTurn?.navigationGeneration ?? navigationGenerationRef.current;
    if (pendingNewTurn?.cancelDispatched) {
      return;
    }
    if (pendingNewTurn) {
      pendingNewTurn.cancelDispatched = true;
    }
    try {
      await ipc.chat.cancel({ conversationId });
    } catch (error) {
      updateConversationTurnState(conversationId, {
        cancelRequested: false,
        normalCancelRequested: false,
      });
      if (pendingNewTurn) {
        pendingNewTurn.cancelDispatched = false;
      }
      const visibleConversationTarget = conversationTargetForNewInput();
      const cancelStillVisible =
        navigationIsCurrent(cancelNavigationGeneration) &&
        pendingNewTurn?.freshViewRequested !== true &&
        (visibleConversationTarget === conversationId ||
          (pendingNewTurn !== null &&
            visibleConversationTarget === null));
      if (cancelStillVisible) {
        setChatError(errorMessage(error));
      }
    }
  }

  async function requestChatCancelAndRevert(
    conversationId: string,
    pendingNewTurn: PendingNewTurn | null,
  ) {
    // Read the live translate function through the ref.
    const t = tRef.current;
    const turnState = conversationTurnState(conversationId);
    if (!turnState?.cancelAndRevertRequested || turnState.cancelAndRevertDispatched) {
      return;
    }
    updateConversationTurnState(conversationId, {
      cancelAndRevertDispatched: true,
      cancelAndRevertTurn: true,
    });
    if (pendingNewTurn) {
      pendingNewTurn.cancelDispatched = true;
    }
    const operationIsVisible = () => {
      const pendingNavigation = pendingConversationNavigationRef.current;
      return (
        currentConversationIdRef.current === conversationId &&
        (!pendingNavigation || pendingNavigation.conversationId === conversationId)
      );
    };

    let noticeText: string;
    let commandFailed = false;
    try {
      const result = await ipc.chat.cancelAndRevert({
        conversationId,
      });
      noticeText =
        result.revertedFiles > 0
          ? t("Turn stopped and reverted ({revertedFiles} files)", {
              revertedFiles: result.revertedFiles,
            })
          : t("Turn stopped");
    } catch (error) {
      commandFailed = true;
      noticeText = t("Stop & revert failed: {error}", {
        error: errorMessage(error),
      });
    }

    if (operationIsVisible()) {
      await loadMessages(conversationId, {
        preserveToolCalls: true,
        navigationGeneration: navigationGenerationRef.current,
      }).catch(() => undefined);
      setChatError(null);
    }
    appendNotice(conversationId, noticeText);

    if (commandFailed) {
      if (pendingNewTurn) {
        pendingNewTurn.cancelDispatched = false;
      }
      updateConversationTurnState(conversationId, {
        cancelAndRevertRequested: false,
        cancelAndRevertDispatched: false,
        cancelAndRevertDrain: false,
        cancelAndRevertTurn: false,
        cancelAndRevertPending: false,
        stopAndRevertAvailable: false,
      });
      return;
    }
    if (
      conversationTurnStore
        .getSnapshot()
        .runningConversationIds.has(conversationId)
    ) {
      updateConversationTurnState(conversationId, {
        cancelAndRevertRequested: false,
        cancelAndRevertDispatched: false,
        cancelAndRevertDrain: true,
      });
    } else {
      updateConversationTurnState(conversationId, {
        cancelAndRevertRequested: false,
        cancelAndRevertDispatched: false,
        cancelAndRevertDrain: false,
        cancelAndRevertPending: false,
        stopAndRevertAvailable: false,
      });
      dispatchNextQueuedMessage(conversationId);
    }
  }

  function replaceConversations(rows: ConversationSummary[]) {
    initialConversationsLoadedRef.current = true;
    const sorted = sortConversations(rows);
    conversationsRef.current = sorted;
    setConversations(sorted);
  }

  function upsertConversationSummary(summary: ConversationSummary) {
    setConversations((current) => {
      const next = upsertConversation(current, summary);
      conversationsRef.current = next;
      return next;
    });
  }

  function setSelectedModel(next: SetStateAction<string>) {
    setSelectedModelState((current) => {
      const resolved = typeof next === "function" ? next(current) : next;
      selectedModelRef.current = resolved;
      return resolved;
    });
  }

  function setSelectedEffort(next: SetStateAction<string>) {
    setSelectedEffortState((current) => {
      const resolved = typeof next === "function" ? next(current) : next;
      selectedEffortRef.current = resolved;
      return resolved;
    });
  }

  function requestSettingsForConversation(
    conversation: ConversationSummary,
    useViewedSettings: boolean,
  ): ConversationRequestSettings {
    if (useViewedSettings) {
      const stored = conversationRequestSettingsRef.current.get(
        conversation.id,
      );
      const selectedMetadata = modelsRef.current.find(
        (item) => item.id === selectedModelRef.current,
      );
      return {
        model: selectedModelRef.current,
        effort: selectedEffortRef.current,
        agentBackend:
          stored?.agentBackend !== undefined
            ? stored.agentBackend
            : (selectedMetadata?.agentBackend ?? conversation.agentBackend),
        agentSessionModel:
        stored?.agentSessionModel !== undefined
          ? stored.agentSessionModel
          : selectedMetadata?.agentBackend
            ? requestModelForChatModel(
                selectedMetadata,
                selectedModelRef.current,
              )
            : conversation.agentSessionModel,
      };
    }
    const stored = conversationRequestSettingsRef.current.get(conversation.id);
    const backend =
      stored?.agentBackend !== undefined
        ? stored.agentBackend
        : conversation.agentBackend;
    const preferredModel =
      stored?.model ??
      (isSessionBackend(backend)
        ? (conversation.agentSessionModel ?? conversation.model)
        : conversation.model);
    const availableModels = modelsRef.current;
    const modelInfo =
      availableModels.find(
        (item) =>
          item.id === preferredModel ||
          (item.agentBackend === backend &&
            requestModelForChatModel(item, item.id) === preferredModel),
      ) ?? availableModels[0];
    const model = modelInfo?.id ?? conversationDefaultsRef.current.model;
    const preferredEffort = stored?.effort ?? conversationDefaultsRef.current.effort;
    const effort = modelInfo?.reasoningEfforts.includes(preferredEffort)
      ? preferredEffort
      : (modelInfo?.defaultEffort ?? conversationDefaultsRef.current.effort);
    return {
      model,
      effort,
      agentBackend: backend,
      agentSessionModel:
        isSessionBackend(backend) ? preferredModel : null,
    };
  }

  async function workspaceIdForConversation(conversationId: string) {
    const workspace = await ipc.workspace.current({
      conversationId,
    });
    return workspace?.id ?? null;
  }

  function persistConversationDefaults(
    patch: Partial<ConversationDefaults>,
  ): Promise<void> {
    if (patch.model !== undefined) {
      hasExplicitDefaultModelRef.current = true;
    }
    const next = { ...conversationDefaultsRef.current, ...patch };
    conversationDefaultsRef.current = next;
    const pending = conversationDefaultsSaveRef.current
      .catch(() => undefined)
      .then(() =>
        ipc.preferences.setConversationDefaults({
          defaultModel: next.model,
          defaultEffort: next.effort,
          defaultAgentMode: next.agentMode,
          defaultApprovalPolicy: next.approvalPolicy,
          defaultFastMode: next.fastMode,
        }),
      )
      .then(() => undefined);
    conversationDefaultsSaveRef.current = pending;
    return pending;
  }

  function handleModelChange(model: string) {
    setSelectedModel(model);
    const metadata = models.find((item) => item.id === model);
    const effort = metadata?.reasoningEfforts.includes(selectedEffortRef.current)
      ? selectedEffortRef.current
      : (metadata?.defaultEffort ?? selectedEffortRef.current);
    if (effort !== selectedEffortRef.current) {
      setSelectedEffort(effort);
    }
    const conversationId = currentConversationIdRef.current;
    if (conversationId) {
      const conversation = conversationsRef.current.find(
        (item) => item.id === conversationId,
      );
      const agentBackend = agentBackendForModel(
        metadata,
        modeForConversationSelection(conversation) === "agent",
      );
      conversationRequestSettingsRef.current.set(conversationId, {
        model,
        effort,
        agentBackend,
        agentSessionModel:
          agentBackend
            ? requestModelForChatModel(metadata, model)
            : null,
      });
      void ipc.storage.setConversationAgentBackend({
          conversationId,
          agentBackend,
          agentSessionModel:
            agentBackend
              ? requestModelForChatModel(metadata, model)
              : null,
        })
        .then(upsertConversationSummary)
        .catch((error) => setChatError(errorMessage(error)));
    }
    void persistConversationDefaults({ model, effort }).catch((error) =>
      setChatError(errorMessage(error)),
    );
  }

  function handleEffortChange(effort: string) {
    setSelectedEffort(effort);
    const conversationId = currentConversationIdRef.current;
    if (conversationId) {
      conversationRequestSettingsRef.current.set(conversationId, {
        model: selectedModelRef.current,
        effort,
      });
    }
    void persistConversationDefaults({ effort }).catch((error) =>
      setChatError(errorMessage(error)),
    );
  }

  function notifyApproval(payload: ChatDeltaPayload) {
    if (payload.status !== "awaiting_approval") {
      return;
    }
    // Read the live translate function through the ref.
    const t = tRef.current;
    const command = truncateNotificationText(
      payload.command ?? payload.toolKind ?? t("tool call"),
    );
    void notify(
      t("Halv needs approval"),
      t("Wants to run: {command}", { command }),
      "permission",
    );
  }

  function notifyTerminal(payload: ChatDeltaPayload) {
    if (!isTerminalDelta(payload.kind)) {
      return;
    }
    // Read the live translate function through the ref.
    const t = tRef.current;
    const now = Date.now();
    if (now - lastNotifiedAtRef.current < 60_000) {
      return;
    }
    lastNotifiedAtRef.current = now;

    if (payload.kind === "done") {
      const title =
        conversationsRef.current.find(
          (conversation) => conversation.id === payload.conversationId,
        )?.title ?? t("Turn complete");
      void notify(t("Halv finished"), title);
      return;
    }

    void notify(
      t("Halv hit an error"),
      truncateNotificationText(payload.text ?? t("Chat request failed")),
    );
  }

  const selectedModelInfo = useMemo(
    () => models.find((model) => model.id === selectedModel) ?? models[0],
    [models, selectedModel],
  );
  const currentConversation = useMemo(
    () =>
      conversations.find(
        (conversation) => conversation.id === currentConversationId,
      ),
    [conversations, currentConversationId],
  );
  const agentSessionsState = useSyncExternalStore(
    agentSessionStore.subscribe,
    agentSessionStore.getSnapshot,
  );
  const diffRailState = useSyncExternalStore(
    diffRailStore.subscribe,
    diffRailStore.getSnapshot,
  );
  const proMode = useSyncExternalStore(
    agentSessionProModeStore.subscribe,
    agentSessionProModeStore.getSnapshot,
  );
  useEffect(() => {
    void agentSessionStore.connect();
    return () => agentSessionStore.disconnect();
  }, []);
  const openDiffRail = useCallback((focus = false) => {
    setTerminalOpen(true);
    diffRailStore.setOpen(true);
    if (focus) {
      setDiffRailFocusSignal((current) => current + 1);
    }
  }, []);
  const toggleDiffRail = useCallback(() => {
    const opening = !diffRailStore.getSnapshot().ui.open;
    if (opening) {
      setTerminalOpen(true);
    }
    diffRailStore.setOpen(opening);
  }, []);
  const previousProModeRef = useRef(proMode);
  useEffect(() => {
    const previous = previousProModeRef.current;
    previousProModeRef.current = proMode;
    if (guidedTaskPhase === "prepared" && currentConversation?.draftOnly) {
      return;
    }
    const mode = conversationProModeSwitch(
      previous,
      proMode,
      currentConversation,
    );
    if (!mode || !currentConversation) return;
    const conversationId = currentConversation.id;
    const modelInfo = modelsRef.current.find(
      (item) => item.id === currentConversation.model,
    );
    const agentBackend =
      currentConversation.agentBackend ?? agentBackendForModel(modelInfo, true);
    void (async () => {
      let summary = await ipc.storage.setConversationMode({ conversationId, mode });
      if (!currentConversation.agentBackend && agentBackend) {
        summary = await ipc.storage.setConversationAgentBackend({
            conversationId,
            agentBackend,
            agentSessionModel: requestModelForChatModel(
              modelInfo,
              currentConversation.model,
            ),
          });
      }
      upsertConversationSummary(summary);
    })().catch((error) => setChatError(errorMessage(error)));
  }, [currentConversation, proMode]);
  const refreshTerminalSessions = useCallback(async () => {
    const sessions = await ipc.terminal.list();
    setTerminalSessions(sessions);
    return sessions;
  }, []);
  useEffect(() => {
    void refreshTerminalSessions().catch((error) =>
      console.error("Terminal session refresh failed", error),
    );
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void ipc.events.listen<TerminalEvent>("terminal_event", (payload) => {
      if (disposed) return;
      terminalActivityStore.noteEvent(payload);
      setTerminalSessions((current) => {
        if (!current.some((session) => session.id === payload.sessionId)) {
          void refreshTerminalSessions().catch((error) =>
            console.error("Terminal session refresh failed", error),
          );
          return current;
        }
        if (payload.kind === "data") return current;
        return current.map((session) =>
          session.id === payload.sessionId
            ? {
                ...session,
                status: payload.kind === "error" ? "failed" : "exited",
                exitCode: payload.exitCode,
                signal: payload.signal,
                exitedAt: new Date().toISOString(),
              }
            : session,
        );
      });
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshTerminalSessions]);
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void ipc.events.listen<AgentSessionEventPayload>("agent-session-event", (payload) => {
      if (disposed) return;
      const acceptedCompletion = terminalActivityStore.noteAgentEvent(payload.sessionId, payload.event);
      if (
        acceptedCompletion &&
        payload.event.type === "embeddedTurnFinished" &&
        payload.event.outcome === "completed"
      ) {
        // A completed turn in any terminal is a first result (ADR 0018).
        markFirstResult();
      }
      if (!guidedTerminalTrackingArmedRef.current ||
          payload.sessionId !== guidedTerminalSessionIdRef.current) return;
      if (acceptedCompletion && payload.event.type === "embeddedTurnFinished") {
        guidedTerminalTrackingArmedRef.current = false;
        if (payload.event.outcome === "completed") {
          setGuidedTaskPhase("success");
          void recordAnalyticsEvent("first_task_completed");
          setOnboardingPreference("completed");
          writeOnboardingPreference("completed");
        } else {
          setGuidedTaskCancelled(true);
        }
      }
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    }).catch((error) => console.error("Agent activity subscription failed", error));
    return () => { disposed = true; unlisten?.(); };
  }, []);
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    let reloadTimer: number | undefined;
    void ipc.events.listen<{ conversationId: string }>("conversation-activity", (payload) => {
      if (disposed) return;
      const conversation = conversationsRef.current.find(
        (candidate) => candidate.id === payload.conversationId,
      );
      if (conversation) {
        upsertConversationSummary({
          ...conversation,
          updatedAt: new Date().toISOString(),
        });
      }
      if (reloadTimer !== undefined) window.clearTimeout(reloadTimer);
      reloadTimer = window.setTimeout(() => {
        reloadTimer = undefined;
        if (disposed) return;
        void loadConversations().catch((error) =>
          console.error("Failed to refresh conversations after activity", error),
        );
      }, 500);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      if (reloadTimer !== undefined) window.clearTimeout(reloadTimer);
      unlisten?.();
    };
  }, []);
  useEffect(() => {
    void refreshTerminalSessions().catch((error) =>
      console.error("Terminal session refresh failed", error),
    );
  }, [agentSessionsState.entries.length, refreshTerminalSessions]);
  const sidebarTerminals = useMemo<SidebarTerminalSession[]>(
    () => {
      const managedIds = new Set(terminalSessions.map((terminal) => terminal.id));
      const managed = terminalSessions.map((terminal) => {
        const entry = agentSessionsState.entries.find(
          (candidate) => candidate.session.id === terminal.id,
        );
        const customTitle =
          entry?.session.customTitle?.trim() || terminal.title?.trim() || null;
        return {
          id: terminal.id,
          title:
            customTitle ||
            (entry ? agentSessionTitle(entry.session) : terminal.projectName),
          customTitle,
          cwd: terminal.cwd,
          agentKind: entry?.session.agentKind ?? terminal.agentKind ?? null,
          conversationId:
            entry?.session.conversationId ?? terminal.conversationId ?? null,
          continuedFrom: terminal.continuedFrom ?? null,
          // A live agent pane without any signal shows "not reporting",
          // as in Orca, not a spinner that never stops.
          pane:
            (entry?.session.agentKind ?? terminal.agentKind) &&
            terminal.status === "running"
              ? (agentPaneViewFor(terminalActivitySnapshot, terminal.id) ??
                (entry?.session.mode === "gui"
                  ? null
                  : unreportedAgentPaneView(Date.parse(terminal.startedAt) || 0)))
              : null,
          status: effectiveTerminalStatus(
            terminal.status,
            entry?.session.status,
            terminalActivityBySession[terminal.id],
            entry?.session.mode,
          ),
        };
      });
      const external = agentSessionsState.entries
        .filter(
          (entry) =>
            entry.session.mode === "terminal_ghostty" &&
            !managedIds.has(entry.session.id),
        )
        .map((entry) => ({
          id: entry.session.id,
          title: agentSessionTitle(entry.session),
          customTitle: entry.session.customTitle?.trim() || null,
          cwd: entry.session.cwd,
          agentKind: entry.session.agentKind,
          conversationId: entry.session.conversationId ?? null,
          status: entry.session.status,
        }));
      return [...managed, ...external];
    },
    [agentSessionsState.entries, terminalActivityBySession, terminalActivitySnapshot, terminalSessions],
  );
  const sidebarTerminalsRef = useRef(sidebarTerminals);
  sidebarTerminalsRef.current = sidebarTerminals;
  useEffect(() => {
    const notifier = createAgentStatusNotifier({
      current: (sessionId) =>
        agentPaneViewFor(terminalActivityStore.getSnapshot(), sessionId),
      paneViewed: (sessionId) =>
        terminalActivityStore.getSnapshot().focusedSessionId === sessionId,
      deliver: (sessionId, kind, view) => {
        const terminal = sidebarTerminalsRef.current.find(
          (row) => row.id === sessionId,
        );
        if (!terminal?.agentKind) return;
        const conversation = terminal.conversationId
          ? conversationsRef.current.find(
              (candidate) => candidate.id === terminal.conversationId,
            )
          : undefined;
        const title = conversation
          ? conversationDisplayTitle(
              conversation,
              conversationNamingTitle(
                sidebarTerminalsRef.current,
                conversation.id,
              ),
            )
          : terminal.title;
        const text = agentPaneNotificationText(
          view,
          { agent: agentSessionAgentLabel(terminal.agentKind), title },
          tRef.current,
        );
        if (text) void notifyTerminalPane(sessionId, text.title, text.body, kind);
      },
    });
    const unsubscribe = terminalActivityStore.subscribePanes(
      (sessionId, view, previous) => {
        if (view) notifier.update(sessionId, view, previous);
        else notifier.forget(sessionId);
      },
    );
    return () => {
      unsubscribe();
      notifier.dispose();
    };
  }, []);
  // A click on a terminal banner opens the conversation of the pane and
  // focuses the pane itself. The panel that shows the pane marks it viewed,
  // so a pane that no panel can show keeps its unread mark.
  const focusNotifiedTerminalRef = useRef<(sessionId: string) => void>(
    () => undefined,
  );
  focusNotifiedTerminalRef.current = (sessionId) => {
    setDotOpen(false);
    if (settingsOpen) closeSettings();
    setWorkspaceView("chat");
    const entry = agentSessionsState.entries.find(
      (candidate) => candidate.session.id === sessionId,
    );
    const conversationId =
      entry?.session.conversationId ??
      terminalSessions.find((session) => session.id === sessionId)
        ?.conversationId ??
      null;
    const conversation = conversationId
      ? conversationsRef.current.find((candidate) => candidate.id === conversationId)
      : undefined;
    if (conversation) {
      void openConversation(conversation.id);
      if (!conversationUsesAgentSurface(conversation)) {
        setTerminalFocusSessionId(sessionId);
        setTerminalOpen(true);
      }
    } else if (entry && entry.session.mode !== "terminal_embedded") {
      agentSessionStore.select(sessionId);
      setAgentSessionsOpen(true);
    } else {
      setTerminalFocusSessionId(sessionId);
      setTerminalOpen(true);
    }
    terminalFocusBridge.request(sessionId);
  };
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void ipc.events
      .listen<{ sessionId: string }>(
        "terminal-notification-clicked",
        (payload) => {
          if (!disposed) focusNotifiedTerminalRef.current(payload.sessionId);
        },
      )
      .then((cleanup) => {
        if (disposed) cleanup();
        else unlisten = cleanup;
      })
      .catch((error) =>
        console.error("Notification click subscription failed", error),
      );
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  // Dot, Settings, and the browser view hide the terminal panes. The user
  // does not look at a pane then, so its news stays unread and notifies.
  const terminalPanesCovered =
    dotOpen || settingsOpen || workspaceView === "browser";
  useEffect(() => {
    terminalActivityStore.setPaneCovered(terminalPanesCovered);
  }, [terminalPanesCovered]);
  const currentConversationDisplayTitle = currentConversation
    ? conversationDisplayTitle(
        currentConversation,
        // The oldest terminal with a custom title names the conversation.
        // The oldest agent terminal names it when no custom title exists.
        conversationNamingTitle(sidebarTerminals, currentConversation.id),
      )
    : null;
  const currentConversationMode = modeForConversationSelection(
    currentConversation,
  );
  const currentAgentSessionEntry = useMemo(
    () =>
      agentSessionForConversationMode(
        agentSessionsState.entries,
        currentConversationId,
        currentConversationMode,
      ),
    [
      agentSessionsState.entries,
      currentConversationId,
      currentConversationMode,
    ],
  );
  const resolvedAgentModel = resolvedAgentSessionModelForConversation(
    agentSessionsState,
    currentConversationId,
  );
  const terminalAgentSavingsTokens = useMemo(
    () =>
      agentSessionsState.entries
        .filter((entry) => entry.session.mode !== "gui")
        .reduce((total, entry) => total + entry.totals.savingsTokens, 0),
    [agentSessionsState.entries],
  );
  useEffect(() => {
    void refreshSavingsTotals().catch(() => undefined);
  }, [terminalAgentSavingsTokens]);
  // Only show the terminal surface for an agent conversation once it actually
  // has a session or restored terminals. A freshly created agent chat shows the
  // task composer until the first turn opens a terminal.
  // With workspace tabs, a conversation's body follows one rule: the
  // terminal body once it has a session, a running terminal, saved panes,
  // or an agent that starts now. The tab never shows a half-open panel.
  const liveConversationIds = useMemo(
    () =>
      liveConversationIdsOf({
        agentEntries: agentSessionsState.entries,
        terminals: terminalSessions,
      }),
    [agentSessionsState.entries, terminalSessions],
  );
  // Shell tabs (ADR 0010, stage S5b) need the strip.
  const shellTabsOn = featureFlags.workspaceTabs && featureFlags.shellTabs;
  const liveShellTabIds = useMemo(
    () => liveShellTabIdsOf(terminalSessions),
    [terminalSessions],
  );
  // A shell of an open shell tab shows in its tab only, never in the Chats
  // area of the sidebar (ADR 0010, ADR 0020).
  const sidebarListTerminals = useMemo(() => {
    if (!shellTabsOn) return sidebarTerminals;
    const openShellTabIds = new Set(
      Object.values(workspaceTabsSnapshot.tabs.rows).flatMap((row) => {
        const parsed = parseTabKey(row.key);
        return parsed?.kind === "shell" ? [parsed.id] : [];
      }),
    );
    const tabbed = tabbedShellSessionIds(terminalSessions, openShellTabIds);
    return tabbed.size > 0
      ? sidebarTerminals.filter((terminal) => !tabbed.has(terminal.id))
      : sidebarTerminals;
  }, [shellTabsOn, sidebarTerminals, terminalSessions, workspaceTabsSnapshot.tabs]);
  // A visible shell tab hides the chat surface and the composer.
  const shellTabVisible =
    shellTabsOn &&
    parseTabKey(navigationTabKey(navigation, workspaceTabsSnapshot))?.kind === "shell";
  function tabBodyKind(conversation: ConversationSummary) {
    const entry = agentSessionForConversationMode(
      agentSessionsState.entries,
      conversation.id,
      "agent",
    );
    return conversationBodyKind({
      mode: conversation.mode,
      agentMode: entry?.session.mode ?? null,
      runningOwnedTerminal: terminalSessions.some(
        (terminal) =>
          terminal.conversationId === conversation.id && terminal.status === "running",
      ),
      savedTerminals: conversationHasSavedTerminals(conversation.id),
      opening: terminalOpeningByConversation[conversation.id]?.phase === "opening",
    });
  }
  const showProTerminalSurface = featureFlags.workspaceTabs
    ? shellTabVisible ||
      (Boolean(currentConversation) && tabBodyKind(currentConversation!) !== "chat")
    : conversationUsesAgentSurface(currentConversation) &&
      (currentAgentSessionEntry != null ||
        conversationHasSavedTerminals(currentConversationId));
  const retainedAgentConversations = useMemo(
    () =>
      conversations.filter(
        (conversation) =>
          conversation.mode === "agent" &&
          (conversation.id === currentConversationId ||
            agentSessionsState.entries.some(
              (entry) =>
                entry.session.conversationId === conversation.id &&
                entry.session.mode !== "gui" &&
                ["starting", "running", "waiting_input"].includes(
                  entry.session.status,
                ),
            )),
      ),
    [agentSessionsState.entries, conversations, currentConversationId],
  );
  // The workspace strip (ADR 0010).
  const visibleTabKey = navigationTabKey(navigation, workspaceTabsSnapshot);
  const stripWorkspaceId =
    featureFlags.workspaceTabs && navigation.kind === "workspace"
      ? navigation.workspaceId
      : null;
  const strip = useMemo(
    () =>
      stripWorkspaceId
        ? stripView(
            workspaceTabsSnapshot.tabs,
            workspaceTabsSnapshot.facts,
            stripWorkspaceId,
          )
        : null,
    [stripWorkspaceId, workspaceTabsSnapshot.facts, workspaceTabsSnapshot.tabs],
  );
  const stripDraft = Boolean(
    strip &&
      navigation.kind === "workspace" &&
      (navigation.composing || strip.keys.length === 0),
  );
  // In Pro mode, an empty strip shows the launcher instead of the draft
  // composer (ADR 0020).
  const composingNow = navigation.kind === "workspace" && navigation.composing;
  const launcherShown = workspaceLauncherShown({
    proMode,
    stripKeys: strip?.keys ?? null,
    composing: composingNow,
  });
  // A text conversation holds only while its draft composes.
  useEffect(() => {
    if (composingNow) return;
    textDraftRef.current = false;
    setTextDraft(false);
  }, [composingNow]);
  // The agent names of the strip: the "+" menu and the hover card. Orca's
  // names, but Claude shows as "Claude Code".
  const tabAgentLabel = (kind: string) =>
    kind === "claude" ? t("Claude Code") : agentSessionAgentLabel(kind as AgentKind);
  const stripTabs = useMemo<WorkspaceStripTab[]>(() => {
    if (!strip) return [];
    // The running sessions of a tab: its panes for the hover card.
    const runningOf = (owns: (terminal: TerminalSession) => boolean) =>
      terminalSessions.filter((terminal) => terminal.status === "running" && owns(terminal));
    return strip.keys.flatMap((key): WorkspaceStripTab[] => {
      const parsed = parseTabKey(key);
      if (parsed?.kind === "shell") {
        const row = strip.rows.find((candidate) => candidate.key === key);
        if (!shellTabsOn || !row) return [];
        const panes = runningOf(
          (terminal) => !terminal.conversationId && terminal.shellTabId === parsed.id,
        );
        const shell = panes[0]?.shell.split(/[\\/]/).pop();
        return [
          {
            key,
            title: shellTabDisplayTitle(row),
            identity: null,
            glyph: null,
            closing: closingTabKeys.has(key),
            live: liveShellTabIds.has(parsed.id),
            renamable: true,
            splittable: true,
            program: shell || t("Terminal"),
            paneCount: panes.length,
          },
        ];
      }
      const conversation =
        parsed?.kind === "conversation"
          ? conversations.find((item) => item.id === parsed.id) ??
            pendingTabSummariesRef.current.get(parsed.id)
          : undefined;
      if (!conversation) return [];
      const bound = sidebarTerminals.filter(
        (terminal) => terminal.conversationId === conversation.id,
      );
      const state = tabState({
        paneViews: bound.map((terminal) => terminal.pane),
        terminalStatus: conversationTerminalStatus(
          bound.filter((terminal) => !terminal.pane),
          conversation.id,
        ),
        chatRunning: runningConversationIds.has(conversation.id),
        chatUnviewed: unviewedConversationIds.has(conversation.id),
      });
      // An unsent draft names its tab with the live composer text. The
      // stored title lags behind by one save.
      const liveDraftTitle =
        conversation.draftOnly && conversation.id === currentConversationId
          ? draft.trim().replace(/\s+/g, " ").slice(0, 80)
          : "";
      const identity =
        conversation.mode === "chat"
          ? "chat"
          : (bound.find((terminal) => terminal.agentKind)?.agentKind ??
            agentKindForBackend(conversation.agentBackend) ??
            null);
      return [
        {
          key,
          title:
            liveDraftTitle ||
            conversationDisplayTitle(
              conversation,
              conversationNamingTitle(sidebarTerminals, conversation.id),
            ),
          identity,
          glyph: state.glyph,
          unread: state.unread,
          closing: closingTabKeys.has(key),
          live: liveConversationIds.has(conversation.id),
          renamable: true,
          // An agent conversation shows a terminal panel.
          splittable: conversation.mode === "agent",
          program:
            identity === "chat"
              ? t("Chat")
              : identity
                ? tabAgentLabel(identity)
                : t("Terminal"),
          paneCount: runningOf((terminal) => terminal.conversationId === conversation.id).length,
        },
      ];
    });
    // shellTabDisplayTitle reads shellLiveTitles and t.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    closingTabKeys,
    conversations,
    currentConversationId,
    draft,
    liveConversationIds,
    liveShellTabIds,
    runningConversationIds,
    shellLiveTitles,
    shellTabsOn,
    sidebarTerminals,
    strip,
    t,
    terminalSessions,
    unviewedConversationIds,
  ]);
  const stripWorkspaceLabel = (() => {
    const overview = workspaceOverview.find((item) => item.id === stripWorkspaceId);
    return (
      overview?.currentBranch?.trim() ||
      overview?.branch?.trim() ||
      overview?.name ||
      t("Workspace")
    );
  })();
  const defaultAgentKind: AgentKind =
    agentKindForBackend(agentBackendForModel(selectedModelInfo, true)) ?? "claude";

  const stripPlatform = /mac/i.test(navigator.platform)
    ? "mac"
    : /win/i.test(navigator.platform)
      ? "windows"
      : "linux";
  // The agents of the "+" menu and of the launcher, in one order.
  const stripAgents = useMemo(
    () =>
      (["claude", "codex", "gemini", "antigravity", "kimi", "zai", "grok"] as const).map(
        (kind) => ({
          kind,
          ready: terminalSplitAgentChoice(agentSessionsState.laneOptions, kind).ready,
        }),
      ),
    [agentSessionsState.laneOptions],
  );
  const launcherChoices = useMemo(
    () =>
      launcherItems({
        platform: stripPlatform,
        agents: stripAgents,
        defaultAgent: defaultAgentKind,
        shellTabs: shellTabsOn,
      }),
    [defaultAgentKind, shellTabsOn, stripAgents, stripPlatform],
  );
  const launcherWorkspace = launcherShown
    ? (workspaceOverview.find((item) => item.id === stripWorkspaceId) ?? null)
    : null;
  const launcherProject = launcherShown
    ? (projects.find(
        (item) =>
          item.id ===
          (launcherWorkspace?.projectId ??
            workspaceTabsSnapshot.facts.workspaces.get(stripWorkspaceId ?? "")?.projectId),
      ) ?? null)
    : null;
  const stripPlusItems = useMemo(
    () =>
      plusMenuItems({
        platform: stripPlatform,
        agents: stripAgents,
        defaultAgent: defaultAgentKind,
        shellTabs: shellTabsOn,
        reopen: {
          shown: true,
          available: stripWorkspaceId
            ? hasClosedTab(closedTabs, stripWorkspaceId, canReopenClosedTab)
            : false,
        },
      }),
    // canReopenClosedTab reads the store, which workspaceTabsSnapshot follows.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      closedTabs,
      defaultAgentKind,
      shellTabsOn,
      stripAgents,
      stripPlatform,
      stripWorkspaceId,
      workspaceTabsSnapshot,
    ],
  );
  // The chord hints of the tab menu. A disabled chord shows no hint.
  const chordHint = (id: string) => {
    const binding = shortcutForAction(id, shortcutOverrides);
    return binding === "disabled" ? null : formatShortcut(binding);
  };
  const tabMenuShortcuts = {
    // Cmd+D and Shift+Cmd+D split a pane on macOS (TerminalPane).
    splitRight: /mac/i.test(navigator.platform) ? formatShortcut("mod+d") : null,
    splitDown: /mac/i.test(navigator.platform) ? formatShortcut("mod+shift+d") : null,
    close: chordHint("workspace.close_pane_or_tab"),
    rename: chordHint("workspace.rename_tab"),
  };
  // The tab bodies that stay mounted (retention v1). Only bodies with a
  // terminal mount. A chat body is the one shared chat surface.
  const tabBodyConversations = useMemo(() => {
    if (!featureFlags.workspaceTabs) return retainedAgentConversations;
    const homedKeys = Object.values(workspaceTabsSnapshot.tabs.rows)
      .filter((row) => homeWorkspaceId(row, workspaceTabsSnapshot.facts))
      .map((row) => row.key);
    return mountedTabKeys({
      keys: homedKeys,
      visibleKey: currentConversationId ? conversationTabKey(currentConversationId) : null,
      liveConversationIds,
      closingKeys: closingTabKeys,
      looseConversationId: navigation.kind === "loose" ? navigation.conversationId : null,
    }).flatMap((key) => {
      const parsed = parseTabKey(key);
      const conversation =
        parsed?.kind === "conversation"
          ? conversations.find((item) => item.id === parsed.id) ??
            pendingTabSummariesRef.current.get(parsed.id)
          : undefined;
      return conversation ? [conversation] : [];
    });
    // tabBodyKind reads the same inputs as the dependencies below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    closingTabKeys,
    conversations,
    currentConversationId,
    featureFlags.workspaceTabs,
    liveConversationIds,
    navigation,
    retainedAgentConversations,
    workspaceTabsSnapshot.facts,
    workspaceTabsSnapshot.tabs,
  ]);
  // The shell tab bodies that stay mounted: the visible one, every one with
  // a running shell, and every one that closes now (retention v1).
  const tabBodyShells = useMemo(() => {
    if (!shellTabsOn) return [];
    const rows = Object.values(workspaceTabsSnapshot.tabs.rows).filter(
      (row) =>
        row.conversationId === null && homeWorkspaceId(row, workspaceTabsSnapshot.facts),
    );
    const mounted = new Set(
      mountedTabKeys({
        keys: rows.map((row) => row.key),
        visibleKey: navigationTabKey(navigation, workspaceTabsSnapshot),
        liveConversationIds: new Set(),
        liveShellTabIds,
        closingKeys: closingTabKeys,
      }),
    );
    return rows
      .filter((row) => mounted.has(row.key))
      .sort((left, right) => (left.key < right.key ? -1 : left.key > right.key ? 1 : 0));
  }, [closingTabKeys, liveShellTabIds, navigation, shellTabsOn, workspaceTabsSnapshot]);
  // The header shows the workspace switcher only while the tree is hidden.
  const projectTreeHidden = sidebarCollapsed || !featureFlags.projectTree;
  const tabKeysBlocked =
    keyboardShortcutsOpen ||
    commandPaletteOpen ||
    settingsOpen ||
    projectModal !== null ||
    trustPrompt !== null;
  // Option+Cmd+Up and Down: the workspace above or below in the sidebar.
  // The tree rows give the order that the user sees. Without the tree, the
  // overview order (projects, then their workspaces) is the list.
  function cycleWorkspace(direction: "up" | "down") {
    const shown = [
      ...document.querySelectorAll<HTMLElement>('[role="treeitem"][data-tree-key^="workspace:"]'),
    ]
      .filter((row) => row.getClientRects().length > 0)
      .map((row) => (row.dataset.treeKey ?? "").slice("workspace:".length));
    const available = workspaceOverview.filter((item) => item.available);
    const ids = (shown.length > 0 ? shown : available.map((item) => item.id)).filter((id) =>
      available.some((item) => item.id === id),
    );
    const nav = navigationRef.current;
    const current = nav.kind === "workspace" ? nav.workspaceId : null;
    const target = cycledWorkspaceId(ids, current, direction);
    const overview = available.find((item) => item.id === target);
    if (!target || target === current || !overview) return;
    void handleSelectTreeWorkspace(overview.projectId, target);
  }
  // The rows of the Ctrl+Tab switcher: the tabs of the strip, most recent
  // first.
  function recentTabItems(): RecentTabItem[] | null {
    const view = visibleStrip();
    if (!view) return null;
    const shownTabs = new Map(stripTabs.map((tab) => [tab.key, tab]));
    const nav = navigationRef.current;
    const activeKey = nav.kind === "workspace" && !nav.composing ? view.activeKey : null;
    const order = recentTabOrder(
      view.rows.filter((row) => shownTabs.has(row.key)),
      activeKey,
    );
    return order.flatMap((key) => {
      const tab = shownTabs.get(key);
      return tab ? [{ key, title: tab.title, identity: tab.identity, glyph: tab.glyph }] : [];
    });
  }
  useWorkspaceTabKeys({
    enabled: Boolean(strip),
    shellTabs: shellTabsOn && Boolean(strip),
    // The macOS menu gave Cmd+W to the page (ADR 0010). Elsewhere the chain
    // comes with shell tabs.
    closeChain: shellTabsOn || /mac/i.test(navigator.platform),
    workspaces: featureFlags.workspaceTabs,
    blocked: tabKeysBlocked,
    overrides: shortcutOverrides,
    actions: {
      selectByNumber: selectWorkspaceTabByNumber,
      cycle: cycleWorkspaceTab,
      newAgentTab: () => void startAgentTab(defaultAgentKind),
      newShellTab: () => void openShellTab(),
      closePaneOrTab: () => void runCloseChain(),
      reopenClosedTab,
      renameTab: requestVisibleTabRename,
      cycleWorkspace,
    },
  });
  const chatBodyLoading =
    featureFlags.workspaceTabs &&
    currentConversationId !== null &&
    messagesConversationId !== currentConversationId;
  const modeTransitionSessionIdsRef = useRef(new Set<string>());
  useEffect(() => {
    const entry = currentAgentSessionEntry;
    const model = currentConversation?.agentSessionModel ?? entry?.session.model;
    const decision = entry
      ? agentSessionModeDecision(
          true,
          probeForGhostty(connectGateProbe),
          entry.session,
        )
      : null;
    if (
      !isSessionBackend(currentConversation?.agentBackend) ||
      currentConversationMode !== "agent" ||
      !entry ||
      entry.session.mode !== "gui" ||
      decision?.action !== "replace" ||
      !model ||
      modeTransitionSessionIdsRef.current.has(entry.session.id)
    ) {
      return;
    }
    const lane = restartLaneForAgentSession(
      entry.session,
      currentConversation.credentialProfileId,
    );
    if (!lane) {
      setChatError(
        t("This credential-backed agent session cannot switch surfaces yet."),
      );
      return;
    }

    modeTransitionSessionIdsRef.current.add(entry.session.id);
    const opensTerminal =
      entry.session.mode === "gui" && decision.mode !== "gui";
    if (opensTerminal) {
      updateTerminalOpening(currentConversation.id, { type: "start" });
    }
    void (async () => {
      const previousMode = entry.session.mode;
      if (entry.session.mode !== "terminal_ghostty") {
        await agentSessionStore.kill(entry.session.id);
      }
      await agentSessionStore.create({
        agentKind: entry.session.agentKind,
        lane,
        cwd: entry.session.cwd,
        projectId: entry.session.projectId ?? undefined,
        conversationId: currentConversation.id,
        conversationTitle: currentConversation.title,
        model,
        resumeCliSessionId: entry.session.cliSessionId ?? undefined,
        mode: decision.mode,
        permissionMode: approvalPolicyForConversation(currentConversation),
        titleState: {
          defaultTitle:
            entry.session.defaultTitle ?? agentSessionTitle(entry.session),
          generatedTitle: entry.session.generatedTitle,
          aiVaultTitle: entry.session.aiVaultTitle,
          customTitle: entry.session.customTitle,
          generation: entry.session.generation ?? 0,
        },
      });
      if (opensTerminal && decision.mode === "terminal_ghostty") {
        updateTerminalOpening(currentConversation.id, { type: "attach" });
      }
      const notice = conversationHandoffNotice(previousMode, decision.mode);
      if (notice && !opensTerminal) {
        appendNotice(currentConversation.id, notice);
      }
    })()
      .catch((error) => {
        const message = errorMessage(error);
        if (opensTerminal) {
          updateTerminalOpening(currentConversation.id, {
            type: "fail",
            message,
          });
        }
        setChatError(message);
      })
      .finally(() => {
        modeTransitionSessionIdsRef.current.delete(entry.session.id);
      });
  }, [
    connectGateProbe,
    currentAgentSessionEntry,
    currentConversation,
    currentConversationMode,
  ]);
  const activeCredentialProfileId =
    currentConversationId === null
      ? draftCredentialProfileId
      : (currentConversation?.credentialProfileId ?? null);
  const fastModeEnabled = currentConversation?.fastMode ?? draftFastMode;
  const fastModeAvailable =
    status.auth !== null &&
    "chatgpt" in status.auth &&
    Boolean(selectedModelInfo?.supportsFastMode);
  const selectedProject = useMemo(
    () => projects.find((project) => project.id === selectedProjectId) ?? null,
    [projects, selectedProjectId],
  );
  const currentConversationProject = useMemo(() => {
    const projectId = currentConversation?.projectId;
    if (!projectId) {
      return null;
    }
    const listed = projects.find((project) => project.id === projectId);
    if (
      listed &&
      (!currentConversation.projectRootPath ||
        listed.rootPath === currentConversation.projectRootPath)
    ) {
      return listed;
    }
    if (currentConversation.projectName && currentConversation.projectRootPath) {
      return {
        id: projectId,
        name: currentConversation.projectName,
        rootPath: currentConversation.projectRootPath,
        trustLevel:
          currentConversation.projectTrustLevel ?? listed?.trustLevel ?? null,
        approvalPolicy: listed?.approvalPolicy ?? null,
        createdAt: currentConversation.createdAt,
        lastOpenedAt: null,
      };
    }
    return null;
  }, [currentConversation, projects]);
  const composerProject = currentConversation
    ? currentConversationProject
    : selectedProject;
  const activeCommandRegistry = useMemo(
    () =>
      commandRegistryWithCustomPrompts(customPrompts).map((command) => ({
        ...command,
        description: commandDescriptionLabel(command, t),
      })),
    [customPrompts, t],
  );
  const selectedWorkspace = workspaces.find(
    (workspace) => workspace.id === selectedWorkspaceId,
  );
  const treeWorkspaceId =
    featureFlags.workspaceTabs && navigation.kind === "workspace"
      ? navigation.workspaceId
      : currentConversation
        ? effectiveWorkspaceId(currentConversation)
        : selectedWorkspaceId;
  // With workspace tabs, the tree rows of a workspace are its open tabs in
  // strip order, then its live conversations without a tab.
  const treeTabs = useMemo(() => {
    if (!featureFlags.workspaceTabs) return null;
    // Before the tab facts load, the workspaces show no rows. They never
    // show the history rows first and then jump to the tab rows.
    const stripsByWorkspace = new Map<string, string[]>();
    if (!workspaceTabsSnapshot.facts.complete) {
      return { stripsByWorkspace, liveConversationIds: new Set<string>() };
    }
    for (const workspaceId of workspaceTabsSnapshot.facts.workspaces.keys()) {
      const ids = stripView(
        workspaceTabsSnapshot.tabs,
        workspaceTabsSnapshot.facts,
        workspaceId,
      ).keys.flatMap((key) => {
        const parsed = parseTabKey(key);
        return parsed?.kind === "conversation" ? [parsed.id] : [];
      });
      if (ids.length > 0) stripsByWorkspace.set(workspaceId, ids);
    }
    return { stripsByWorkspace, liveConversationIds };
  }, [
    featureFlags.workspaceTabs,
    liveConversationIds,
    workspaceTabsSnapshot.facts,
    workspaceTabsSnapshot.tabs,
  ]);
  const selectTreeWorkspaceRef = useRef<
    (projectId: number, workspaceId: string) => Promise<void>
  >(async () => undefined);
  // The row menu actions of the tree (stage S7). Stable wrappers call the
  // functions of the latest render.
  type TreeActions = NonNullable<ThreadSidebarProjectTree["actions"]>;
  const treeActionImplRef = useRef<
    Omit<TreeActions, "newAgentHere"> & { newAgentHere: (workspaceId: string) => Promise<void> }
  >(null!);
  treeActionImplRef.current = {
    closeConversationTab: async (conversationId) => {
      await closeWorkspaceTab(conversationTabKey(conversationId));
    },
    archiveConversation,
    setConversationUnread,
    newAgentHere: async (workspaceId) => {
      showWorkspace(workspaceId);
      await startAgentTab(defaultAgentKind);
    },
    newTerminalHere: newTerminalInWorkspace,
    openWorkspaceIn: (workspaceId, target) => ipc.workspace.openIn({ workspaceId, target }),
    setWorkspacePinned: setWorkspacePinnedFromTree,
    inspectWorkspace: async (projectId, workspaceId) =>
      (await ipc.workspace.list({ projectId })).find((item) => item.id === workspaceId) ?? null,
    openTerminalCount,
    removeWorktree: removeWorktreeFromTree,
    removeProject: removeProjectFromTree,
    listBaseRefs: (projectId) => ipc.workspace.listBaseRefs({ projectId }),
    createWorktree: createWorktreeFromTree,
  };
  const treeActions = useMemo<TreeActions>(() => {
    const call = treeActionImplRef;
    return {
      closeConversationTab: (id) => call.current.closeConversationTab(id),
      archiveConversation: (id) => call.current.archiveConversation(id),
      setConversationUnread: (id, unread) => call.current.setConversationUnread(id, unread),
      newAgentHere: featureFlags.workspaceTabs
        ? (workspaceId) => call.current.newAgentHere(workspaceId)
        : null,
      newTerminalHere: (projectId, workspaceId) =>
        call.current.newTerminalHere(projectId, workspaceId),
      openWorkspaceIn: (workspaceId, target) => call.current.openWorkspaceIn(workspaceId, target),
      setWorkspacePinned: (workspaceId, pinned) =>
        call.current.setWorkspacePinned(workspaceId, pinned),
      inspectWorkspace: (projectId, workspaceId) =>
        call.current.inspectWorkspace(projectId, workspaceId),
      openTerminalCount: (scope) => call.current.openTerminalCount(scope),
      removeWorktree: (workspaceId) => call.current.removeWorktree(workspaceId),
      removeProject: (projectId) => call.current.removeProject(projectId),
      listBaseRefs: (projectId) => call.current.listBaseRefs(projectId),
      createWorktree: (projectId, request) => call.current.createWorktree(projectId, request),
    };
  }, [featureFlags.workspaceTabs]);
  const sidebarProjectTree = useMemo(
    () =>
      featureFlags.projectTree
        ? {
            workspaces: workspaceOverview,
            currentWorkspaceId: treeWorkspaceId,
            tabs: treeTabs,
            projectsLoaded,
            onSelectWorkspace: (projectId: number, workspaceId: string) => {
              void selectTreeWorkspaceRef.current(projectId, workspaceId);
              if (window.matchMedia("(max-width: 767px)").matches) {
                setSidebarCollapsed(true);
              }
            },
            actions: treeActions,
          }
        : null,
    [
      featureFlags.projectTree,
      projectsLoaded,
      treeActions,
      treeTabs,
      treeWorkspaceId,
      workspaceOverview,
    ],
  );
  const activeEnvironmentConfigPath =
    selectedWorkspace?.localEnvironmentConfigPath ??
    selectedEnvironmentConfigPath;
  const activeLocalEnvironment = localEnvironments.find(
    (environment) =>
      environment.configPath === activeEnvironmentConfigPath &&
      environment.config &&
      !environment.error,
  );
  const actionPlatform = currentActionPlatform();
  // Rust finds an action by its index in the file, so keep that index.
  const localEnvironmentActions =
    activeLocalEnvironment?.config?.actions
      .map((action, index) => ({ action, index }))
      .filter(
        ({ action }) => !action.platform || action.platform === actionPlatform,
      ) ?? [];
  const activeAgentMode = currentConversation
    ? currentConversation.agentMode
    : draftAgentMode;
  const activeWorkflowMode = currentConversation
    ? currentConversation.workflowMode
    : draftWorkflowMode;
  function approvalPolicyForConversation(
    conversation: ConversationSummary,
  ): ApprovalPolicy {
    const project =
      projects.find((item) => item.id === conversation.projectId) ?? null;
    return effectivePolicy(
      conversation.approvalPolicyOverride ?? project?.approvalPolicy ?? null,
      conversationDefaultsRef.current.approvalPolicy,
      project?.trustLevel ?? null,
    );
  }

  const effectiveApprovalPolicy = effectivePolicy(
    (currentConversation
      ? currentConversation.approvalPolicyOverride
      : draftApprovalPolicy) ??
      composerProject?.approvalPolicy ??
      null,
    conversationDefaultsRef.current.approvalPolicy,
    composerProject?.trustLevel ?? null,
  );
  const agentModeAvailable = agentCapabilities?.sandboxAvailable ?? false;
  const agentModeStatus =
    agentCapabilities?.message ?? t("Checking sandbox availability");
  const toolCalls = currentConversationId
    ? (toolCallsByConversation[currentConversationId] ?? [])
    : [];
  const taskPlan = currentConversationId
    ? (taskPlansByConversation[currentConversationId] ?? null)
    : null;
  const subagents = currentConversationId
    ? (subagentsByConversation[currentConversationId] ?? [])
    : [];
  const noticeConversationId = currentConversationId ?? NEW_THREAD;
  // A detached draft turn (the user asked for a fresh view while it was
  // still running) keeps its NEW_THREAD keys until a conversation id adopts
  // them. The fresh draft must not render that turn's terminal state.
  const newThreadTurnDetached =
    currentConversationId === null &&
    conversationTurnState(null)?.pendingNewTurn?.freshViewRequested === true;
  const storedNotices = currentConversationId
    ? (noticesByConversation[currentConversationId] ?? EMPTY_NOTICES)
    : newThreadTurnDetached
      ? EMPTY_NOTICES
      : (noticesByConversation[NEW_THREAD] ?? EMPTY_NOTICES);
  const openingNoticeText = terminalOpeningNotice(
    newThreadTurnDetached
      ? IDLE_TERMINAL_OPENING
      : terminalOpeningByConversation[noticeConversationId] ??
          IDLE_TERMINAL_OPENING,
  );
  const notices = openingNoticeText
    ? [
        ...storedNotices,
        {
          id: `terminal-opening-${noticeConversationId}`,
          text: openingNoticeText,
          createdAt: new Date().toISOString(),
        },
      ]
    : storedNotices;
  const terminalHandoffNotice = [...notices]
    .reverse()
    .find(
      (notice) =>
        [
          "Opening terminal…",
          conversationHandoffNotice("gui", "terminal_embedded"),
        ].includes(notice.text) ||
        notice.text.startsWith("Terminal failed to open —"),
    );
  const activeCompaction = currentConversationId
    ? (compactionsByConversation[currentConversationId] ?? null)
    : null;
  const pendingApproval = currentConversationId
    ? (pendingApprovals[currentConversationId] ?? null)
    : null;
  const visibleQueuedMessages = useMemo(
    () =>
      queuedMessages.filter(
        (item) =>
          item.conversationId === currentConversationId ||
          item.conversationId === null ||
          (item.conversationId === NEW_THREAD && currentConversationId === null),
      ),
    [currentConversationId, queuedMessages],
  );
  const turnDiffs = currentConversationId
    ? (turnDiffsByConversation[currentConversationId] ?? [])
    : [];

  useEffect(() => {
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");

    function syncTheme() {
      const nextResolvedTheme = resolveThemePreference(
        themePreference,
        mediaQuery.matches,
      );
      setResolvedTheme(nextResolvedTheme);
      applyDocumentTheme(themePreference, nextResolvedTheme);
    }

    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, themePreference);
    } catch {
      // Theme persistence is best effort; the DOM class is still applied.
    }

    syncTheme();

    if (themePreference !== "system") {
      return undefined;
    }

    mediaQuery.addEventListener("change", syncTheme);
    return () => mediaQuery.removeEventListener("change", syncTheme);
  }, [themePreference]);

  useEffect(() => {
    const resolved = resolveLanguagePreference(
      languagePreference,
      readSystemLanguageTags(),
    );
    setActiveLocale(resolved);
    writeLanguagePreference(languagePreference);
    document.documentElement.lang = getLocaleDefinition(resolved).htmlLang;

    if (languagePreference !== "system") return undefined;

    function syncSystemLanguage() {
      const nextLocale = resolveLanguagePreference(
        "system",
        readSystemLanguageTags(),
      );
      setActiveLocale(nextLocale);
      document.documentElement.lang = getLocaleDefinition(nextLocale).htmlLang;
    }

    window.addEventListener("languagechange", syncSystemLanguage);
    return () =>
      window.removeEventListener("languagechange", syncSystemLanguage);
  }, [languagePreference]);

  useEffect(() => {
    const mediaQuery = window.matchMedia("(max-width: 767px)");
    const collapseForNarrowShell = (event: MediaQueryListEvent) => {
      if (event.matches) {
        setSidebarCollapsed(true);
      }
    };
    mediaQuery.addEventListener("change", collapseForNarrowShell);
    return () =>
      mediaQuery.removeEventListener("change", collapseForNarrowShell);
  }, []);

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      document.documentElement.classList.remove("theme-loading");
    });
    return () => window.cancelAnimationFrame(frame);
  }, []);

  useEffect(() => {
    applyUiFontSize(readUiFontSize());
  }, []);

  // Rust checks for updates (ADR 0012). Before an update restart, the
  // frontend sends its pending analytics.
  useEffect(() => appUpdateStore.registerFlush(() => flushAnalytics()), []);

  useEffect(function flushAnalyticsWhenVisible() {
    function flushWhenVisible() {
      if (document.visibilityState === "visible") {
        void flushAnalytics();
      }
    }

    void flushAnalytics();
    document.addEventListener("visibilitychange", flushWhenVisible);
    return () =>
      document.removeEventListener("visibilitychange", flushWhenVisible);
  }, []);

  useEffect(() => {
    function refreshGateOnFocus() {
      // The gate, the status and the model list share one read, so each
      // command runs once. The CLI probes are slow, even off the main thread.
      const read = readFocusRefresh();
      void refreshConnectGateAccounts(read.agentAccounts);
      void refreshStatus(read.status);
      void loadModels(read.modelInputs).catch((error) =>
        console.error("Model list refresh failed", error),
      );
    }

    void refreshConnectGateAccounts();
    const refreshTimer = window.setInterval(() => {
      if (document.visibilityState === "visible") refreshGateOnFocus();
    }, 5 * 60 * 1000);
    window.addEventListener("focus", refreshGateOnFocus);
    return () => { window.removeEventListener("focus", refreshGateOnFocus); window.clearInterval(refreshTimer); };
  }, [refreshConnectGateAccounts]);

  useEffect(() => {
    setGateNowMs(Date.now());
  }, [status.account]);

  // A payment issue never opens a modal (ADR 0021). Settings > Account
  // shows the inline payment card with the "Fix payment" button.

  async function refreshStatus(
    read: Promise<StatusState> = readStatusState(),
  ) {
    try {
      const nextStatus = await read;
      setStatus((current) => mergeStatusState(current, nextStatus));
    } catch (error) {
      setStatus({
        account: null,
        auth: null,
        anthropic: null,
        sidecar: null,
        sidecarStats: null,
        error: errorMessage(error),
      });
    }
  }

  async function refreshAccountEntitlements() {
    try {
      const account = await ipc.auth.refreshEntitlements();
      setStatus((current) => ({ ...current, account, error: null }));
    } catch (error) {
      setStatus((current) => ({ ...current, error: errorMessage(error) }));
    }
  }

  async function refreshSavingsTotals() {
    lastTodayDateRef.current = localDateKey();
    try {
      const [totals, today, week, activity] = await Promise.all([
        readSavingsTotals(),
        readSavingsToday(),
        readSavingsWeek(),
        readDailyTokenActivity(tokenActivitySinceIso()),
      ]);
      setSavingsTotals(totals);
      setTodaySavings(today);
      setWeeklySavings(week);
      setDailyTokenActivity(activity);
    } catch (error) {
      setStatus((current) => ({ ...current, error: errorMessage(error) }));
    }
  }

  async function loadAgentCapabilities() {
    try {
      setAgentCapabilities(await ipc.chat.agentCapabilities());
    } catch (error) {
      setAgentCapabilities({
        platform: "unknown",
        sandboxAvailable: false,
        message: errorMessage(error),
      });
    }
  }

  async function loadConversations() {
    const rows = await ipc.storage.listConversations();
    replaceConversations(rows);
    return rows;
  }

  async function loadCredentialProfiles() {
    const profiles = await ipc.storage.listCredentialProfiles();
    setCredentialProfiles(profiles);
    setCredentialProfilesLoaded(true);
    return profiles;
  }

  function handleCredentialProfileChange(
    credentialProfileId: string | null,
    conversation?: ConversationSummary,
  ) {
    if (conversation) {
      upsertConversationSummary(conversation);
      return;
    }
    setDraftCredentialProfileId(credentialProfileId);
  }

  function refreshCredentialProfilesAfterSettingsChange() {
    void Promise.all([loadCredentialProfiles(), loadConversations()]).catch(
      () => undefined,
    );
  }

  function closeSettings() {
    setSettingsOpen(false);
    void loadCredentialProfiles().catch(() => undefined);
  }

  async function refreshAutomationUnreadCount() {
    try {
      setAutomationUnreadCount(
        await ipc.automation.unreadCount(),
      );
    } catch {
      // Automations are additive; a stale unread badge must not block startup.
    }
  }

  async function refreshCustomPrompts(projectRoot: string | null) {
    const requestId = customPromptRequestRef.current + 1;
    customPromptRequestRef.current = requestId;
    try {
      const prompts = await ipc.list.customPrompts({
        projectRoot,
      });
      if (customPromptRequestRef.current === requestId) {
        setCustomPrompts(prompts);
      }
    } catch {
      if (customPromptRequestRef.current === requestId) {
        setCustomPrompts([]);
      }
    }
  }

  function rememberProjectSelection(projectId: number | null) {
    projectSelectionPreferenceRef.current = projectId;
    writeProjectSelectionPreference(projectId);
  }

  function updateSelectedProjectId(projectId: number | null) {
    selectedProjectIdRef.current = projectId;
    setSelectedProjectId(projectId);
    // The funnel counts a project on any path, not only in the guided card.
    if (projectId !== null) void recordAnalyticsEvent("project_selected");
  }

  // The first completed task on any lane (ADR 0018). Native storage
  // deduplicates the event; the flag persists for the next launch.
  function markFirstResult() {
    if (firstResultSeenRef.current) return;
    firstResultSeenRef.current = true;
    setFirstResultSeen(true);
    writeFirstResultSeen();
    void recordAnalyticsEvent("first_task_completed");
  }

  async function loadProjects() {
    const rows = await ipc.project.list();
    setProjects(rows);
    setProjectsLoaded(true);
    setSelectedProjectId((current) => {
      if (current !== null && rows.some((project) => project.id === current)) {
        return current;
      }
      const preference = projectSelectionPreferenceRef.current;
      if (preference === null) {
        return null;
      }
      if (
        typeof preference === "number" &&
        rows.some((project) => project.id === preference)
      ) {
        return preference;
      }
      // The fallback is a guess, not a user pick; persisting it would pin
      // the wrong project for every later launch.
      const fallback = rows[0]?.id ?? null;
      return fallback;
    });
    return rows;
  }

  // The one-time scan of the folder projects from before ADR 0019. It runs
  // after the first project list, once per start, and only with the
  // Projects tree. The tree state marks each scanned root, so a later start
  // scans only the new roots.
  // Terminal adoption (ADR 0020) waits for this scan, so a terminal in a
  // repository of a folder goes to the repository project and not to the
  // folder project.
  const folderScanStartedRef = useRef(false);
  const [folderScanSettled, setFolderScanSettled] = useState(false);
  useEffect(() => {
    if (!projectsLoaded || !featureFlags.projectTree || folderScanStartedRef.current) return;
    folderScanStartedRef.current = true;
    void scanFolderProjectsOnce(projects, projectTreeStateStore.getSnapshot, folderScanDeps())
      .then(async (added) => {
        if (added.length === 0) return;
        await loadProjects();
        await refreshWorkspaceOverview();
      })
      .catch(() => undefined)
      .finally(() => setFolderScanSettled(true));
    // loadProjects reads only setters and refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [featureFlags.projectTree, projects, projectsLoaded]);

  async function loadWorkspaces(
    projectId: number | null,
    conversationId: string | null,
  ) {
    if (projectId == null) {
      setWorkspaces([]);
      setSelectedWorkspaceId(null);
      return [];
    }
    try {
      const [rows, current] = await Promise.all([
        ipc.workspace.list({ projectId }),
        conversationId
          ? ipc.workspace.current({
              conversationId,
            })
          : Promise.resolve(null),
      ]);
      setWorkspaces(rows);
      const pending = pendingWorkspaceSelectionRef.current;
      const pendingId =
        pending?.projectId === projectId &&
        rows.some((workspace) => workspace.id === pending.workspaceId)
          ? pending.workspaceId
          : null;
      if (pending?.projectId === projectId) pendingWorkspaceSelectionRef.current = null;
      setSelectedWorkspaceId((selected) => {
        const next =
          current?.id ??
          pendingId ??
          (selected && rows.some((workspace) => workspace.id === selected)
            ? selected
            : rows.find((workspace) => workspace.kind === "primary")?.id ??
              rows[0]?.id ??
              null);
        selectedWorkspaceIdRef.current = next;
        return next;
      });
      setWorkspaceError(null);
      return rows;
    } catch (error) {
      setWorkspaces([]);
      setSelectedWorkspaceId(null);
      setWorkspaceError(errorMessage(error));
      return [];
    }
  }

  async function loadLocalEnvironments(projectId: number | null) {
    if (projectId == null) {
      setLocalEnvironments([]);
      setSelectedEnvironmentConfigPath(null);
      return [];
    }
    try {
      const rows = await ipc.local.environmentList({ projectId });
      setLocalEnvironments(rows);
      // Repository files can run scripts, so only a stored user choice selects one.
      setSelectedEnvironmentConfigPath(
        resolveLocalEnvironmentSelection(
          readLocalEnvironmentSelection(projectId, window.localStorage),
          rows,
        ),
      );
      return rows;
    } catch (error) {
      setLocalEnvironments([]);
      setSelectedEnvironmentConfigPath(null);
      setWorkspaceError(errorMessage(error));
      return [];
    }
  }

  async function loadWorkspaceBaseRefs(projectId: number | null) {
    if (projectId == null) {
      setWorkspaceBaseRefs(["HEAD"]);
      setSelectedWorktreeBaseRef("HEAD");
      return;
    }
    try {
      const refs = await ipc.workspace.listBaseRefs({
        projectId,
      });
      const nextRefs = refs.length > 0 ? refs : ["HEAD"];
      setWorkspaceBaseRefs(nextRefs);
      setSelectedWorktreeBaseRef((current) =>
        nextRefs.includes(current) ? current : "HEAD",
      );
    } catch (error) {
      setWorkspaceBaseRefs(["HEAD"]);
      setSelectedWorktreeBaseRef("HEAD");
      setWorkspaceError(errorMessage(error));
    }
  }

  async function loadBranchStatus(
    projectId: number | null,
    workspaceId: string | null = selectedWorkspaceIdRef.current,
  ) {
    if (projectId == null) {
      setBranchStatus(null);
      return;
    }
    try {
      setBranchStatus(
        await ipc.project.gitStatus({
          id: projectId,
          workspaceId,
        }),
      );
      setPrMessage(null);
    } catch (error) {
      setBranchStatus({
        isGitRepo: false,
        branch: null,
        defaultBranch: null,
        clean: null,
        warning: errorMessage(error),
      });
    }
  }

  // Loads the data of one conversation. Only openConversation calls it
  // without a navigation generation, and only openConversation shows the
  // conversation (onApplied).
  async function loadMessages(
    conversationId: string,
    options: {
      preserveToolCalls?: boolean;
      navigationGeneration?: number;
      onApplied?: () => void;
    } = {},
  ) {
    setUnviewedConversationIds((current) =>
      clearConversationUnviewed(current, conversationId),
    );
    const startsNavigation = options.navigationGeneration === undefined;
    if (startsNavigation && conversationId !== currentConversationIdRef.current) setDotOpen(false);
    pendingConversationLoadRef.current = conversationId;
    const navigationGeneration =
      options.navigationGeneration ?? beginNavigation();
    const pendingNavigation = startsNavigation
      ? { conversationId, generation: navigationGeneration }
      : null;
    if (pendingNavigation) {
      pendingConversationNavigationRef.current = pendingNavigation;
    }
    try {
      const [
        rows,
        persistedTurnDiffs,
        persistedTaskPlan,
        persistedSubagents,
        persistedCompaction,
      ] = await Promise.all([
        ipc.storage.getMessages({
          conversationId,
        }),
        ipc.chat.listTurnDiffs({
          conversationId,
        }),
        ipc.storage.getConversationTaskPlan({
          conversationId,
        }),
        ipc.chat.listSubagents({
          conversationId,
        }),
        ipc.storage.getLatestConversationCompaction({ conversationId }),
      ]);
      if (!navigationIsCurrent(navigationGeneration)) {
        return null;
      }
      const streamBuffer = conversationTurnStore
        .getSnapshot()
        .streamBuffersByConversation.get(conversationId);
      setMessages(
        mergePersistedMessagesWithStream(
          rows,
          conversationId,
          streamBuffer,
        ),
      );
      markMessagesConversation(conversationId);
      setTurnDiffsByConversation((current) => ({
        ...current,
        [conversationId]: persistedTurnDiffs,
      }));
      setTaskPlansByConversation((current) => {
        const next = { ...current };
        if (persistedTaskPlan) {
          next[conversationId] = persistedTaskPlan;
        } else {
          delete next[conversationId];
        }
        return next;
      });
      setSubagentsByConversation((current) => ({
        ...current,
        [conversationId]: persistedSubagents,
      }));
      setCompactionsByConversation((current) => {
        const next = { ...current };
        if (persistedCompaction) {
          next[conversationId] = persistedCompaction;
        } else {
          delete next[conversationId];
        }
        return next;
      });
      switchComposerDraft(conversationId);
      options.onApplied?.();
      setChatError(null);

      const conversation = conversationsRef.current.find(
        (item) => item.id === conversationId,
      );
      if (conversation) {
        const requestSettings = requestSettingsForConversation(conversation, false);
        conversationRequestSettingsRef.current.set(
          conversationId,
          requestSettings,
        );
        setSelectedModel(requestSettings.model);
        setSelectedEffort(requestSettings.effort);
        // Follow the conversation's project on screen, but do not persist:
        // the stored preference records only explicit project picks. A
        // conversation load here would otherwise overwrite it on every
        // navigation and the app would reopen on the wrong project.
        const conversationProjectId = conversation.projectId ?? null;
        setSelectedProjectId(conversationProjectId);
      }
      if (conversation?.projectId) {
        void ipc.project.open({ id: conversation.projectId })
          .then(() => loadProjects())
          .catch((error) => {
            // The chat still works; the user must see the project problem.
            console.error("project_open failed on conversation load", error);
            setChatError(errorMessage(error));
          });
      }
      return rows;
    } finally {
      if (pendingConversationNavigationRef.current === pendingNavigation) {
        pendingConversationNavigationRef.current = null;
      }
      if (pendingConversationLoadRef.current === conversationId) {
        pendingConversationLoadRef.current = null;
      }
    }
  }

  async function loadSubagents(conversationId: string) {
    const threads = await ipc.chat.listSubagents({
      conversationId,
    });
    setSubagentsByConversation((current) => ({
      ...current,
      [conversationId]: threads,
    }));
    return threads;
  }

  async function loadModels(
    read: Promise<ModelInputsRead> = readModelInputs(),
  ) {
    const { modelRows, laneOptions, probe, auth, profiles } = await read;
    setConnectGateLaneOptions(laneOptions);
    setConnectGateProbe(probe);
    setCredentialProfiles(profiles);
    setCredentialProfilesLoaded(true);
    const nativeModels = modelRows.length > 0 ? modelRows : defaultModels;
    const nextModels = composerModelsWithAgentSessions(
      nativeModels,
      laneOptions,
      probe,
    );
    const nextConnections = connectGateConnections(laneOptions, profiles);
    const nextVisibleModels = visibleComposerModels(nextModels, {
      proMode,
      agentBackend: null,
      directApiModelsAvailable:
        nextConnections.openai || nextConnections.anthropic,
    });
    const currentDefaults = conversationDefaultsRef.current;
    const nextDefaults = resolveConversationDefaults(
      {
        defaultModel: hasExplicitDefaultModelRef.current
          ? currentDefaults.model
          : null,
        defaultEffort: currentDefaults.effort,
        defaultAgentMode: currentDefaults.agentMode,
        defaultApprovalPolicy: currentDefaults.approvalPolicy,
        defaultFastMode: currentDefaults.fastMode,
      },
      nextVisibleModels,
      nextConnections,
      auth,
    );
    setModels(nextModels);
    if (!nextDefaults) return;
    conversationDefaultsRef.current = nextDefaults;
    setSelectedModel((current) =>
      !hasExplicitDefaultModelRef.current &&
      currentConversationIdRef.current === null
        ? nextDefaults.model
        : nextVisibleModels.some((model) => model.id === current)
        ? current
        : nextDefaults.model,
    );
    setSelectedEffort((current) => {
      const activeModel =
        nextModels.find((model) => model.id === selectedModelRef.current) ??
        nextModels[0];
      return activeModel.reasoningEfforts.includes(current)
        ? current
        : activeModel.defaultEffort;
    });
  }

  useEffect(() => {
    let cancelled = false;

    async function loadStatus() {
      try {
        const nextStatus = await readStatusState();
        if (!cancelled) {
          setStatus((current) => mergeStatusState(current, nextStatus));
        }
      } catch (error) {
        if (!cancelled) {
          setStatus({
            account: null,
            auth: null,
            anthropic: null,
            sidecar: null,
            sidecarStats: null,
            error: errorMessage(error),
          });
        }
      }
    }

    function tickStatus() {
      void loadStatus();
      if (localDateKey() !== lastTodayDateRef.current) {
        void refreshSavingsTotals();
      }
    }

    function refreshEntitlements() {
      void refreshAccountEntitlements();
    }

    void loadStatus().then(() => {
      if (!cancelled) {
        refreshEntitlements();
      }
    });
    void refreshSavingsTotals();
    const statusInterval = window.setInterval(tickStatus, 5000);
    const entitlementInterval = window.setInterval(
      refreshEntitlements,
      4 * 60 * 60 * 1000,
    );

    return () => {
      cancelled = true;
      window.clearInterval(statusInterval);
      window.clearInterval(entitlementInterval);
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function loadInitialData() {
      try {
        const [
          modelRows,
          preferences,
          laneOptions,
          probe,
          auth,
          profiles,
          conversationRows,
        ] = await Promise.all([
          ipc.chat.listModels(),
          ipc.preferences.get().catch(() => null),
          listAgentSessionLaneOptions().catch(() => []),
          probeAgentSessions().catch(() => null),
          ipc.auth.credentialStatus().catch(() => null),
          ipc.storage.listCredentialProfiles().catch(() => []),
          loadConversations(),
          loadProjects(),
          refreshSavingsTotals(),
          loadAgentCapabilities(),
          refreshAutomationUnreadCount(),
        ]);
        if (cancelled) {
          return;
        }
        const nativeModels = modelRows.length > 0 ? modelRows : defaultModels;
        const nextModels = composerModelsWithAgentSessions(
          nativeModels,
          laneOptions,
          probe,
        );
        setConnectGateLaneOptions(laneOptions);
        setConnectGateProbe(probe);
        setCredentialProfiles(profiles);
        setCredentialProfilesLoaded(true);
        hasExplicitDefaultModelRef.current = preferences?.defaultModel != null;
        const initialConnections = connectGateConnections(
          laneOptions,
          profiles,
        );
        const initialVisibleModels = visibleComposerModels(nextModels, {
          proMode,
          agentBackend: null,
          directApiModelsAvailable:
            initialConnections.openai || initialConnections.anthropic,
        });
        const defaults = resolveConversationDefaults(
          preferences,
          initialVisibleModels,
          initialConnections,
          auth,
        );
        setModels(nextModels);
        if (defaults) {
          conversationDefaultsRef.current = defaults;
          setSelectedModel(defaults.model);
          setSelectedEffort(defaults.effort);
        }
        setDraftAgentMode(agentModeForNewDraft());
        setDraftApprovalPolicy(preferences?.defaultApprovalPolicy ?? null);
        setDraftFastMode(preferences?.defaultFastMode ?? false);
        setShortcutOverrides(preferences?.keyboardShortcuts ?? {});
        const lastActiveConversationId = resolveLastActiveConversationId(
          readLastActiveConversationId(window.localStorage),
          conversationRows.map((conversation) => conversation.id),
        );
        const conversationIdToLoad = conversationIdToRestore({
          lastActiveConversationId,
          replayOnboarding: ONBOARDING_REPLAY_ENABLED,
        });
        // With workspace tabs, the tab restore picks the first view once
        // the tab facts are complete.
        if (conversationIdToLoad && !featureFlagStore.getSnapshot().workspaceTabs) {
          await openConversation(conversationIdToLoad);
        }
      } catch (error) {
        if (!cancelled) {
          setChatError(errorMessage(error));
        }
      } finally {
        if (!cancelled) {
          setInitialDataLoaded(true);
        }
      }
    }

    void loadInitialData();

    return () => {
      cancelled = true;
    };
  }, []);

  // This listener registers once for the app lifetime. A re-registration
  // opens an async gap that can drop deltas and terminal events.
  useEffect(() => {
    void conversationTurnStore.connect().catch((error) =>
      setChatError(errorMessage(error)),
    );

    return () => {
      conversationTurnStore.disconnect();
    };
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let disposed = false;
    void ipc.events.listen<Automation>("automation_due", (payload) => {
      void runDueAutomation(payload);
    }).then((listener) => {
      if (disposed) listener();
      else unlisten = listener;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    void ipc.tray.setLabels({labels: {
      today: t("Saved today: {amount}", {amount:"{amount}"}),
      total: t("Saved all time: {amount}", {amount:"{amount}"}),
      quota: t("{provider} {window}: {percent}% left · resets in {reset}", {provider:"{provider}",window:"{window}",percent:"{percent}",reset:"{reset}"}),
      unavailable: t("{provider} {window}: unavailable", {provider:"{provider}",window:"{window}"}),
      checked: t("Last checked: {time}", {time:"{time}"}),
      loading: t("Loading usage…"),
      newChat: t("New chat"), show: t("Show Halv"), quit: t("Quit Halv"), stop: t("Quit and Stop Agents"),
    }}).catch(error => console.warn("Tray labels could not be updated", error));
  }, [t]);

  // "How Halv estimates savings" links in the meter and the drawer.
  useEffect(() => {
    const openSavingsMethodPage = () => {
      setSavingsDrawerOpen(false);
      setSettingsSavingsMethodFocusSignal((current) => current + 1);
      setSettingsOpen(true);
    };
    window.addEventListener(OPEN_SAVINGS_METHOD_EVENT, openSavingsMethodPage);
    return () =>
      window.removeEventListener(OPEN_SAVINGS_METHOD_EVENT, openSavingsMethodPage);
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let disposed = false;
    void ipc.events.listen<void>("halv://tray/usage", () => setSavingsDrawerOpen(true)).then(listener => {
      if (disposed) listener(); else unlisten = listener;
    });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let disposed = false;
    // This listener registers once; refs supply the current values.
    void ipc.events.listen<void>(TRAY_NEW_CHAT_EVENT, () => {
      startNewConversationForProjectRef.current(selectedProjectIdRef.current);
    }).then((listener) => {
      if (disposed) listener();
      else unlisten = listener;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let disposed = false;
    // Rust already started the check, so the menu only opens the card.
    void ipc.events.listen<void>(APP_MENU_CHECK_FOR_UPDATES_EVENT, () => {
      appUpdateStore.reveal();
    }).then((listener) => {
      if (disposed) listener();
      else unlisten = listener;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    currentConversationIdRef.current = currentConversationId;
  }, [currentConversationId]);

  useEffect(() => {
    selectedWorkspaceIdRef.current = selectedWorkspaceId;
  }, [selectedWorkspaceId]);

  useEffect(() => {
    conversationsRef.current = conversations;
  }, [conversations]);

  useEffect(() => {
    modelsRef.current = models;
  }, [models]);

  useEffect(() => {
    if (currentConversationIdRef.current) {
      return;
    }
    const defaults = conversationDefaultsRef.current;
    setDraftAgentMode(agentModeForNewDraft());
    setDraftApprovalPolicy(defaults.approvalPolicy);
  }, [selectedProjectId]);

  useEffect(() => {
    void refreshCustomPrompts(composerProject?.rootPath ?? null);
  }, [composerProject?.rootPath]);

  useEffect(() => {
    if (commandPaletteOpen) {
      void refreshCustomPrompts(composerProject?.rootPath ?? null);
    }
  }, [commandPaletteOpen]);

  useEffect(() => {
    function closeHeaderMenuOnPointerDown(event: PointerEvent) {
      if (!headerMenuRef.current?.contains(event.target as Node)) {
        headerMenuRef.current?.removeAttribute("open");
      }
    }
    function closeHeaderMenuOnEscape(event: KeyboardEvent) {
      if (event.key === "Escape") {
        headerMenuRef.current?.removeAttribute("open");
      }
    }
    document.addEventListener("pointerdown", closeHeaderMenuOnPointerDown);
    document.addEventListener("keydown", closeHeaderMenuOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeHeaderMenuOnPointerDown);
      document.removeEventListener("keydown", closeHeaderMenuOnEscape);
    };
  }, []);

  useEffect(() => {
    void loadBranchStatus(composerProject?.id ?? null, selectedWorkspaceId);
  }, [composerProject?.id, selectedWorkspaceId]);

  useEffect(() => {
    if (!composerProject?.id || !selectedWorkspace?.id) {
      return;
    }
    const target = {
      projectId: composerProject.id,
      workspaceId: selectedWorkspace.id,
    };
    let current = true;
    void ipc.crux
      .workspaceOpen(target)
      .then((result) =>
        current ? offerCruxIndexApproval(t, target, result) : false,
      )
      .catch(() => undefined);
    return () => {
      current = false;
    };
  }, [composerProject?.id, selectedWorkspace?.id]);

  useEffect(() => {
    void loadWorkspaces(composerProject?.id ?? null, currentConversationId);
  }, [composerProject?.id, currentConversationId]);

  // The tree overview reloads at start, after a project or worktree change
  // (each one reloads `projects` or `workspaces`), and on window focus,
  // because a terminal can switch a branch.
  const workspaceOverviewKey =
    featureFlags.projectTree || featureFlags.workspaceTabs
    ? [
        ...projects.map((project) => project.id),
        ...workspaces.map(
          (workspace) =>
            `${workspace.id}:${workspace.pinned}:${workspace.state}:${workspace.currentBranch ?? ""}`,
        ),
      ].join("|")
    : null;
  // A handoff or a worktree removal changes the workspaces now.
  async function refreshWorkspaceOverview() {
    const rows = await ipc.workspace.overview();
    workspaceOverviewRef.current = rows;
    setWorkspaceOverview(rows);
    syncTabFacts();
    return rows;
  }
  useEffect(() => {
    if (workspaceOverviewKey === null) return undefined;
    let disposed = false;
    let latest = 0;
    const refresh = () => {
      const request = ++latest;
      void ipc.workspace
        .overview()
        .then((rows) => {
          if (!disposed && request === latest) {
            workspaceOverviewRef.current = rows;
            setWorkspaceOverview(rows);
          }
        })
        .catch(() => undefined);
    };
    refresh();
    window.addEventListener("focus", refresh);
    return () => {
      disposed = true;
      window.removeEventListener("focus", refresh);
    };
  }, [workspaceOverviewKey]);

  // Workspace tabs (ADR 0010). The store reads its snapshot through its
  // queue. The App lists are its facts: views stay empty and no intent
  // writes until the lists loaded.
  const tabsOn = featureFlags.workspaceTabs;
  useEffect(() => {
    if (tabsOn) workspaceTabsStore.refresh();
  }, [tabsOn]);
  useEffect(() => {
    if (tabsOn) syncTabFacts();
    // syncTabFacts reads the refs that these lists update.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabsOn, conversations, workspaceOverview]);
  useEffect(() => {
    const nav = navigationRef.current;
    if (!tabsOn && nav.kind !== "legacy") {
      navigate(legacyNavigation(currentConversationIdRef.current));
    } else if (tabsOn && nav.kind === "legacy") {
      tabsRestoredRef.current = false;
    }
    // navigate is stable in behavior; the flag alone drives this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabsOn]);

  // The first start with the flag seeds the strips once. Then the app
  // shows the workspace of the last session, else the last conversation,
  // else the primary workspace of the selected project.
  const tabsRestoredRef = useRef(false);
  const [tabsReady, setTabsReady] = useState(false);
  useEffect(() => {
    if (!tabsOn || tabsRestoredRef.current) return;
    if (
      !workspaceTabsSnapshot.loaded ||
      !workspaceTabsSnapshot.facts.complete ||
      agentSessionsState.loading
    ) {
      return;
    }
    tabsRestoredRef.current = true;
    void (async () => {
      if (!workspaceTabsStore.getSnapshot().seeded) {
        const terminals = await ipc.terminal.list().catch(() => []);
        await workspaceTabsStore.seedOnce({
          savedConversationScopes: savedConversationPanelScopes(conversationsRef.current),
          liveConversationIds: liveConversationIdsOf({
            agentEntries: agentSessionStore.getSnapshot().entries,
            terminals,
          }),
          lastActiveConversationId: readLastActiveConversationId(window.localStorage),
        });
      }
      restoreTabNavigation();
      setTabsReady(true);
    })();
    // The restore runs once, when the facts are complete.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    agentSessionsState.loading,
    tabsOn,
    workspaceTabsSnapshot.facts.complete,
    workspaceTabsSnapshot.loaded,
  ]);

  // Terminal adoption (ADR 0020) runs once the tabs, the lists and the
  // agent sessions loaded. It pauses while the bottom terminal panel or the
  // Agent sessions panel shows terminals that it could move.
  const adoptionReady =
    featureFlags.workspaceTabs &&
    tabsReady &&
    projectsLoaded &&
    (folderScanSettled || !featureFlags.projectTree) &&
    workspaceTabsSnapshot.facts.complete &&
    !agentSessionsState.loading;
  const adoptionPaused = (terminalOpen && !showProTerminalSurface) || agentSessionsOpen;
  adoptionInputRef.current = adoptionReady
    ? {
        terminals: terminalSessions,
        workspaces: workspaceOverview,
        shellTabs: shellTabsOn,
        conversationHomes: new Map(
          [...liveConversationIds].map((id) => {
            const fact = workspaceTabsSnapshot.facts.conversations.get(id);
            return [id, fact ? conversationHome(fact, workspaceTabsSnapshot.facts.workspaces) : null];
          }),
        ),
        liveConversationIds,
        tabbedConversationIds: new Set(
          Object.values(workspaceTabsSnapshot.tabs.rows).flatMap((row) =>
            row.conversationId ? [row.conversationId] : [],
          ),
        ),
        paused: adoptionPaused,
      }
    : null;
  useEffect(() => {
    if (adoptionReady && !adoptionPaused) void runTerminalAdoption();
    // runTerminalAdoption reads adoptionInputRef, which follows these.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    adoptionPaused,
    adoptionReady,
    liveConversationIds,
    navigation,
    shellTabsOn,
    terminalSessions,
    workspaceOverview,
    workspaceTabsSnapshot.facts,
    workspaceTabsSnapshot.tabs,
  ]);

  function restoreTabNavigation() {
    const facts = workspaceTabsStore.getSnapshot().facts;
    const pending = pendingTabOpenRef.current;
    if (pending) {
      void openConversation(pending);
      return;
    }
    const nav = navigationRef.current;
    if (nav.kind === "workspace" && facts.workspaces.has(nav.workspaceId)) {
      navigate(nav);
      return;
    }
    if (nav !== startNavigationRef.current && nav.kind === "loose") return;
    if (
      nav !== startNavigationRef.current &&
      nav.kind === "workspace" &&
      nav.composing &&
      !nav.workspaceId
    ) {
      // A new task started before the facts loaded: compose in the
      // primary workspace of the selected project.
      startComposing();
      return;
    }
    const lastActive = resolveLastActiveConversationId(
      readLastActiveConversationId(window.localStorage),
      conversationsRef.current.map((conversation) => conversation.id),
    );
    const lastFact = lastActive ? facts.conversations.get(lastActive) : undefined;
    if (lastActive && lastFact && conversationHome(lastFact, facts.workspaces)) {
      void openConversation(lastActive);
      return;
    }
    const projectId = selectedProjectIdRef.current;
    if (projectId !== null && facts.workspaces.has(`primary-${projectId}`)) {
      navigate(workspaceNavigation(`primary-${projectId}`));
      return;
    }
    const first = [...facts.workspaces.values()].find((workspace) => workspace.kind === "primary");
    navigate(first ? workspaceNavigation(first.id) : looseNavigation(null));
  }

  // A conversation that opened before the facts knew it gets its tab now.
  useEffect(() => {
    const pending = pendingTabOpenRef.current;
    if (!tabsOn || !pending || !tabsRestoredRef.current) return;
    const facts = workspaceTabsSnapshot.facts;
    const nav = navigationRef.current;
    if (!facts.complete || !facts.conversations.has(pending)) return;
    if (nav.kind !== "loose" || nav.conversationId !== pending) {
      pendingTabOpenRef.current = null;
      return;
    }
    void openConversation(pending, { load: messagesConversationIdRef.current !== pending });
    // openConversation reads refs; the facts drive this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabsOn, workspaceTabsSnapshot.facts]);

  // The visible tab can change without an open: a close, a cascade, the
  // restore. The composer and the messages then follow it.
  useEffect(() => {
    if (!tabsOn || !currentConversationId) return;
    if (messagesConversationIdRef.current === currentConversationId) return;
    if (pendingConversationLoadRef.current === currentConversationId) return;
    switchComposerDraft(currentConversationId);
    void loadMessages(currentConversationId, {
      navigationGeneration: beginNavigation(),
    });
    // The current conversation alone drives this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentConversationId, tabsOn]);

  // The workspace on screen is the composer workspace: its project and its
  // workspace pick follow navigation.
  const navigationWorkspace =
    tabsOn && navigation.kind === "workspace" ? navigation.workspaceId : null;
  useEffect(() => {
    if (!navigationWorkspace) return;
    const workspace = workspaceTabsSnapshot.facts.workspaces.get(navigationWorkspace);
    if (!workspace) return;
    if (selectedProjectIdRef.current !== workspace.projectId) {
      pendingWorkspaceSelectionRef.current = {
        projectId: workspace.projectId,
        workspaceId: navigationWorkspace,
      };
      updateSelectedProjectId(workspace.projectId);
      return;
    }
    if (
      selectedWorkspaceIdRef.current !== navigationWorkspace &&
      workspaces.some((item) => item.id === navigationWorkspace)
    ) {
      selectedWorkspaceIdRef.current = navigationWorkspace;
      setSelectedWorkspaceId(navigationWorkspace);
    } else if (selectedWorkspaceIdRef.current !== navigationWorkspace) {
      pendingWorkspaceSelectionRef.current = {
        projectId: workspace.projectId,
        workspaceId: navigationWorkspace,
      };
    }
    // The selection setters are stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [navigationWorkspace, workspaceTabsSnapshot.facts, workspaces]);

  useEffect(() => {
    void loadLocalEnvironments(composerProject?.id ?? null);
    void loadWorkspaceBaseRefs(composerProject?.id ?? null);
  }, [composerProject?.id]);

  useEffect(() => {
    if (
      selectedModelInfo &&
      !selectedModelInfo.reasoningEfforts.includes(selectedEffort)
    ) {
      setSelectedEffort(selectedModelInfo.defaultEffort);
    }
  }, [selectedEffort, selectedModelInfo]);

  useEffect(() => {
    function handleGlobalKeyDown(event: KeyboardEvent) {
      // The composer preventDefaults the keys it consumes (e.g. Esc to
      // dismiss the slash palette) — those must not reach global shortcuts.
      if (event.defaultPrevented) {
        return;
      }
      const matches = (id: string) =>
        shortcutMatchesEvent(
          event,
          shortcutForAction(id, shortcutOverrides),
        );
      const hasShortcutModifier = event.metaKey || event.ctrlKey;

      if (keyboardShortcutsOpen) {
        return;
      }

      if (matches("application.keyboard_shortcuts")) {
        event.preventDefault();
        setCommandPaletteOpen(false);
        setKeyboardShortcutsOpen(true);
        return;
      }

      if (matches("application.command_menu")) {
        event.preventDefault();
        setSettingsOpen(false);
        setCommandPaletteOpen((current) => !current);
        return;
      }

      if (commandPaletteOpen) {
        return;
      }

      if (matches("workspace.open_review")) {
        event.preventDefault();
        openDiffRail(true);
        return;
      }

      if (matches("workspace.toggle_review")) {
        event.preventDefault();
        toggleDiffRail();
        return;
      }

      if (
        event.target instanceof Element &&
        event.target.closest("[data-terminal-root]")
      ) {
        return;
      }

      if (matches("application.settings")) {
        event.preventDefault();
        setSettingsOpen((current) => !current);
        return;
      }

      if (
        pendingApproval &&
        !hasShortcutModifier &&
        (event.key === "Enter" || event.key === "Backspace")
      ) {
        if (
          isInteractiveElementFocused() ||
          settingsOpen ||
          projectSwitcherOpen ||
          projectModal !== null ||
          trustPrompt !== null
        ) {
          return;
        }
        const card = document.querySelector("[data-approval-card='current']");
        if (!card || !elementIntersectsViewport(card)) {
          return;
        }
        event.preventDefault();
        const action = event.key === "Enter" ? "approve" : "deny";
        const button = card?.querySelector<HTMLButtonElement>(
          `[data-approval-action="${action}"]`,
        );
        button?.click();
        return;
      }

      if (matches("task.quick_chat")) {
        event.preventDefault();
        setSettingsOpen(false);
        void handleSelectProject(null);
        return;
      }

      if (matches("task.new")) {
        event.preventDefault();
        setSettingsOpen(false);
        startNewConversation();
        return;
      }

      if (matches("workspace.open_folder")) {
        event.preventDefault();
        setSettingsOpen(false);
        openAddFolderModal();
        return;
      }

      if (
        matches("navigation.back") ||
        matches("task.previous")
      ) {
        event.preventDefault();
        navigateRelativeTask(-1);
        return;
      }

      if (
        matches("navigation.forward") ||
        matches("task.next")
      ) {
        event.preventDefault();
        navigateRelativeTask(1);
        return;
      }

      if (matches("appearance.increase_font")) {
        event.preventDefault();
        applyUiFontSize(readUiFontSize() + 1);
        return;
      }

      if (matches("appearance.decrease_font")) {
        event.preventDefault();
        applyUiFontSize(readUiFontSize() - 1);
        return;
      }

      if (matches("application.toggle_sidebar")) {
        event.preventDefault();
        setSidebarCollapsed((current) => !current);
        return;
      }

      if (matches("workspace.open_browser")) {
        event.preventDefault();
        setWorkspaceView((current) =>
          current === "browser" ? "chat" : "browser",
        );
        return;
      }

      if (
        matches("workspace.toggle_bottom_panel") ||
        matches("workspace.toggle_terminal")
      ) {
        event.preventDefault();
        setTerminalOpen((current) => !current);
        return;
      }

      if (matches("task.search")) {
        event.preventDefault();
        setSidebarCollapsed(false);
        setSearchFocusSignal((current) => current + 1);
        return;
      }

      if (matches("task.find")) {
        event.preventDefault();
        if (currentConversationIdRef.current) {
          setTaskFindOpen(true);
        }
        return;
      }

      if (matches("input.dictation")) {
        event.preventDefault();
        setDictationSignal((current) => current + 1);
        return;
      }

      if (event.key === "Escape") {
        if (taskFindOpen) {
          event.preventDefault();
          setTaskFindOpen(false);
        } else if (
          conversationTurnState(currentConversationIdRef.current)
            ?.cancelAndRevertPending
        ) {
          event.preventDefault();
        } else if (
          conversationTurnStore.getSnapshot().runningConversationIds.has(
            conversationTurnKey(currentConversationIdRef.current),
          )
        ) {
          event.preventDefault();
          void handleCancel();
        } else if (queuedRef.current.length > 0) {
          event.preventDefault();
          dispatchNextQueuedMessage(currentConversationIdRef.current);
        }
      }
    }

    window.addEventListener("keydown", handleGlobalKeyDown);
    return () => window.removeEventListener("keydown", handleGlobalKeyDown);
  }, [
    commandPaletteOpen,
    conversations,
    currentConversationId,
    keyboardShortcutsOpen,
    openDiffRail,
    pendingApproval,
    projectModal,
    projectSwitcherOpen,
    settingsOpen,
    shortcutOverrides,
    taskFindOpen,
    toggleDiffRail,
    trustPrompt,
  ]);

  useEffect(() => {
    function cyclePinned(event: KeyboardEvent) {
      if (
        event.defaultPrevented || event.isComposing || keyboardShortcutsOpen ||
        commandPaletteOpen || settingsOpen || projectModal || trustPrompt ||
        (event.target instanceof Element && event.target.closest('[role="dialog"]'))
      ) return;
      const matches = (id: string) =>
        shortcutMatchesEvent(event, shortcutForAction(id, shortcutOverrides));
      const offset = matches("task.previous_pinned")
        ? -1
        : matches("task.next_pinned") ? 1 : null;
      if (offset === null) return;
      // Capture before xterm: these app shortcuts must never become PTY input,
      // including when there are no pins or the current pin is the only one.
      event.preventDefault();
      event.stopImmediatePropagation();
      const currentId =
        pendingConversationNavigationRef.current?.conversationId ??
        currentConversationIdRef.current;
      const target = nextPinnedConversation(conversationsRef.current, currentId, offset);
      if (!target || target === currentId) return;
      for (const terminal of terminalSessions) {
        if (terminal.conversationId === target) {
          terminalActivityStore.markViewed(terminal.id);
        }
      }
      setTaskFindOpen(false);
      void openConversation(target);
    }
    window.addEventListener("keydown", cyclePinned, true);
    return () => window.removeEventListener("keydown", cyclePinned, true);
  }, [
    keyboardShortcutsOpen,
    commandPaletteOpen,
    settingsOpen,
    projectModal,
    trustPrompt,
    shortcutOverrides,
    terminalSessions,
  ]);

  async function connectChatGpt() {
    const auth = await ipc.auth.loginChatgpt();
    setStatus((current) => ({ ...current, auth, error: null }));
    await loadModels().catch((error) =>
      console.error("Model list refresh failed", error),
    );
  }

  async function handleChatGptLogin() {
    setLoginPending(true);
    try {
      await connectChatGpt();
    } catch (error) {
      setStatus((current) => ({
        ...current,
        error: errorMessage(error),
      }));
    } finally {
      setLoginPending(false);
    }
  }

  function handleAccountStatusChange(account: AccountAuthStatus) {
    setAccountFlowOpen((current) =>
      current === "paywall" && account.entitlements?.paymentIssue
        ? current
        : null,
    );
    setStatus((current) => ({ ...current, account, error: null }));
  }

  function handleTrialCheckoutOpened() {
    void recordAnalyticsEvent("checkout_opened");
  }

  async function handleAccountSignOut() {
    try {
      const account = await ipc.auth.signout();
      handleAccountStatusChange(account);
    } catch (error) {
      setStatus((current) => ({ ...current, error: errorMessage(error) }));
    }
  }

  async function handleAccountRefresh() {
    const account = await ipc.auth.refreshEntitlements();
    handleAccountStatusChange(account);
  }

  async function handleLogout() {
    if (
      !(await confirmDialog({
        message: t("Disconnect the active ChatGPT session from Halv?"),
      }))
    ) {
      return;
    }
    try {
      const auth = await ipc.auth.logout();
      setStatus((current) => ({ ...current, auth, error: null }));
      await loadModels().catch((error) =>
        console.error("Model list refresh failed", error),
      );
    } catch (error) {
      setStatus((current) => ({ ...current, error: errorMessage(error) }));
    }
  }

  async function setOpenAiApiKey(apiKey: string) {
    const auth = await ipc.auth.setApiKey({ apiKey });
    setStatus((current) => ({ ...current, auth, error: null }));
    await loadModels().catch((error) =>
      console.error("Model list refresh failed", error),
    );
  }

  async function setAnthropicApiKey(apiKey: string) {
    const anthropic = await ipc.auth.setAnthropicApiKey({ apiKey });
    const auth = await ipc.auth.credentialStatus();
    setStatus((current) => ({ ...current, anthropic, auth, error: null }));
    await loadModels().catch((error) =>
      console.error("Model list refresh failed", error),
    );
  }

  async function addZaiApiKey(apiKey: string) {
    await ipc.credential.profileAdd({
      provider: "zai",
      name: "GLM Coding Plan",
      kind: "api_key",
      secret: apiKey,
    });
    await refreshConnectGateAccounts();
    await loadModels().catch((error) =>
      console.error("Model list refresh failed", error),
    );
  }

  async function addGrokApiKey(apiKey: string) {
    await ipc.credential.profileAdd({
      provider: "xai",
      name: "Grok",
      kind: "api_key",
      secret: apiKey,
    });
    await refreshConnectGateAccounts();
    await loadModels().catch((error) =>
      console.error("Model list refresh failed", error),
    );
  }

  async function handleRestartSidecar() {
    if (sidecarRestartPendingRef.current) {
      return;
    }
    sidecarRestartPendingRef.current = true;
    setSidecarRestartPending(true);
    try {
      const sidecar = await ipc.sidecar.restart();
      const sidecarStats =
        sidecar.state === "running"
          ? await ipc.sidecar.stats().catch(() => null)
          : null;
      setStatus((current) => ({
        ...current,
        sidecar,
        sidecarStats,
        error: null,
      }));
      await refreshStatus();
    } catch (error) {
      const restartError = errorMessage(error);
      await refreshStatus();
      setStatus((current) => ({ ...current, error: restartError }));
    } finally {
      sidecarRestartPendingRef.current = false;
      setSidecarRestartPending(false);
    }
  }

  async function handleInstallHeadroom() {
    const sidecar = await ipc.sidecar.installHeadroom();
    const sidecarStats =
      sidecar.state === "running"
        ? await ipc.sidecar.stats().catch(() => null)
        : null;
    setStatus((current) => ({
      ...current,
      sidecar,
      sidecarStats,
      error: null,
    }));
    return sidecar;
  }

  async function handleUpdateHeadroom() {
    const sidecar = await ipc.sidecar.updateHeadroom();
    const sidecarStats =
      sidecar.state === "running"
        ? await ipc.sidecar.stats().catch(() => null)
        : null;
    setStatus((current) => ({
      ...current,
      sidecar,
      sidecarStats,
      error: null,
    }));
    return sidecar;
  }

  async function handleSelectProject(projectId: number | null) {
    const prepareFirstRunTask =
      firstRunProjectPickPendingRef.current && projectId !== null;
    firstRunProjectPickPendingRef.current = false;
    if (projectId === null) {
      rememberProjectSelection(null);
      updateSelectedProjectId(null);
      setProjectSwitcherOpen(false);
      setProjectError(null);
      if (currentConversationIdRef.current !== null) {
        startNewConversationForProject(null);
      }
      return;
    }
    try {
      const project = await ipc.project.open({ id: projectId });
      rememberProjectSelection(project.id);
      updateSelectedProjectId(project.id);
      setProjectSwitcherOpen(false);
      setProjectError(null);
      if (currentConversationIdRef.current !== null) {
        startNewConversationForProject(project.id);
      }
      await loadProjects();
      if (prepareFirstRunTask) {
        prepareGuidedFirstTask();
      }
    } catch (error) {
      setProjectError(errorMessage(error));
    }
  }

  async function handleSwitchProjectByFragment(fragment: string) {
    const query = fragment.trim().toLowerCase();
    const project = projects.find(
      (item) =>
        item.name.toLowerCase().includes(query) ||
        item.rootPath.toLowerCase().includes(query),
    );
    if (!project) {
      throw new Error(t("No project matches: {fragment}", { fragment }));
    }
    await handleSelectProject(project.id);
  }

  function openAddFolderModal() {
    setProjectModal("add-folder");
    setProjectPathDraft("");
    setProjectError(null);
    setProjectSwitcherOpen(false);
  }

  function openCloneGithubModal() {
    setProjectModal("clone-github");
    setCloneUrlDraft("");
    setCloneDestParentDraft("~/programming");
    setProjectError(null);
    setProjectSwitcherOpen(false);
  }

  async function handleOnboardingPickProject() {
    if (firstRunProjectPickPendingRef.current) return;
    firstRunProjectPickPendingRef.current = true;
    setFirstRunProjectPending(true);
    setProjectError(null);
    let useManualPicker = false;
    try {
      const path = await open({
        directory: true,
        multiple: false,
        title: t("Choose a project folder"),
      });
      if (typeof path === "string") await handleAddFolder(path);
    } catch (error) {
      setProjectError(errorMessage(error));
      useManualPicker = true;
      openAddFolderModal();
    } finally {
      firstRunProjectPickPendingRef.current = useManualPicker;
      setFirstRunProjectPending(false);
    }
  }

  function prepareGuidedFirstTask(prompt = firstTaskPrompt(firstTaskGoal)) {
    firstRunProjectPickPendingRef.current = false;
    void recordAnalyticsEvent("project_selected");
    setDraft(prompt);
    setGuidedTaskPhase("prepared");
    setGuidedTaskRunMode(null);
    guidedTerminalConversationIdRef.current = null;
    guidedTerminalSessionIdRef.current = null;
    guidedTerminalSawActiveRef.current = false;
    guidedTerminalSawSustainedOutputRef.current = false;
    guidedTerminalTrackingArmedRef.current = false;
    setComposerFocusSignal((current) => current + 1);
  }

  function dismissGuidedFirstTask() {
    firstRunProjectPickPendingRef.current = false;
    setGuidedTaskPhase("idle");
    setGuidedTaskRunMode(null);
    guidedTerminalConversationIdRef.current = null;
    guidedTerminalSessionIdRef.current = null;
    guidedTerminalSawActiveRef.current = false;
    guidedTerminalSawSustainedOutputRef.current = false;
    guidedTerminalTrackingArmedRef.current = false;
    // A snooze, not a dismissal: the guide returns on the next fresh launch.
    setOnboardingPreference("dismissed");
  }

  function closeGuidedTaskStatus() {
    if (guidedTaskPhase === "success") {
      setGuidedTaskPhase("idle");
      return;
    }
    dismissGuidedFirstTask();
  }

  async function handleAddFolder(path: string) {
    const prepareFirstRunTask = firstRunProjectPickPendingRef.current;
    const preparedPrompt =
      guidedTaskPhase === "prepared" ? draft : firstTaskPrompt(firstTaskGoal);
    setProjectBusy(true);
    setProjectError(null);
    try {
      const project = await ipc.project.addLocal({ path });
      if (featureFlags.projectTree) {
        // A folder that holds git repositories adds each one and groups
        // them under the folder (ADR 0019). A failed scan keeps the folder.
        await importNestedRepos(project, projects, folderScanDeps()).catch(() => undefined);
      }
      rememberProjectSelection(project.id);
      updateSelectedProjectId(project.id);
      setProjectModal(null);
      if (currentConversationIdRef.current !== null) {
        startNewConversationForProject(project.id);
      }
      await loadProjects();
      if (prepareFirstRunTask) {
        prepareGuidedFirstTask(preparedPrompt);
      }
    } catch (error) {
      setProjectError(errorMessage(error));
    } finally {
      setProjectBusy(false);
    }
  }

  async function handleCloneGithub(url: string, destParent: string) {
    const prepareFirstRunTask = firstRunProjectPickPendingRef.current;
    setProjectBusy(true);
    setProjectError(null);
    try {
      const project = await ipc.project.cloneGithub({
        url,
        destParent: destParent.trim() || null,
      });
      rememberProjectSelection(project.id);
      updateSelectedProjectId(project.id);
      setProjectModal(null);
      if (currentConversationIdRef.current !== null) {
        startNewConversationForProject(project.id);
      }
      await loadProjects();
      if (prepareFirstRunTask) {
        prepareGuidedFirstTask();
      }
    } catch (error) {
      setProjectError(errorMessage(error));
    } finally {
      setProjectBusy(false);
    }
  }

  async function handleRemoveProject(projectId: number) {
    try {
      await removeProjectNow(projectId);
    } catch (error) {
      setProjectError(errorMessage(error));
    }
  }

  async function removeProjectNow(projectId: number) {
    await ipc.project.remove({ id: projectId });
    // SQLite can give the id to the next project, so the removed project
    // leaves its group now.
    projectTreeStateStore.update((state) => withProjectInGroup(state, projectId, null));
    if (selectedProjectId === projectId) {
      rememberProjectSelection(null);
      setSelectedProjectId(null);
    }
    setProjectError(null);
    await loadProjects();
    await loadConversations();
  }

  function appendNotice(
    conversationId: string,
    text: string,
    options: { copyable?: boolean } = {},
  ) {
    const notice: ThreadNotice = {
      id: `notice-${Date.now()}-${Math.random().toString(36).slice(2)}`,
      text,
      createdAt: new Date().toISOString(),
      copyable: options.copyable,
    };
    setNoticesByConversation((current) => ({
      ...current,
      [conversationId]: [...(current[conversationId] ?? []), notice],
    }));
    return notice.id;
  }

  function appendTransientNotice(conversationId: string, text: string) {
    const noticeId = appendNotice(conversationId, text);
    window.setTimeout(() => {
      setNoticesByConversation((current) => {
        const notices = current[conversationId];
        if (!notices?.some((notice) => notice.id === noticeId)) {
          return current;
        }
        return {
          ...current,
          [conversationId]: notices.filter((notice) => notice.id !== noticeId),
        };
      });
    }, 2400);
  }

  // Pro mode runs the first turn inside the embedded terminal, so the
  // terminal opens at send instead of after the first GUI response. Returns
  // false when the regular pipeline should handle the send.
  async function sendIntoConversationTerminal(input: {
    conversationId: string | null;
    text: string;
    agentBackend: import("./types").AgentBackend | null;
    model: string;
    effort?: string;
    projectId: number | null;
    permissionMode: ApprovalPolicy;
  }): Promise<boolean> {
    const agentKind = agentKindForBackend(input.agentBackend);
    if (!agentKind) return false;
    const snapshot = agentSessionStore.getSnapshot();
    const liveEntry = input.conversationId
      ? snapshot.entries.find(
          (entry) =>
            entry.session.conversationId === input.conversationId &&
            entry.session.mode === "terminal_embedded" &&
            ["starting", "running", "waiting_input"].includes(
              entry.session.status,
            ),
        ) ?? null
      : null;
    const project =
      projects.find((item) => item.id === input.projectId) ?? null;
    if (!liveEntry && !project) return false;
    let lane: AgentSessionCreateLane | null = null;
    if (!liveEntry) {
      try {
        lane = await resolveTerminalSplitAgentLane(
          snapshot.laneOptions,
          agentKind,
          () => agentSessionStore.refreshLaneOptions(),
        );
      } catch {
        return false;
      }
    }
    let conversationId = input.conversationId;
    try {
      let conversationSummary =
        conversationsRef.current.find((item) => item.id === conversationId) ??
        null;
      let conversationTitle = conversationSummary?.title ?? null;
      if (!conversationId) {
        let summary = await ipc.storage.setConversationGoal({
            conversationId: null,
            goal: null,
            model: input.model,
            projectId: input.projectId,
            workspaceId: selectedWorkspaceIdRef.current,
          });
        summary = await ipc.storage.setConversationMode({ conversationId: summary.id, mode: "agent" });
        summary = await ipc.storage.setConversationAgentBackend({
            conversationId: summary.id,
            agentBackend: input.agentBackend,
            agentSessionModel: input.model,
          });
        if (draftApprovalPolicy !== null) {
          // The chat pipeline persists the draft approval policy at
          // creation; without this, later resumes and splits fall back to
          // the default preset instead of the one the composer showed.
          summary = await ipc.storage.setConversationApprovalPolicy({ conversationId: summary.id, policy: draftApprovalPolicy });
        }
        upsertConversationSummary(summary);
        conversationId = summary.id;
        conversationSummary = summary;
        conversationTitle = summary.title;
        switchComposerDraft(conversationId, true);
        void openConversation(conversationId, { summary, load: false });
        void loadConversations();
      }
      updateTerminalOpening(conversationId, { type: "start" });
      let sessionId: string;
      // A fresh CLI gets the message as its positional prompt argument;
      // typing into a booting TUI raced its raw-mode setup and lost text.
      let promptAtSpawn = false;
      if (liveEntry) {
        sessionId = liveEntry.session.id;
      } else {
        promptAtSpawn = agentSupportsInitialPrompt(agentKind);
        const created = await agentSessionStore.create({
          agentKind,
          lane: lane!,
          cwd: agentLaunchCwd({ conversation: conversationSummary, project: project! }),
          projectId: input.projectId ?? undefined,
          conversationId,
          conversationTitle: conversationTitle ?? t("Conversation"),
          model: input.model,
          effort: input.effort,
          mode: "terminal_embedded",
          cols: 100,
          rows: 24,
          permissionMode: input.permissionMode,
          initialPrompt: promptAtSpawn ? input.text : undefined,
        });
        sessionId = created.id;
      }
      setTerminalFocusSessionId(sessionId);
      if (guidedTerminalTrackingArmedRef.current) {
        guidedTerminalConversationIdRef.current = conversationId;
        guidedTerminalSessionIdRef.current = sessionId;
      }
      signalTerminalPromptSubmitted(sessionId);
      handleTerminalActivityChange(sessionId, "running");
      clearComposerDraftLocally(conversationId);
      try {
        const summary = await ipc.storage.consumeConversationDraft({ conversationId });
        upsertConversationSummary(summary);
      } catch (error) {
        setChatError(errorMessage(error));
      }
      if (promptAtSpawn) {
        void watchTerminalOpening(sessionId, conversationId);
      } else {
        void deliverTextToTerminal(sessionId, conversationId, input.text);
      }
      return true;
    } catch (error) {
      const message = errorMessage(error);
      if (conversationId) {
        updateTerminalOpening(conversationId, { type: "fail", message });
        const savedDraft = conversationsRef.current.find(
          (conversation) => conversation.id === conversationId,
        )?.draft;
        if (savedDraft) {
          restoreComposerDraftLocally(conversationId, savedDraft, []);
        }
      }
      setChatError(message);
      return true;
    }
  }

  // The message already rode along as the CLI's prompt argument; this only
  // waits for terminal output before it clears the opening spinner.
  async function watchTerminalOpening(sessionId: string, conversationId: string) {
    try {
      const deadline = Date.now() + 15_000;
      for (;;) {
        const attached = await ipc.terminal.attach({
          sessionId,
        });
        if (attached.session.status !== "running") {
          throw new Error(t("The agent terminal exited while opening."));
        }
        if (
          attached.sequence > 0 ||
          attached.scrollback.length > 0 ||
          Date.now() > deadline
        ) {
          break;
        }
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
      updateTerminalOpening(conversationId, { type: "attach" });
    } catch (error) {
      const message = errorMessage(error);
      updateTerminalOpening(conversationId, { type: "fail", message });
      setChatError(message);
    }
  }

  // The CLI must have its TUI up before input lands in its composer; text
  // and the Enter key go separately so the submit never races the paste.
  async function deliverTextToTerminal(
    sessionId: string,
    conversationId: string,
    text: string,
  ) {
    try {
      const deadline = Date.now() + 15_000;
      for (;;) {
        const attached = await ipc.terminal.attach({
          sessionId,
        });
        if (attached.session.status !== "running") {
          throw new Error(
            t("The agent terminal exited before the message could be delivered."),
          );
        }
        if (attached.sequence > 0 || attached.scrollback.length > 0) {
          break;
        }
        if (Date.now() > deadline) {
          // A write to a terminal with no output lands in a raw shell.
          // Fail the delivery instead of writing blind.
          throw new Error(
            t(
              "The agent terminal did not become ready to receive the message.",
            ),
          );
        }
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
      await new Promise((resolve) => setTimeout(resolve, 400));
      // The agent reads this text as typed input. Remove the control
      // characters, so pasted text cannot send escape sequences to it.
      await ipc.terminal.write({ sessionId, data: sanitizeTerminalPaste(text) });
      await new Promise((resolve) => setTimeout(resolve, 160));
      await ipc.terminal.write({ sessionId, data: "\r" });
      updateTerminalOpening(conversationId, { type: "attach" });
    } catch (error) {
      const message = errorMessage(error);
      updateTerminalOpening(conversationId, { type: "fail", message });
      setChatError(message);
    }
  }

  function updateTerminalOpening(
    conversationId: string,
    event: TerminalOpeningEvent,
  ) {
    setTerminalOpeningByConversation((current) => {
      const nextState = reduceTerminalOpening(
        current[conversationId] ?? IDLE_TERMINAL_OPENING,
        event,
      );
      return { ...current, [conversationId]: nextState };
    });
  }

  function moveTerminalOpening(fromId: string, toId: string) {
    if (fromId === toId) return;
    setTerminalOpeningByConversation((current) => {
      const pending = current[fromId];
      if (!pending || pending.phase === "idle") return current;
      const next = { ...current, [toId]: pending };
      delete next[fromId];
      return next;
    });
  }

  function appendSkippedMentionNotices(
    conversationId: string,
    skippedMentions: string[] | undefined,
  ) {
    for (const path of skippedMentions ?? []) {
      appendNotice(
        conversationId,
        t("⚠ Skipped @{mention} (outside project or unreadable)", {
          mention: `{${path}}`,
        }),
      );
    }
  }

  function applyFastResponseMetadata(response: ChatSendResponse) {
    if (response.serviceTier) {
      lastServiceTierByConversationRef.current.set(
        response.conversationId,
        response.serviceTier,
      );
    }
    if (response.fastModeFallback) {
      appendNotice(
        response.conversationId,
        t("Fast mode isn't available for your account — sent at standard speed."),
      );
    }
  }

  function activeGoalForConversation(conversationId: string) {
    const goal = conversationsRef.current
      .find((conversation) => conversation.id === conversationId)
      ?.goal?.trim();
    return goal || null;
  }

  async function clearGoalSilently(conversationId: string) {
    const summary = await ipc.storage.setConversationGoal({
        conversationId,
        goal: null,
        model: null,
        projectId: null,
      });
    upsertConversationSummary(summary);
    await loadConversations();
  }

  function handleGoalSignal(payload: ChatDeltaPayload) {
    if (!payload.goalSignal) {
      return;
    }
    // Read the live translate function through the ref.
    const t = tRef.current;
    const goal = activeGoalForConversation(payload.conversationId);
    if (!goal) {
      return;
    }

    if (payload.goalSignal === "done") {
      appendNotice(payload.conversationId, t("Goal completed: {goal}", { goal }));
      void clearGoalSilently(payload.conversationId).catch((error) =>
        setChatError(errorMessage(error)),
      );
      return;
    }

    // Goal signals are only attached to successful Done deltas, so cancelled or
    // failed turns never enter this auto-continue path.
    void sendMessageRef.current(GOAL_CONTINUE_TEXT, payload.conversationId, {
      origin: "goal",
      queueIfPending: true,
    });
  }

  function queuedMessageId() {
    return (
      window.crypto?.randomUUID?.() ??
      `queued-${Date.now()}-${Math.random().toString(36).slice(2)}`
    );
  }

  function replaceQueuedMessages(next: QueuedMessage[]) {
    queuedRef.current = next;
    setQueuedMessages(next);
  }

  function enqueueMessage(
    text: string,
    conversationId: string | null,
    origin: QueuedMessage["origin"] = "user",
    mentions: string[] = [],
    attachments: ImageAttachment[] = [],
    pinnedProjectId?: number | null,
    pinnedWorkspaceId?: string | null,
  ) {
    replaceQueuedMessages([
      ...queuedRef.current,
      {
        id: queuedMessageId(),
        text,
        conversationId,
        origin,
        mentions: mentions.length > 0 ? mentions : undefined,
        attachments: attachments.length > 0 ? attachments : undefined,
        pinnedProjectId,
        pinnedWorkspaceId,
      },
    ]);
  }

  function projectIdForQueuedTarget(conversationId: string | null) {
    if (conversationId && conversationId !== NEW_THREAD) {
      return (
        conversationsRef.current.find(
          (conversation) => conversation.id === conversationId,
        )?.projectId ?? null
      );
    }
    return selectedProjectId;
  }

  function handleQueueMessage(
    text: string,
    mentions: string[] = [],
    attachments: ImageAttachment[] = [],
  ) {
    const trimmed = text.trim();
    if (!trimmed && attachments.length === 0) {
      return;
    }
    const conversationTarget = conversationTargetForNewInput();
    const inputProjectId = projectIdForQueuedTarget(conversationTarget);
    const inputRoute = resolveInputRoute(trimmed, inputProjectId);
    if (inputRoute.kind === "blocked_shell") {
      if (inputRoute.reason === "project_required") {
        setChatError(t("Shell mode requires a project."));
      }
      return;
    }
    if (inputRoute.kind === "shell" && attachments.length > 0) {
      setChatError(
        t("Image attachments are only available for chat messages, not shell commands."),
      );
      return;
    }
    const targetIsRunning =
      conversationSendRoute(
        conversationTarget,
        conversationTurnStore.getSnapshot().runningConversationIds,
        NEW_THREAD,
      ) === "queue";
    const targetTurnState = conversationTurnState(conversationTarget);
    if (!targetIsRunning && !targetTurnState?.cancelAndRevertPending) {
      void sendMessage(trimmed, conversationTarget, { mentions, attachments });
      return;
    }
    const queuedConversationId =
      conversationTarget === null && targetIsRunning
        ? NEW_THREAD
        : conversationTarget;
    enqueueMessage(
      trimmed,
      queuedConversationId,
      "user",
      mentions,
      attachments,
      pinnedProjectIdForQueuedInput(trimmed, inputProjectId),
      conversationTarget === null || queuedConversationId === NEW_THREAD
        ? selectedWorkspaceIdRef.current
        : null,
    );
  }

  async function handleSteerMessage(
    text: string,
    mentions: string[] = [],
  ) {
    const conversationId = conversationTargetForNewInput();
    if (!conversationId) {
      throw new Error(t("The conversation is not ready for steering yet."));
    }
    try {
      await ipc.chat.steer({
        conversationId,
        text,
        mentions: mentions.length > 0 ? mentions : undefined,
      });
      setChatError(null);
      if (currentConversationIdRef.current === conversationId) {
        const conversation = conversationsRef.current.find(
          (item) => item.id === conversationId,
        );
        setMessages((current) => [
          ...current,
          {
            id: `local-steer-${Date.now()}`,
            conversationId,
            role: "user",
            content: text.trim(),
            createdAt: new Date().toISOString(),
            model: conversation?.agentSessionModel ?? conversation?.model,
            error: false,
          },
        ]);
        setTranscriptScrollSignal((current) => current + 1);
      }
    } catch (error) {
      setChatError(errorMessage(error));
      throw error;
    }
  }

  function handleRemoveQueued(id: string) {
    replaceQueuedMessages(queuedRef.current.filter((item) => item.id !== id));
  }

  function purgeQueuedForConversation(
    conversationId: string,
    includeNullBound: boolean,
  ) {
    replaceQueuedMessages(
      queuedRef.current.filter(
        (item) =>
          item.conversationId !== conversationId &&
          !(includeNullBound && item.conversationId === null),
      ),
    );
  }

  function purgeGoalKickoffs(conversationId: string) {
    replaceQueuedMessages(
      queuedRef.current.filter(
        (item) =>
          item.text !== GOAL_KICKOFF_TEXT ||
          item.conversationId !== conversationId,
      ),
    );
  }

  function takeNextQueuedMessage(turnConversationId: string | null) {
    const nextIndex = queuedRef.current.findIndex(
      (item) => {
        if (
          item.conversationId === NEW_THREAD ||
          (item.conversationId !== turnConversationId &&
            !(turnConversationId !== null && item.conversationId === null))
        ) {
          return false;
        }
        const target = queuedMessageTarget(item, turnConversationId);
        return !conversationTurnStore.getSnapshot().runningConversationIds.has(
          conversationTurnKey(target),
        );
      },
    );
    if (nextIndex === -1) {
      return null;
    }
    const nextMessage = queuedRef.current[nextIndex];
    replaceQueuedMessages(
      queuedRef.current.filter((item) => item.id !== nextMessage.id),
    );
    return nextMessage;
  }

  function takeQueueHead() {
    const nextIndex = queuedRef.current.findIndex((item) => {
      const target = queuedMessageTarget(item, null);
      return !conversationTurnStore
        .getSnapshot()
        .runningConversationIds.has(conversationTurnKey(target));
    });
    if (nextIndex === -1) {
      return null;
    }
    const nextMessage = queuedRef.current[nextIndex];
    if (!nextMessage) {
      return null;
    }
    replaceQueuedMessages(
      queuedRef.current.filter((item) => item.id !== nextMessage.id),
    );
    return nextMessage;
  }

  function queuedMessageTarget(
    item: QueuedMessage,
    fallbackConversationId: string | null,
  ) {
    if (item.conversationId === NEW_THREAD) {
      return null;
    }
    return item.conversationId ?? fallbackConversationId;
  }

  function dispatchQueuedMessage(
    queuedMessage: QueuedMessage,
    fallbackConversationId: string | null,
  ) {
    const targetConversationId = queuedMessageTarget(
      queuedMessage,
      fallbackConversationId,
    );
    void sendMessageRef.current(queuedMessage.text, targetConversationId, {
      origin: queuedMessage.origin ?? "user",
      mentions: queuedMessage.mentions,
      attachments: queuedMessage.attachments,
      pinnedProjectId: queuedMessage.pinnedProjectId,
      pinnedWorkspaceId: queuedMessage.pinnedWorkspaceId,
    });
  }

  function dispatchQueuedNewThreadMessage() {
    const nextMessage = queuedRef.current.find(
      (item) => item.conversationId === NEW_THREAD,
    );
    if (!nextMessage) {
      return false;
    }
    replaceQueuedMessages(
      queuedRef.current.filter((item) => item.id !== nextMessage.id),
    );
    dispatchQueuedMessage(nextMessage, null);
    return true;
  }

  function dispatchNextQueuedMessage(turnConversationId: string | null): boolean {
    const nextQueuedMessage =
      takeNextQueuedMessage(turnConversationId) ?? takeQueueHead();
    if (!nextQueuedMessage) {
      return false;
    }
    dispatchQueuedMessage(nextQueuedMessage, turnConversationId);
    return true;
  }

  async function sendMessage(
    text: string,
    targetConversationId?: string | null,
    options: SendMessageOptions = {},
  ) {
    const trimmedText = text.trim();
    const attachments = options.attachments ?? [];
    if (!trimmedText && attachments.length === 0) {
      return;
    }
    const startingConversationId =
      targetConversationId !== undefined
        ? targetConversationId
        : conversationTargetForNewInput();
    const startingTurnKey = conversationTurnKey(startingConversationId);
    const startingTurnState = conversationTurnState(startingConversationId);
    if (
      conversationSendRoute(
        startingConversationId,
        conversationTurnStore.getSnapshot().runningConversationIds,
        NEW_THREAD,
      ) === "queue" ||
      startingTurnState?.cancelAndRevertPending
    ) {
      if (options.queueIfPending) {
        enqueueMessage(
          trimmedText,
          startingConversationId ?? NEW_THREAD,
          options.origin ?? "user",
          options.mentions,
          attachments,
          options.pinnedProjectId,
          options.pinnedWorkspaceId ?? selectedWorkspaceIdRef.current,
        );
      }
      return;
    }

    let savedDraftSummary: ConversationSummary | null = null;
    if (startingConversationId) {
      try {
        savedDraftSummary = await flushConversationDraft(
          startingConversationId,
        );
      } catch {
        return;
      }
    }

    const turnNavigationGeneration = navigationGenerationRef.current;
    const startingConversation = startingConversationId
      ? (conversationsRef.current.find(
          (conversation) => conversation.id === startingConversationId,
        ) ??
        savedDraftSummary)
      : null;
    const liveProjectId =
      startingConversation?.projectId ??
      (startingConversationId ? null : selectedProjectId);
    const requestProjectId = requestProjectIdForInput(
      startingConversationId !== null,
      liveProjectId,
      options.pinnedProjectId,
    );
    let responseConversationId: string | null = null;
    let turnFailed = false;
    const requestSettings = startingConversation
      ? requestSettingsForConversation(
          startingConversation,
          currentConversationIdRef.current === startingConversationId,
        )
      : startingConversationId
        ? {
            model: conversationDefaultsRef.current.model,
            effort: conversationDefaultsRef.current.effort,
          }
        : {
            model: selectedModelRef.current,
            effort: selectedEffortRef.current,
          };
    const requestModelSelection = requestSettings.model;
    const requestModelInfo = modelsRef.current.find(
      (model) => model.id === requestModelSelection,
    );
    const requestModel =
      requestSettings.agentSessionModel ??
      requestModelForChatModel(requestModelInfo, requestModelSelection);
    const targetsProjectFreeDraft =
      requestProjectId === null &&
      (startingConversation === null || startingConversation.draftOnly);
    const requestConversationMode = targetsProjectFreeDraft
      ? "chat"
      : startingConversation
        ? modeForConversationSelection(startingConversation)
        : modeForComposerDraft(proMode, textDraftRef.current);
    const requestAgentBackend =
      requestSettings.agentBackend ??
      agentBackendForModel(
        requestModelInfo,
        requestConversationMode === "agent",
      );
    const requestEffort = requestSettings.effort;
    const requestAgentSessionEffort =
      (requestModelInfo?.reasoningEfforts.length ?? 0) > 1
        ? requestEffort
        : undefined;
    const requestFastMode = startingConversation?.fastMode ?? draftFastMode;
    const requestCredentialProfileId =
      startingConversationId === null ? draftCredentialProfileId : null;
    const requestWorkflowMode = startingConversation
      ? startingConversation.workflowMode
      : draftWorkflowMode;
    const inputRoute =
      requestWorkflowMode === "plan"
        ? ({ kind: "chat" } as const)
        : resolveInputRoute(trimmedText, requestProjectId);
    if (inputRoute.kind === "blocked_shell") {
      if (inputRoute.reason === "project_required") {
        setChatError(t("Shell mode requires a project."));
      }
      return;
    }
    if (inputRoute.kind === "shell" && attachments.length > 0) {
      setChatError(
        t("Image attachments are only available for chat messages, not shell commands."),
      );
      return;
    }
    if (options.clearComposerDraft) {
      clearComposerDraftLocally(startingConversationId);
    }
    const requestSessionMode = agentSessionModeForProMode(
      requestConversationMode === "agent",
      probeForGhostty(connectGateProbe),
    );
    const opensTerminalForSend =
      inputRoute.kind === "chat" &&
      isSessionBackend(requestAgentBackend) &&
      requestSessionMode !== "gui";
    const requestAgentMode =
      !targetsProjectFreeDraft &&
      (options.guidedTask === true ||
        (startingConversation ? startingConversation.agentMode : draftAgentMode));
    // Pro mode: the terminal opens at send and the turn runs inside it —
    // no GUI turn to wait through before the surface appears.
    if (
      opensTerminalForSend &&
      requestSessionMode === "terminal_embedded" &&
      options.origin !== "goal" &&
      requestWorkflowMode !== "plan" &&
      attachments.length === 0 &&
      (await sendIntoConversationTerminal({
        conversationId: startingConversationId,
        text: trimmedText,
        agentBackend: requestAgentBackend,
        model: requestModel,
        effort: requestAgentSessionEffort,
        projectId: requestProjectId,
        permissionMode: options.guidedTask ? "read_only" : effectivePolicy(
          (startingConversation
            ? startingConversation.approvalPolicyOverride
            : draftApprovalPolicy) ??
            composerProject?.approvalPolicy ??
            null,
          conversationDefaultsRef.current.approvalPolicy,
          composerProject?.trustLevel ?? null,
        ),
      }))
    ) {
      return;
    }
    const optimisticConversationId = startingConversationId ?? "pending";
    const pendingNewTurn: PendingNewTurn | null =
      startingConversationId === null
        ? {
            conversationId: null,
            freshViewRequested: false,
            cancelDispatched: false,
            navigationGeneration: turnNavigationGeneration,
            knownConversationIds: new Set(
              [
                ...conversationsRef.current.map(
                  (conversation) => conversation.id,
                ),
                ...[
                  ...conversationTurnStore.getSnapshot()
                    .runningConversationIds,
                ].filter(
                  (conversationId) => conversationId !== NEW_THREAD,
                ),
              ],
            ),
          }
        : null;
    const turnIsVisible = (conversationId: string | null) =>
      navigationIsCurrent(turnNavigationGeneration) &&
      pendingNewTurn?.freshViewRequested !== true &&
      (currentConversationIdRef.current === conversationId ||
        (startingConversationId === null &&
          currentConversationIdRef.current === null));
    conversationTurnStore.setConversationTurnState(startingTurnKey, {
      startedAt: Date.now(),
      pendingNewTurn,
      cancelRequested: false,
      normalCancelRequested: false,
      cancelAndRevertRequested: false,
      cancelAndRevertDispatched: false,
      cancelAndRevertPending: false,
      cancelAndRevertTurn: false,
      cancelAndRevertDrain: false,
      stopAndRevertAvailable:
        requestProjectId !== null &&
        requestWorkflowMode !== "plan" &&
        (inputRoute.kind === "shell" || requestAgentMode),
    });
    conversationRequestSettingsRef.current.set(
      startingTurnKey,
      requestSettings,
    );
    setConversationRunning(startingConversationId, true);
    if (opensTerminalForSend) {
      updateTerminalOpening(startingTurnKey, { type: "start" });
    }
    if (turnIsVisible(startingConversationId)) {
      setChatError(null);
    }
    if (startingConversationId) {
      conversationTurnStore.setStreamBuffer(startingConversationId, "");
      setReasoningByConversation((current) => ({
        ...current,
        [startingConversationId]: "",
      }));
    }
    if (startingConversationId) {
      setToolCallsByConversation((current) => {
        const next = { ...current };
        delete next[startingConversationId];
        return next;
      });
      setTaskPlansByConversation((current) => {
        const next = { ...current };
        delete next[startingConversationId];
        return next;
      });
      setPendingApprovals((current) => {
        const next = { ...current };
        delete next[startingConversationId];
        return next;
      });
    }
    const optimisticConversationVisible =
      currentConversationIdRef.current === startingConversationId ||
      (startingConversationId === null && currentConversationIdRef.current === null);
    if (optimisticConversationVisible) {
      setMessages((current) => [
        ...current,
        {
          id: `local-${Date.now()}`,
          conversationId: optimisticConversationId,
          role: "user",
          content: trimmedText,
          createdAt: new Date().toISOString(),
          model: requestModel,
          error: false,
          attachments: attachments.length > 0 ? attachments : undefined,
        },
      ]);
      if (
        options.origin !== "goal" ||
        trimmedText === GOAL_KICKOFF_TEXT
      ) {
        setTranscriptScrollSignal((current) => current + 1);
      }
    }

    try {
      const requestWorkspaceId = startingConversationId
        ? await workspaceIdForConversation(startingConversationId)
        : (options.pinnedWorkspaceId ?? selectedWorkspaceIdRef.current);
      const response = inputRoute.kind === "shell"
        ? await ipc.chat.runShell({
            conversationId: startingConversationId,
            command: inputRoute.command,
            model: requestModel,
            projectId: inputRoute.projectId,
            workspaceId: requestWorkspaceId,
            agentMode: requestAgentMode,
            approvalPolicy:
              options.guidedTask
                ? "read_only"
                : startingConversationId === null ? draftApprovalPolicy : null,
            fastMode: requestFastMode,
          })
          : await ipc.chat.send({
            conversationId: startingConversationId,
            text: trimmedText,
            model: requestModel,
            effort: requestEffort,
            agentSessionEffort: requestAgentSessionEffort,
            fastMode: requestFastMode,
            projectId: requestProjectId,
            workspaceId: requestWorkspaceId,
            agentMode: requestAgentMode,
            approvalPolicy:
              options.guidedTask
                ? "read_only"
                : startingConversationId === null ? draftApprovalPolicy : null,
            workflowMode: requestWorkflowMode,
            ...(requestCredentialProfileId
              ? { credentialProfileId: requestCredentialProfileId }
              : {}),
            userInitiated: options.origin !== "goal",
            mentions: options.mentions?.length ? options.mentions : undefined,
              attachments: attachments.length > 0 ? attachments : undefined,
              agentBackend: requestAgentBackend,
              conversationMode:
                startingConversationId === null
                  ? requestConversationMode
                  : undefined,
        });
      responseConversationId = response.conversationId;
      if (options.guidedTask) {
        guidedChatConversationIdRef.current = response.conversationId;
      }
      if (opensTerminalForSend && guidedTerminalTrackingArmedRef.current) {
        guidedTerminalConversationIdRef.current = response.conversationId;
      }
      if (opensTerminalForSend) {
        moveTerminalOpening(startingTurnKey, response.conversationId);
      }
      if (isSessionBackend(requestAgentBackend)) {
        await agentSessionStore.refreshSessions().catch(() => undefined);
      }
      if (opensTerminalForSend && requestSessionMode === "terminal_ghostty") {
        updateTerminalOpening(response.conversationId, { type: "attach" });
      }
        if (requestCredentialProfileId && inputRoute.kind === "chat") {
        setDraftCredentialProfileId((current) =>
          current === requestCredentialProfileId ? null : current,
        );
      }
      applyFastResponseMetadata(response);
      if (pendingNewTurn) {
        resolvePendingNewTurnConversation(
          pendingNewTurn,
          response.conversationId,
        );
      }
      appendSkippedMentionNotices(response.conversationId, response.skippedMentions);
      if (turnIsVisible(response.conversationId)) {
        switchComposerDraft(response.conversationId, true);
        void openConversation(response.conversationId, { load: false });
        await loadMessages(response.conversationId, {
          preserveToolCalls: true,
          navigationGeneration: turnNavigationGeneration,
        });
      }
      await loadConversations();
      await refreshStatus();
      await refreshSavingsTotals();
    } catch (error) {
      turnFailed = true;
      let requestConversationId =
        pendingNewTurn?.conversationId ??
        startingConversationId;
      const message = errorMessage(error);
      if (opensTerminalForSend) {
        updateTerminalOpening(
          responseConversationId ??
            pendingNewTurn?.conversationId ??
            startingTurnKey,
          { type: "fail", message },
        );
      }
      const workspaceBusyFailure =
        message.toLowerCase().includes(WORKSPACE_BUSY_ERROR_TEXT) &&
        inputRoute.kind === "chat";
      let conversationsReloaded = false;
      if (
        workspaceBusyFailure &&
        !requestConversationId &&
        pendingNewTurn &&
        requestProjectId !== null
      ) {
        const conversationRows = await loadConversations().catch(() => null);
        conversationsReloaded = conversationRows !== null;
        const createdConversation = conversationRows?.find(
          (conversation) =>
            !pendingNewTurn.knownConversationIds.has(conversation.id) &&
            conversation.projectId === requestProjectId &&
            conversation.messageCount === 0,
        );
        if (createdConversation) {
          resolvePendingNewTurnConversation(
            pendingNewTurn,
            createdConversation.id,
          );
          requestConversationId = createdConversation.id;
        }
      }
      const failedTurnState = conversationTurnState(requestConversationId);
      if (
        !failedTurnState?.cancelAndRevertTurn &&
        turnIsVisible(requestConversationId) &&
        (!requestConversationId ||
          !deletedConversationIdsRef.current.has(requestConversationId))
      ) {
        if (
          workspaceBusyFailure &&
          requestConversationId &&
          requestProjectId !== null
        ) {
          workspaceBusyRetryRef.current = {
            conversationId: requestConversationId,
            projectId: requestProjectId,
            text: trimmedText,
            options: { ...options, attachments },
          };
        } else {
          workspaceBusyRetryRef.current = null;
        }
        setChatError(message);
      }
      if (!conversationsReloaded) {
        await loadConversations().catch(() => undefined);
      }
      if (
        options.clearComposerDraft &&
        startingConversationId &&
        turnIsVisible(startingConversationId)
      ) {
        const savedDraft = conversationsRef.current.find(
          (conversation) => conversation.id === startingConversationId,
        )?.draft;
        if (savedDraft) {
          restoreComposerDraftLocally(
            startingConversationId,
            savedDraft,
            attachments,
          );
        }
      }
      await refreshStatus().catch(() => undefined);
      await refreshSavingsTotals().catch(() => undefined);
    } finally {
      const drainConversationId =
        responseConversationId ??
        pendingNewTurn?.conversationId ??
        startingConversationId;
      const finishedTurnState = conversationTurnState(drainConversationId);
      const wasCancelRequested = finishedTurnState?.cancelRequested ?? false;
      const wasCancelAndRevert =
        finishedTurnState?.cancelAndRevertTurn ?? false;
      const cancelAndRevertDrain =
        finishedTurnState?.cancelAndRevertDrain ?? false;
      setConversationRunning(drainConversationId, false);
      if (drainConversationId) {
        conversationTurnStore.deleteStreamBuffer(drainConversationId);
      }
      if (drainConversationId === null) {
        conversationRequestSettingsRef.current.delete(NEW_THREAD);
      }
      clearConversationTurnState(drainConversationId);
      if (cancelAndRevertDrain) {
        dispatchNextQueuedMessage(drainConversationId);
      } else if (
        !wasCancelAndRevert &&
        (!turnFailed || wasCancelRequested)
      ) {
        dispatchNextQueuedMessage(drainConversationId);
      }
    }
  }

  sendMessageRef.current = sendMessage;
  startNewConversationForProjectRef.current = startNewConversationForProject;

  function handleSend(
    mentions: string[] = [],
    attachments: ImageAttachment[] = [],
  ) {
    const text = draft.trim();
    if (!text && attachments.length === 0) {
      return;
    }
    const currentConversationId = currentConversationIdRef.current;
    const currentConversation = currentConversationId
      ? conversationsRef.current.find(
          (conversation) => conversation.id === currentConversationId,
        )
      : null;
    const shouldTrackGuidedTask =
      guidedTaskPhase === "prepared" &&
      (currentConversationId === null ||
        currentConversation?.draftOnly === true ||
        currentConversation === undefined);
    if (shouldTrackGuidedTask) {
      setGuidedTaskCancelled(false);
      guidedTaskPromptRef.current = text;
      guidedChatConversationIdRef.current = currentConversationId;
      const terminalMode = activeAgentMode;
      setGuidedTaskRunMode(terminalMode ? "terminal" : "chat");
      guidedTerminalTrackingArmedRef.current = terminalMode;
      guidedTerminalConversationIdRef.current = null;
      guidedTerminalSessionIdRef.current = null;
      guidedTerminalSawActiveRef.current = false;
      guidedTerminalSawSustainedOutputRef.current = false;
    }
    const targetConversationId = conversationTargetForNewInput();
    // A send never waits for an account (ADR 0021).
    void recordAnalyticsEvent("active_day");
    // The funnel counts the first task on any path (native dedupes it).
    void recordAnalyticsEvent("first_task_started");
    if (
      conversationSendRoute(
        targetConversationId,
        conversationTurnStore.getSnapshot().runningConversationIds,
        NEW_THREAD,
      ) === "queue"
    ) {
      handleQueueMessage(text, mentions, attachments);
      if (shouldTrackGuidedTask) {
        setGuidedTaskPhase("waiting");
      }
      setDraft("");
      setDraftAttachments([]);
      return;
    }
    void sendMessage(text, targetConversationId, {
      mentions,
      attachments,
      clearComposerDraft: true,
      guidedTask: shouldTrackGuidedTask,
    });
    if (shouldTrackGuidedTask) {
      setGuidedTaskPhase("waiting");
    }
  }

  function handleSuggestion(text: string) {
    setDraft(text);
    setComposerFocusSignal((current) => current + 1);
  }

  async function handleEditMessage(message: UiMessage) {
    if (
      conversationTurnStore
        .getSnapshot()
        .runningConversationIds.has(message.conversationId) ||
      conversationTurnState(message.conversationId)?.cancelAndRevertPending ||
      message.role !== "user" ||
      currentConversationIdRef.current !== message.conversationId
    ) {
      return;
    }
    const editNavigationGeneration = navigationGenerationRef.current;
    const editIsVisible = () =>
      navigationIsCurrent(editNavigationGeneration) &&
      currentConversationIdRef.current === message.conversationId;
    let editTarget = message;
    let summary: ConversationSummary | null = null;

    for (let attempt = 0; attempt < 2; attempt += 1) {
      try {
        summary = await ipc.storage.truncateFrom({
          conversationId: editTarget.conversationId,
          messageId: editTarget.id,
        });
        break;
      } catch (error) {
        console.error("Failed to truncate a message during Edit", error);
        if (!isMissingEditMessageError(error)) {
          if (editIsVisible()) {
            setChatError(storageTruncateErrorMessage(error));
          }
          return;
        }

        let refreshedMessages: StoredMessage[] | null = null;
        try {
          refreshedMessages = await loadMessages(message.conversationId, {
            navigationGeneration: editNavigationGeneration,
          });
        } catch (refreshError) {
          console.error("Failed to refresh messages after an Edit lookup miss", refreshError);
        }
        await loadConversations().catch((refreshError) =>
          console.error("Failed to refresh conversations after an Edit lookup miss", refreshError),
        );
        await refreshSavingsTotals().catch((refreshError) =>
          console.error("Failed to refresh savings after an Edit lookup miss", refreshError),
        );
        if (!editIsVisible()) {
          return;
        }

        const retryTarget = refreshedMessages
          ? findEditRetryMessage(editTarget, refreshedMessages)
          : null;
        if (attempt === 0 && retryTarget) {
          editTarget = retryTarget;
          continue;
        }
        setChatError(
          t(
            "That message is no longer in this conversation — it may have been reverted. Refreshing the thread.",
          ),
        );
        return;
      }
    }

    if (!summary) {
      return;
    }
    try {
      upsertConversationSummary(summary);
      const messagesApplied = await loadMessages(message.conversationId, {
        navigationGeneration: editNavigationGeneration,
      });
      await loadConversations();
      await refreshSavingsTotals();
      if (!messagesApplied || !editIsVisible()) {
        return;
      }
      setDraft(editTarget.content);
      setDraftAttachments(editTarget.attachments ?? []);
      setComposerMentionSeed((current) => ({
        signal: current.signal + 1,
        mentions: editTarget.mentions ?? [],
      }));
      setComposerFocusSignal((current) => current + 1);
    } catch (error) {
      if (editIsVisible()) {
        setChatError(errorMessage(error));
      }
    }
  }

  async function handleRegenerateMessage(message: UiMessage) {
    if (
      conversationTurnState(message.conversationId)?.cancelAndRevertPending ||
      conversationTurnStore
        .getSnapshot()
        .runningConversationIds.has(message.conversationId) ||
      message.role !== "assistant" ||
      currentConversationIdRef.current !== message.conversationId
    ) {
      return;
    }

    const conversationId = message.conversationId;
    const regenerateNavigationGeneration = navigationGenerationRef.current;
    const regenerateIsVisible = () =>
      navigationIsCurrent(regenerateNavigationGeneration) &&
      currentConversationIdRef.current === conversationId;
    const regenerateConversation = conversationsRef.current.find(
      (conversation) => conversation.id === conversationId,
    );
    if (!regenerateConversation) {
      setChatError(t("Conversation is no longer available."));
      return;
    }
    const requestSettings = requestSettingsForConversation(
      regenerateConversation,
      true,
    );
    const requestModelInfo = modelsRef.current.find(
      (model) => model.id === requestSettings.model,
    );
    const requestModel =
      requestSettings.agentSessionModel ??
      requestModelForChatModel(requestModelInfo, requestSettings.model);
    const requestEffort = requestSettings.effort;
    let turnFailed = false;
    conversationTurnStore.setConversationTurnState(conversationId, {
      startedAt: Date.now(),
      pendingNewTurn: null,
      cancelRequested: false,
      normalCancelRequested: false,
      cancelAndRevertRequested: false,
      cancelAndRevertDispatched: false,
      cancelAndRevertPending: false,
      cancelAndRevertTurn: false,
      cancelAndRevertDrain: false,
      stopAndRevertAvailable: Boolean(
        regenerateConversation.agentMode &&
          regenerateConversation.workflowMode !== "plan" &&
          typeof regenerateConversation.projectId === "number",
      ),
    });
    conversationRequestSettingsRef.current.set(
      conversationId,
      requestSettings,
    );
    setConversationRunning(conversationId, true);
    conversationTurnStore.setStreamBuffer(conversationId, "");
    setChatError(null);
    setReasoningByConversation((current) => ({
      ...current,
      [conversationId]: "",
    }));
    setToolCallsByConversation((current) => {
      const next = { ...current };
      delete next[conversationId];
      return next;
    });
    setTaskPlansByConversation((current) => {
      const next = { ...current };
      delete next[conversationId];
      return next;
    });
    setPendingApprovals((current) => {
      const next = { ...current };
      delete next[conversationId];
      return next;
    });
    if (regenerateIsVisible()) {
      setMessages((current) => removeTrailingAssistantMessages(current));
    }

    try {
      const response = await ipc.chat.regenerate({
        conversationId,
        model: requestModel,
        effort: requestEffort,
        agentSessionEffort:
          (requestModelInfo?.reasoningEfforts.length ?? 0) > 1
            ? requestEffort
            : undefined,
      });
      applyFastResponseMetadata(response);
      appendSkippedMentionNotices(conversationId, response.skippedMentions);
      if (regenerateIsVisible()) {
        await loadMessages(conversationId, {
          preserveToolCalls: true,
          navigationGeneration: regenerateNavigationGeneration,
        });
      }
      await loadConversations();
      await refreshStatus();
      await refreshSavingsTotals();
    } catch (error) {
      turnFailed = true;
      if (regenerateIsVisible()) {
        await loadMessages(conversationId, {
          preserveToolCalls: true,
          navigationGeneration: regenerateNavigationGeneration,
        }).catch(() => undefined);
      }
      if (
        !conversationTurnState(conversationId)?.cancelAndRevertTurn &&
        regenerateIsVisible() &&
        !deletedConversationIdsRef.current.has(conversationId)
      ) {
        setChatError(errorMessage(error));
      }
      await loadConversations().catch(() => undefined);
      await refreshStatus().catch(() => undefined);
      await refreshSavingsTotals().catch(() => undefined);
    } finally {
      conversationTurnStore.deleteStreamBuffer(conversationId);
      const finishedTurnState = conversationTurnState(conversationId);
      const wasCancelRequested = finishedTurnState?.cancelRequested ?? false;
      const wasCancelAndRevert =
        finishedTurnState?.cancelAndRevertTurn ?? false;
      const cancelAndRevertDrain =
        finishedTurnState?.cancelAndRevertDrain ?? false;
      setConversationRunning(conversationId, false);
      clearConversationTurnState(conversationId);
      if (cancelAndRevertDrain) {
        dispatchNextQueuedMessage(conversationId);
      } else if (
        !wasCancelAndRevert &&
        (!turnFailed || wasCancelRequested)
      ) {
        dispatchNextQueuedMessage(conversationId);
      }
    }
  }

  async function handleCancel() {
    const viewedConversationId = currentConversationIdRef.current;
    const turnKey = runningTurnKeyForView(viewedConversationId);
    if (!turnKey) {
      return;
    }
    if (guidedTaskPhase === "waiting" && guidedTaskRunMode === "chat" &&
        viewedConversationId === guidedChatConversationIdRef.current) {
      setGuidedTaskCancelled(true);
      void recordAnalyticsEvent("first_task_failed", "cancelled");
    }
    const turnConversationId = turnKey === NEW_THREAD ? null : turnKey;
    const viewedTurnState = conversationTurnState(turnConversationId);
    if (
      !viewedTurnState ||
      viewedTurnState.cancelAndRevertRequested
    ) {
      return;
    }
    updateConversationTurnState(turnConversationId, {
      cancelRequested: true,
      normalCancelRequested: true,
    });
    const pendingNewTurn = viewedTurnState.pendingNewTurn;
    const conversationId =
      pendingNewTurn?.conversationId ??
      (pendingNewTurn ? null : turnConversationId);
    if (!conversationId) {
      if (!pendingNewTurn) {
        updateConversationTurnState(turnConversationId, {
          cancelRequested: false,
          normalCancelRequested: false,
        });
      }
      return;
    }
    await requestChatCancel(conversationId, pendingNewTurn);
  }

  async function handleCancelAndRevert() {
    const viewedConversationId = currentConversationIdRef.current;
    const turnKey = runningTurnKeyForView(viewedConversationId);
    if (!turnKey) {
      return;
    }
    if (guidedTaskPhase === "waiting" && guidedTaskRunMode === "chat" &&
        viewedConversationId === guidedChatConversationIdRef.current) {
      setGuidedTaskCancelled(true);
      void recordAnalyticsEvent("first_task_failed", "cancelled");
    }
    const turnConversationId = turnKey === NEW_THREAD ? null : turnKey;
    const viewedTurnState = conversationTurnState(turnConversationId);
    if (
      !viewedTurnState ||
      viewedTurnState.cancelAndRevertRequested ||
      viewedTurnState.cancelRequested
    ) {
      return;
    }
    updateConversationTurnState(turnConversationId, {
      cancelAndRevertRequested: true,
      cancelAndRevertDispatched: false,
      cancelAndRevertDrain: false,
      cancelAndRevertPending: true,
      cancelRequested: false,
      normalCancelRequested: false,
    });

    const pendingNewTurn = viewedTurnState.pendingNewTurn;
    const conversationId =
      pendingNewTurn?.conversationId ??
      (pendingNewTurn ? null : turnConversationId);
    if (!conversationId) {
      if (!pendingNewTurn) {
        updateConversationTurnState(turnConversationId, {
          cancelAndRevertRequested: false,
          cancelAndRevertTurn: false,
          cancelAndRevertPending: false,
          stopAndRevertAvailable: false,
        });
      }
      return;
    }
    await requestChatCancelAndRevert(conversationId, pendingNewTurn);
  }

  async function handleDeleteConversation(conversationId: string) {
    const shouldCancelStream =
      conversationId === currentConversationIdRef.current ||
      conversationTurnStore
        .getSnapshot()
        .runningConversationIds.has(conversationId);
    purgeQueuedForConversation(
      conversationId,
      Boolean(conversationTurnState(conversationId)?.pendingNewTurn),
    );
    deletedConversationIdsRef.current.add(conversationId);
    const tabsOn = featureFlagStore.getSnapshot().workspaceTabs;
    // ADR 0010: a delete stops the processes of the conversation first.
    // The foreign key cascade then removes its tab. The next tab shows.
    let successorKey: TabKey | null = null;
    let deletedHome: string | null = null;
    try {
      if (tabsOn) {
        const handle = terminalPanelRegistry.request(conversationPanelScope(conversationId));
        if (handle) await handle.closeAll({ park: false });
        else await stopConversationSessions(conversationId);
        const snapshot = workspaceTabsStore.getSnapshot();
        const key = conversationTabKey(conversationId);
        const row = snapshot.tabs.rows[key];
        deletedHome = row ? homeWorkspaceId(row, snapshot.facts) : null;
        if (deletedHome) {
          const view = stripView(snapshot.tabs, snapshot.facts, deletedHome);
          if (view.activeKey === key) successorKey = closeSuccessor(view, key);
        }
      }
      if (shouldCancelStream) {
        await ipc.chat.cancel({ conversationId }).catch(() => undefined);
      }
      await flushConversationDraft(conversationId).catch(() => undefined);
      const draftTimer = draftSaveTimersRef.current.get(conversationId);
      if (draftTimer !== undefined) {
        window.clearTimeout(draftTimer);
        draftSaveTimersRef.current.delete(conversationId);
      }
      await ipc.storage.deleteConversation({ conversationId });
      if (tabsOn) {
        const wasVisible = conversationId === currentConversationIdRef.current;
        workspaceTabsStore.close(conversationTabKey(conversationId));
        workspaceTabsStore.refresh();
        const successor = parseTabKey(successorKey);
        const after = workspaceTabsStore.getSnapshot();
        const emptyHome =
          deletedHome &&
          stripView(after.tabs, after.facts, deletedHome).keys.length === 0
            ? deletedHome
            : null;
        if (wasVisible && successor?.kind === "conversation") {
          void openConversation(successor.id);
        } else if (wasVisible && emptyHome && agentSessionProModeStore.getSnapshot()) {
          // An empty strip shows the launcher in Pro mode (ADR 0020).
          switchComposerDraft(null);
          markMessagesConversation(null);
          setMessages([]);
          navigate(workspaceNavigation(emptyHome));
          focusVisibleTab();
        } else if (wasVisible) {
          startNewConversation();
        }
      } else if (conversationId === currentConversationIdRef.current) {
        startNewConversation();
      }
      draftsByConversationRef.current.delete(conversationId);
      draftAttachmentsByConversationRef.current.delete(conversationId);
      setToolCallsByConversation((current) => {
        const next = { ...current };
        delete next[conversationId];
        return next;
      });
      setTaskPlansByConversation((current) => {
        const next = { ...current };
        delete next[conversationId];
        return next;
      });
      setSubagentsByConversation((current) => {
        const next = { ...current };
        delete next[conversationId];
        return next;
      });
      setPendingApprovals((current) => {
        const next = { ...current };
        delete next[conversationId];
        return next;
      });
      setTurnDiffsByConversation((current) => {
        const next = { ...current };
        delete next[conversationId];
        return next;
      });
      setCompactionsByConversation((current) => {
        const next = { ...current };
        delete next[conversationId];
        return next;
      });
      setConversationRunning(conversationId, false);
      clearConversationTurnState(conversationId);
      conversationTurnStore.deleteStreamBuffer(conversationId);
      setUnviewedConversationIds((current) =>
        clearConversationUnviewed(current, conversationId),
      );
      await loadConversations();
      await refreshSavingsTotals();
    } catch (error) {
      deletedConversationIdsRef.current.delete(conversationId);
      setChatError(errorMessage(error));
    }
  }

  async function handleRenameConversation(title: string) {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      throw new Error(t("Open a conversation before renaming it."));
    }
    const summary = await ipc.storage.setConversationTitle({ conversationId, title });
    upsertConversationSummary(summary);
    await loadConversations();
    appendTransientNotice(
      conversationId,
      t("Renamed to “{title}”", { title: summary.title }),
    );
  }

  async function handleDeleteCurrentConversation() {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      throw new Error(t("Open a conversation before deleting it."));
    }
    const conversation = conversationsRef.current.find(
      (item) => item.id === conversationId,
    );
    if (
      !(await confirmDialog({
        danger: true,
        message: conversation?.title
          ? t("Delete “{title}”? Token savings remain in your totals.", {
              title: conversation.title,
            })
          : t("Delete “this conversation”? Token savings remain in your totals."),
      }))
    ) {
      return;
    }
    await handleDeleteConversation(conversationId);
  }

  async function handleSetConversationArchived(
    conversationId: string,
    archived: boolean,
  ) {
    const summary = await ipc.storage.setConversationArchived({ conversationId, archived });
    upsertConversationSummary(summary);
    await loadConversations();
  }

  async function handleSetConversationPinned(
    conversationId: string,
    pinned: boolean,
  ) {
    try {
      const summary = await ipc.storage.setConversationPinned({ conversationId, pinned });
      upsertConversationSummary(summary);
      await loadConversations();
    } catch (error) {
      setChatError(errorMessage(error));
    }
  }

  async function handleUnarchiveConversation(conversationId: string) {
    try {
      await handleSetConversationArchived(conversationId, false);
      const nav = navigationRef.current;
      if (
        featureFlagStore.getSnapshot().workspaceTabs &&
        nav.kind === "loose" &&
        nav.conversationId === conversationId
      ) {
        // The view moves into the strip. The loose panel gives up the scope.
        await openConversation(conversationId, { load: false });
      }
    } catch (error) {
      setChatError(errorMessage(error));
    }
  }

  async function handleArchiveCurrentConversation() {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      throw new Error(t("Open a conversation before archiving it."));
    }
    await archiveConversation(conversationId);
  }

  // Archives a conversation: the current one (command palette), or a row of
  // the tree.
  async function archiveConversation(conversationId: string) {
    if (
      conversationTurnStore
        .getSnapshot()
        .runningConversationIds.has(conversationId)
    ) {
      throw new Error(t("Wait for the current turn to finish before archiving."));
    }

    if (featureFlagStore.getSnapshot().workspaceTabs) {
      // ADR 0010: the tab closes first (one prompt, park, the next tab
      // shows). An archived conversation then opens in the loose view only.
      const key = conversationTabKey(conversationId);
      const hasTab = Boolean(workspaceTabsStore.getSnapshot().tabs.rows[key]);
      if (hasTab && !(await closeWorkspaceTab(key))) return;
      await ipc.storage.setConversationArchived({ conversationId, archived: true });
      await loadConversations();
      workspaceTabsStore.refresh();
      if (!hasTab && currentConversationIdRef.current === conversationId) {
        startNewConversation();
      }
      return;
    }

    const wasCurrent = currentConversationIdRef.current === conversationId;
    await ipc.storage.setConversationArchived({
      conversationId,
      archived: true,
    });
    const rows = await loadConversations();
    if (!wasCurrent) return;
    const next = rows.find((conversation) => conversation.archivedAt == null);
    if (next) {
      await openConversation(next.id);
    } else {
      startNewConversation();
    }
  }

  async function handleCopyLastResponse() {
    const latest = [...messages]
      .reverse()
      .find((message) => message.role === "assistant" && !message.error);
    if (!latest?.content.trim()) {
      throw new Error(t("There is no assistant response to copy."));
    }
    if (!(await copyText(latest.content))) {
      throw new Error(t("The response could not be copied."));
    }
    appendTransientNotice(
      currentConversationIdRef.current ?? NEW_THREAD,
      t("Copied latest response"),
    );
  }

  async function forkConversationAt(
    conversationId: string,
    uptoSequence: number,
  ) {
    if (currentConversationIdRef.current !== conversationId) {
      throw new Error(t("Open the source conversation before forking it."));
    }
    if (
      conversationTurnStore
        .getSnapshot()
        .runningConversationIds.has(conversationId)
    ) {
      throw new Error(t("Wait for the current turn to finish before forking."));
    }

    const navigationGeneration = beginNavigation();
    const summary = await ipc.fork.conversation({
      conversationId,
      uptoSequence,
    });
    upsertConversationSummary(summary);
    if (!navigationIsCurrent(navigationGeneration)) {
      return;
    }

    const pendingNavigation = {
      conversationId: summary.id,
      generation: navigationGeneration,
    };
    pendingConversationNavigationRef.current = pendingNavigation;
    try {
      await openConversation(summary.id, { summary, navigationGeneration });
    } finally {
      if (pendingConversationNavigationRef.current === pendingNavigation) {
        pendingConversationNavigationRef.current = null;
      }
    }
  }

  async function handleForkMessage(message: UiMessage) {
    const uptoSequence = userMessageForkSequence(message);
    if (uptoSequence === null) {
      setChatError(t("Wait for this user turn to be persisted before forking."));
      return;
    }
    try {
      await forkConversationAt(message.conversationId, uptoSequence);
    } catch (error) {
      setChatError(errorMessage(error));
    }
  }

  async function handleForkLatestUserTurn() {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      throw new Error(t("Open a conversation before forking it."));
    }
    const uptoSequence = resolveLatestUserForkSequence(messages);
    if (uptoSequence === null) {
      throw new Error(t("No persisted user turn is available to fork."));
    }
    await forkConversationAt(conversationId, uptoSequence);
  }

  function showCommandStatus() {
    const conversation = conversationsRef.current.find(
      (item) => item.id === currentConversationIdRef.current,
    );
    const values = {
      model: selectedModelRef.current,
      effort: selectedEffortRef.current,
      fastModeStatus: fastModeStatusText(),
      mode: chatModeFor(activeAgentMode, effectiveApprovalPolicy).replace("_", " "),
      workflow: activeWorkflowMode,
      messages: conversation?.messageCount ?? messages.length,
    };
    appendTransientNotice(
      currentConversationIdRef.current ?? NEW_THREAD,
      composerProject
        ? t(
            "Model {model} · effort {effort} · {fastModeStatus} · mode {mode} · workflow {workflow} · project {project} · messages {messages}",
            { ...values, project: composerProject.name },
          )
        : t(
            "Model {model} · effort {effort} · {fastModeStatus} · mode {mode} · workflow {workflow} · project none · messages {messages}",
            values,
          ),
    );
  }

  async function handleApproveLastDenied() {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      throw new Error(t("Open a conversation before approving an action."));
    }
    const approved = await ipc.chat.approveLastDenied({
      conversationId,
    });
    if (!approved) {
      appendTransientNotice(conversationId, t("Nothing to approve"));
      return;
    }
    await sendMessage(APPROVED_RETRY_TEXT, conversationId, {
      queueIfPending: true,
    });
  }

  async function handlePersonalityCommand(
    personality: PersonalityPreference | null,
  ) {
    const preferences = personality
      ? await ipc.preferences.setPersonality({
          personality,
        })
      : await ipc.preferences.get();
    appendTransientNotice(
      currentConversationIdRef.current ?? NEW_THREAD,
      preferences.personality === "friendly"
        ? t("Personality: friendly")
        : preferences.personality === "pragmatic"
          ? t("Personality: pragmatic")
          : t("Personality: none"),
    );
  }

  async function handlePonytailCommand(mode: PonytailMode | null) {
    const preferences = mode
      ? await ipc.preferences.setPonytail({ mode })
      : await ipc.preferences.get();
    appendTransientNotice(
      currentConversationIdRef.current ?? NEW_THREAD,
      preferences.ponytail === "off"
        ? t("Ponytail: off")
        : preferences.ponytail === "lite"
          ? t("Ponytail: lite")
          : preferences.ponytail === "full"
            ? t("Ponytail: full")
            : t("Ponytail: ultra"),
    );
  }

  function openMemoriesSettings() {
    setSettingsMemoriesFocusSignal((current) => current + 1);
    setSettingsOpen(true);
  }

  function openPrivacySettings() {
    setSettingsPrivacyFocusSignal((current) => current + 1);
    setSettingsOpen(true);
  }

  // Opens Settings > AI connections. With addProvider, the sign-in of a new
  // wallet account starts there.
  function openConnectionsSettings(addProvider: WalletProvider | null) {
    setSettingsConnectionsRequest((current) => ({
      signal: (current?.signal ?? 0) + 1,
      addProvider,
    }));
    setSavingsDrawerOpen(false);
    setSettingsOpen(true);
  }

  async function handleConversationMemoriesMode(mode: "off" | "on") {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      throw new Error(
        t("Open a conversation before changing its memories setting."),
      );
    }
    const summary = await ipc.storage.setConversationMemoriesMode({ conversationId, mode });
    upsertConversationSummary(summary);
    appendTransientNotice(
      conversationId,
      mode === "on"
        ? t("Memories on for this conversation")
        : t("Memories off for this conversation"),
    );
  }

  function fastModeStatusText(auth: AuthStatus | null = status.auth) {
    const conversation = conversationsRef.current.find(
      (item) => item.id === currentConversationIdRef.current,
    );
    const enabled = conversation?.fastMode ?? draftFastMode;
    const subscriptionSupported = auth !== null && "chatgpt" in auth;
    const modelSupported = Boolean(
      models.find((model) => model.id === selectedModelRef.current)
        ?.supportsFastMode,
    );
    const effective = enabled && subscriptionSupported && modelSupported;
    const actualTier = currentConversationIdRef.current
      ? lastServiceTierByConversationRef.current.get(
          currentConversationIdRef.current,
        )
      : undefined;
    const values = {
      enabled: enabled ? t("on") : t("off"),
      auth: subscriptionSupported ? t("supported") : t("unsupported"),
      model: modelSupported ? t("supported") : t("unsupported"),
      request: effective ? t("fast") : t("standard"),
    };
    return actualTier
      ? t(
          "Fast mode {enabled} · auth {auth} · model {model} · request {request} · last response {actualTier}",
          { ...values, actualTier },
        )
      : t(
          "Fast mode {enabled} · auth {auth} · model {model} · request {request}",
          values,
        );
  }

  function showFastModeStatus() {
    appendNotice(
      currentConversationIdRef.current ?? NEW_THREAD,
      fastModeStatusText(),
    );
  }

  async function handleFastModeChange(enabled: boolean, announce = true) {
    const conversationId = currentConversationIdRef.current;
    if (conversationId) {
      const summary = await ipc.storage.setConversationFastMode({ conversationId, enabled });
      upsertConversationSummary(summary);
    } else {
      setDraftFastMode(enabled);
    }
    await persistConversationDefaults({ fastMode: enabled });
    if (announce) {
      appendNotice(
        conversationId ?? NEW_THREAD,
        enabled ? t("Fast mode on") : t("Fast mode off"),
      );
    }
  }

  async function showCommandDoctor() {
    const noticeConversationId =
      currentConversationIdRef.current ?? NEW_THREAD;
    const [
      version,
      account,
      auth,
      anthropic,
      sidecar,
      capabilities,
      extensions,
    ] = await Promise.all([
      getVersion(),
      ipc.auth.status(),
      ipc.auth.credentialStatus(),
      ipc.auth.anthropicStatus(),
      ipc.sidecar.status(),
      ipc.chat.agentCapabilities(),
      ipc.extensions.snapshot({
        projectId: composerProject?.id ?? null,
        workspaceId: selectedWorkspaceIdRef.current,
      }),
    ]);
    const sidecarHealthy =
      sidecar.state === "running"
        ? await ipc.sidecar.stats()
            .then(() => true)
            .catch(() => false)
        : false;
    const mcpStates = await Promise.all(
      extensions.mcpServers.map(async (server) => {
        if (!server.enabled) {
          return t("{serverName}: disabled", {
            serverName: server.name.replace(/\s+/g, " "),
          });
        }
        const connected = await ipc.mcp.serverTest({
          serverId: server.id,
          projectId: composerProject?.id ?? null,
          workspaceId: selectedWorkspaceIdRef.current,
        })
          .then(() => true)
          .catch(() => false);
        return connected
          ? t("{serverName}: enabled · connected", {
              serverName: server.name.replace(/\s+/g, " "),
            })
          : t("{serverName}: enabled · unreachable", {
              serverName: server.name.replace(/\s+/g, " "),
            });
      }),
    );
    const providers = [
      "chatgpt" in auth ? t("OpenAI ChatGPT") : null,
      "apiKey" in auth ? t("OpenAI API key") : null,
      anthropic.configured ? t("Anthropic API key") : null,
    ].filter((provider): provider is string => provider !== null);
    const conversation = conversationsRef.current.find(
      (item) => item.id === currentConversationIdRef.current,
    );
    const sidecarDetails =
      sidecar.state === "running"
        ? t(
            "state running · port {port} · managed {managed} · health {health} · version {version} · pinned {pinnedVersion}",
            {
              port: sidecar.port,
              managed: sidecar.managedByHalv ? t("yes") : t("no"),
              health: sidecarHealthy ? t("healthy") : t("unhealthy"),
              version: sidecar.version ?? t("unknown"),
              pinnedVersion: sidecar.pinnedVersion,
            },
          )
        : t(
            "state {state} · port {port} · managed unavailable · health unavailable · pinned unavailable",
            { state: sidecar.state, port: sidecar.port },
          );
    const report = [
      t("Halv diagnostics"),
      t("App: {version} · platform {platform}", {
        version,
        platform: capabilities.platform,
      }),
      t("Model: {model} · effort {effort}", {
        model: selectedModelRef.current,
        effort: selectedEffortRef.current,
      }),
      t("Fast: {status}", { status: fastModeStatusText(auth) }),
      capabilities.sandboxAvailable
        ? t("Approval: {approval} · sandbox available", {
            approval: effectiveApprovalPolicy,
          })
        : t("Approval: {approval} · sandbox unavailable", {
            approval: effectiveApprovalPolicy,
          }),
      t("Proxy: {details}", { details: sidecarDetails }),
      providers.length > 0
        ? account.signedIn
          ? t("Auth: Halv account signed in · providers {providers}", {
              providers: providers.join(", "),
            })
          : t("Auth: Halv account signed out · providers {providers}", {
              providers: providers.join(", "),
            })
        : account.signedIn
          ? t("Auth: Halv account signed in · providers none")
          : t("Auth: Halv account signed out · providers none"),
      t("MCP:"),
      ...(mcpStates.length > 0
        ? mcpStates.map((state) => t("  - {state}", { state }))
        : [t("  - none configured")]),
      t(
        "Context: conversation {conversation} · project {project} · workspace {workspace}",
        {
          conversation: conversation?.id ?? t("none"),
          project: conversation?.projectId ?? composerProject?.id ?? t("none"),
          workspace: selectedWorkspaceIdRef.current ?? t("primary/none"),
        },
      ),
    ].join("\n");
    appendNotice(noticeConversationId, report, { copyable: true });
    setTranscriptScrollSignal((current) => current + 1);
  }

  function showCommandUsage() {
    const stats = status.sidecarStats;
    const week = stats?.planWindows?.find((window) => window.label === "7d");
    const values = {
      today: formatMoney(todaySavings?.dollarsUsd ?? 0),
      allTime: formatMoney(savingsTotals?.dollarsUsd ?? 0),
      tokens: formatCompactTokens(savingsTotals?.tokensSaved ?? 0),
      requests: stats?.requestsTotal ?? 0,
      input: formatCompactTokens(stats?.processInputTokens ?? 0),
      output: formatCompactTokens(stats?.outputTokens ?? 0),
    };
    appendTransientNotice(
      currentConversationIdRef.current ?? NEW_THREAD,
      week
        ? t(
            "{today} saved today · {allTime} saved all time at API prices ({tokens} tokens) · {requests} proxied requests · {input} input tokens · {output} output tokens · 7d {percent}% left",
            {
              ...values,
              percent: Math.max(0, Math.round(100 - week.usedPercent)),
            },
          )
        : t(
            "{today} saved today · {allTime} saved all time at API prices ({tokens} tokens) · {requests} proxied requests · {input} input tokens · {output} output tokens · 7d unavailable",
            values,
          ),
    );
  }

  async function handleCompactConversation() {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      throw new Error(t("Open a conversation before compacting its context."));
    }
    if (
      conversationTurnStore
        .getSnapshot()
        .runningConversationIds.has(conversationId)
    ) {
      throw new Error(
        t("Wait for the current turn to finish before compacting context."),
      );
    }
    if (completedTurnCount(messages) < 2) {
      throw new Error(
        t("At least 2 completed turns are required before compacting context."),
      );
    }

    const compaction = await ipc.compact.conversation({
        conversationId,
        model: selectedModelRef.current,
        effort: selectedEffortRef.current,
      });
    setCompactionsByConversation((current) => ({
      ...current,
      [conversationId]: compaction,
    }));
    setChatError(null);
  }

  function focusSubagentActivity(missingPanelIsError = true) {
    const panel = document.querySelector<HTMLElement>(
      '[aria-label="Subagent activity"]',
    );
    if (!panel) {
      if (missingPanelIsError) {
        throw new Error(t("This conversation has no subagent activity yet."));
      }
      return;
    }
    panel.scrollIntoView({ behavior: "smooth", block: "center" });
    panel.focus({ preventScroll: true });
  }

  async function handleExportConversation(
    conversationId: string,
    format: ConversationExportFormat,
  ) {
    try {
      const exported = await ipc.export.conversationToFile({
        conversationId,
        format,
      });
      if (exported) {
        setChatError(null);
        appendTransientNotice(
          currentConversationIdRef.current ?? NEW_THREAD,
          format === "markdown" ? t("Exported markdown") : t("Exported JSON"),
        );
      }
    } catch (error) {
      setChatError(errorMessage(error));
    }
  }

  function startNewConversationForProject(
    projectId: number | null,
    options: { text?: boolean } = {},
  ) {
    setDotOpen(false);
    beginNavigation();
    textDraftRef.current = options.text === true;
    setTextDraft(textDraftRef.current);
    pendingConversationNavigationRef.current = null;
    const pendingNewTurn = conversationTurnState(null)?.pendingNewTurn;
    if (anySendPending && pendingNewTurn) {
      pendingNewTurn.freshViewRequested = true;
    }
    switchComposerDraft(null);
    startComposing(projectId);
    // The draft tab shows the composer: the user types next.
    if (featureFlagStore.getSnapshot().workspaceTabs) {
      setComposerFocusSignal((current) => current + 1);
    }
    setMessages([]);
    setDraft("");
    setDraftAttachments([]);
    setComposerMentionSeed((current) => ({
      signal: current.signal + 1,
      mentions: [],
    }));
    const defaults = conversationDefaultsRef.current;
    setSelectedModel(defaults.model);
    setSelectedEffort(defaults.effort);
    setDraftCredentialProfileId(null);
    setDraftAgentMode(agentModeForNewDraft());
    setDraftApprovalPolicy(defaults.approvalPolicy);
    setDraftWorkflowMode("default");
    setDraftFastMode(defaults.fastMode);
    setTaskFindOpen(false);
    setChatError(null);
  }

  function startNewConversation() {
    startNewConversationForProject(selectedProjectId);
  }

  // "Text conversation" (ADR 0020): the composer of the visible workspace
  // starts a chat conversation, also in Pro mode. The user picks the agent
  // and the model in the composer, as before Pro mode.
  function startTextConversation() {
    startNewConversationForProject(selectedProjectId, { text: true });
    focusVisibleTab();
  }

  function navigateRelativeTask(offset: -1 | 1) {
    if (conversations.length === 0) return;
    const currentIndex = conversations.findIndex(
      (conversation) => conversation.id === currentConversationIdRef.current,
    );
    const nextIndex =
      currentIndex === -1
        ? offset > 0
          ? 0
          : conversations.length - 1
        : currentIndex + offset;
    const target = conversations[nextIndex];
    if (!target) return;
    setTaskFindOpen(false);
    void openConversation(target.id);
  }

  async function handleSetConversationGoal(goal: string | null) {
    try {
      const creatingConversation = currentConversationIdRef.current === null;
      let summary = await ipc.storage.setConversationGoal({
          conversationId: currentConversationIdRef.current,
          goal,
          model: selectedModel,
          projectId:
            currentConversation?.projectId ??
            (currentConversationIdRef.current ? null : selectedProjectId),
          workspaceId: currentConversationIdRef.current
            ? null
            : selectedWorkspaceIdRef.current,
        });
      if (creatingConversation) {
        summary = await ipc.storage.setConversationMode({
            conversationId: summary.id,
            mode: modeForNewConversation(proMode),
          });
      }
      if (creatingConversation && draftAgentMode && summary.projectId != null) {
        summary = await ipc.chat.setAgentMode({
          conversationId: summary.id,
          enabled: true,
        });
      }
      if (creatingConversation && draftWorkflowMode === "plan") {
        summary = await ipc.chat.setWorkflowMode({
          conversationId: summary.id,
          mode: "plan",
        });
      }
      if (creatingConversation && draftApprovalPolicy !== null) {
        summary = await ipc.storage.setConversationApprovalPolicy({
            conversationId: summary.id,
            policy: draftApprovalPolicy,
          });
      }
      if (creatingConversation && draftFastMode) {
        summary = await ipc.storage.setConversationFastMode({
            conversationId: summary.id,
            enabled: true,
          });
      }
      switchComposerDraft(summary.id, creatingConversation);
      void openConversation(summary.id, { summary, load: false });
      setChatError(null);
      upsertConversationSummary(summary);
      purgeGoalKickoffs(summary.id);
      if (goal !== null) {
        appendNotice(summary.id, t("Goal set: {goal}", { goal }));
        if (
          conversationTurnStore
            .getSnapshot()
            .runningConversationIds.has(summary.id)
        ) {
          enqueueMessage(GOAL_KICKOFF_TEXT, summary.id, "goal");
        } else {
          void sendMessage(GOAL_KICKOFF_TEXT, summary.id, { origin: "goal" });
        }
      } else {
        appendNotice(summary.id, t("Goal cleared"));
      }
      await loadConversations();
    } catch (error) {
      setChatError(errorMessage(error));
    }
  }

  async function enableAgentModeForCurrentTarget(project: Project) {
    if (currentConversationIdRef.current) {
      const summary = await ipc.chat.setAgentMode({
        conversationId: currentConversationIdRef.current,
        enabled: true,
      });
      setConversations((current) => upsertConversation(current, summary));
      await loadConversations();
    } else {
      setDraftAgentMode(true);
    }
  }

  async function handleAgentModeChange(enabled: boolean): Promise<boolean> {
    setChatError(null);
    if (!enabled) {
      if (currentConversationIdRef.current) {
        try {
          const summary = await ipc.chat.setAgentMode({
            conversationId: currentConversationIdRef.current,
            enabled: false,
          });
          setConversations((current) => upsertConversation(current, summary));
          await loadConversations();
          await persistConversationDefaults({ agentMode: false });
          return true;
        } catch (error) {
          setChatError(errorMessage(error));
          return false;
        }
      }
      setDraftAgentMode(false);
      await persistConversationDefaults({ agentMode: false });
      return true;
    }

    if (!agentModeAvailable) {
      setChatError(agentModeStatus);
      return false;
    }
    if (!composerProject) {
      setChatError(t("Agent mode requires a project."));
      return false;
    }

    try {
      await enableAgentModeForCurrentTarget(composerProject);
      await persistConversationDefaults({ agentMode: true });
      return true;
    } catch (error) {
      setChatError(errorMessage(error));
      return false;
    }
  }

  async function handleWorkflowModeChange(mode: WorkflowMode): Promise<void> {
    setChatError(null);
    if (!currentConversationIdRef.current) {
      setDraftWorkflowMode(mode);
      return;
    }
    try {
      const summary = await ipc.chat.setWorkflowMode({
        conversationId: currentConversationIdRef.current,
        mode,
      });
      upsertConversationSummary(summary);
      await loadConversations();
    } catch (error) {
      setChatError(errorMessage(error));
      throw error;
    }
  }

  async function handleTrustDecision(level: "trusted" | "untrusted") {
    if (!trustPrompt) {
      return;
    }
    setTrustBusy(true);
    setProjectError(null);
    try {
      const project = await ipc.project.setTrust({
        id: trustPrompt.id,
        level,
      });
      setProjects((current) =>
        current.map((item) => (item.id === project.id ? project : item)),
      );
      setTrustPrompt(null);
      await loadProjects();
      await enableAgentModeForCurrentTarget(project);
    } catch (error) {
      setProjectError(errorMessage(error));
    } finally {
      setTrustBusy(false);
    }
  }

  async function handleToolDecision(
    callId: string,
    decision: "approved" | "approvedForSession" | "denied",
    paths?: string[],
    elicitationContent?: Record<string, McpElicitationValue>,
  ) {
    if (approvalDecisionsPendingRef.current.has(callId)) {
      return;
    }
    const pending = new Set(approvalDecisionsPendingRef.current);
    pending.add(callId);
    approvalDecisionsPendingRef.current = pending;
    setApprovalDecisionsPending(pending);
    setChatError(null);
    const submittedApproval = Object.values(pendingApprovals).find(
      (approval) => approval.callId === callId,
    );
    try {
      if (
        submittedApproval?.toolKind === "mcp_elicitation" &&
        decision !== "denied" &&
        submittedApproval.elicitationMode === "url" &&
        submittedApproval.elicitationUrl
      ) {
        await openExternalLink(submittedApproval.elicitationUrl);
      }
      const payload = submittedApproval?.toolKind === "mcp_elicitation"
        ? {
            mcpElicitation: {
              action:
                decision === "denied"
                  ? ("decline" as const)
                  : ("accept" as const),
              content:
                decision === "denied" || submittedApproval.elicitationMode === "url"
                  ? null
                  : (elicitationContent ?? {}),
            },
          }
        : paths && paths.length > 0
          ? {
              approvedPaths: {
                paths,
                remember: decision === "approvedForSession",
              },
            }
          : decision;
      await ipc.chat.toolDecision({ callId, decision: payload });
      setPendingApprovals((current) => {
        const activeApproval = Object.values(current).find(
          (approval) => approval.callId === callId,
        );
        return activeApproval === submittedApproval
          ? removePendingApprovalByCallId(current, callId)
          : current;
      });
    } catch (error) {
      setChatError(errorMessage(error));
    } finally {
      const remaining = new Set(approvalDecisionsPendingRef.current);
      remaining.delete(callId);
      approvalDecisionsPendingRef.current = remaining;
      setApprovalDecisionsPending(remaining);
    }
  }

  async function handleInterruptSubagent(agentId: string) {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      return;
    }
    try {
      await ipc.chat.interruptSubagent({ conversationId, agentId });
      await loadSubagents(conversationId);
    } catch (error) {
      setChatError(errorMessage(error));
    }
  }

  async function handleInterruptAllSubagents() {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      return;
    }
    try {
      await ipc.chat.interruptAllSubagents({ conversationId });
      await loadSubagents(conversationId);
    } catch (error) {
      setChatError(errorMessage(error));
    }
  }

  async function handleSendSubagentMessage(agentId: string, message: string) {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId) {
      throw new Error(t("Open the parent conversation first."));
    }
    await ipc.chat.sendSubagentMessage({
      conversationId,
      agentId,
      message,
    });
    await loadSubagents(conversationId);
  }

  async function handleApprovalPolicyChange(policy: ApprovalPolicy): Promise<boolean> {
    if (policy === "never") {
      const confirmed = await confirmDialog({
        message: t(
          "Never disables approval prompts and denies anything that cannot run safely in the sandbox. Continue?",
        ),
      });
      if (!confirmed) {
        return false;
      }
    }
    if (policy === "skip_all") {
      const confirmed = await confirmDialog({
        danger: true,
        message: t(
          "Full access removes Halv's sandbox. Commands can read, modify, or delete files anywhere on this machine, and no approval prompts will be shown. Only continue if you trust the model and this conversation. Enable Full access?",
        ),
      });
      if (!confirmed) {
        return false;
      }
    }
    try {
      if (currentConversationIdRef.current) {
        const summary = await ipc.storage.setConversationApprovalPolicy({
            conversationId: currentConversationIdRef.current,
            policy,
          });
        setConversations((current) => upsertConversation(current, summary));
        await loadConversations();
      } else {
        setDraftApprovalPolicy(policy);
      }
      await persistConversationDefaults({ approvalPolicy: policy });
      return true;
    } catch (error) {
      setChatError(errorMessage(error));
      return false;
    }
  }

  async function handleChatModeChange(
    mode: Exclude<ChatMode, "legacy_restricted">,
  ): Promise<boolean> {
    if (mode === "chat") {
      return handleAgentModeChange(false);
    }
    if (!composerProject) {
      setChatError(
        mode === "full_access"
          ? t("Full access mode requires a project.")
          : mode === "on_failure"
            ? t("On failure mode requires a project.")
            : mode === "ask"
              ? t("Ask mode requires a project.")
              : t("Auto mode requires a project."),
      );
      return false;
    }
    const policyChanged = await handleApprovalPolicyChange(
      approvalPolicyForMode(mode),
    );
    if (!policyChanged) {
      return false;
    }
    return handleAgentModeChange(true);
  }

  function handlePreparedTaskModeChange(mode: ConversationMode) {
    syncPreparedTaskMode(mode === "agent");
  }

  function syncPreparedTaskMode(enabled: boolean) {
    // Publishing to the external store can synchronously run the sync effect.
    // Reserve the transition first so an intermediate mode cannot reverse it.
    if (preparedTaskModeChangePendingRef.current !== null) return;
    if (enabled === activeAgentMode) {
      agentSessionProModeStore.set(enabled);
      return;
    }
    preparedTaskModeChangePendingRef.current = enabled;
    agentSessionProModeStore.set(enabled);
    const effectiveAgentMode = activeAgentMode;
    void (async () => {
      let changed = false;
      const preparedDraftConversationId =
        currentConversationId !== null &&
        (currentConversation?.draftOnly === true ||
          currentConversation === undefined)
          ? currentConversationId
          : null;
      const previousConversationMode = currentConversation
        ? modeForConversationSelection(currentConversation)
        : modeForNewConversation(effectiveAgentMode);
      const requestedConversationMode = modeForNewConversation(enabled);
      let conversationModeChanged = false;
      try {
        if (
          preparedDraftConversationId !== null &&
          previousConversationMode !== requestedConversationMode
        ) {
          await flushConversationDraft(preparedDraftConversationId);
          const summary = await ipc.storage.setConversationMode({
              conversationId: preparedDraftConversationId,
              mode: requestedConversationMode,
            });
          setConversations((current) => upsertConversation(current, summary));
          conversationModeChanged = true;
        }
        changed = await handleChatModeChange(
          enabled
            ? safeGuidedTourChatMode(effectiveApprovalPolicy)
            : "chat",
        );
      } catch (error) {
        setChatError(errorMessage(error));
      }
      if (
        !changed &&
        conversationModeChanged &&
        preparedDraftConversationId !== null
      ) {
        try {
          const summary = await ipc.storage.setConversationMode({
              conversationId: preparedDraftConversationId,
              mode: previousConversationMode,
            });
          setConversations((current) => upsertConversation(current, summary));
        } catch (error) {
          setChatError(errorMessage(error));
        }
      }
      if (changed && conversationModeChanged) {
        await loadConversations().catch((error) =>
          setChatError(errorMessage(error)),
        );
      }
      if (preparedTaskModeChangePendingRef.current !== enabled) {
        return;
      }
      preparedTaskModeChangePendingRef.current = null;
      if (
        !changed &&
        agentSessionProModeStore.getSnapshot() === enabled
      ) {
        agentSessionProModeStore.set(effectiveAgentMode);
      }
    })();
  }

  useEffect(() => {
    const targetsDraftConversation =
      currentConversationId === null ||
      currentConversation?.draftOnly === true ||
      currentConversation === undefined;
    if (
      guidedTaskPhase !== "prepared" ||
      !targetsDraftConversation ||
      proMode === activeAgentMode
    ) {
      return;
    }
    syncPreparedTaskMode(proMode);
  }, [
    activeAgentMode,
    currentConversation,
    currentConversationId,
    effectiveApprovalPolicy,
    guidedTaskPhase,
    proMode,
  ]);

  function selectWorkspace(workspace: WorkspaceStatus) {
    if (!workspace.available) {
      setWorkspaceError(t("Workspace is no longer available on disk."));
      return;
    }
    if (featureFlagStore.getSnapshot().workspaceTabs) {
      // A workspace shows its strip and its active tab. It writes no tab.
      selectedWorkspaceIdRef.current = workspace.id;
      setSelectedWorkspaceId(workspace.id);
      setWorkspaceError(null);
      showWorkspace(workspace.id);
      return;
    }
    if (
      currentConversationIdRef.current &&
      selectedWorkspaceIdRef.current !== workspace.id
    ) {
      startNewConversation();
    }
    selectedWorkspaceIdRef.current = workspace.id;
    setSelectedWorkspaceId(workspace.id);
    setWorkspaceError(null);
  }

  // A workspace row of the Projects tree. In the composer project it is the
  // switcher choice of today. Another project opens first; its workspace
  // list then loads and takes the pending choice.
  async function handleSelectTreeWorkspace(projectId: number, workspaceId: string) {
    setDotOpen(false);
    const overview = workspaceOverview.find((item) => item.id === workspaceId);
    if (overview && !overview.available) {
      setWorkspaceError(t("Workspace is no longer available on disk."));
      return;
    }
    if (featureFlagStore.getSnapshot().workspaceTabs) {
      // The row shows the strip of the workspace. Its project follows.
      showWorkspace(workspaceId);
      return;
    }
    const listed =
      composerProject?.id === projectId
        ? workspaces.find((item) => item.id === workspaceId)
        : undefined;
    if (
      listed &&
      (selectedProjectIdRef.current === projectId ||
        selectedWorkspaceIdRef.current === workspaceId)
    ) {
      selectWorkspace(listed);
      return;
    }
    pendingWorkspaceSelectionRef.current = { projectId, workspaceId };
    await handleSelectProject(projectId);
    if (selectedProjectIdRef.current !== projectId) {
      // The project did not open; a later load must not take the choice.
      pendingWorkspaceSelectionRef.current = null;
      return;
    }
    await loadWorkspaces(projectId, currentConversationIdRef.current);
  }
  selectTreeWorkspaceRef.current = handleSelectTreeWorkspace;

  function selectLocalEnvironment(configPath: string | null) {
    setSelectedEnvironmentConfigPath(configPath);
    if (composerProject) {
      writeLocalEnvironmentSelection(
        composerProject.id,
        configPath,
        window.localStorage,
      );
    }
  }

  // Asks for an approval when Rust refuses an unapproved script, then tries
  // the operation again. It returns null when the user declines.
  function withLocalEnvironmentApproval<T>(
    projectId: number,
    configPath: string | null | undefined,
    run: () => Promise<T>,
  ) {
    return runWithLocalEnvironmentApproval(configPath, run, {
      list: async () => {
        const rows = await ipc.local.environmentList({ projectId });
        if (projectId === composerProject?.id) {
          setLocalEnvironments(rows);
        }
        return rows;
      },
      confirm: confirmDialog,
      approve: (approvedPath, contentSha256) =>
        ipc.local.environmentApprove({
          projectId,
          configPath: approvedPath,
          contentSha256,
        }),
      t,
    });
  }

  function handleLocalEnvironmentsChanged(
    rows: LocalEnvironmentDescriptor[],
  ) {
    setLocalEnvironments(rows);
    if (
      selectedEnvironmentConfigPath &&
      !rows.some(
        (environment) =>
          environment.configPath === selectedEnvironmentConfigPath &&
          environment.config &&
          !environment.error,
      )
    ) {
      selectLocalEnvironment(null);
    }
  }

  async function runLocalEnvironmentAction(actionIndex: number) {
    const configPath = activeEnvironmentConfigPath;
    const contentSha256 = activeLocalEnvironment?.contentSha256;
    if (
      !composerProject ||
      environmentActionBusy ||
      !configPath ||
      !contentSha256
    ) {
      return;
    }
    const projectId = composerProject.id;
    // With shell tabs, the action runs in a new shell tab named after it.
    // The bottom panel does not show in Pro mode, so its output was hidden.
    const inShellTab = () =>
      shellTabsEnabled() && navigationRef.current.kind === "workspace";
    const showError = (message: string) =>
      inShellTab() ? setTabCloseError(message) : setWorkspaceError(message);
    setEnvironmentActionBusy(true);
    setWorkspaceError(null);
    try {
      // Rust returns the command only from approved content with this hash.
      // Both paths below type only this command, after the approval.
      const action = await withLocalEnvironmentApproval(
        projectId,
        configPath,
        () =>
          ipc.local.environmentResolveAction({
            projectId,
            configPath,
            contentSha256,
            actionIndex,
          }),
      );
      if (!action) return;
      const input = `${action.command.replace(/\s+$/, "")}\r`;
      if (inShellTab()) {
        openShellTab({ title: action.name, input });
        return;
      }
      const session = await ipc.terminal.open({
        projectId,
        workspaceId: selectedWorkspaceIdRef.current,
        cols: 100,
        rows: 24,
        executionProfile: "full_access",
      });
      setTerminalFocusSessionId(session.id);
      setTerminalOpen(true);
      await ipc.terminal.write({ sessionId: session.id, data: input });
    } catch (error) {
      showError(errorMessage(error));
      // The file can change after the menu loads. Show its current actions.
      void loadLocalEnvironments(projectId);
    } finally {
      setEnvironmentActionBusy(false);
    }
  }

  async function handleCreateWorktree() {
    if (!composerProject || workspaceBusy) {
      return;
    }
    setWorkspaceBusy(true);
    setWorkspaceError(null);
    setWorkspaceNotice(null);
    try {
      const projectId = composerProject.id;
      const environmentConfigPath = selectedEnvironmentConfigPath;
      const workspace = await withLocalEnvironmentApproval(
        projectId,
        environmentConfigPath,
        () =>
          ipc.workspace.createWorktree({
            projectId,
            baseRef: selectedWorktreeBaseRef,
            branch: null,
            environmentConfigPath,
          }),
      );
      if (!workspace) return;
      if (currentConversationIdRef.current) {
        startNewConversation();
      }
      selectedWorkspaceIdRef.current = workspace.id;
      setSelectedWorkspaceId(workspace.id);
      await loadWorkspaces(composerProject.id, null);
      selectedWorkspaceIdRef.current = workspace.id;
      setSelectedWorkspaceId(workspace.id);
    } catch (error) {
      setWorkspaceError(errorMessage(error));
    } finally {
      setWorkspaceBusy(false);
    }
  }

  async function handleRunInNewWorktree() {
    const retry = workspaceBusyRetryRef.current;
    if (!retry) {
      setChatError(t("The failed message is no longer available to retry."));
      return;
    }
    if (workspaceBusy) {
      setChatError(t("A workspace operation is already in progress."));
      return;
    }
    setWorkspaceBusy(true);
    setWorkspaceError(null);
    setWorkspaceNotice(null);
    try {
      const environmentConfigPath = selectedEnvironmentConfigPath;
      const workspace = await withLocalEnvironmentApproval(
        retry.projectId,
        environmentConfigPath,
        () =>
          ipc.workspace.createWorktree({
            projectId: retry.projectId,
            baseRef: selectedWorktreeBaseRef,
            branch: null,
            environmentConfigPath,
          }),
      );
      if (!workspace) return;
      const conversationRows = await loadConversations();
      const failedConversation = conversationRows.find(
        (conversation) => conversation.id === retry.conversationId,
      );
      if (!failedConversation) {
        throw new Error(t("The failed conversation is no longer available."));
      }

      if (failedConversation.messageCount === 0) {
        await ipc.workspace.bindConversation({
          conversationId: retry.conversationId,
          workspaceId: workspace.id,
        });
        selectedWorkspaceIdRef.current = workspace.id;
        setSelectedWorkspaceId(workspace.id);
        await loadWorkspaces(retry.projectId, retry.conversationId);
        selectedWorkspaceIdRef.current = workspace.id;
        setSelectedWorkspaceId(workspace.id);
        await openConversation(retry.conversationId);
        workspaceBusyRetryRef.current = null;
        setChatError(null);
        await sendMessage(retry.text, retry.conversationId, retry.options);
        return;
      }

      startNewConversation();
      selectedWorkspaceIdRef.current = workspace.id;
      setSelectedWorkspaceId(workspace.id);
      await loadWorkspaces(retry.projectId, null);
      selectedWorkspaceIdRef.current = workspace.id;
      setSelectedWorkspaceId(workspace.id);
      workspaceBusyRetryRef.current = null;
      await sendMessage(retry.text, null, {
        ...retry.options,
        pinnedWorkspaceId: workspace.id,
      });
    } catch (error) {
      setChatError(errorMessage(error));
    } finally {
      setWorkspaceBusy(false);
    }
  }

  async function handleWorkspaceHandoff(target: "local" | "worktree") {
    const conversationId = currentConversationIdRef.current;
    if (!conversationId || !composerProject || workspaceBusy) {
      return;
    }
    // With workspace tabs, the strip on screen loses this tab during the
    // handoff, so keep the id from the start.
    const handedOffId = conversationId;
    setWorkspaceBusy(true);
    setWorkspaceError(null);
    setWorkspaceNotice(null);
    try {
      const environmentConfigPath = selectedEnvironmentConfigPath;
      // A handoff runs setup only when it creates the worktree.
      const result = await withLocalEnvironmentApproval(
        composerProject.id,
        environmentConfigPath,
        () =>
          ipc.workspace.handoff({
            conversationId,
            target,
            environmentConfigPath,
          }),
      );
      if (!result) return;
      selectedWorkspaceIdRef.current = result.workspace.id;
      setSelectedWorkspaceId(result.workspace.id);
      const handedOff = conversationsRef.current.find((item) => item.id === handedOffId);
      if (featureFlagStore.getSnapshot().workspaceTabs && handedOff) {
        // ADR 0010: the home of a tab is derived from the conversation. The
        // tab moves with the new binding, goes to the end of its new strip,
        // and the strip shows. The workspace list learns a new worktree
        // first, so the tab never shows in a wrong strip.
        await refreshWorkspaceOverview();
        const summary = {
          ...handedOff,
          workspaceId: result.workspace.id,
          projectRootPath: result.workspace.rootPath,
        };
        upsertConversationSummary(summary);
        await openConversation(handedOffId, { summary, load: false });
        workspaceTabsStore.move(conversationTabKey(handedOffId), Number.MAX_SAFE_INTEGER);
      }
      await Promise.all([
        loadConversations(),
        loadWorkspaces(composerProject.id, handedOffId),
      ]);
      setWorkspaceNotice(
        result.warning ??
          (target === "local"
            ? t("Handed off to Local.")
            : t("Handed off to Worktree.")),
      );
    } catch (error) {
      setWorkspaceError(errorMessage(error));
    } finally {
      setWorkspaceBusy(false);
    }
  }

  async function handleCreateWorkspaceBranch() {
    if (!selectedWorkspace || selectedWorkspace.kind !== "worktree" || workspaceBusy) {
      return;
    }
    const title = currentConversation?.title ?? "task";
    const slug = title
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48) || "task";
    const branch = (
      await promptDialog({
        defaultValue: `halv/${slug}`,
        title: t("Create branch in this worktree"),
      })
    )?.trim();
    if (!branch) return;
    setWorkspaceBusy(true);
    setWorkspaceError(null);
    setWorkspaceNotice(null);
    try {
      const workspace = await ipc.workspace.createBranch({
        workspaceId: selectedWorkspace.id,
        branch,
      });
      setWorkspaces((current) =>
        current.map((item) => (item.id === workspace.id ? workspace : item)),
      );
      setWorkspaceNotice(t("Created branch {branch}.", { branch }));
      await loadBranchStatus(composerProject?.id ?? null, workspace.id);
    } catch (error) {
      setWorkspaceError(errorMessage(error));
    } finally {
      setWorkspaceBusy(false);
    }
  }

  async function handlePinWorkspace(
    workspace: WorkspaceStatus,
    pinned: boolean,
  ) {
    setWorkspaceBusy(true);
    setWorkspaceError(null);
    setWorkspaceNotice(null);
    try {
      await ipc.workspace.setPinned({
        workspaceId: workspace.id,
        pinned,
      });
      await loadWorkspaces(composerProject?.id ?? null, currentConversationIdRef.current);
    } catch (error) {
      setWorkspaceError(errorMessage(error));
    } finally {
      setWorkspaceBusy(false);
    }
  }

  async function handleRemoveWorkspace(workspace: WorkspaceStatus) {
    if (
      !(await confirmDialog({
        danger: true,
        message: t("Remove managed worktree {workspace}?", {
          workspace: workspace.currentBranch ?? workspace.name,
        }),
      }))
    ) {
      return;
    }
    setWorkspaceBusy(true);
    setWorkspaceError(null);
    setWorkspaceNotice(null);
    try {
      if (!(await removeWorkspaceNow(workspace))) return;
    } catch (error) {
      setWorkspaceError(errorMessage(error));
    } finally {
      setWorkspaceBusy(false);
    }
  }

  // Removes a managed worktree after the user confirmed. Rust refuses a
  // pinned or dirty worktree and a worktree that a conversation uses.
  // Removal runs the cleanup script of the environment that created the
  // worktree, so Rust can ask for the environment approval first. The
  // function returns false when the user declines.
  async function removeWorkspaceNow(workspace: {
    id: string;
    projectId: number;
    localEnvironmentConfigPath?: string | null;
  }) {
    const removed = await withLocalEnvironmentApproval(
      workspace.projectId,
      workspace.localEnvironmentConfigPath,
      async () => {
        await ipc.workspace.remove({ workspaceId: workspace.id });
        return true;
      },
    );
    if (!removed) return false;
    await loadWorkspaces(composerProject?.id ?? null, currentConversationIdRef.current);
    // The tree lists the workspaces of every project.
    await refreshWorkspaceOverview();
    if (featureFlagStore.getSnapshot().workspaceTabs) {
      // The foreign keys removed the tabs of the worktree.
      await loadConversations();
      workspaceTabsStore.refresh();
      const nav = navigationRef.current;
      if (nav.kind === "workspace" && nav.workspaceId === workspace.id) {
        showWorkspace(`primary-${workspace.projectId}`);
      }
    }
    return true;
  }

  // The row menus of the Projects tree (ADR 0010, stage S7).

  // The tabs of these workspaces close with no prompt, because the user
  // confirmed a removal. Each close stops its processes first (T9).
  async function closeTabsOfWorkspaces(workspaceIds: ReadonlySet<string>) {
    const snapshot = workspaceTabsStore.getSnapshot();
    const keys = Object.values(snapshot.tabs.rows)
      .filter((row) => {
        const home = homeWorkspaceId(row, snapshot.facts);
        return home !== null && workspaceIds.has(home);
      })
      .map((row) => row.key);
    for (const key of keys) {
      if (!(await closeWorkspaceTab(key, { prompt: false }))) {
        throw new Error(t("A tab of this workspace could not close."));
      }
    }
  }

  // Orca's removal from the tree: the tabs and the shells of the worktree
  // stop, then the worktree goes. The row shows "Removing…", and a
  // failure stays under the workspace.
  async function removeWorktreeFromTree(workspaceId: string) {
    const workspace = workspaceOverviewRef.current?.find((item) => item.id === workspaceId);
    if (!workspace || !treeOperationsStore.startRemoval(workspaceId)) return;
    try {
      if (featureFlagStore.getSnapshot().workspaceTabs) {
        await closeTabsOfWorkspaces(new Set([workspaceId]));
      }
      // A shell in the folder would keep a deleted folder open.
      await stopOwnedSessions(
        (terminal) => !terminal.conversationId && pathIsInside(terminal.cwd, workspace.rootPath),
      );
      // The tree row has no environment path. The full workspace has it.
      const status = (await ipc.workspace.list({ projectId: workspace.projectId })).find(
        (item) => item.id === workspaceId,
      );
      await removeWorkspaceNow({
        ...workspace,
        localEnvironmentConfigPath: status?.localEnvironmentConfigPath ?? null,
      });
      treeOperationsStore.finishRemoval(workspaceId);
    } catch (error) {
      treeOperationsStore.failRemoval(workspaceId, errorMessage(error));
    }
  }

  // Orca's "Remove Project": the tabs and the terminals of the project
  // stop, then Halv forgets the project. The folder stays on disk.
  async function removeProjectFromTree(projectId: number) {
    const workspaces = (workspaceOverviewRef.current ?? []).filter(
      (item) => item.projectId === projectId,
    );
    if (workspaces.some((item) => item.managed)) {
      throw new Error(t("Remove the worktrees of this project first."));
    }
    const workspaceIds = new Set([
      ...workspaces.map((item) => item.id),
      `primary-${projectId}`,
    ]);
    if (featureFlagStore.getSnapshot().workspaceTabs) await closeTabsOfWorkspaces(workspaceIds);
    await stopOwnedSessions((terminal) => terminal.projectId === projectId);
    await removeProjectNow(projectId);
    await refreshWorkspaceOverview();
    const nav = navigationRef.current;
    if (nav.kind === "workspace" && workspaceIds.has(nav.workspaceId)) {
      navigate(looseNavigation(null));
      startNewConversationForProject(null);
    }
  }

  // "New worktree…" of the tree: the project shows a pending row until Git
  // made the worktree, then the strip of the new worktree shows (Orca).
  async function createWorktreeFromTree(
    projectId: number,
    request: { branch: string | null; baseRef: string },
  ) {
    const creation = treeOperationsStore.startCreation(projectId, request.branch);
    try {
      const workspace = await ipc.workspace.createWorktree({
        projectId,
        baseRef: request.baseRef,
        branch: request.branch,
        environmentConfigPath:
          composerProject?.id === projectId ? selectedEnvironmentConfigPath : null,
      });
      await refreshWorkspaceOverview();
      treeOperationsStore.finishCreation(creation);
      await selectTreeWorkspaceRef.current(projectId, workspace.id);
    } catch (error) {
      treeOperationsStore.failCreation(creation, errorMessage(error));
    }
  }

  // The running terminals that a removal stops.
  function openTerminalCount(scope: { workspaceId: string } | { projectId: number }) {
    const running = terminalSessionsRef.current.filter((terminal) => terminal.status === "running");
    if ("projectId" in scope) {
      return running.filter((terminal) => terminal.projectId === scope.projectId).length;
    }
    const workspace = workspaceOverviewRef.current?.find((item) => item.id === scope.workspaceId);
    if (!workspace) return 0;
    return running.filter(
      (terminal) => !terminal.conversationId && pathIsInside(terminal.cwd, workspace.rootPath),
    ).length;
  }

  function setConversationUnread(conversationId: string, unread: boolean) {
    setUnviewedConversationIds((current) =>
      unread ? new Set(current).add(conversationId) : clearConversationUnviewed(current, conversationId),
    );
    if (unread) return;
    for (const terminal of terminalSessionsRef.current) {
      if (terminal.conversationId === conversationId) terminalActivityStore.markViewed(terminal.id);
    }
  }

  // "New terminal here": a shell tab with shell tabs, else a terminal of
  // the workspace in the bottom panel.
  async function newTerminalInWorkspace(projectId: number, workspaceId: string) {
    if (shellTabsEnabled()) {
      showWorkspace(workspaceId);
      if (!openShellTab()) throw new Error(t("The terminal tab could not open."));
      return;
    }
    const session = await ipc.terminal.open({
      projectId,
      workspaceId,
      cols: 100,
      rows: 24,
      executionProfile: "full_access",
    });
    setTerminalFocusSessionId(session.id);
    setTerminalOpen(true);
    void refreshTerminalSessions().catch(() => undefined);
  }

  async function setWorkspacePinnedFromTree(workspaceId: string, pinned: boolean) {
    await ipc.workspace.setPinned({ workspaceId, pinned });
    await Promise.all([
      refreshWorkspaceOverview(),
      loadWorkspaces(composerProject?.id ?? null, currentConversationIdRef.current),
    ]);
  }

  async function handleCreateDraftPr() {
    if (!composerProject || prBusy) {
      return;
    }
    setPrBusy(true);
    setPrMessage(null);
    try {
      const message = await ipc.project.createDraftPr({
        id: composerProject.id,
        workspaceId: selectedWorkspaceIdRef.current,
      });
      setPrMessage(message);
      await loadBranchStatus(composerProject.id, selectedWorkspaceIdRef.current);
    } catch (error) {
      setPrMessage(errorMessage(error));
    } finally {
      setPrBusy(false);
    }
  }

  async function runDueAutomation(automation: Automation) {
    let runWorkspaceId = automation.workspaceId ?? null;
    try {
      if (
        automation.target === "local" &&
        automation.workspaceStrategy === "new_worktree"
      ) {
        const workspace = await ipc.workspace.createWorktree({
            projectId: automation.projectId,
            baseRef: automation.baseRef ?? "HEAD",
            branch: null,
            environmentConfigPath: null,
          });
        runWorkspaceId = workspace.id;
      }
      const response = await ipc.chat.send({
        conversationId:
          automation.conversationMode === "existing"
            ? automation.conversationId ?? null
            : null,
        text: automation.prompt,
        model: automation.model,
        effort: automation.effort,
        projectId: automation.projectId,
        workspaceId: runWorkspaceId,
        agentMode: true,
        workflowMode: "default",
        userInitiated: false,
        mentions: undefined,
      });
      await ipc.automation.reportRun({
        automationId: automation.id,
        status: "completed",
        error: null,
        conversationId: response.conversationId,
        workspaceId: runWorkspaceId,
      });
      await loadConversations();
      await refreshAutomationUnreadCount();
      void notify(
        t("Automation finished: {name}", { name: automation.name }),
        t("Conversation {conversationId} is ready.", {
          conversationId: response.conversationId.slice(0, 8),
        }),
      );
    } catch (cause) {
      const message = errorMessage(cause);
      await ipc.automation.reportRun({
        automationId: automation.id,
        status: "failed",
        error: message,
        conversationId: null,
        workspaceId: runWorkspaceId,
      }).catch(() => undefined);
      await refreshAutomationUnreadCount();
      void notify(t("Automation failed: {name}", { name: automation.name }), message);
    }
  }

  async function handleOpenAutomationConversation(conversationId: string) {
    setAutomationsOpen(false);
    await loadConversations();
    await openConversation(conversationId);
  }

  function commandContext(
    args: string,
  ): CommandHandlerContext & {
    setLanguagePreference: typeof setLanguagePreferenceState;
  } {
    return {
      args,
      models: composerVisibleModels,
      selectedModel,
      selectedEffort,
      projects,
      currentProject: composerProject,
      currentConversationId,
      currentConversationStreaming:
        currentConversationId !== null &&
        runningConversationIds.has(currentConversationId),
      latestUserForkSequence: resolveLatestUserForkSequence(messages),
      completedTurns: completedTurnCount(messages),
      chatMode: chatModeFor(activeAgentMode, effectiveApprovalPolicy),
      workflowMode: activeWorkflowMode,
      setDraft,
      setModel: handleModelChange,
      setEffort: handleEffortChange,
      setFastMode: (enabled) => handleFastModeChange(enabled),
      setThemePreference: setThemePreferenceState,
      setLanguagePreference: setLanguagePreferenceState,
      setGoal: handleSetConversationGoal,
      setChatMode: async (mode) => {
        await handleChatModeChange(mode);
      },
      setWorkflowMode: handleWorkflowModeChange,
      startNewConversation,
      openProjectSwitcher: () => {
        setSidebarCollapsed(false);
        setProjectSwitcherOpen(true);
      },
      switchProjectByFragment: handleSwitchProjectByFragment,
      openSettings: () => setSettingsOpen(true),
      openKeyboardShortcuts: () => setKeyboardShortcutsOpen(true),
      openExtensions: () => setExtensionsOpen(true),
      openAutomations: () => setAutomationsOpen(true),
      toggleTerminal: () => setTerminalOpen((current) => !current),
      openShell: () => {
        // A visible shell tab opens the shell in its own panel.
        const visible = shellTabVisible ? visibleTabKey : null;
        if (visible) {
          terminalPanelRegistry.request(tabPanelScope(visible))?.openShell();
          return;
        }
        // The chat surface keeps its shells in the bottom panel.
        if (!showProTerminalSurface) setTerminalOpen(true);
        requestTerminalShell(currentConversationId);
      },
      openReview: () => openDiffRail(true),
      openBrowser: () => { setDotOpen(false); setWorkspaceView("browser"); },
      startDictation: () => setDictationSignal((current) => current + 1),
      restartProxy: handleRestartSidecar,
      createDraftPr: handleCreateDraftPr,
      exportConversation: async (format) => {
        const conversationId = currentConversationIdRef.current;
        if (!conversationId) {
          throw new Error(t("Open a conversation before exporting it."));
        }
        await handleExportConversation(conversationId, format);
      },
      renameConversation: handleRenameConversation,
      deleteConversation: handleDeleteCurrentConversation,
      archiveConversation: handleArchiveCurrentConversation,
      compactConversation: handleCompactConversation,
      forkConversation: handleForkLatestUserTurn,
      copyLastResponse: handleCopyLastResponse,
      approveLastDenied: handleApproveLastDenied,
      setPersonality: handlePersonalityCommand,
      setPonytail: handlePonytailCommand,
      openMemories: openMemoriesSettings,
      setConversationMemoriesMode: handleConversationMemoriesMode,
      showStatus: showCommandStatus,
      showFastMode: showFastModeStatus,
      showDoctor: showCommandDoctor,
      showUsage: showCommandUsage,
      focusSubagents: focusSubagentActivity,
      insertMention: (path) => {
        const mention = path ? `@${path} ` : "@";
        setDraft((current) => `${current}${mention}`);
        setComposerFocusSignal((current) => current + 1);
      },
      sendPrompt: async (prompt) => {
        await sendMessage(prompt, currentConversationIdRef.current, {
          queueIfPending: true,
        });
      },
      focusConversationSearch: () => {
        setSidebarCollapsed(false);
        setSearchFocusSignal((current) => current + 1);
      },
      openCommandPalette: () => setCommandPaletteOpen(true),
      logout: handleLogout,
      quit: async () => {
        await exit(0);
      },
    };
  }

  async function handleRunCommand(
    command: CommandRegistryEntry,
    args: string,
  ) {
    const context = commandContext(args);
    const unavailable = command.unavailableReason?.(context);
    if (unavailable) {
      throw new Error(commandUnavailableReasonLabel(unavailable, t));
    }
    try {
      await command.handler(context);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(commandErrorLabel(message, t));
    }
  }

  function commandUnavailableReason(command: CommandRegistryEntry) {
    const reason = command.unavailableReason?.(commandContext("")) ?? null;
    return reason ? commandUnavailableReasonLabel(reason, t) : null;
  }

  function prefillCommand(command: CommandRegistryEntry) {
    setDraft(`/${command.name} `);
    setComposerFocusSignal((current) => current + 1);
  }

  async function handleRevertTurnDiff(turnId: string) {
    if (
      !currentConversationId ||
      conversationTurnStore
        .getSnapshot()
        .runningConversationIds.has(currentConversationId) ||
      conversationTurnState(currentConversationId)?.cancelAndRevertPending
    ) {
      return;
    }
    try {
      await ipc.chat.revertTurn({ conversationId: currentConversationId, turnId });
      setTurnDiffsByConversation((current) => {
        const existing = current[currentConversationId] ?? [];
        return {
          ...current,
          [currentConversationId]: existing.filter((turnDiff) => turnDiff.turnId !== turnId),
        };
      });
      setChatError(null);
    } catch (error) {
      setChatError(errorMessage(error));
    }
  }

  const emptyState = useMemo(
    () =>
      emptyStateForStatus(
        status,
        selectedModelInfo?.agentBackend
          ? "session"
          : selectedModelInfo?.provider === "anthropic"
            ? "anthropic"
            : "openai",
        t,
      ),
    [selectedModelInfo?.agentBackend, selectedModelInfo?.provider, status, t],
  );
  const thisTurnSavingsPercent = useMemo(
    () => lastTurnSavingsPercent(messages),
    [messages],
  );
  const reasoningText = currentConversationId
    ? (reasoningByConversation[currentConversationId] ?? "")
    : "";

  const anySendPending = runningConversationIds.size > 0;
  const selectedRunningTurnKey = runningTurnKeyForView(currentConversationId);
  const selectedTurnKey =
    selectedRunningTurnKey ?? conversationTurnKey(currentConversationId);
  const selectedTurnState =
    conversationTurnStates.get(selectedTurnKey);
  const selectedSendPending = selectedRunningTurnKey !== null;
  const selectedIsForegroundTurn = selectedRunningTurnKey !== null;
  const cancelAndRevertPending =
    selectedTurnState?.cancelAndRevertPending ?? false;
  const stopAndRevertAvailable =
    selectedTurnState?.stopAndRevertAvailable ?? false;
  const normalCancelRequested =
    selectedTurnState?.normalCancelRequested ?? false;
  const turnStartedAt = selectedTurnState?.startedAt ?? null;
  const streaming =
    selectedSendPending || messages.some((message) => message.streaming);
  const hasAnyConversationMessages = conversations.some(
    (conversation) => conversation.messageCount > 0,
  );
  const firstAssistantReplyLanded = messages.some(
    (message) => message.role === "assistant" && !message.streaming,
  );
  const successfulAssistantReplyLanded = messages.some(
    (message) =>
      message.role === "assistant" &&
      !message.streaming &&
      !message.error &&
      message.content.trim().length > 0,
  );
  const appLooksFresh = resolveAppLooksFresh({
    initialDataLoaded,
    hasConversationMessages: hasAnyConversationMessages,
    hasAssistantReply: firstAssistantReplyLanded,
    replayOnboarding: ONBOARDING_REPLAY_ENABLED,
  });
  const showFirstRunCard = shouldShowFirstRunCard({
    preference: onboardingPreference,
    appIsFresh: appLooksFresh,
    phase: guidedTaskPhase,
    currentConversationId,
    messageCount: messages.length,
    noticeCount: notices.length,
    hasPendingApproval: pendingApproval !== null,
  });
  const composerConnections = useMemo(
    () =>
      connectGateConnections(connectGateLaneOptions, credentialProfiles),
    [connectGateLaneOptions, credentialProfiles],
  );
  const directApiModelsAvailable =
    composerConnections.openai || composerConnections.anthropic ||
    Boolean(status.auth && (
      "chatgpt" in status.auth || "apiKey" in status.auth || "anthropicApiKey" in status.auth
    ));
  const composerVisibleModels = useMemo(
    () =>
      visibleComposerModels(models, {
        proMode,
        agentBackend: currentConversation?.agentBackend ?? null,
        directApiModelsAvailable,
      }),
    [
      currentConversation?.agentBackend,
      directApiModelsAvailable,
      models,
      proMode,
    ],
  );
  const anonymousUsagePreference = useAnonymousUsagePreference();
  const activeFirstRunVariant = firstRunVariant(
    status.auth,
    composerConnections,
  );
  const sidebarPlanProviderConnections = planProviderConnections(
    composerConnections,
    status.auth,
    status.anthropic,
  );
  useEffect(
    function recordFunnelEventsForStateChange() {
      const nextState: AnalyticsState = {
        // ChatGPT OAuth and pasted API keys count as a connected account too.
        accountConnected:
          Object.values(composerConnections).some(Boolean) ||
          hasProviderCredentials(status.auth, status.anthropic),
        tokensSaved: todaySavings?.tokensSaved ?? 0,
        trialConfirmed: status.account?.entitlements?.trialConfirmed === true,
        paymentConfirmed: status.account?.entitlements?.paymentConfirmed === true,
        signedIn: status.account?.signedIn ?? false,
      };
      // `first_open` waits for consent "on" (ADR 0021). The native queue
      // drops events while the answer is unset, so the first state after
      // a yes records `first_open` again.
      const { events, state } = analyticsEventsWithConsent(
        anonymousUsagePreference.consent,
        analyticsPreviousStateRef.current,
        nextState,
      );
      analyticsPreviousStateRef.current = state;
      for (const event of events) {
        void recordAnalyticsEvent(event);
      }
    },
    [
      anonymousUsagePreference.consent,
      composerConnections,
      status.account?.entitlements?.trialConfirmed,
      status.account?.entitlements?.paymentConfirmed,
      status.account?.signedIn,
      status.anthropic,
      status.auth,
      todaySavings?.tokensSaved,
    ],
  );
  const guidedTaskPayoff = onboardingPayoffCopy(
    guidedTaskRunMode === "terminal" ? null : thisTurnSavingsPercent,
    todaySavings?.tokensSaved ?? 0,
    todaySavings?.dollarsUsd ?? 0,
    formatMoney,
  );
  // A completed chat turn in any conversation is a first result (ADR 0018).
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void ipc.events
      .listen<ChatDeltaPayload>("chat_delta", (payload) => {
        if (disposed || payload.kind !== "done") return;
        markFirstResult();
      })
      .then((cleanup) => {
        if (disposed) cleanup();
        else unlisten = cleanup;
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  const anonymousValueDelivered = valueDelivered({
    firstResultSeen,
    onboardingCompleted: onboardingPreference === "completed",
    tokensSaved: savingsTotals?.tokensSaved ?? 0,
  });
  const firstResultCelebration =
    (todaySavings?.tokensSaved ?? 0) > 0
      ? {
          amount:
            (todaySavings?.dollarsUsd ?? 0) > 0
              ? formatMoney(todaySavings?.dollarsUsd ?? 0)
              : null,
          recent: (todaySavings?.recentSavings ?? []).filter(
            (value) => value > 0,
          ),
        }
      : null;
  const trialEndSeconds =
    status.account?.signedIn &&
    status.account.entitlements?.paymentConfirmed !== true
      ? (status.account.entitlements?.trialEnd ?? null)
      : null;
  const trialGoal = useMemo(
    () =>
      trialEndSeconds === null
        ? null
        : trialGoalProgress({
            trialEndMs: trialEndSeconds * 1000,
            nowMs: gateNowMs,
            activeDayKeys: dailyTokenActivity
              .filter((day) => day.inputTokens > 0 || day.outputTokens > 0)
              .map((day) => day.date),
          }),
    [dailyTokenActivity, gateNowMs, trialEndSeconds],
  );
  // Halv Cloud access of this install (ADR 0021). The onboarding replay
  // behaves like a signed-out install.
  const halvCloudAccess = cloudAccess(
    ONBOARDING_REPLAY_ENABLED ? null : status.account,
  );
  // The status bar cards read the account outside the props tree.
  useEffect(() => {
    accountStatusStore.publish(
      ONBOARDING_REPLAY_ENABLED ? null : status.account,
    );
  }, [status.account]);
  // Missing or old entitlements never block local work (ADR 0021). Halv
  // refreshes them in the background until the sync succeeds.
  const accountNeedsSync = status.account
    ? gateDecision(status.account, gateNowMs) === "verify"
    : false;
  useEffect(() => {
    if (!accountNeedsSync) return;
    let cancelled = false;
    const refresh = () => {
      void ipc.auth
        .refreshEntitlements()
        .then((account) => {
          if (!cancelled) setStatus((current) => ({ ...current, account }));
        })
        .catch(() => undefined);
    };
    refresh();
    const interval = window.setInterval(refresh, 30_000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [accountNeedsSync]);
  function dismissCloudPitch() {
    writeCloudPitchDismissed();
    setCloudPitchDismissed(true);
  }
  const guidedTerminalConversationId = guidedTerminalConversationIdRef.current;
  const guidedTerminalStatus = guidedTerminalConversationId
    ? conversationTerminalStatus(
        sidebarTerminals,
        guidedTerminalConversationId,
      )
    : null;
  const guidedTaskVisible = currentConversationId === (
    guidedTaskRunMode === "terminal"
      ? guidedTerminalConversationId ?? guidedChatConversationIdRef.current
      : guidedChatConversationIdRef.current
  );
  const guidedTaskFailed = guidedTaskPhase === "waiting" && !streaming && Boolean(
    guidedTaskCancelled || chatError || guidedTerminalStatus === "failed" ||
    messages.some((message) => message.role === "assistant" && message.error)
  );
  const guidedTaskStatusVisible =
    guidedTaskVisible &&
    (guidedTaskPhase === "waiting" || guidedTaskPhase === "success");
  const welcomeScreenShown = shouldShowWelcomeScreen({
    welcomeSeen,
    connectionRequired: connectGateState === "required",
    replayOnboarding: ONBOARDING_REPLAY_ENABLED,
  });
  // An install without a consent answer sees one banner (ADR 0021). The
  // welcome screen and the first-task cards ask in their own place.
  const showConsentBanner = shouldShowConsentBanner({
    consent: anonymousUsagePreference.consent,
    welcomeScreenShown,
    firstTaskShown: showFirstRunCard || guidedTaskPhase === "prepared",
    shownOnEarlierLaunch: consentBannerShownOnEarlierLaunch,
  });
  // The pitch waits until the first-task card and the consent banner close,
  // so the chat column shows one card at a time. The consent banner shows
  // on one launch only, so the pitch can show on a later launch.
  const showCloudPitch =
    workspaceView === "chat" &&
    !guidedTaskStatusVisible &&
    !showConsentBanner &&
    cloudPitchVisible({
      valueDelivered: anonymousValueDelivered,
      access: halvCloudAccess,
      dismissed: cloudPitchDismissed,
      shownOnEarlierLaunch: cloudPitchShownOnEarlierLaunch,
    });

  useEffect(() => {
    if ((guidedTaskPhase === "prepared" || guidedTaskPhase === "waiting") && status.sidecar?.state === "error") {
      void recordAnalyticsEvent("setup_failed", "engine");
    }
  }, [guidedTaskPhase, status.sidecar?.state]);

  useEffect(() => {
    if (!showCloudPitch) return;
    writeCloudPitchShown();
    void recordAnalyticsEvent("trial_prompted");
  }, [showCloudPitch]);

  useEffect(() => {
    if (showConsentBanner) writeConsentBannerShown();
  }, [showConsentBanner]);

  useEffect(() => {
    if (welcomeScreenShown) setWelcomeShownThisLaunch(true);
    if (welcomeScreenShown || showFirstRunCard) writeCloudInfoSeen();
  }, [welcomeScreenShown, showFirstRunCard]);

  useEffect(() => {
    if (guidedTaskFailed) void recordAnalyticsEvent("first_task_failed", guidedTaskCancelled ? "cancelled" : "task");
  }, [guidedTaskFailed, guidedTaskCancelled]);

  useEffect(() => {
    if (
      composerVisibleModels.some(
        (model) =>
          model.id === selectedModelRef.current ||
          (model.agentBackend === "codex_session" &&
            model.agentSessionModel === selectedModelRef.current),
      )
    ) {
      return;
    }
    const currentDefaults = conversationDefaultsRef.current;
    const fallback = firstRunComposerModel(
      composerVisibleModels,
      hasExplicitDefaultModelRef.current ? currentDefaults.model : null,
      composerConnections,
      status.auth,
    );
    if (!fallback) return;
    const effort = fallback.reasoningEfforts.includes(currentDefaults.effort)
      ? currentDefaults.effort
      : fallback.defaultEffort;
    conversationDefaultsRef.current = {
      ...currentDefaults,
      model: fallback.id,
      effort,
    };
    setSelectedModel(fallback.id);
    setSelectedEffort(effort);
  }, [composerConnections, composerVisibleModels, status.auth]);

  useEffect(() => {
    if (
      guidedTaskRunMode === "terminal" ||
      guidedTaskPhase !== "waiting" ||
      guidedTaskCancelled || streaming ||
      !successfulAssistantReplyLanded ||
      currentConversationId !== guidedChatConversationIdRef.current
    ) {
      return;
    }
    setGuidedTaskPhase("success");
    void recordAnalyticsEvent("first_task_completed");
    setOnboardingPreference("completed");
    writeOnboardingPreference("completed");
  }, [currentConversationId, guidedTaskPhase, guidedTaskRunMode, guidedTaskCancelled, streaming, successfulAssistantReplyLanded]);

  useEffect(() => {
    if (guidedTerminalStatus === "running") {
      guidedTerminalSawActiveRef.current = true;
      return;
    }
    if (guidedTerminalStatus === "starting") return;
    if (
      guidedTaskRunMode !== "terminal" ||
      guidedTaskCancelled ||
      !guidedTerminalTurnCompleted(
        guidedTaskPhase,
        guidedTerminalSawActiveRef.current,
        guidedTerminalSawSustainedOutputRef.current,
        guidedTerminalStatus,
      )
    ) {
      return;
    }
    guidedTerminalTrackingArmedRef.current = false;
    setGuidedTaskPhase("success");
    void recordAnalyticsEvent("first_task_completed");
    setOnboardingPreference("completed");
    writeOnboardingPreference("completed");
  }, [guidedTaskPhase, guidedTaskRunMode, guidedTaskCancelled, guidedTerminalStatus]);

  useEffect(() => {
    setConnectGateState((current) =>
      nextConnectGateState(
        current,
        connectGateAccountsResolved &&
          credentialProfilesLoaded &&
          status.auth !== null &&
          status.anthropic !== null,
        connectGateLaneOptions,
        credentialProfiles,
        hasProviderCredentials(status.auth, status.anthropic),
        connectGateDismissed,
      ),
    );
  }, [
    connectGateAccountsResolved,
    connectGateDismissed,
    connectGateLaneOptions,
    credentialProfiles,
    credentialProfilesLoaded,
    status.anthropic,
    status.auth,
  ]);

  const accountStatus = status.account;

  if (!accountStatus) {
    return (
      <main className="flex min-h-screen items-center justify-center bg-surface-base px-4 text-content-primary">
        <section className="rounded-lg border border-surface-border/30 bg-surface-panel px-5 py-4 shadow-xl shadow-[rgb(var(--shadow-color)/0.18)]">
          <p className="font-mono text-xs uppercase tracking-[0.22em] text-accent">
            {t("Halv")}
          </p>
          <p className="mt-2 text-sm text-content-secondary">
            {t("Checking account...")}
          </p>
          {status.error ? (
            <p className="mt-2 text-sm text-danger">{status.error}</p>
          ) : null}
        </section>
      </main>
    );
  }

  if (accountFlowOpen === "account") {
    return (
      <AccountGate
        authStatus={accountStatus}
        onBack={() => setAccountFlowOpen(null)}
        onStatusChange={handleAccountStatusChange}
      />
    );
  }

  // The plans page opens only on a click on "Plans". It always has a way
  // back (ADR 0021). No account state opens it alone.
  if (accountFlowOpen === "paywall") {
    return (
      <Paywall
        authStatus={accountStatus}
        onBack={() => setAccountFlowOpen(null)}
        onCheckoutOpened={handleTrialCheckoutOpened}
        onStatusChange={handleAccountStatusChange}
      />
    );
  }

  if (connectGateState === "resolving") {
    return (
      <main className="flex min-h-screen items-center justify-center bg-surface-base px-4 text-content-primary">
        <section className="rounded-lg border border-surface-border/30 bg-surface-panel px-5 py-4 shadow-xl shadow-[rgb(var(--shadow-color)/0.18)]">
          <p className="font-mono text-xs uppercase tracking-[0.22em] text-accent">
            {t("Halv")}
          </p>
          <p className="mt-2 text-sm text-content-secondary">
            {t("Checking account...")}
          </p>
          {status.error ? (
            <p className="mt-2 text-sm text-danger">{status.error}</p>
          ) : null}
        </section>
      </main>
    );
  }

  if (welcomeScreenShown) {
    return (
      <WelcomeScreen
        onGetStarted={() => {
          writeWelcomeSeen();
          setWelcomeSeen(true);
        }}
        onSkip={() => {
          // A snooze for this launch. The guide returns while the app is
          // still fresh, so the user still declares a first task.
          writeWelcomeSeen();
          setWelcomeSeen(true);
          setOnboardingPreference("dismissed");
        }}
      />
    );
  }

  if (
    connectGateState === "required" ||
    connectGateState === "optional"
  ) {
    return (
      <ConnectGate
        anthropic={status.anthropic}
        auth={status.auth}
        credentialProfiles={credentialProfiles}
        gateState={connectGateState}
        laneOptions={connectGateLaneOptions}
        onAddGrokApiKey={addGrokApiKey}
        onAddZaiApiKey={addZaiApiKey}
        onConnectionsChanged={refreshConnectGateAccounts}
        onContinue={() => {
          if (onboardingPreference === "available" && !hasAnyConversationMessages) {
            const model = firstRunComposerModel(models, null, composerConnections, status.auth);
            if (model) {
              setSelectedModel(model.id);
              setSelectedEffort(model.defaultEffort);
              conversationDefaultsRef.current = {
                ...conversationDefaultsRef.current,
                model: model.id,
                effort: model.defaultEffort,
              };
            }
          }
          if (connectGateState === "optional") {
            try {
              window.localStorage.setItem(
                CONNECT_GATE_DISMISSED_STORAGE_KEY,
                "1",
              );
            } catch {
              // The current launch still keeps the gate dismissed.
            }
            setConnectGateDismissed(true);
          }
          setConnectGateState("passed");
        }}
        onLoginChatGpt={async () => {
          await connectChatGpt();
          await refreshConnectGateAccounts();
        }}
        onSetAnthropicApiKey={setAnthropicApiKey}
        onSetOpenAiApiKey={setOpenAiApiKey}
        probe={connectGateProbe}
        shortcutOverrides={shortcutOverrides}
        sidecar={status.sidecar}
      />
    );
  }

  return (
    <main
      className="flex h-screen flex-col overflow-hidden bg-surface-base text-content-primary"
      data-workspace-tabs={
        featureFlags.workspaceTabs ? (tabsReady ? "ready" : "loading") : undefined
      }
    >
      <div
        className="flex h-9 w-full flex-none items-center border-b border-surface-border/25 bg-surface-sunken px-3"
        data-tauri-drag-region
      >
        <div className="w-20 flex-none pointer-events-none" />
        <div className="pointer-events-none text-xs font-semibold text-content-secondary">
          {t("Halv")}
        </div>
      </div>

      <div className="relative flex min-h-0 flex-1 overflow-hidden">
        {!sidebarCollapsed ? (
          <div
            aria-hidden="true"
            className="absolute inset-0 z-30 bg-black/35 backdrop-blur-[1px] md:hidden"
            onClick={() => setSidebarCollapsed(true)}
          />
        ) : null}
        <ThreadSidebar
          projectTree={sidebarProjectTree}
          dotOpen={dotOpen}
          onOpenDot={() => setDotOpen(true)}
          collapsed={sidebarCollapsed}
          conversations={conversations}
          currentConversationId={dotOpen ? null : currentConversationId}
          projectSwitcherOpen={projectSwitcherOpen}
          runningConversationIds={runningConversationIds}
          terminals={sidebarListTerminals}
          unviewedConversationIds={unviewedConversationIds}
          searchFocusSignal={searchFocusSignal}
          projects={projects}
          onDeleteConversation={(conversationId) =>
            void handleDeleteConversation(conversationId)
          }
          onExportConversation={(conversationId, format) =>
            void handleExportConversation(conversationId, format)
          }
          onRenameConversation={(conversationId, title) =>
            void ipc.storage.setConversationTitle({
              conversationId,
              title,
            })
              .then((summary) => {
                upsertConversationSummary(summary);
                return loadConversations();
              })
              .catch((error) => setChatError(errorMessage(error)))
          }
          onSetConversationPinned={(conversationId, pinned) =>
            void handleSetConversationPinned(conversationId, pinned)
          }
          onUnarchiveConversation={(conversationId) =>
            void handleUnarchiveConversation(conversationId)
          }
          onAddFolder={openAddFolderModal}
          onCloneGithub={openCloneGithubModal}
          onLoadConversation={(conversationId) => {
            setDotOpen(false);
            void openConversation(conversationId);
            if (window.matchMedia("(max-width: 767px)").matches) {
              setSidebarCollapsed(true);
            }
          }}
          onFocusTerminal={(sessionId) => {
            setDotOpen(false);
            const entry = agentSessionsState.entries.find(
              (candidate) => candidate.session.id === sessionId,
            );
            if (entry?.session.conversationId) {
              void openConversation(entry.session.conversationId);
            } else if (entry) {
              agentSessionStore.select(sessionId);
              setAgentSessionsOpen(true);
            } else {
              setTerminalFocusSessionId(sessionId);
              setTerminalOpen(true);
            }
            if (window.matchMedia("(max-width: 767px)").matches) {
              setSidebarCollapsed(true);
            }
          }}
          onRenameTerminal={(sessionId, title) => {
            const entry = agentSessionsState.entries.find(
              (candidate) => candidate.session.id === sessionId,
            );
            const rename = entry
              ? agentSessionStore.rename(sessionId, title)
              : ipc.terminal.rename({
                  sessionId,
                  title,
                });
            void rename
              .then(() => refreshTerminalSessions())
              .catch((error) => setChatError(errorMessage(error)));
          }}
          onOpenShellInTab={
            shellTabsOn ? (sessionId) => void openShellInTab(sessionId) : null
          }
          onNewConversation={() => {
            startNewConversation();
            if (window.matchMedia("(max-width: 767px)").matches) {
              setSidebarCollapsed(true);
            }
          }}
          onOpenSettings={() => {
            setSettingsOpen(true);
            if (window.matchMedia("(max-width: 767px)").matches) {
              setSidebarCollapsed(true);
            }
          }}
          onProjectSwitcherOpenChange={(open) => {
            setProjectSwitcherOpen(open);
            if (!open) {
              firstRunProjectPickPendingRef.current = false;
            }
          }}
          onRemoveProject={(projectId) => void handleRemoveProject(projectId)}
          onSelectProject={(projectId) => {
            void handleSelectProject(projectId);
            if (window.matchMedia("(max-width: 767px)").matches) {
              setSidebarCollapsed(true);
            }
          }}
          onToggleCollapsed={() => setSidebarCollapsed((current) => !current)}
          selectedProjectId={
            currentConversation ? (currentConversation.projectId ?? null) : selectedProjectId
          }
        />

        {savingsDrawerOpen ? (
          <div
            className="fixed inset-x-0 bottom-[max(1.5rem,24px)] top-9 z-50 bg-black/25"
            onClick={() => setSavingsDrawerOpen(false)}
          >
            <aside
              aria-labelledby="savings-drawer-title"
              aria-modal="true"
              className="ml-auto flex h-full w-[480px] max-w-full flex-col border-l border-surface-border/25 bg-surface-panel shadow-2xl shadow-black/20"
              id="savings-drawer"
              onClick={(event) => event.stopPropagation()}
              role="dialog"
            >
              <header className="flex items-start gap-3 border-b border-surface-border/25 p-4">
                <div className="min-w-0 flex-1">
                  <h2
                    className="text-sm font-semibold text-content-primary"
                    id="savings-drawer-title"
                  >
                    {t("Savings")}
                  </h2>
                  <p className="mt-0.5 text-xs text-content-muted">
                    {t("Measured at the local proxy · updates live")}
                  </p>
                </div>
                <button
                  aria-label={t("Close savings")}
                  className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-content-muted hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/35"
                  onClick={() => setSavingsDrawerOpen(false)}
                  type="button"
                >
                  ×
                </button>
              </header>
              <div className="min-h-0 flex-1 overflow-y-auto p-4">
                <SavingsPanel
                  account={status.account}
                  apiKeyConnected={Boolean(
                    status.auth && "apiKey" in status.auth,
                  )}
                  dailyTokenActivity={dailyTokenActivity}
                  planProviderConnections={sidebarPlanProviderConnections}
                  onSignIn={() => setAccountFlowOpen("account")}
                  stats={status.sidecarStats}
                  routing={routingSavingsFromTotals(weeklySavings?.routing)}
                  totals={savingsTotals}
                  variant="drawer"
                  weeklyDollarsUsd={weeklySavings?.dollarsUsd ?? null}
                  weeklyRecentDollars={weeklySavings?.recentDollars ?? null}
                  weeklyTokensSaved={weeklySavings?.tokensSaved ?? null}
                />
              </div>
            </aside>
          </div>
        ) : null}

        <section className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        {dotOpen && <DotPanel suspended={settingsOpen || commandPaletteOpen || savingsDrawerOpen || keyboardShortcutsOpen || extensionsOpen || localEnvironmentsOpen || automationsOpen || projectSwitcherOpen || projectModal !== null || trustPrompt !== null || accountFlowOpen !== null} />}
        <header
          className="z-20 flex-none border-b border-surface-border/20 bg-surface-glass/90 px-3 py-2.5 backdrop-blur-xl md:px-5"
          data-app-header
        >
          <div className="flex min-h-9 items-center gap-3">
            <button
              aria-label={
                sidebarCollapsed
                  ? t("Open conversation history")
                  : t("Close conversation history")
              }
              className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-surface-border/25 text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 md:hidden"
              onClick={() => setSidebarCollapsed((current) => !current)}
              title={t("Conversation history")}
              type="button"
            >
              <span aria-hidden>☰</span>
            </button>
            {featureFlags.workspaceTabs && navigation.kind !== "loose" ? (
              // The strip replaces the title. It keeps its row in every
              // state, so a tab switch never moves the header.
              <div className="flex min-w-0 flex-1 items-center gap-2">
                {projectTreeHidden && composerProject ? (
                  <WorkspaceSwitcher
                    baseRefs={workspaceBaseRefs}
                    busy={workspaceBusy}
                    conversationBound={false}
                    environments={localEnvironments}
                    error={workspaceError}
                    onCreate={() => void handleCreateWorktree()}
                    onConfigureEnvironments={() => setLocalEnvironmentsOpen(true)}
                    onPin={(workspace, pinned) =>
                      void handlePinWorkspace(workspace, pinned)
                    }
                    onRemove={(workspace) => void handleRemoveWorkspace(workspace)}
                    onSelect={selectWorkspace}
                    onSelectBaseRef={setSelectedWorktreeBaseRef}
                    onSelectEnvironment={selectLocalEnvironment}
                    selectedEnvironmentConfigPath={selectedEnvironmentConfigPath}
                    selectedBaseRef={selectedWorktreeBaseRef}
                    selectedWorkspaceId={selectedWorkspaceId}
                    workspaces={workspaces}
                  />
                ) : null}
                {strip ? (
                  <WorkspaceTabStrip
                    activeKey={stripDraft ? null : visibleTabKey}
                    agentLabel={tabAgentLabel}
                    draft={stripDraft && !launcherShown}
                    draftTitle={textDraft ? t("Text conversation") : undefined}
                    onActivate={(key) => activateWorkspaceTab(key as TabKey)}
                    onClose={(key) => void closeWorkspaceTab(key as TabKey)}
                    onMove={(key, beforeKey) => {
                      // The strip can hide rows, so the index counts every
                      // tab of the workspace.
                      const view = visibleStrip();
                      if (!view) return;
                      workspaceTabsStore.move(
                        key as TabKey,
                        moveIndexBefore(view.keys, key, beforeKey),
                      );
                    }}
                    onCloseDraft={() => {
                      const view = visibleStrip();
                      const nav = navigationRef.current;
                      if (view && view.keys.length > 0 && view.activeKey) {
                        activateWorkspaceTab(view.activeKey);
                      } else if (proMode && nav.kind === "workspace") {
                        // An empty strip goes back to the launcher (ADR 0020).
                        switchComposerDraft(null);
                        navigate(workspaceNavigation(nav.workspaceId));
                        focusVisibleTab();
                      }
                    }}
                    onPlusAgent={(kind) => void startAgentTab(kind as AgentKind)}
                    onPlusAgentSettings={() => openConnectionsSettings(null)}
                    onPlusTextConversation={startTextConversation}
                    onPlusNewTerminal={shellTabsOn ? () => void openShellTab() : undefined}
                    onPlusReopen={reopenClosedTab}
                    onRename={(key, title) => renameWorkspaceTab(key as TabKey, title)}
                    renameRequest={tabRenameRequest}
                    menuShortcuts={tabMenuShortcuts}
                    onTabMenu={(key, action) =>
                      handleTabMenu(
                        key as TabKey,
                        action,
                        stripTabs.map((tab) => tab.key as TabKey),
                      )
                    }
                    plusItems={stripPlusItems}
                    tabs={stripTabs}
                    trailing={
                      shellTabsOn && localEnvironmentActions.length > 0 ? (
                        // Orca's Quick Commands, at the right end of the
                        // strip (ADR 0020): a local environment action runs
                        // in a new shell tab. Without an action, the button
                        // does not show.
                        <WorkspaceRunButton
                          actions={localEnvironmentActions}
                          busy={environmentActionBusy}
                          onConfigure={() => setLocalEnvironmentsOpen(true)}
                          onRun={(index) => void runLocalEnvironmentAction(index)}
                        />
                      ) : null
                    }
                    workspaceLabel={stripWorkspaceLabel}
                  />
                ) : (
                  <div className="workspace-tab-strip" />
                )}
                <RecentTabSwitcher
                  binding={shortcutForAction("workspace.recent_tab", shortcutOverrides)}
                  enabled={Boolean(strip) && !tabKeysBlocked}
                  items={recentTabItems}
                  onCommit={(key) => activateWorkspaceTab(key as TabKey)}
                />
              </div>
            ) : currentConversation ? (
              <div className="flex min-w-0 flex-1 items-center gap-3">
                <p className="truncate text-sm font-semibold text-content-primary">
                  {currentConversationDisplayTitle}
                </p>
                {resolvedAgentModel &&
                !conversationUsesAgentSurface(currentConversation) ? (
                  // Terminal conversations can run several agents at once;
                  // a single model chip in the header is wrong more often
                  // than right.
                  <span className="hidden shrink-0 font-mono text-[10px] text-content-muted lg:inline">
                    {resolvedAgentModel}
                  </span>
                ) : null}
                {currentConversation.goal ? (
                  <button
                    aria-label={t("Clear pinned goal")}
                    className="shrink-0 rounded-full bg-accent/10 px-2.5 py-1 font-mono text-[10px] text-accent transition-colors hover:bg-accent/15 focus:outline-none focus:ring-2 focus:ring-accent/30"
                    onClick={() => void handleSetConversationGoal(null)}
                    title={t("{goal} - click to clear", {
                      goal: currentConversation.goal,
                    })}
                    type="button"
                  >
                    {t("/goal pinned")}
                  </button>
                ) : null}
              </div>
            ) : (
              <div className="min-w-0 flex-1">
                {composerProject ? (
                  <WorkspaceSwitcher
                    baseRefs={workspaceBaseRefs}
                    busy={workspaceBusy}
                    conversationBound={false}
                    environments={localEnvironments}
                    error={workspaceError}
                    onCreate={() => void handleCreateWorktree()}
                    onConfigureEnvironments={() => setLocalEnvironmentsOpen(true)}
                    onPin={(workspace, pinned) =>
                      void handlePinWorkspace(workspace, pinned)
                    }
                    onRemove={(workspace) => void handleRemoveWorkspace(workspace)}
                    onSelect={selectWorkspace}
                    onSelectBaseRef={setSelectedWorktreeBaseRef}
                    onSelectEnvironment={selectLocalEnvironment}
                    selectedEnvironmentConfigPath={selectedEnvironmentConfigPath}
                    selectedBaseRef={selectedWorktreeBaseRef}
                    selectedWorkspaceId={selectedWorkspaceId}
                    workspaces={workspaces}
                  />
                ) : (
                  <BranchStatusChip status={branchStatus} />
                )}
              </div>
            )}

            {featureFlags.workspaceTabs ? (
              // The visible agent tab shows its savings meter here. A tab
              // body has no header of its own (Orca). The slot keeps a fixed
              // width while the strip has an agent tab, so a tab switch
              // never resizes the tabs.
              <div
                className="workspace-header-meter"
                data-reserved={stripTabs.some(
                  (tab) => tab.identity !== null && tab.identity !== "chat",
                )}
                ref={setHeaderMeterSlot}
              />
            ) : null}

            {featureFlags.workspaceTabs ? (
              // The panel toggle keeps its place in every tab, so the header
              // never shifts. The diff rail belongs to a conversation.
              <button
                aria-label={
                  diffRailState.ui.open ? t("Close diff rail") : t("Review changes")
                }
                aria-pressed={diffRailState.ui.open}
                className="workspace-panel-toggle"
                disabled={!currentConversation}
                onClick={() => {
                  if (diffRailState.ui.open) toggleDiffRail();
                  else openDiffRail(true);
                }}
                title={diffRailState.ui.open ? t("Close diff rail") : t("Review changes")}
                type="button"
              >
                <svg aria-hidden fill="none" height="14" stroke="currentColor" strokeWidth="1.6" viewBox="0 0 16 16" width="14">
                  <rect height="12" rx="2" width="13" x="1.5" y="2" />
                  <path d="M10 2v12" />
                </svg>
              </button>
            ) : null}

            {!featureFlags.workspaceTabs && currentConversation && composerProject ? (
              <WorkspaceSwitcher
                baseRefs={workspaceBaseRefs}
                busy={workspaceBusy}
                conversationBound
                environments={localEnvironments}
                error={workspaceError}
                onCreate={() => void handleCreateWorktree()}
                onConfigureEnvironments={() => setLocalEnvironmentsOpen(true)}
                onPin={(workspace, pinned) =>
                  void handlePinWorkspace(workspace, pinned)
                }
                onRemove={(workspace) => void handleRemoveWorkspace(workspace)}
                onSelect={selectWorkspace}
                onSelectBaseRef={setSelectedWorktreeBaseRef}
                onSelectEnvironment={selectLocalEnvironment}
                selectedEnvironmentConfigPath={selectedEnvironmentConfigPath}
                selectedBaseRef={selectedWorktreeBaseRef}
                selectedWorkspaceId={selectedWorkspaceId}
                workspaces={workspaces}
              />
            ) : null}

            <AgentSessionProModeToggle
              ghostty={probeForGhostty(connectGateProbe)}
              showIntroHint={!showFirstRunCard && guidedTaskPhase === "idle"}
            />

            <MobileHeaderButton />

            {/* Keep this source anchor for proModeComposer.test.ts.
>
              Commands
            */}
            <button
              className="hidden shrink-0 items-center gap-2 rounded-lg border border-surface-border/25 px-3 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 sm:flex"
              onClick={() => setCommandPaletteOpen(true)}
              title={t("Open command palette")}
              type="button"
            >
              {t("Commands")}
              <kbd className="font-mono text-[10px] text-content-muted">⌘K</kbd>
            </button>

            <details className="group/header-menu relative" ref={headerMenuRef}>
              <summary
                aria-label={t("Workspace and application menu")}
                className="relative flex h-9 w-9 cursor-pointer list-none items-center justify-center rounded-lg border border-surface-border/25 text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                title={t("Workspace and application menu")}
              >
                <span aria-hidden className="text-base leading-none">{"•••"}</span>
                {automationUnreadCount > 0 ? (
                  <span className="absolute -right-1 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full bg-accent px-1 font-mono text-[8px] text-accent-contrast">
                    {automationUnreadCount}
                  </span>
                ) : null}
              </summary>
              <div className="absolute right-0 top-[calc(100%+0.5rem)] z-50 max-h-[70vh] w-64 overflow-y-auto rounded-xl border border-surface-border/30 bg-surface-glass/95 p-1.5 shadow-xl shadow-[rgb(var(--shadow-color)/0.18)] backdrop-blur-xl">
                <div className="px-2.5 pb-1.5 pt-1 font-mono text-[9px] uppercase tracking-[0.16em] text-content-muted">
                  {t("Workspace")}
                </div>
                <button
                  className="flex w-full items-center justify-between rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    setWorkspaceView((current) =>
                      current === "browser" ? "chat" : "browser",
                    );
                  }}
                  type="button"
                >
                  <span>
                    {workspaceView === "browser"
                      ? t("Return to chat")
                      : t("Browser")}
                  </span>
                  <span className="font-mono text-[10px] text-content-muted">⌘⇧B</span>
                </button>
                <button
                  className="flex w-full items-center justify-between rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    setTerminalOpen((current) => !current);
                  }}
                  type="button"
                >
                  <span>
                    {terminalOpen ? t("Close terminal") : t("Terminal")}
                  </span>
                  <span className="font-mono text-[10px] text-content-muted">⌘J</span>
                </button>
                <button
                  className="flex w-full items-center justify-between rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    if (diffRailState.ui.open) {
                      toggleDiffRail();
                    } else {
                      openDiffRail(true);
                    }
                  }}
                  type="button"
                >
                  <span>
                    {diffRailState.ui.open
                      ? t("Close diff rail")
                      : t("Review changes")}
                  </span>
                  {diffRailState.ui.open ? <span aria-hidden>✓</span> : null}
                </button>

                {currentConversation && composerProject && selectedWorkspace ? (
                  <button
                    className="block w-full rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-45"
                    disabled={workspaceBusy || selectedSendPending}
                    onClick={(event) => {
                      closeParentDetails(event.currentTarget);
                      void handleWorkspaceHandoff(
                        selectedWorkspace.kind === "worktree" ? "local" : "worktree",
                      );
                    }}
                    type="button"
                  >
                    {workspaceBusy
                      ? t("Handing off…")
                      : selectedWorkspace.kind === "worktree"
                        ? t("Hand off to Local")
                        : t("Hand off to Worktree")}
                  </button>
                ) : null}

                {currentConversation &&
                selectedWorkspace?.kind === "worktree" &&
                !selectedWorkspace.currentBranch ? (
                  <button
                    className="block w-full rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-45"
                    disabled={workspaceBusy || selectedSendPending}
                    onClick={(event) => {
                      closeParentDetails(event.currentTarget);
                      void handleCreateWorkspaceBranch();
                    }}
                    type="button"
                  >
                    {t("Create branch here")}
                  </button>
                ) : null}

                {localEnvironmentActions.length === 0 && composerProject ? (
                  // The Run button shows only with an action (ADR 0020). The
                  // settings stay one click away here.
                  <button
                    className="block w-full rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                    onClick={(event) => {
                      closeParentDetails(event.currentTarget);
                      setLocalEnvironmentsOpen(true);
                    }}
                    type="button"
                  >
                    {t("Set up local environment…")}
                  </button>
                ) : null}

                {localEnvironmentActions.map(({ action, index }) => (
                  <button
                    aria-label={t("Run local environment action {name}", {
                      name: action.name,
                    })}
                    className="flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-45"
                    disabled={environmentActionBusy || !composerProject}
                    key={`${activeEnvironmentConfigPath}-${index}-${action.name}`}
                    onClick={(event) => {
                      closeParentDetails(event.currentTarget);
                      void runLocalEnvironmentAction(index);
                    }}
                    title={`${action.name}: ${action.command}`}
                    type="button"
                  >
                    <span aria-hidden>
                      {action.icon === "run"
                        ? "▶"
                        : action.icon === "debug"
                          ? "◆"
                          : action.icon === "test"
                            ? "✓"
                            : "⌘"}
                    </span>
                    <span className="truncate">{action.name}</span>
                  </button>
                ))}

                <button
                  className="block w-full rounded-lg px-3 py-2 text-left text-sm font-medium text-accent hover:bg-accent/10 disabled:cursor-not-allowed disabled:opacity-45"
                  disabled={
                    !composerProject ||
                    prBusy ||
                    (selectedWorkspace?.kind === "worktree" &&
                      !selectedWorkspace.currentBranch)
                  }
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    void handleCreateDraftPr();
                  }}
                  title={
                    selectedWorkspace?.kind === "worktree" &&
                      !selectedWorkspace.currentBranch
                      ? t("Create a branch here before opening a pull request")
                      : t("Create a draft pull request")
                  }
                  type="button"
                >
                  {prBusy
                    ? t("Creating draft PR…")
                    : t("Create draft pull request")}
                </button>

                <div className="my-1 h-px bg-surface-border/20" />
                <div className="px-2.5 pb-1.5 pt-1 font-mono text-[9px] uppercase tracking-[0.16em] text-content-muted">
                  {t("Application")}
                </div>
                <button
                  className="block w-full rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    setAutomationsOpen(true);
                  }}
                  type="button"
                >
                  <span className="flex items-center justify-between gap-3">
                    <span>{t("Automations")}</span>
                    {automationUnreadCount > 0 ? (
                      <span className="rounded-full bg-accent px-1.5 py-0.5 font-mono text-[9px] text-accent-contrast">
                        {automationUnreadCount}
                      </span>
                    ) : null}
                  </span>
                </button>
                <button
                  className="block w-full rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    setAgentSessionsOpen(true);
                  }}
                  type="button"
                >
                  {t("Agent sessions")}
                </button>
                <button
                  className="block w-full rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    setExtensionsOpen(true);
                  }}
                  type="button"
                >
                  {t("Extensions")}
                </button>
                <button
                  className="block w-full rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-45"
                  disabled={!composerProject}
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    setLocalEnvironmentsOpen(true);
                  }}
                  type="button"
                >
                  {t("Local environments")}
                </button>
                {featureFlags.mobileRemote ? (
                  <button
                    className="block w-full rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                    onClick={(event) => {
                      closeParentDetails(event.currentTarget);
                      mobileConnectStore.open();
                    }}
                    type="button"
                  >
                    {t("Connect phone…")}
                  </button>
                ) : null}
                <button
                  className="block w-full rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    setSettingsOpen(true);
                  }}
                  type="button"
                >
                  {t("Settings")}
                </button>
                <button
                  className="flex w-full items-center justify-between rounded-lg px-3 py-2 text-left text-sm text-content-secondary hover:bg-surface-raised hover:text-content-primary sm:hidden"
                  onClick={(event) => {
                    closeParentDetails(event.currentTarget);
                    setCommandPaletteOpen(true);
                  }}
                  type="button"
                >
                  <span>{t("All commands")}</span>
                  <span className="font-mono text-[10px] text-content-muted">⌘K</span>
                </button>
              </div>
            </details>
          </div>

          {status.error ? (
            <div className="mt-3 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger">
              {t("IPC error: {error}", { error: status.error })}
            </div>
          ) : null}

          {projectError && !projectModal ? (
            <div className="mt-3 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger">
              {t("Project error: {error}", { error: projectError })}
            </div>
          ) : null}

          {prMessage ? (
            <div className="mt-3 rounded-md border border-surface-border/30 bg-surface-raised/65 px-3 py-2 text-xs text-content-secondary">
              {prMessage}
            </div>
          ) : null}

          {workspaceNotice ? (
            <div className="mt-3 rounded-md border border-accent/25 bg-accent/10 px-3 py-2 text-xs text-content-secondary">
              {workspaceNotice}
            </div>
          ) : null}

          {tabCloseError ? (
            <div
              className="mt-3 flex items-start gap-2 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger"
              role="alert"
            >
              <span className="min-w-0 flex-1">{tabCloseError}</span>
              <button
                aria-label={t("Dismiss")}
                className="shrink-0 rounded px-1 text-danger/80 hover:text-danger"
                onClick={() => setTabCloseError(null)}
                type="button"
              >
                ×
              </button>
            </div>
          ) : null}
        </header>

        {showConsentBanner ? <ConsentAskBanner /> : null}

        {showCloudPitch ? (
          <HalvCloudCard
            access={halvCloudAccess}
            account={status.account}
            onAccountRefresh={refreshAccountEntitlements}
            onCheckoutOpened={() => writeCloudPitchDismissed()}
            onDismiss={dismissCloudPitch}
            onSignIn={() => setAccountFlowOpen("account")}
            onStatusChange={handleAccountStatusChange}
            payoff={guidedTaskPayoff}
            placement="pitch"
          />
        ) : null}

        {workspaceView === "chat" && guidedTaskStatusVisible ? (
          <GuidedTaskStatus
            onDismiss={closeGuidedTaskStatus}
            terminalFollowUp={guidedTaskRunMode === "terminal" ? firstTaskFollowUp(firstTaskGoal) : undefined}
            onContinue={async (nextTask) => {
              if (guidedTaskFailed) {
                setGuidedTaskCancelled(false);
                guidedTerminalSawActiveRef.current = false;
                guidedTerminalSawSustainedOutputRef.current = false;
                guidedTerminalSessionIdRef.current = null;
                guidedTerminalTrackingArmedRef.current = guidedTaskRunMode === "terminal";
                void recordAnalyticsEvent("active_day");
                await sendMessage(guidedTaskPromptRef.current, currentConversationId, { guidedTask: true });
              } else if (guidedTaskRunMode === "terminal") {
                const sessionId = guidedTerminalSessionIdRef.current;
                try {
                  if (!sessionId) throw new TerminalDraftError("unavailable");
                  await terminalDraftBridge.prepare(sessionId, nextTask ?? firstTaskFollowUp(firstTaskGoal));
                  setGuidedTaskPhase("idle");
                } catch (error) {
                  if (error instanceof TerminalDraftError) {
                    throw new Error(error.reason === "occupied"
                      ? t("Finish or clear the draft already in this terminal.")
                      : error.reason === "invalid"
                        ? t("Enter a task without terminal control characters.")
                        : t("The agent terminal is not ready. Try again."));
                  }
                  throw error;
                }
              } else {
                setGuidedTaskPhase("idle");
                handleSuggestion(firstTaskFollowUp(firstTaskGoal));
              }
            }}
            onInspectSavings={() => setSavingsDrawerOpen(true)}
            celebration={firstResultCelebration}
            error={chatError}
            payoff={guidedTaskPayoff}
            phase={guidedTaskFailed ? "failed" : guidedTaskPhase}
          />
        ) : null}

        {workspaceView === "browser" ? (
          <BrowserPanel
            onSettingsOpened={() => setBrowserSettingsRequested(false)}
            onClose={() => setWorkspaceView("chat")}
            onSendAnnotations={async (message) => {
              setWorkspaceView("chat");
              await sendMessage(
                message,
                currentConversationIdRef.current,
                { queueIfPending: true },
              );
            }}
            open
            openSettings={browserSettingsRequested}
          />
        ) : (
          <div className="conversation-mode-stack">
            {tabBodyConversations.map((conversation) => {
              const entry = agentSessionForConversationMode(
                agentSessionsState.entries,
                conversation.id,
                "agent",
              );
              const tabs = featureFlags.workspaceTabs;
              // A prepared draft has no PTY to restore or retain yet.
              if (
                tabs
                  ? tabBodyKind(conversation) === "chat"
                  : !entry && !conversationHasSavedTerminals(conversation.id)
              ) {
                return null;
              }
              const active =
                showProTerminalSurface &&
                conversation.id === currentConversationId;
              const tabProject = tabs
                ? (projects.find((item) => item.id === conversation.projectId) ?? null)
                : null;
              const tabWorkspaceId = tabs
                ? (homeWorkspaceId(
                    {
                      key: conversationTabKey(conversation.id),
                      conversationId: conversation.id,
                      shellWorkspaceId: null,
                      customTitle: null,
                      defaultTitle: null,
                      position: 0,
                      createdAt: 0,
                      activatedAt: null,
                    },
                    workspaceTabsSnapshot.facts,
                  ) ?? null)
                : null;
              const startingAgent =
                tabs &&
                !entry &&
                terminalOpeningByConversation[conversation.id]?.phase === "opening"
                  ? agentSessionAgentLabel(
                      agentKindForBackend(conversation.agentBackend) ?? defaultAgentKind,
                    )
                  : null;
              return (
                <div
                  aria-hidden={!active}
                  className="conversation-mode-surface conversation-agent-mode-surface"
                  data-active={active}
                  data-workspace-tab-body={tabs ? conversationTabKey(conversation.id) : undefined}
                  key={conversation.id}
                  // A hidden tab body takes no focus and no keys (T14).
                  {...(tabs && !active ? { inert: "" } : {})}
                >
                  {entry?.session.mode === "terminal_ghostty" ? (
                    <AgentSessionView
                      entry={entry}
                      onKill={(sessionId) => agentSessionStore.kill(sessionId)}
                      onSend={(sessionId, text) =>
                        agentSessionStore.send(sessionId, text)
                      }
                    />
                  ) : (
                    <div className="conversation-terminal-handoff-surface">
                      {active &&
                      terminalHandoffNotice &&
                      // A tab body says "Starting…" itself. Only a failure
                      // keeps the notice above the panes.
                      (!tabs || terminalHandoffNotice.text.startsWith("Terminal failed to open")) ? (
                        <div className="conversation-terminal-handoff-notice">
                          <span aria-hidden>⚑</span>
                          {terminalHandoffNotice.text}
                        </div>
                      ) : null}
                      <div className="terminal-diff-surface terminal-diff-surface-agent">
                        <TerminalPanel
                          projects={projects}
                          agentEntry={entry}
                          conversationApprovalPolicy={approvalPolicyForConversation(
                            conversation,
                          )}
                          conversationId={conversation.id}
                          conversationModel={
                            conversation.agentSessionModel ?? conversation.model
                          }
                          conversationTitle={conversation.title}
                          claimScope={tabs}
                          fill={tabs}
                          headerSlot={active ? headerMeterSlot : null}
                          shellExitRules={shellTabsOn && navigation.kind === "workspace"}
                          onLastPaneClosed={
                            shellTabsOn && navigation.kind === "workspace"
                              ? () =>
                                  void closeWorkspaceTab(conversationTabKey(conversation.id), {
                                    prompt: false,
                                  })
                              : undefined
                          }
                          startingLabel={startingAgent}
                          focusSessionId={entry?.session.id ?? null}
                          onActivePaneChange={
                            active ? reportActiveTerminalPane : undefined
                          }
                          onClose={() => undefined}
                          onSessionAttached={() => {
                            updateTerminalOpening(conversation.id, {
                              type: "attach",
                            });
                          }}
                          onPromptSubmitSignalHandled={
                            acknowledgeTerminalPromptSubmitSignal
                          }
                          promptSubmitSignals={terminalPromptSubmitSignals}
                          onSessionAttachError={(_sessionId, message) => {
                            updateTerminalOpening(conversation.id, {
                              type: "fail",
                              message,
                            });
                          }}
                          onSessionsChanged={() => {
                            void refreshTerminalSessions().catch((error) =>
                              console.error(
                                "Terminal session refresh failed",
                                error,
                              ),
                            );
                          }}
                          onPanesChange={
                            active ? publishDiffRailPanes : undefined
                          }
                          onPromptSubmitGate={() => {
                            // A terminal prompt never waits for an account
                            // (ADR 0021). The callback only counts the task.
                            void recordAnalyticsEvent("active_day");
                            void recordAnalyticsEvent("first_task_started");
                            return false;
                          }}
                          open
                          project={tabProject}
                          shortcutOverrides={shortcutOverrides}
                          visible={active}
                          workspaceId={tabWorkspaceId}
                        />
                        {active ? (
                          <>
                            <TaskBoard
                              conversationId={conversation.id}
                              onAccountStatusChange={handleAccountStatusChange}
                              onSignIn={() => setAccountFlowOpen("account")}
                            />
                            <DiffRailErrorBoundary>
                              <DiffRail
                                conversationId={conversation.id}
                                focusSignal={diffRailFocusSignal}
                                projects={projects}
                                workspaces={workspaces}
                              />
                            </DiffRailErrorBoundary>
                          </>
                        ) : null}
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
            {tabBodyShells.map((row) => {
              const shellTabId = parseTabKey(row.key)?.id ?? "";
              const active = shellTabVisible && row.key === visibleTabKey;
              const workspace = row.shellWorkspaceId
                ? workspaceTabsSnapshot.facts.workspaces.get(row.shellWorkspaceId)
                : undefined;
              const shellProject =
                projects.find((item) => item.id === workspace?.projectId) ?? null;
              return (
                <div
                  aria-hidden={!active}
                  className="conversation-mode-surface conversation-agent-mode-surface"
                  data-active={active}
                  data-workspace-tab-body={row.key}
                  key={row.key}
                  // A hidden tab body takes no focus and no keys (T14).
                  {...(!active ? { inert: "" } : {})}
                >
                  <div className="conversation-terminal-handoff-surface">
                    <div className="terminal-diff-surface terminal-diff-surface-agent">
                      <TerminalPanel
                        claimScope
                        fill
                        onActivePaneChange={active ? reportActiveTerminalPane : undefined}
                        onActiveTitleChange={(title) => noteShellLiveTitle(row.key, title)}
                        onClose={() => undefined}
                        onLastPaneClosed={() =>
                          void closeWorkspaceTab(row.key, { prompt: false })
                        }
                        onSessionsChanged={() => {
                          void refreshTerminalSessions().catch((error) =>
                            console.error("Terminal session refresh failed", error),
                          );
                        }}
                        onShellOpenedOnMount={(session) =>
                          handleFirstShellOpened(row.key, session)
                        }
                        open
                        openShellOnMount={firstShellRequestsRef.current.has(row.key)}
                        project={shellProject}
                        projects={projects}
                        shellExitRules
                        shellTabId={shellTabId}
                        shortcutOverrides={shortcutOverrides}
                        visible={active}
                        workspaceId={row.shellWorkspaceId}
                      />
                    </div>
                  </div>
                </div>
              );
            })}
            <div
              aria-hidden={showProTerminalSurface}
              className="conversation-mode-surface conversation-chat-mode-surface"
              data-active={!showProTerminalSurface}
            >
              {guidedTaskPhase === "prepared" && selectedProject ? (
                <div className="relative min-h-0 flex-1 overflow-y-auto px-6 py-5">
                  <div className="mx-auto flex min-h-full items-center">
                    <PreparedFirstTask
                      draft={draft}
                      error={chatError || (status.sidecar?.state === "error" ? status.sidecar.message : null)}
                      engineReady={status.sidecar?.state === "running"}
                      pending={firstRunProjectPending || sidecarRestartPending || selectedSendPending}
                      onDraftChange={setDraft}
                      onRetryEngine={() => void handleRestartSidecar()}
                      mode={activeAgentMode ? "agent" : "chat"}
                      onChangeFolder={handleOnboardingPickProject}
                      onDismiss={dismissGuidedFirstTask}
                      onModeChange={handlePreparedTaskModeChange}
                      onSend={() => handleSend()}
                      projectPath={selectedProject.rootPath}
                    />
                  </div>
                </div>
              ) : showFirstRunCard ? (
                <div className="relative min-h-0 flex-1 overflow-y-auto px-6 py-5">
                  <div className="mx-auto flex min-h-full max-w-2xl items-center">
                    <FirstRunCard
                      goal={firstTaskGoal}
                      onGoalChange={setFirstTaskGoal}
                      pending={firstRunProjectPending}
                      onDismiss={dismissGuidedFirstTask}
                      onOpenPrivacySettings={openPrivacySettings}
                      onPickProject={handleOnboardingPickProject}
                      showCloudInfo={
                        !cloudInfoSeenOnEarlierLaunch && !welcomeShownThisLaunch
                      }
                      variant={activeFirstRunVariant}
                    />
                  </div>
                </div>
              ) : launcherShown ? (
                <WorkspaceLauncher
                  agentLabel={tabAgentLabel}
                  focusSignal={composerFocusSignal}
                  items={launcherChoices}
                  onAgent={(kind) => void startAgentTab(kind as AgentKind)}
                  onTerminal={() => void openShellTab()}
                  onText={startTextConversation}
                  primary={launcherWorkspace?.kind === "primary"}
                  projectAvatar={launcherProject ? projectAvatar(launcherProject) : null}
                  projectName={launcherProject?.name ?? null}
                  workspaceLabel={stripWorkspaceLabel}
                />
              ) : chatBodyLoading ? (
                // A tab switch never shows the messages of the previous
                // conversation. The note shows only for a slow load.
                <div aria-busy="true" className="workspace-chat-loading">
                  <span>{t("Loading conversation…")}</span>
                </div>
              ) : (
                <>
                  <TaskFindBar
                    messages={messages}
                    onClose={() => setTaskFindOpen(false)}
                    open={taskFindOpen}
                  />
                  <MessageList
                    compaction={activeCompaction}
                    actionsDisabled={
                      selectedSendPending || cancelAndRevertPending
                    }
                    chatError={chatError}
                    connectPending={loginPending}
                    conversationId={currentConversationId}
                    emptyState={emptyState}
                    messages={messages}
                    sessionBacked={isSessionBackend(
                      currentConversation?.agentBackend,
                    )}
                    notices={notices}
                    onApprovalDecision={(callId, decision, paths) =>
                      void handleToolDecision(callId, decision, paths)
                    }
                    onEditMessage={(message) => void handleEditMessage(message)}
                    onForkMessage={(message) => void handleForkMessage(message)}
                    onConnectChatGpt={() => void handleChatGptLogin()}
                    onOpenSettings={() => setSettingsOpen(true)}
                    onRegenerateMessage={(message) =>
                      void handleRegenerateMessage(message)
                    }
                    onRunInNewWorktree={
                      workspaceBusyRetryRef.current
                        ? handleRunInNewWorktree
                        : undefined
                    }
                    onRestartProxy={() => void handleRestartSidecar()}
                    onRevertTurnDiff={(turnId) =>
                      void handleRevertTurnDiff(turnId)
                    }
                    onSuggestion={handleSuggestion}
                    onInterruptSubagent={(agentId) =>
                      void handleInterruptSubagent(agentId)
                    }
                    onInterruptAllSubagents={() =>
                      void handleInterruptAllSubagents()
                    }
                    onSendSubagentMessage={handleSendSubagentMessage}
                    approvalDecisionPending={
                      pendingApproval
                        ? approvalDecisionsPending.has(pendingApproval.callId)
                        : false
                    }
                    pendingApproval={pendingApproval}
                    restartPending={sidecarRestartPending}
                    scrollToBottomSignal={transcriptScrollSignal}
                    streaming={streaming}
                    subagents={subagents}
                    taskPlan={taskPlan}
                    toolCalls={toolCalls}
                    turnStartedAt={
                      selectedIsForegroundTurn ? turnStartedAt : null
                    }
                    turnDiffs={turnDiffs}
                  />
                  {status.sidecar?.state === "error" && messages.length === 0 ? (
                    <details className="mx-auto mb-3 w-[calc(100%-2rem)] max-w-md flex-none rounded-lg border border-surface-border/25 bg-surface-raised/35 px-3 py-2 text-xs text-content-muted">
                      <summary className="cursor-pointer list-none font-medium text-content-secondary focus:outline-none focus:ring-2 focus:ring-accent/30">
                        {t("Technical details")}
                      </summary>
                      <p className="mt-2 break-words font-mono text-[10px] leading-5">
                        {status.sidecar.message}
                      </p>
                    </details>
                  ) : null}
                </>
              )}
            </div>
          </div>
        )}

        {!showProTerminalSurface ? (
          <div className="terminal-diff-surface" data-open={terminalOpen}>
            <TerminalPanel
              claimScope={featureFlags.workspaceTabs}
              projects={projects}
              conversationApprovalPolicy={effectiveApprovalPolicy}
              conversationId={currentConversationId}
              conversationModel={
                currentConversation?.agentSessionModel ?? selectedModel
              }
              conversationTitle={currentConversation?.title ?? null}
              focusSessionId={terminalFocusSessionId}
              onActivePaneChange={reportActiveTerminalPane}
              onClose={() => setTerminalOpen(false)}
              onOpenAgentTerminal={() => setAgentSessionsOpen(true)}
              onPanesChange={publishDiffRailPanes}
              onRestoreAvailable={() => setTerminalOpen(true)}
              onPromptSubmitSignalHandled={
                acknowledgeTerminalPromptSubmitSignal
              }
              promptSubmitSignals={terminalPromptSubmitSignals}
              onSessionsChanged={setTerminalSessions}
              open={terminalOpen}
              project={composerProject}
              shortcutOverrides={shortcutOverrides}
              workspaceId={selectedWorkspaceId}
            />
            {terminalOpen ? (
              <DiffRailErrorBoundary>
                <DiffRail
                  conversationId={currentConversationId}
                  focusSignal={diffRailFocusSignal}
                  projects={projects}
                  workspaces={workspaces}
                />
              </DiffRailErrorBoundary>
            ) : null}
          </div>
        ) : null}


        {selectedSendPending && reasoningText.trim().length > 0 ? (
          <div className="flex-none bg-surface-base px-5 pt-2">
            <div className="mx-auto max-w-5xl">
              <ReasoningSummary text={reasoningText} />
            </div>
          </div>
        ) : null}

        <SubagentOverviewStrip
          onExpand={() => focusSubagentActivity(false)}
          onStopAll={() => void handleInterruptAllSubagents()}
          threads={subagents}
          toolCalls={toolCalls}
        />

        {showProTerminalSurface ||
        showFirstRunCard ||
        guidedTaskPhase === "prepared" ||
        launcherShown ? null : (
          <ChatComposer
          attachments={draftAttachments}
          agentBackend={currentConversation?.agentBackend ?? null}
          agentMode={activeAgentMode}
          agentModeAvailable={agentModeAvailable}
          agentModeStatus={agentModeStatus}
          approvalPolicy={effectiveApprovalPolicy}
          workflowMode={activeWorkflowMode}
          cancelAndRevertPending={cancelAndRevertPending}
          commands={activeCommandRegistry}
          dictationSignal={dictationSignal}
          dictationShortcut={formatShortcut(
            shortcutForAction("input.dictation", shortcutOverrides),
          )}
          focusSignal={composerFocusSignal}
          mentionSeedPaths={composerMentionSeed.mentions}
          mentionSeedSignal={composerMentionSeed.signal}
          currentProject={composerProject}
          workspaceId={selectedWorkspaceId}
          currentConversationId={currentConversationId}
          credentialProfiles={credentialProfiles}
          credentialProfilesLoaded={credentialProfilesLoaded}
          credentialProfileId={activeCredentialProfileId}
          draft={draft}
          fastMode={fastModeEnabled}
          fastModeAvailable={fastModeAvailable}
          directApiModelsAvailable={directApiModelsAvailable}
          models={models}
          // A chat conversation, and the draft of a text conversation,
          // list the chat models also in Pro mode (ADR 0020).
          proMode={proMode && !textDraft && currentConversation?.mode !== "chat"}
          onCancel={() => void handleCancel()}
          onCancelAndRevert={() => void handleCancelAndRevert()}
          onCredentialProfileChange={handleCredentialProfileChange}
          onDraftChange={setDraft}
          onAttachmentsChange={setDraftAttachments}
          onEffortChange={handleEffortChange}
          onFastModeChange={(enabled) => {
            void handleFastModeChange(enabled, false).catch((error) => {
              setChatError(errorMessage(error));
            });
          }}
          onModelChange={handleModelChange}
          onOpenProjectSwitcher={() => {
            setSidebarCollapsed(false);
            setProjectSwitcherOpen(true);
          }}
          onCommandPaletteOpen={() => {
            void refreshCustomPrompts(composerProject?.rootPath ?? null);
          }}
          onQueueMessage={handleQueueMessage}
          onSteer={handleSteerMessage}
          onRemoveQueued={handleRemoveQueued}
          onRunCommand={handleRunCommand}
          onModeChange={(mode) => void handleChatModeChange(mode)}
          onWorkflowModeChange={handleWorkflowModeChange}
          onSend={handleSend}
          proxyDown={status.sidecar?.state === "error"}
          queuedMessages={visibleQueuedMessages}
              resolvedModel={resolvedAgentModel}
          selectedEffort={selectedEffort}
          selectedModel={selectedModel}
          sendPending={selectedSendPending || cancelAndRevertPending}
          steerAvailable={
            currentAgentSessionEntry?.session.mode === "gui" &&
            currentAgentSessionEntry.session.status === "running" &&
            currentAgentSessionEntry.session.agentKind !== "kimi"
          }
          stopAndRevertAvailable={
            selectedIsForegroundTurn && stopAndRevertAvailable
          }
          stopRequested={selectedIsForegroundTurn && normalCancelRequested}
          />
        )}
        </section>
      </div>

      {trialGoal ? (
        <TrialGoalCard
          onOpenPlans={() => setAccountFlowOpen("paywall")}
          progress={trialGoal}
        />
      ) : null}

      <StatusBar
        onAddAccount={(provider) => openConnectionsSettings(provider)}
        onManageAccounts={() => openConnectionsSettings(null)}
        onOpenUsageDetails={() => setSavingsDrawerOpen(true)}
        onToggleSavingsDrawer={() =>
          setSavingsDrawerOpen((current) => !current)
        }
        planProviderConnections={sidebarPlanProviderConnections}
        savingsDrawerOpen={savingsDrawerOpen}
        stats={status.sidecarStats}
        terminalSessions={terminalSessions}
        weeklyApproximate={(weeklySavings?.routing?.chains ?? 0) > 0}
        weeklyDollarsUsd={weeklySavings?.dollarsUsd ?? null}
        weeklyRecentDollars={weeklySavings?.recentDollars ?? null}
        weeklyTokensSaved={weeklySavings?.tokensSaved ?? null}
      />

      <SettingsPanel
        onHalvSignIn={() => setAccountFlowOpen("account")}
        onAccountRefresh={handleAccountRefresh}
        onAccountSignOut={handleAccountSignOut}
        onAccountStatusChange={handleAccountStatusChange}
        loginPending={loginPending}
        onClose={closeSettings}
        onCredentialProfilesChanged={
          refreshCredentialProfilesAfterSettingsChange
        }
        credentialProfilesFocusSignal={settingsCredentialProfilesFocusSignal}
        connectionsRequest={settingsConnectionsRequest}
        onLoginChatGpt={handleChatGptLogin}
        onLogout={handleLogout}
        onOpenBrowserSettings={() => {
          setSettingsOpen(false);
          setDotOpen(false);
          setBrowserSettingsRequested(true);
          setWorkspaceView("browser");
        }}
        onOpenKeyboardShortcuts={() => setKeyboardShortcutsOpen(true)}
        onInstallHeadroom={handleInstallHeadroom}
        onRestartSidecar={handleRestartSidecar}
        onLanguageChange={setLanguagePreferenceState}
        onThemeChange={setThemePreferenceState}
        onUpdateHeadroom={handleUpdateHeadroom}
        open={settingsOpen}
        memoriesFocusSignal={settingsMemoriesFocusSignal}
        privacyFocusSignal={settingsPrivacyFocusSignal}
        savingsMethodFocusSignal={settingsSavingsMethodFocusSignal}
        savingsTotals={savingsTotals}
        languagePreference={languagePreference}
        status={status}
        themePreference={themePreference}
        todaySavings={todaySavings}
      />

      <KeyboardShortcutsPanel
        onChange={setShortcutOverrides}
        onClose={() => setKeyboardShortcutsOpen(false)}
        open={keyboardShortcutsOpen}
        overrides={shortcutOverrides}
      />

      <ExtensionsPanel
        onClose={() => setExtensionsOpen(false)}
        open={extensionsOpen}
        projectId={composerProject?.id ?? null}
        workspaceId={selectedWorkspaceId}
      />

      <LocalEnvironmentsPanel
        environments={localEnvironments}
        onChanged={handleLocalEnvironmentsChanged}
        onClose={() => setLocalEnvironmentsOpen(false)}
        onSelect={selectLocalEnvironment}
        open={localEnvironmentsOpen}
        project={composerProject}
        selectedConfigPath={selectedEnvironmentConfigPath}
      />

      <AutomationsPanel
        conversations={conversations}
        currentConversationId={currentConversationId}
        currentProjectId={composerProject?.id ?? null}
        currentWorkspaceId={selectedWorkspaceId}
        models={models}
        onClose={() => setAutomationsOpen(false)}
        onOpenConversation={(conversationId) => {
          void handleOpenAutomationConversation(conversationId);
        }}
        onUnreadCountChange={setAutomationUnreadCount}
        open={automationsOpen}
        projects={projects}
      />

      <AgentSessionsPanel
        conversationId={currentConversationId}
        conversationModel={
          currentConversation?.agentSessionModel ?? selectedModel
        }
        conversationTitle={currentConversation?.title ?? null}
        defaultCwd={
          selectedWorkspace?.rootPath ?? composerProject?.rootPath ?? ""
        }
        onClose={() => {
          setAgentSessionsOpen(false);
          void loadModels().catch((error) =>
            console.error("Model list refresh failed", error),
          );
        }}
        onManageKeys={() => {
          setAgentSessionsOpen(false);
          setSettingsCredentialProfilesFocusSignal((current) => current + 1);
          setSettingsOpen(true);
        }}
        onOpenInTab={
          featureFlags.workspaceTabs
            ? (conversationId, sessionId) => {
                setAgentSessionsOpen(false);
                void openConversation(conversationId);
                terminalFocusBridge.request(sessionId);
              }
            : undefined
        }
        open={agentSessionsOpen}
        projectId={composerProject?.id ?? null}
        shortcutOverrides={shortcutOverrides}
      />

      <CommandPaletteDialog
        commands={activeCommandRegistry}
        getShortcut={(command) => {
          if (!command.shortcutActionId) return command.shortcut ?? null;
          const binding = shortcutForAction(
            command.shortcutActionId,
            shortcutOverrides,
          );
          return binding === "disabled" ? null : formatShortcut(binding);
        }}
        getUnavailableReason={commandUnavailableReason}
        onClose={() => setCommandPaletteOpen(false)}
        onCreateWorktree={composerProject ? handleCreateWorktree : null}
        onPrefill={prefillCommand}
        onRun={(command) => handleRunCommand(command, "")}
        open={commandPaletteOpen}
      />

      <ProjectPathModal
        error={projectModal === "add-folder" ? projectError : null}
        onClose={() => {
          if (!projectBusy) {
            firstRunProjectPickPendingRef.current = false;
            setProjectModal(null);
            setProjectError(null);
          }
        }}
        onError={setProjectError}
        onPathChange={setProjectPathDraft}
        onSubmit={(path) => void handleAddFolder(path)}
        open={projectModal === "add-folder"}
        path={projectPathDraft}
        pending={projectBusy}
      />

      <ProjectCloneModal
        destParent={cloneDestParentDraft}
        error={projectModal === "clone-github" ? projectError : null}
        onClose={() => {
          if (!projectBusy) {
            firstRunProjectPickPendingRef.current = false;
            setProjectModal(null);
            setProjectError(null);
          }
        }}
        onDestParentChange={setCloneDestParentDraft}
        onError={setProjectError}
        onSubmit={(url, destParent) => void handleCloneGithub(url, destParent)}
        onUrlChange={setCloneUrlDraft}
        open={projectModal === "clone-github"}
        pending={projectBusy}
        url={cloneUrlDraft}
      />

      <TrustPromptDialog
        error={projectError}
        onClose={() => {
          if (!trustBusy) {
            setTrustPrompt(null);
            setProjectError(null);
          }
        }}
        onDecision={(level) => void handleTrustDecision(level)}
        open={trustPrompt !== null}
        pending={trustBusy}
        project={trustPrompt}
      />

      <MobileRemoteHost />
      <AppDialogHost />
    </main>
  );
}

function ProjectPathModal({
  open: modalOpen,
  path,
  pending,
  error,
  onPathChange,
  onError,
  onSubmit,
  onClose,
}: {
  open: boolean;
  path: string;
  pending: boolean;
  error: string | null;
  onPathChange: (path: string) => void;
  onError: (error: string) => void;
  onSubmit: (path: string) => void;
  onClose: () => void;
}) {
  const t = useT();
  if (!modalOpen) {
    return null;
  }

  async function handleBrowse() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t("Choose a project folder"),
      });
      if (typeof selected === "string") {
        onPathChange(selected);
      }
    } catch (error) {
      onError(errorMessage(error));
    }
  }

  return (
    <ProjectModalFrame title={t("Add Folder")} onClose={onClose} pending={pending}>
      <form
        className="space-y-4"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(path);
        }}
      >
        <div className="block text-sm text-content-secondary">
          <label
            className="font-medium text-content-primary"
            htmlFor="project-path-input"
          >
            {t("Folder path")}
          </label>
          <div className="mt-2 flex gap-2">
            <input
              className="min-w-0 flex-1 rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none transition-colors duration-150 placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
              disabled={pending}
              id="project-path-input"
              onChange={(event) => onPathChange(event.target.value)}
              placeholder="/Users/you/programming/repo"
              value={path}
            />
            <button
              className="shrink-0 rounded-lg border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
              disabled={pending}
              onClick={() => void handleBrowse()}
              type="button"
            >
              {t("Browse…")}
            </button>
          </div>
        </div>
        {error ? <ProjectModalError error={error} /> : null}
        <ProjectModalActions
          pending={pending}
          pendingLabel={t("Adding...")}
          submitDisabled={!path.trim()}
          submitLabel={t("Add folder")}
          onClose={onClose}
        />
      </form>
    </ProjectModalFrame>
  );
}

function ProjectCloneModal({
  open: modalOpen,
  url,
  destParent,
  pending,
  error,
  onUrlChange,
  onDestParentChange,
  onError,
  onSubmit,
  onClose,
}: {
  open: boolean;
  url: string;
  destParent: string;
  pending: boolean;
  error: string | null;
  onUrlChange: (url: string) => void;
  onDestParentChange: (destParent: string) => void;
  onError: (error: string) => void;
  onSubmit: (url: string, destParent: string) => void;
  onClose: () => void;
}) {
  const t = useT();
  if (!modalOpen) {
    return null;
  }

  async function handleBrowse() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t("Choose a project folder"),
      });
      if (typeof selected === "string") {
        onDestParentChange(selected);
      }
    } catch (error) {
      onError(errorMessage(error));
    }
  }

  return (
    <ProjectModalFrame
      title={t("Clone From GitHub")}
      onClose={onClose}
      pending={pending}
    >
      <form
        className="space-y-4"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(url, destParent);
        }}
      >
        <label className="block text-sm text-content-secondary">
          <span className="font-medium text-content-primary">
            {t("Repository")}
          </span>
          <input
            className="mt-2 w-full rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none transition-colors duration-150 placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
            disabled={pending}
            onChange={(event) => onUrlChange(event.target.value)}
            placeholder="openai/codex or https://github.com/openai/codex.git"
            value={url}
          />
        </label>
        <div className="block text-sm text-content-secondary">
          <label
            className="font-medium text-content-primary"
            htmlFor="clone-dest-parent-input"
          >
            {t("Destination parent")}
          </label>
          <div className="mt-2 flex gap-2">
            <input
              className="min-w-0 flex-1 rounded-md border border-surface-border/30 bg-surface-raised px-3 py-2 text-sm text-content-primary outline-none transition-colors duration-150 placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
              disabled={pending}
              id="clone-dest-parent-input"
              onChange={(event) => onDestParentChange(event.target.value)}
              placeholder="~/programming"
              value={destParent}
            />
            <button
              className="shrink-0 rounded-lg border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
              disabled={pending}
              onClick={() => void handleBrowse()}
              type="button"
            >
              {t("Browse…")}
            </button>
          </div>
        </div>
        <div className="rounded-md border border-surface-border/25 bg-surface-raised/60 px-3 py-2 text-xs text-content-muted">
          {t(
            "The backend will prefer authenticated `gh repo clone`; otherwise it uses `git clone`.",
          )}
        </div>
        {pending ? (
          <div className="rounded-md border border-accent/25 bg-accent/10 px-3 py-2 text-xs text-accent-strong">
            {t("Cloning repository...")}
          </div>
        ) : null}
        {error ? <ProjectModalError error={error} /> : null}
        <ProjectModalActions
          pending={pending}
          pendingLabel={t("Cloning...")}
          submitDisabled={!url.trim()}
          submitLabel={t("Clone")}
          onClose={onClose}
        />
      </form>
    </ProjectModalFrame>
  );
}

function TrustPromptDialog({
  open,
  project,
  pending,
  error,
  onDecision,
  onClose,
}: {
  open: boolean;
  project: Project | null;
  pending: boolean;
  error: string | null;
  onDecision: (level: "trusted" | "untrusted") => void;
  onClose: () => void;
}) {
  const t = useT();
  if (!open || !project) {
    return null;
  }

  return (
    <ProjectModalFrame title={t("Trust Project")} onClose={onClose} pending={pending}>
      <div className="space-y-4 text-sm text-content-secondary">
        <div className="rounded-md border border-warning/30 bg-warning-surface/45 px-3 py-2 text-warning">
          {t(
            "Code and instructions in this folder can influence agent behavior. A malicious project can use prompt injection to try to make the model run commands you did not intend.",
          )}
        </div>
        <div className="space-y-1">
          <div className="font-medium text-content-primary">{project.name}</div>
          <code className="block truncate rounded-md border border-surface-border/25 bg-surface-raised px-2 py-1 text-xs text-content-secondary">
            {project.rootPath}
          </code>
        </div>
        {error ? <ProjectModalError error={error} /> : null}
        <div className="flex flex-wrap justify-end gap-2">
          <button
            className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={pending}
            onClick={onClose}
            type="button"
          >
            {t("Cancel")}
          </button>
          <button
            className="rounded-md border border-warning/35 bg-warning-surface/45 px-3 py-2 text-sm font-medium text-warning transition-colors duration-150 hover:bg-warning-surface focus:outline-none focus:ring-2 focus:ring-warning/30 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={pending}
            onClick={() => onDecision("untrusted")}
            type="button"
          >
            {t("Mark untrusted")}
          </button>
          <button
            className="rounded-md bg-accent px-3 py-2 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
            disabled={pending}
            onClick={() => onDecision("trusted")}
            type="button"
          >
            {t("Trust")}
          </button>
        </div>
      </div>
    </ProjectModalFrame>
  );
}

function BranchStatusChip({ status }: { status: BranchStatus | null }) {
  const t = useT();
  const label = status?.branch ?? (status?.isGitRepo ? t("detached") : t("no git"));
  return (
    <div
      className="flex min-w-0 max-w-56 items-center gap-2 rounded-lg px-2 py-1.5 text-xs text-content-secondary"
      title={status?.warning ?? undefined}
    >
      <span
        className={`h-2 w-2 shrink-0 rounded-full ${
          status?.warning ? "bg-warning" : "bg-accent"
        }`}
        aria-hidden
      />
      <span className="min-w-0 truncate font-mono">{label}</span>
      {status?.autoCreatedBranch ? (
        <span className="ml-1 text-accent-strong">{t("auto")}</span>
      ) : null}
      {status?.clean === false ? (
        <span className="shrink-0 rounded-full bg-warning-surface/70 px-2 py-0.5 font-mono text-[10px] text-warning">
          {t("dirty")}
        </span>
      ) : null}
    </div>
  );
}

function ProjectModalFrame({
  title,
  pending,
  children,
  onClose,
}: {
  title: string;
  pending: boolean;
  children: ReactNode;
  onClose: () => void;
}) {
  const t = useT();
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-surface-sunken/70 px-4">
      <div className="w-full max-w-lg rounded-md border border-surface-border/30 bg-surface-glass p-4 shadow-2xl shadow-[rgb(var(--shadow-color)/0.28)] backdrop-blur-xl">
        <div className="mb-4 flex items-center justify-between gap-3">
          <h2 className="text-base font-semibold text-content-primary">{title}</h2>
          <button
            className="rounded-md border border-surface-border/30 px-2 py-1 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
            disabled={pending}
            onClick={onClose}
            type="button"
          >
            {t("Close")}
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

function ProjectModalError({ error }: { error: string }) {
  return (
    <div className="max-h-32 overflow-y-auto whitespace-pre-wrap rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger">
      {error}
    </div>
  );
}

function ProjectModalActions({
  pending,
  pendingLabel,
  submitDisabled,
  submitLabel,
  onClose,
}: {
  pending: boolean;
  pendingLabel: string;
  submitDisabled: boolean;
  submitLabel: string;
  onClose: () => void;
}) {
  const t = useT();
  return (
    <div className="flex justify-end gap-2">
      <button
        className="rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-50"
        disabled={pending}
        onClick={onClose}
        type="button"
      >
        {t("Cancel")}
      </button>
      <button
        className="rounded-md bg-accent px-3 py-2 text-sm font-semibold text-accent-contrast transition-colors duration-150 hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
        disabled={pending || submitDisabled}
        type="submit"
      >
        {pending ? pendingLabel : submitLabel}
      </button>
    </div>
  );
}

export default App;

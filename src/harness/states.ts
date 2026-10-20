import { MARKETING_WORKSPACE_FLAGS } from "./marketing-workspace.ts";

export const VISUAL_STATE_NAMES = [
  "welcome",
  "connect-gate",
  "connect-gate-pending",
  "connect-gate-failed",
  "connect-gate-keys",
  "first-run-fresh",
  "first-run-codex",
  "chat-not-connected",
  "chat-active",
  "savings-dashboard",
  "paywall",
  "returning-signed-out",
  "status-bar-orca",
] as const;

export type VisualStateName = (typeof VISUAL_STATE_NAMES)[number];

export const MARKETING_STATE_NAMES = [
  "marketing-chat-dark",
  "marketing-chat-light",
  "marketing-pro-dark",
  "marketing-pro-light",
  "marketing-app-dark",
  "marketing-app-light",
  "marketing-pro-split-dark",
  "marketing-pro-split-light",
  "marketing-subagents-dark",
  "marketing-subagents-light",
  "marketing-crux-dark",
  "marketing-crux-light",
  "marketing-savings",
  "marketing-savings-dark",
  "marketing-savings-light",
  "marketing-ghostty-dark",
  "marketing-ghostty-light",
  "marketing-workspace-dark",
  "marketing-workspace-light",
] as const;

export type MarketingStateName = (typeof MARKETING_STATE_NAMES)[number];

// Feature states have their own visual specs, outside the generic suite.
export const FEATURE_STATE_NAMES = [
  "agent-status-dark",
  "agent-status-light",
  "project-tree-dark",
  "project-tree-light",
  "project-tree-new-dark",
  "project-tree-new-light",
  "workspace-tabs-dark",
  "workspace-tabs-light",
  "agent-glyphs-dark",
  "agent-glyphs-light",
  "account-restart-dark",
  "account-restart-light",
  "account-swap-dark",
  "account-swap-light",
  "account-rotation-busy-dark",
] as const;

export type FeatureStateName = (typeof FEATURE_STATE_NAMES)[number];
export type HarnessStateName =
  | VisualStateName
  | MarketingStateName
  | FeatureStateName;

// ?update=<state> seeds the app update snapshot (see app-update-fixtures.ts).
export const APP_UPDATE_HARNESS_STATES = [
  "checking",
  "uptodate",
  "available",
  "downloading",
  "ready",
  "busy",
  "blockers",
  "waiting",
  "installing",
  "error",
  "updated",
  "windows",
  "no-supervisor",
  "read-only",
] as const;

export type AppUpdateHarnessState = (typeof APP_UPDATE_HARNESS_STATES)[number];

export type HarnessScenario =
  | "normal"
  | "fresh"
  | "paywall"
  | "usage-limit"
  | "proxy-error";

export type HarnessFixtureConfig = {
  scenario: HarnessScenario;
  account: "signed-in" | "paywall" | "signed-out";
  credentials: "chatgpt" | "codex" | "api-key" | "none";
  history: "full" | "empty" | "marketing" | "marketing-pro";
  sidecar: "running" | "missing" | "error";
  // "none" is a new install: no saving yet, so no trial card (ADR 0018).
  savings: "standard" | "aggressive" | "none";
  apiRequests: "standard" | "none";
  chatGptLogin: "success" | "pending" | "failure";
  marketingProSplit?: boolean;
  marketingTerminalScene?: "pro-split" | "crux";
  marketingSubagents?: boolean;
  marketingSavings?: boolean;
  // The 0.8.0 hero of halv.ai: a folder group, workspace tabs and a split
  // of Claude Code and Codex (marketing-workspace.ts).
  marketingWorkspace?: boolean;
  // "orca" seeds a wallet that the Orca app owns, with several accounts.
  // "orca-limit" puts the active Claude account near its 5-hour limit.
  // "orca-restart" seeds agent panes that still run the old accounts.
  wallet?: "halv" | "orca" | "orca-limit" | "orca-restart";
  // "none" starts with no project, as a new viewer does.
  projects?: "none";
  update?: AppUpdateHarnessState;
  // Seeds one agent pane in each status.
  agentStatus?: boolean;
  // Seeds two worktrees of project 1, with agent panes in each workspace.
  worktrees?: boolean;
  // Adds an agent conversation of project 1 with no live session (the
  // workspace strip specs).
  parkedConversation?: boolean;
  // Adds three agents of project 1: a read finished turn, a pane without a
  // signal, and a failed turn (the agent glyph specs).
  glyphShowcase?: boolean;
};

export type HarnessBootstrapAction =
  | "none"
  | "connect-pending"
  | "connect-failed"
  | "connect-keys"
  | "disconnect-api-key"
  | "open-chat"
  | "open-marketing-chat"
  | "open-marketing-app"
  | "open-marketing-pro-split"
  | "open-marketing-subagents"
  | "open-marketing-crux"
  | "open-marketing-savings"
  | "open-marketing-ghostty"
  | "open-marketing-workspace"
  | "open-agent-sessions"
  | "open-savings"
  | "open-status-bar-wallet"
  | "open-agent-status";

export type HarnessStateDefinition = {
  fixture: HarnessFixtureConfig;
  onboarding: "available" | "dismissed" | "preserve";
  bootstrap: HarnessBootstrapAction;
  readyText: string | null;
  theme?: "dark" | "light";
  agentSessionProMode?: boolean;
  welcome?: "seen" | "unseen";
  // The feature flags when the URL has no ?flags=, for example
  // "projectTree,workspaceTabs". Absent: every flag is off.
  flags?: string;
};

export type HarnessSelection = {
  name: string;
  definition: HarnessStateDefinition;
  visual: boolean;
  // ?flags=projectTree,workspaceTabs sets the feature flags (ADR 0010).
  flags?: string | null;
};

export const FIXED_VISUAL_NOW = "2026-08-13T15:00:00.000Z";

const RUNNING_FIXTURE = {
  scenario: "normal",
  account: "signed-in",
  credentials: "chatgpt",
  history: "full",
  sidecar: "running",
  savings: "standard",
  apiRequests: "standard",
  chatGptLogin: "success",
} as const satisfies HarnessFixtureConfig;

const AGENT_STATUS_FIXTURE = {
  ...RUNNING_FIXTURE,
  history: "marketing-pro",
  savings: "aggressive",
  apiRequests: "none",
  marketingProSplit: true,
  agentStatus: true,
} as const satisfies HarnessFixtureConfig;

const PROJECT_TREE_FIXTURE = {
  ...RUNNING_FIXTURE,
  worktrees: true,
} as const satisfies HarnessFixtureConfig;

// A new viewer: no project and no conversation.
const NEW_VIEWER_FIXTURE = {
  ...RUNNING_FIXTURE,
  history: "empty",
  projects: "none",
} as const satisfies HarnessFixtureConfig;

const WORKSPACE_TABS_FIXTURE = {
  ...PROJECT_TREE_FIXTURE,
  parkedConversation: true,
} as const satisfies HarnessFixtureConfig;

const AGENT_GLYPHS_FIXTURE = {
  ...WORKSPACE_TABS_FIXTURE,
  glyphShowcase: true,
} as const satisfies HarnessFixtureConfig;

const MARKETING_WORKSPACE_FIXTURE = {
  ...RUNNING_FIXTURE,
  savings: "aggressive",
  apiRequests: "none",
  marketingWorkspace: true,
} as const satisfies HarnessFixtureConfig;

const CONNECT_FIXTURE = {
  ...RUNNING_FIXTURE,
  credentials: "none",
  history: "empty",
  savings: "none",
} as const satisfies HarnessFixtureConfig;

export const HARNESS_STATES = {
  welcome: {
    fixture: { ...CONNECT_FIXTURE, account: "signed-out" },
    onboarding: "available",
    bootstrap: "none",
    readyText: "More getting it done.",
    welcome: "unseen",
  },
  "connect-gate": {
    fixture: CONNECT_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "Bring the account you already use.",
  },
  "connect-gate-pending": {
    fixture: { ...CONNECT_FIXTURE, chatGptLogin: "pending" },
    onboarding: "dismissed",
    bootstrap: "connect-pending",
    readyText: "Waiting for ChatGPT…",
  },
  "connect-gate-failed": {
    fixture: { ...CONNECT_FIXTURE, chatGptLogin: "failure" },
    onboarding: "dismissed",
    bootstrap: "connect-failed",
    readyText: "ChatGPT connection was not completed",
  },
  "connect-gate-keys": {
    fixture: CONNECT_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "connect-keys",
    readyText: "Anthropic API key",
  },
  "first-run-fresh": {
    fixture: {
      ...RUNNING_FIXTURE,
      credentials: "api-key",
      history: "empty",
      savings: "none",
    },
    onboarding: "available",
    bootstrap: "none",
    readyText: "Make this useful for you.",
  },
  "first-run-codex": {
    fixture: {
      ...RUNNING_FIXTURE,
      account: "signed-out",
      credentials: "codex",
      history: "empty",
      savings: "none",
    },
    onboarding: "available",
    bootstrap: "none",
    readyText: "Make this useful for you.",
  },
  "chat-not-connected": {
    fixture: {
      ...RUNNING_FIXTURE,
      credentials: "api-key",
      history: "empty",
      savings: "none",
    },
    onboarding: "dismissed",
    bootstrap: "disconnect-api-key",
    readyText: "Not connected",
  },
  "chat-active": {
    fixture: { ...RUNNING_FIXTURE, savings: "aggressive" },
    onboarding: "dismissed",
    bootstrap: "open-chat",
    readyText: "Use a root HTML entry in development",
  },
  "marketing-chat-dark": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing",
      savings: "aggressive",
      apiRequests: "none",
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-chat",
    readyText: "Shared chunks stay stable",
    theme: "dark",
  },
  "marketing-chat-light": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing",
      savings: "aggressive",
      apiRequests: "none",
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-chat",
    readyText: "Shared chunks stay stable",
    theme: "light",
  },
  "marketing-pro-dark": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
    },
    onboarding: "dismissed",
    bootstrap: "open-agent-sessions",
    readyText: "Review the release checklist",
    theme: "dark",
    agentSessionProMode: true,
  },
  "marketing-pro-light": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
    },
    onboarding: "dismissed",
    bootstrap: "open-agent-sessions",
    readyText: "Review the release checklist",
    theme: "light",
    agentSessionProMode: true,
  },
  "marketing-app-dark": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
      marketingProSplit: true,
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-app",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "marketing-app-light": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
      marketingProSplit: true,
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-app",
    readyText: null,
    theme: "light",
    agentSessionProMode: true,
  },
  "marketing-pro-split-dark": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
      marketingTerminalScene: "pro-split",
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-pro-split",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "marketing-pro-split-light": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
      marketingTerminalScene: "pro-split",
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-pro-split",
    readyText: null,
    theme: "light",
    agentSessionProMode: true,
  },
  "marketing-subagents-dark": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing",
      savings: "aggressive",
      apiRequests: "none",
      marketingSubagents: true,
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-subagents",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "marketing-subagents-light": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing",
      savings: "aggressive",
      apiRequests: "none",
      marketingSubagents: true,
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-subagents",
    readyText: null,
    theme: "light",
    agentSessionProMode: true,
  },
  "marketing-crux-dark": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
      marketingTerminalScene: "crux",
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-crux",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "marketing-crux-light": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
      marketingTerminalScene: "crux",
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-crux",
    readyText: null,
    theme: "light",
    agentSessionProMode: true,
  },
  "marketing-savings": {
    fixture: {
      ...RUNNING_FIXTURE,
      savings: "aggressive",
      apiRequests: "none",
      marketingSavings: true,
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-savings",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "marketing-savings-dark": {
    fixture: {
      ...RUNNING_FIXTURE,
      savings: "aggressive",
      apiRequests: "none",
      marketingSavings: true,
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-savings",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "marketing-savings-light": {
    fixture: {
      ...RUNNING_FIXTURE,
      savings: "aggressive",
      apiRequests: "none",
      marketingSavings: true,
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-savings",
    readyText: null,
    theme: "light",
    agentSessionProMode: true,
  },
  "marketing-ghostty-dark": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
      marketingProSplit: true,
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-ghostty",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "marketing-ghostty-light": {
    fixture: {
      ...RUNNING_FIXTURE,
      history: "marketing-pro",
      savings: "aggressive",
      apiRequests: "none",
      marketingProSplit: true,
    },
    onboarding: "dismissed",
    bootstrap: "open-marketing-ghostty",
    readyText: null,
    theme: "light",
    agentSessionProMode: true,
  },
  "marketing-workspace-dark": {
    fixture: MARKETING_WORKSPACE_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "open-marketing-workspace",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
    flags: MARKETING_WORKSPACE_FLAGS,
  },
  "marketing-workspace-light": {
    fixture: MARKETING_WORKSPACE_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "open-marketing-workspace",
    readyText: null,
    theme: "light",
    agentSessionProMode: true,
    flags: MARKETING_WORKSPACE_FLAGS,
  },
  "savings-dashboard": {
    fixture: { ...RUNNING_FIXTURE, savings: "aggressive" },
    onboarding: "dismissed",
    bootstrap: "open-savings",
    readyText: "2.8M",
  },
  paywall: {
    fixture: { ...RUNNING_FIXTURE, scenario: "paywall", account: "paywall" },
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "audit.trial@example.com",
  },
  // A signed-out user who comes back reads the app, not a wall (ADR 0018).
  "returning-signed-out": {
    fixture: {
      ...RUNNING_FIXTURE,
      account: "signed-out",
      history: "full",
    },
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "Streaming markdown polish",
  },
  "status-bar-orca": {
    fixture: { ...RUNNING_FIXTURE, wallet: "orca" },
    onboarding: "dismissed",
    bootstrap: "open-status-bar-wallet",
    readyText: "Synced with Orca",
  },
  "account-restart-dark": {
    fixture: { ...AGENT_STATUS_FIXTURE, wallet: "orca-restart" },
    onboarding: "dismissed",
    bootstrap: "open-agent-status",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "account-restart-light": {
    fixture: { ...AGENT_STATUS_FIXTURE, wallet: "orca-restart" },
    onboarding: "dismissed",
    bootstrap: "open-agent-status",
    readyText: null,
    theme: "light",
    agentSessionProMode: true,
  },
  "account-swap-dark": {
    fixture: { ...RUNNING_FIXTURE, wallet: "orca-limit" },
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: null,
    theme: "dark",
  },
  "account-swap-light": {
    fixture: { ...RUNNING_FIXTURE, wallet: "orca-limit" },
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: null,
    theme: "light",
  },
  // The Claude account near its limit while two Claude panes are busy.
  "account-rotation-busy-dark": {
    fixture: { ...AGENT_STATUS_FIXTURE, wallet: "orca-limit" },
    onboarding: "dismissed",
    bootstrap: "open-agent-status",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "agent-status-dark": {
    fixture: AGENT_STATUS_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "open-agent-status",
    readyText: null,
    theme: "dark",
    agentSessionProMode: true,
  },
  "agent-status-light": {
    fixture: AGENT_STATUS_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "open-agent-status",
    readyText: null,
    theme: "light",
    agentSessionProMode: true,
  },
  "project-tree-dark": {
    fixture: PROJECT_TREE_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "Docs refresh sweep",
    theme: "dark",
  },
  "project-tree-light": {
    fixture: PROJECT_TREE_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "Docs refresh sweep",
    theme: "light",
  },
  // A new viewer on the Projects view: no project, no conversation, and
  // no Recent list. Open it with ?flags=projectTree.
  "project-tree-new-dark": {
    fixture: NEW_VIEWER_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "No projects yet",
    theme: "dark",
  },
  "project-tree-new-light": {
    fixture: NEW_VIEWER_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "No projects yet",
    theme: "light",
  },
  // The workspace strip (ADR 0010) in Pro mode, on the Projects tree
  // fixture: three workspaces, live agents, and chat conversations.
  "workspace-tabs-dark": {
    fixture: WORKSPACE_TABS_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "Docs refresh sweep",
    theme: "dark",
    agentSessionProMode: true,
  },
  "workspace-tabs-light": {
    fixture: WORKSPACE_TABS_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "Docs refresh sweep",
    theme: "light",
    agentSessionProMode: true,
  },
  // Every agent glyph (Orca's vocabulary) in the tree, the chips and the
  // strip. Open it with ?flags=projectTree,workspaceTabs.
  "agent-glyphs-dark": {
    fixture: AGENT_GLYPHS_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "Docs refresh sweep",
    theme: "dark",
    agentSessionProMode: true,
  },
  "agent-glyphs-light": {
    fixture: AGENT_GLYPHS_FIXTURE,
    onboarding: "dismissed",
    bootstrap: "none",
    readyText: "Docs refresh sweep",
    theme: "light",
    agentSessionProMode: true,
  },
} as const satisfies Record<HarnessStateName, HarnessStateDefinition>;

const LEGACY_SCENARIOS: Record<HarnessScenario, HarnessStateDefinition> = {
  normal: {
    fixture: RUNNING_FIXTURE,
    onboarding: "preserve",
    bootstrap: "none",
    readyText: null,
  },
  fresh: {
    fixture: {
      ...RUNNING_FIXTURE,
      scenario: "fresh",
      credentials: "none",
      history: "empty",
      sidecar: "missing",
    },
    onboarding: "preserve",
    bootstrap: "none",
    readyText: null,
  },
  paywall: {
    fixture: { ...RUNNING_FIXTURE, scenario: "paywall", account: "paywall" },
    onboarding: "preserve",
    bootstrap: "none",
    readyText: null,
  },
  "usage-limit": {
    fixture: { ...RUNNING_FIXTURE, scenario: "usage-limit" },
    onboarding: "preserve",
    bootstrap: "none",
    readyText: null,
  },
  "proxy-error": {
    fixture: {
      ...RUNNING_FIXTURE,
      scenario: "proxy-error",
      sidecar: "error",
    },
    onboarding: "preserve",
    bootstrap: "none",
    readyText: null,
  },
};

export function readHarnessSelection(search: string): HarnessSelection {
  const params = new URLSearchParams(search);
  const requestedState = params.get("state");
  const update = readAppUpdateHarnessState(params.get("update"));
  const flags = params.get("flags");
  if (requestedState !== null) {
    if (isHarnessStateName(requestedState)) {
      const definition: HarnessStateDefinition = HARNESS_STATES[requestedState];
      return {
        flags: flags ?? definition.flags ?? null,
        name: requestedState,
        definition: {
          ...definition,
          ...(params.get("theme") === "light" || params.get("theme") === "dark"
            ? { theme: params.get("theme") as "light" | "dark" }
            : {}),
          ...(update ? { fixture: { ...definition.fixture, update } } : {}),
        },
        visual: true,
      };
    }
    throw new Error(`[ux-audit] Unknown state: ${requestedState}`);
  }

  const requestedScenario = params.get("scenario") ?? "normal";
  const scenario = isHarnessScenario(requestedScenario)
    ? requestedScenario
    : "normal";
  const definition = LEGACY_SCENARIOS[scenario];
  return {
    flags,
    name: scenario,
    definition: update
      ? { ...definition, fixture: { ...definition.fixture, update } }
      : definition,
    visual: false,
  };
}

function readAppUpdateHarnessState(value: string | null) {
  if (value === null) return undefined;
  if ((APP_UPDATE_HARNESS_STATES as readonly string[]).includes(value)) {
    return value as AppUpdateHarnessState;
  }
  throw new Error(`[ux-audit] Unknown update state: ${value}`);
}

function isHarnessStateName(value: string): value is HarnessStateName {
  return (
    (VISUAL_STATE_NAMES as readonly string[]).includes(value) ||
    (MARKETING_STATE_NAMES as readonly string[]).includes(value) ||
    (FEATURE_STATE_NAMES as readonly string[]).includes(value)
  );
}

function isHarnessScenario(value: string): value is HarnessScenario {
  return Object.prototype.hasOwnProperty.call(LEGACY_SCENARIOS, value);
}

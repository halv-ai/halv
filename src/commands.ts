import type { ChatMode } from "./modes";
import type { Translate } from "./i18n/runtime";
import { parseMemoriesCommand } from "./memoriesCommand";
import { parseFastCommand } from "./fastCommand";
import {
  expandCustomPrompt,
  resolveCustomPromptNames,
  type CustomPrompt,
} from "./customPrompts";
import type {
  ChatModel,
  ConversationExportFormat,
  PersonalityPreference,
  PonytailMode,
  Project,
  WorkflowMode,
} from "./types";
import { isThemePreference, type ThemePreference } from "./theme";
import { featureFlagStore } from "./featureFlags";
import { mobileConnectStore } from "./mobileConnect";

export type CommandCategory =
  | "Conversation"
  | "Workspace"
  | "Agent"
  | "Appearance"
  | "Input"
  | "Application"
  | "Labs";

export type CommandHandlerContext = {
  args: string;
  models: ChatModel[];
  selectedModel: string;
  selectedEffort: string;
  projects: Project[];
  currentProject: Project | null;
  currentConversationId: string | null;
  currentConversationStreaming: boolean;
  latestUserForkSequence: number | null;
  completedTurns: number;
  chatMode: ChatMode;
  workflowMode: WorkflowMode;
  /**
   * The Halv Engine switch is off and the engine reports no error. The
   * restart command then stays unavailable, so it never starts the engine
   * behind the switch.
   */
  halvEngineOff?: boolean;
  setDraft: (draft: string) => void;
  setModel: (model: string) => void;
  setEffort: (effort: string) => void;
  setFastMode: (enabled: boolean) => Promise<void>;
  setThemePreference: (theme: ThemePreference) => void;
  setGoal: (goal: string | null) => Promise<void>;
  setChatMode: (mode: Exclude<ChatMode, "legacy_restricted">) => Promise<void>;
  setWorkflowMode: (mode: WorkflowMode) => Promise<void>;
  startNewConversation: () => void;
  openProjectSwitcher: () => void;
  switchProjectByFragment: (fragment: string) => Promise<void>;
  openSettings: () => void;
  openKeyboardShortcuts: () => void;
  openExtensions: () => void;
  openAutomations: () => void;
  toggleTerminal: () => void;
  openShell: () => void;
  openReview: () => void;
  openBrowser: () => void;
  startDictation: () => void;
  restartProxy: () => Promise<void>;
  createDraftPr: () => Promise<void>;
  exportConversation: (format: ConversationExportFormat) => Promise<void>;
  renameConversation: (title: string) => Promise<void>;
  deleteConversation: () => Promise<void>;
  archiveConversation: () => Promise<void>;
  compactConversation: () => Promise<void>;
  forkConversation: () => Promise<void>;
  copyLastResponse: () => Promise<void>;
  approveLastDenied: () => Promise<void>;
  setPersonality: (personality: PersonalityPreference | null) => Promise<void>;
  setPonytail: (mode: PonytailMode | null) => Promise<void>;
  openMemories: () => void;
  setConversationMemoriesMode: (mode: "off" | "on") => Promise<void>;
  showStatus: () => void;
  showFastMode: () => void;
  showDoctor: () => Promise<void>;
  showUsage: () => void;
  focusSubagents: () => void;
  insertMention: (path: string) => void;
  sendPrompt: (prompt: string) => Promise<void>;
  focusConversationSearch: () => void;
  openCommandPalette: () => void;
  logout: () => Promise<void>;
  quit: () => Promise<void>;
};

export type CommandRegistryEntry = {
  id: string;
  name: string;
  argsHint: string;
  description: string;
  category: CommandCategory;
  aliases?: string[];
  keywords?: string[];
  shortcut?: string;
  shortcutActionId?: string;
  customPrompt?: Pick<CustomPrompt, "name" | "scope">;
  unavailableReason?: (context: CommandHandlerContext) => string | null;
  handler: (context: CommandHandlerContext) => void | Promise<void>;
};

export function commandCategoryLabel(
  category: CommandCategory,
  t: Translate,
) {
  switch (category) {
    case "Conversation":
      return t("Conversation");
    case "Workspace":
      return t("Workspace");
    case "Agent":
      return t("Agent");
    case "Appearance":
      return t("Appearance");
    case "Input":
      return t("Input");
    case "Application":
      return t("Application");
    case "Labs":
      return t("Labs");
  }
}

export function commandUnavailableReasonLabel(reason: string, t: Translate) {
  switch (reason) {
    case "Select a project first":
      return t("Select a project first");
    case "Open a conversation first":
      return t("Open a conversation first");
    case "Mobile is a Labs feature. Run /labs-mobile to turn it on.":
      return t("Mobile is a Labs feature. Run /labs-mobile to turn it on.");
    case "Wait for the current turn to finish before compacting context":
      return t("Wait for the current turn to finish before compacting context");
    case "At least 2 completed turns are required before compacting context":
      return t("At least 2 completed turns are required before compacting context");
    case "Wait for the current turn to finish before forking":
      return t("Wait for the current turn to finish before forking");
    case "No persisted user turn is available to fork":
      return t("No persisted user turn is available to fork");
    case "Choose Auto or Ask first":
      return t("Choose Auto or Ask first");
    case "Halv Engine is off. Turn it on in Settings.":
      return t("Halv Engine is off. Turn it on in Settings.");
    default:
      return reason;
  }
}

export function commandErrorLabel(message: string, t: Translate) {
  if (message === "Usage: /plan [on|off]") {
    return t("Usage: /plan [on|off]");
  }
  const usage = /^Usage: \/(\S+) requires (.+)\.$/.exec(message);
  if (usage) {
    return t("Usage: /{name} requires {argsHint}.", {
      name: usage[1],
      argsHint: usage[2],
    });
  }
  const unknownExportFormat = /^Unknown export format: (.+)$/.exec(message);
  if (unknownExportFormat) {
    return t("Unknown export format: {format}", {
      format: unknownExportFormat[1],
    });
  }
  const unknownModel = /^Unknown model: (.+)$/.exec(message);
  if (unknownModel) {
    return t("Unknown model: {model}", { model: unknownModel[1] });
  }
  const unknownEffort = /^Unknown effort: (.+)$/.exec(message);
  if (unknownEffort) {
    return t("Unknown effort: {effort}", { effort: unknownEffort[1] });
  }
  const unknownMode = /^Unknown mode: (.+)$/.exec(message);
  if (unknownMode) {
    return t("Unknown mode: {mode}", { mode: unknownMode[1] });
  }
  const unknownPersonality = /^Unknown personality: (.+)$/.exec(message);
  if (unknownPersonality) {
    return t("Unknown personality: {personality}", {
      personality: unknownPersonality[1],
    });
  }
  const unknownTheme = /^Unknown theme: (.+)$/.exec(message);
  if (unknownTheme) {
    return t("Unknown theme: {theme}", { theme: unknownTheme[1] });
  }
  return message;
}

const effortValues = ["low", "medium", "high", "xhigh"];
function requireArgs(name: string, args: string) {
  const value = args.trim();
  if (!value) {
    throw new Error(`Usage: /${name} requires ${commandArgsHint(name)}.`);
  }
  return value;
}

function commandArgsHint(name: string) {
  return commandRegistry.find((command) => command.name === name)?.argsHint ?? "arguments";
}

function requiresProject(context: CommandHandlerContext) {
  return context.currentProject ? null : "Select a project first";
}

function requiresConversation(context: CommandHandlerContext) {
  return context.currentConversationId ? null : "Open a conversation first";
}

function compactUnavailableReason(context: CommandHandlerContext) {
  return (
    requiresConversation(context) ??
    (context.currentConversationStreaming
      ? "Wait for the current turn to finish before compacting context"
      : context.completedTurns < 2
        ? "At least 2 completed turns are required before compacting context"
        : null)
  );
}

function forkUnavailableReason(context: CommandHandlerContext) {
  return (
    requiresConversation(context) ??
    (context.currentConversationStreaming
      ? "Wait for the current turn to finish before forking"
      : context.latestUserForkSequence === null
        ? "No persisted user turn is available to fork"
        : null)
  );
}

function modeUnavailableReason(context: CommandHandlerContext) {
  return context.args.trim().toLowerCase() === "chat"
    ? null
    : requiresProject(context);
}

export const commandRegistry: CommandRegistryEntry[] = [
  {
    id: "conversation.goal",
    name: "goal",
    argsHint: "<text> | clear",
    description: "Set or clear the standing goal for this conversation.",
    category: "Conversation",
    keywords: ["objective", "autonomous"],
    async handler({ args, setGoal }) {
      const goal = requireArgs("goal", args);
      await setGoal(goal.toLowerCase() === "clear" ? null : goal);
    },
  },
  {
    id: "conversation.new",
    name: "clear",
    argsHint: "",
    description: "Start a fresh conversation.",
    category: "Conversation",
    aliases: ["new"],
    shortcut: "⌘N",
    shortcutActionId: "task.new",
    handler({ startNewConversation }) {
      startNewConversation();
    },
  },
  {
    id: "conversation.compact",
    name: "compact",
    argsHint: "",
    description: "Summarize older context while keeping the full transcript visible.",
    category: "Conversation",
    keywords: ["context", "summary", "tokens"],
    unavailableReason: compactUnavailableReason,
    async handler({ compactConversation }) {
      await compactConversation();
    },
  },
  {
    id: "conversation.fork",
    name: "fork",
    argsHint: "",
    description: "Fork this conversation from its latest user turn.",
    category: "Conversation",
    keywords: ["branch", "copy", "history"],
    unavailableReason: forkUnavailableReason,
    async handler({ forkConversation }) {
      await forkConversation();
    },
  },
  {
    id: "conversation.search",
    name: "search",
    argsHint: "",
    description: "Focus full-text conversation search.",
    category: "Conversation",
    keywords: ["find", "history"],
    handler({ focusConversationSearch }) {
      focusConversationSearch();
    },
  },
  {
    id: "conversation.rename",
    name: "rename",
    argsHint: "<title>",
    description: "Rename the current conversation.",
    category: "Conversation",
    aliases: ["name"],
    unavailableReason: requiresConversation,
    async handler({ args, renameConversation }) {
      await renameConversation(requireArgs("rename", args));
    },
  },
  {
    id: "conversation.copy-last",
    name: "copy",
    argsHint: "",
    description: "Copy the latest assistant response.",
    category: "Conversation",
    aliases: ["copy-last"],
    unavailableReason: requiresConversation,
    async handler({ copyLastResponse }) {
      await copyLastResponse();
    },
  },
  {
    id: "conversation.delete",
    name: "delete",
    argsHint: "",
    description: "Delete the current conversation while preserving its savings totals.",
    category: "Conversation",
    aliases: ["delete-thread"],
    unavailableReason: requiresConversation,
    async handler({ deleteConversation }) {
      await deleteConversation();
    },
  },
  {
    id: "conversation.archive",
    name: "archive",
    argsHint: "",
    description: "Archive the current conversation.",
    category: "Conversation",
    unavailableReason: requiresConversation,
    async handler({ archiveConversation }) {
      await archiveConversation();
    },
  },
  {
    id: "conversation.export",
    name: "export",
    argsHint: "<markdown|json>",
    description: "Export the current conversation.",
    category: "Conversation",
    unavailableReason: requiresConversation,
    async handler({ args, exportConversation }) {
      const format = requireArgs("export", args).toLowerCase();
      if (format !== "markdown" && format !== "json") {
        throw new Error(`Unknown export format: ${format}`);
      }
      await exportConversation(format);
    },
  },
  {
    id: "agent.model",
    name: "model",
    argsHint: "<id>",
    description: "Switch the active model.",
    category: "Agent",
    keywords: ["provider", "gpt", "claude"],
    handler({ args, models, setModel }) {
      const requested = requireArgs("model", args);
      const model = models.find((item) => item.id === requested);
      if (!model) {
        throw new Error(`Unknown model: ${requested}`);
      }
      setModel(model.id);
    },
  },
  {
    id: "agent.effort",
    name: "effort",
    argsHint: "<model-supported-level>",
    description: "Switch reasoning effort.",
    category: "Agent",
    keywords: ["reasoning", "thinking"],
    handler({ args, selectedModel, models, setEffort }) {
      const requested = requireArgs("effort", args);
      const selectedModelInfo = models.find((model) => model.id === selectedModel);
      const allowed = selectedModelInfo?.reasoningEfforts ?? effortValues;
      if (!allowed.includes(requested)) {
        throw new Error(`Unknown effort: ${requested}`);
      }
      setEffort(requested);
    },
  },
  {
    id: "agent.fast",
    name: "fast",
    argsHint: "[on|off|status]",
    description: "Show or set Fast mode for this conversation.",
    category: "Agent",
    keywords: ["speed", "service tier", "credits"],
    async handler({ args, setFastMode, showFastMode }) {
      const action = parseFastCommand(args);
      if (action.kind === "status") {
        showFastMode();
        return;
      }
      await setFastMode(action.enabled);
    },
  },
  {
    id: "agent.mode",
    name: "mode",
    argsHint: "<chat|sandbox|ask|full_access>",
    description: "Choose chat-only, project sandbox, approval, or full-access mode.",
    category: "Agent",
    aliases: ["permissions"],
    unavailableReason: modeUnavailableReason,
    async handler({ args, setChatMode }) {
      const raw = requireArgs("mode", args).toLowerCase();
      const requested = raw === "full" || raw === "full-access" ? "full_access" : raw;
      if (!["chat", "sandbox", "ask", "full_access"].includes(requested)) {
        throw new Error(`Unknown mode: ${raw}`);
      }
      await setChatMode(requested as "chat" | "sandbox" | "ask" | "full_access");
    },
  },
  {
    id: "agent.personality",
    name: "personality",
    argsHint: "<none|friendly|pragmatic>",
    description: "Show or set the global response personality.",
    category: "Agent",
    async handler({ args, setPersonality }) {
      const requested = args.trim().toLowerCase();
      if (!requested) {
        await setPersonality(null);
        return;
      }
      if (!["none", "friendly", "pragmatic"].includes(requested)) {
        throw new Error(`Unknown personality: ${requested}`);
      }
      await setPersonality(requested as PersonalityPreference);
    },
  },
  {
    id: "agent.ponytail",
    name: "ponytail",
    argsHint: "<off|lite|full|ultra>",
    description: "Show or set the global Ponytail mode.",
    category: "Agent",
    async handler({ args, setPonytail }) {
      const requested = args.trim().toLowerCase();
      if (!requested) {
        await setPonytail(null);
        return;
      }
      if (!["off", "lite", "full", "ultra"].includes(requested)) {
        throw new Error(`Unknown Ponytail mode: ${requested}`);
      }
      await setPonytail(requested as PonytailMode);
    },
  },
  {
    id: "agent.memories",
    name: "memories",
    argsHint: "",
    description: "Manage memories or turn them on or off for this conversation.",
    category: "Agent",
    keywords: ["remember", "context", "preferences"],
    async handler({ args, openMemories, setConversationMemoriesMode }) {
      const action = parseMemoriesCommand(args);
      if (action.kind === "open") {
        openMemories();
        return;
      }
      await setConversationMemoriesMode(action.mode);
    },
  },
  {
    id: "agent.plan",
    name: "plan",
    argsHint: "",
    description: "Toggle persistent read-only Plan mode for this conversation.",
    category: "Agent",
    keywords: ["planning", "read-only", "tasks", "checklist"],
    async handler({ args, workflowMode, setWorkflowMode }) {
      const requested = args.trim().toLowerCase();
      if (requested && !["on", "off", "toggle"].includes(requested)) {
        throw new Error("Usage: /plan [on|off]");
      }
      const next =
        requested === "on"
          ? "plan"
          : requested === "off"
            ? "default"
            : workflowMode === "plan"
              ? "default"
              : "plan";
      await setWorkflowMode(next);
    },
  },
  {
    id: "agent.threads",
    name: "agent",
    argsHint: "",
    description: "Jump to the current conversation's inspectable subagent activity.",
    category: "Agent",
    aliases: ["subagents"],
    unavailableReason: requiresConversation,
    handler({ focusSubagents }) {
      focusSubagents();
    },
  },
  {
    id: "agent.status",
    name: "status",
    argsHint: "",
    description: "Show the current model, mode, project, and conversation state.",
    category: "Agent",
    handler({ showStatus }) {
      showStatus();
    },
  },
  {
    id: "agent.approve",
    name: "approve",
    argsHint: "",
    description: "Approve and ask the agent to retry the last denied action.",
    category: "Agent",
    unavailableReason: requiresConversation,
    async handler({ approveLastDenied }) {
      await approveLastDenied();
    },
  },
  {
    id: "agent.doctor",
    name: "doctor",
    argsHint: "",
    description: "Show safe configuration and connection diagnostics.",
    category: "Agent",
    aliases: ["debug-config"],
    keywords: ["diagnostics", "configuration", "health"],
    async handler({ showDoctor }) {
      await showDoctor();
    },
  },
  {
    id: "agent.usage",
    name: "usage",
    argsHint: "",
    description: "Show observed token, savings, API, and weekly plan usage.",
    category: "Agent",
    keywords: ["limits", "tokens", "cost"],
    handler({ showUsage }) {
      showUsage();
    },
  },
  {
    id: "agent.mention",
    name: "mention",
    argsHint: "<path>",
    description: "Insert a project file mention into the composer.",
    category: "Agent",
    unavailableReason: requiresProject,
    handler({ args, insertMention }) {
      insertMention(args.trim());
    },
  },
  {
    id: "input.dictation",
    name: "dictate",
    argsHint: "",
    description: "Start or stop speech dictation in the message composer.",
    category: "Input",
    keywords: ["speech", "voice", "microphone"],
    shortcut: "⌃⇧D",
    shortcutActionId: "input.dictation",
    handler({ startDictation }) {
      startDictation();
    },
  },
  {
    id: "workspace.init",
    name: "init",
    argsHint: "",
    description: "Ask the agent to create or improve this project's AGENTS.md.",
    category: "Workspace",
    unavailableReason(context) {
      return requiresProject(context) ??
        (context.chatMode === "chat" ? "Choose Auto or Ask first" : null);
    },
    async handler({ sendPrompt }) {
      await sendPrompt(
        "Create or improve the AGENTS.md for this project. Inspect the repository first, preserve useful existing instructions, and document only durable commands, conventions, and verification steps that future coding agents need.",
      );
    },
  },
  {
    id: "workspace.project",
    name: "project",
    argsHint: "<name-or-path>",
    description: "Switch the active project.",
    category: "Workspace",
    keywords: ["folder", "repository"],
    async handler({ args, openProjectSwitcher, switchProjectByFragment }) {
      const fragment = args.trim();
      if (!fragment) {
        openProjectSwitcher();
        return;
      }
      await switchProjectByFragment(fragment);
    },
  },
  {
    id: "workspace.terminal",
    name: "terminal",
    argsHint: "",
    description: "Open or close the persistent interactive terminal.",
    category: "Workspace",
    keywords: ["shell", "pty", "console"],
    shortcut: "⌘J",
    shortcutActionId: "workspace.toggle_bottom_panel",
    unavailableReason: requiresProject,
    handler({ toggleTerminal }) {
      toggleTerminal();
    },
  },
  {
    id: "workspace.new_shell",
    name: "shell",
    argsHint: "",
    description: "Open a shell beside the active terminal pane.",
    category: "Workspace",
    keywords: ["terminal", "shell", "split", "pane", "console"],
    unavailableReason: requiresProject,
    handler({ openShell }) {
      openShell();
    },
  },
  {
    id: "workspace.review",
    name: "review",
    argsHint: "",
    description: "Open the diff rail for the active terminal pane.",
    category: "Workspace",
    aliases: ["diff"],
    keywords: ["diff", "changes", "terminal", "pane"],
    unavailableReason: requiresProject,
    shortcutActionId: "workspace.open_review",
    handler({ openReview }) {
      openReview();
    },
  },
  {
    id: "workspace.browser",
    name: "browser",
    argsHint: "",
    description: "Open the shared browser for preview, interaction, and annotations.",
    category: "Workspace",
    keywords: ["web", "preview", "annotation", "computer use"],
    shortcut: "⌘⇧B",
    shortcutActionId: "workspace.open_browser",
    handler({ openBrowser }) {
      openBrowser();
    },
  },
  {
    id: "workspace.pull-request",
    name: "pr",
    argsHint: "",
    description: "Create a draft pull request for the current project.",
    category: "Workspace",
    aliases: ["pull-request"],
    unavailableReason: requiresProject,
    async handler({ createDraftPr }) {
      await createDraftPr();
    },
  },
  {
    id: "application.restart-proxy",
    name: "restart-proxy",
    argsHint: "",
    description: "Restart the proxy.",
    category: "Application",
    aliases: ["restart-headroom"],
    keywords: ["repair", "sidecar"],
    unavailableReason(context) {
      return context.halvEngineOff
        ? "Halv Engine is off. Turn it on in Settings."
        : null;
    },
    async handler({ restartProxy }) {
      await restartProxy();
    },
  },
  {
    id: "appearance.theme",
    name: "theme",
    argsHint: "<system|light|dark>",
    description: "Switch the app theme.",
    category: "Appearance",
    handler({ args, setThemePreference }) {
      const requested = requireArgs("theme", args);
      if (!isThemePreference(requested)) {
        throw new Error(`Unknown theme: ${requested}`);
      }
      setThemePreference(requested);
    },
  },
  {
    id: "application.automations",
    name: "automations",
    argsHint: "",
    description: "Schedule local work or hand tasks to Halv Cloud.",
    category: "Application",
    aliases: ["tasks"],
    keywords: ["schedule", "background", "cloud", "handoff"],
    handler({ openAutomations }) {
      openAutomations();
    },
  },
  {
    id: "application.extensions",
    name: "extensions",
    argsHint: "",
    description: "Manage skills, plugins, and MCP servers.",
    category: "Application",
    aliases: ["skills", "mcp", "apps", "plugins"],
    keywords: ["skills", "plugins", "mcp", "tools"],
    handler({ openExtensions }) {
      openExtensions();
    },
  },
  {
    id: "application.keyboard-shortcuts",
    name: "shortcuts",
    argsHint: "",
    description: "Find, customize, or reset keyboard shortcuts.",
    category: "Application",
    aliases: ["keyboard-shortcuts", "keybindings"],
    shortcut: "⌘⇧/",
    shortcutActionId: "application.keyboard_shortcuts",
    handler({ openKeyboardShortcuts }) {
      openKeyboardShortcuts();
    },
  },
  {
    id: "application.connect_phone",
    name: "connect-phone",
    argsHint: "",
    description: "Show the QR code that connects your phone.",
    category: "Application",
    aliases: ["phone", "mobile"],
    keywords: ["iphone", "android", "pair", "qr", "remote", "mobile"],
    unavailableReason() {
      return featureFlagStore.getSnapshot().mobileRemote
        ? null
        : "Mobile is a Labs feature. Run /labs-mobile to turn it on.";
    },
    handler() {
      mobileConnectStore.open();
    },
  },
  {
    id: "application.settings",
    name: "settings",
    argsHint: "",
    description: "Open application settings.",
    category: "Application",
    keywords: ["account", "api", "preferences"],
    shortcutActionId: "application.settings",
    handler({ openSettings }) {
      openSettings();
    },
  },
  {
    id: "application.logout",
    name: "logout",
    argsHint: "",
    description: "Disconnect the active ChatGPT session.",
    category: "Application",
    aliases: ["signout"],
    async handler({ logout }) {
      await logout();
    },
  },
  {
    id: "application.quit",
    name: "quit",
    argsHint: "",
    description: "Quit Halv.",
    category: "Application",
    aliases: ["exit"],
    async handler({ quit }) {
      await quit();
    },
  },
  {
    id: "application.commands",
    name: "help",
    argsHint: "",
    description: "Show every available command.",
    category: "Application",
    aliases: ["commands"],
    shortcut: "⌘K",
    shortcutActionId: "application.command_menu",
    handler({ openCommandPalette }) {
      openCommandPalette();
    },
  },
  // Labs commands turn staged features on or off (ADR 0010).
  {
    id: "labs.project_tree",
    name: "labs-projects",
    argsHint: "",
    description: "Turn the Projects tree in the sidebar on or off.",
    category: "Labs",
    keywords: ["labs", "experimental", "sidebar", "worktree", "tree"],
    handler() {
      featureFlagStore.toggle("projectTree");
    },
  },
  {
    id: "labs.workspace_tabs",
    name: "labs-tabs",
    argsHint: "",
    description: "Turn workspace tabs on or off.",
    category: "Labs",
    keywords: ["labs", "experimental", "tabs", "workspace"],
    handler() {
      featureFlagStore.toggle("workspaceTabs");
    },
  },
  {
    id: "labs.shell_tabs",
    name: "labs-shell-tabs",
    argsHint: "",
    description: "Turn shell tabs on or off.",
    category: "Labs",
    keywords: ["labs", "experimental", "tabs", "terminal", "shell"],
    handler() {
      featureFlagStore.toggle("shellTabs");
    },
  },
  {
    id: "labs.mobile_remote",
    name: "labs-mobile",
    argsHint: "",
    description: "Turn the phone remote on or off.",
    category: "Labs",
    keywords: ["labs", "experimental", "phone", "mobile", "remote", "iphone"],
    handler() {
      featureFlagStore.toggle("mobileRemote");
    },
  },
];

export function commandDescriptionLabel(
  command: CommandRegistryEntry,
  t: Translate,
) {
  switch (command.description) {
    case "Set or clear the standing goal for this conversation.":
      return t("Set or clear the standing goal for this conversation.");
    case "Start a fresh conversation.":
      return t("Start a fresh conversation.");
    case "Summarize older context while keeping the full transcript visible.":
      return t("Summarize older context while keeping the full transcript visible.");
    case "Fork this conversation from its latest user turn.":
      return t("Fork this conversation from its latest user turn.");
    case "Focus full-text conversation search.":
      return t("Focus full-text conversation search.");
    case "Rename the current conversation.":
      return t("Rename the current conversation.");
    case "Copy the latest assistant response.":
      return t("Copy the latest assistant response.");
    case "Delete the current conversation while preserving its savings totals.":
      return t("Delete the current conversation while preserving its savings totals.");
    case "Archive the current conversation.":
      return t("Archive the current conversation.");
    case "Export the current conversation.":
      return t("Export the current conversation.");
    case "Switch the active model.":
      return t("Switch the active model.");
    case "Switch reasoning effort.":
      return t("Switch reasoning effort.");
    case "Show or set Fast mode for this conversation.":
      return t("Show or set Fast mode for this conversation.");
    case "Choose chat-only, project sandbox, approval, or full-access mode.":
      return t("Choose chat-only, project sandbox, approval, or full-access mode.");
    case "Show or set the global response personality.":
      return t("Show or set the global response personality.");
    case "Manage memories or turn them on or off for this conversation.":
      return t("Manage memories or turn them on or off for this conversation.");
    case "Toggle persistent read-only Plan mode for this conversation.":
      return t("Toggle persistent read-only Plan mode for this conversation.");
    case "Jump to the current conversation's inspectable subagent activity.":
      return t("Jump to the current conversation's inspectable subagent activity.");
    case "Show the current model, mode, project, and conversation state.":
      return t("Show the current model, mode, project, and conversation state.");
    case "Approve and ask the agent to retry the last denied action.":
      return t("Approve and ask the agent to retry the last denied action.");
    case "Show safe configuration and connection diagnostics.":
      return t("Show safe configuration and connection diagnostics.");
    case "Show observed token, savings, API, and weekly plan usage.":
      return t("Show observed token, savings, API, and weekly plan usage.");
    case "Insert a project file mention into the composer.":
      return t("Insert a project file mention into the composer.");
    case "Start or stop speech dictation in the message composer.":
      return t("Start or stop speech dictation in the message composer.");
    case "Ask the agent to create or improve this project's AGENTS.md.":
      return t("Ask the agent to create or improve this project's AGENTS.md.");
    case "Switch the active project.":
      return t("Switch the active project.");
    case "Open or close the persistent interactive terminal.":
      return t("Open or close the persistent interactive terminal.");
    case "Open a shell beside the active terminal pane.":
      return t("Open a shell beside the active terminal pane.");
    case "Review working-tree changes and leave inline feedback.":
      return t("Review working-tree changes and leave inline feedback.");
    case "Open the shared browser for preview, interaction, and annotations.":
      return t("Open the shared browser for preview, interaction, and annotations.");
    case "Create a draft pull request for the current project.":
      return t("Create a draft pull request for the current project.");
    case "Restart the proxy.":
      return t("Restart the proxy.");
    case "Switch the app theme.":
      return t("Switch the app theme.");
    case "Schedule local work or hand tasks to Halv Cloud.":
      return t("Schedule local work or hand tasks to Halv Cloud.");
    case "Manage skills, plugins, and MCP servers.":
      return t("Manage skills, plugins, and MCP servers.");
    case "Find, customize, or reset keyboard shortcuts.":
      return t("Find, customize, or reset keyboard shortcuts.");
    case "Open application settings.":
      return t("Open application settings.");
    case "Disconnect the active ChatGPT session.":
      return t("Disconnect the active ChatGPT session.");
    case "Quit Halv.":
      return t("Quit Halv.");
    case "Show every available command.":
      return t("Show every available command.");
    case "Turn the Projects tree in the sidebar on or off.":
      return t("Turn the Projects tree in the sidebar on or off.");
    case "Turn workspace tabs on or off.":
      return t("Turn workspace tabs on or off.");
    case "Turn shell tabs on or off.":
      return t("Turn shell tabs on or off.");
    case "Turn the phone remote on or off.":
      return t("Turn the phone remote on or off.");
    case "Show the QR code that connects your phone.":
      return t("Show the QR code that connects your phone.");
    default:
      if (!command.customPrompt) {
        return command.description;
      }
      switch (command.customPrompt.scope) {
        case "user":
          return t("Run the user custom prompt “{name}”.", {
            name: command.customPrompt.name,
          });
        case "codex":
          return t("Run the codex custom prompt “{name}”.", {
            name: command.customPrompt.name,
          });
        case "project":
          return t("Run the project custom prompt “{name}”.", {
            name: command.customPrompt.name,
          });
      }
  }
}

export function commandRegistryWithCustomPrompts(
  prompts: CustomPrompt[],
): CommandRegistryEntry[] {
  const reservedNames = commandRegistry.flatMap((command) => [
    command.name,
    ...(command.aliases ?? []),
  ]);
  const customCommands = resolveCustomPromptNames(prompts, reservedNames).map(
    (prompt): CommandRegistryEntry => ({
      id: `custom-prompt.${prompt.scope}.${prompt.name.toLowerCase()}`,
      name: prompt.commandName,
      argsHint: "[arguments]",
      description: `Run the ${prompt.scope} custom prompt “${prompt.name}”.`,
      customPrompt: { name: prompt.name, scope: prompt.scope },
      category: "Input",
      keywords: ["custom", "prompt", prompt.scope],
      async handler({ args, sendPrompt }) {
        await sendPrompt(expandCustomPrompt(prompt.body, args));
      },
    }),
  );
  return [...commandRegistry, ...customCommands];
}

export function commandSearchText(command: CommandRegistryEntry) {
  return [
    command.name,
    command.description,
    command.category,
    ...(command.aliases ?? []),
    ...(command.keywords ?? []),
  ]
    .join(" ")
    .toLowerCase();
}

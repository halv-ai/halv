import { invoke } from "@tauri-apps/api/core";
import type {
  AgentSession,
  AgentSessionLaneOptions,
  AgentSessionStore,
} from "../agentSessions.ts";
import { composerModelsWithClaudeCode } from "../claudeComposerModels.ts";
import {
  ONBOARDING_STORAGE_KEY,
  WELCOME_SEEN_STORAGE_KEY,
} from "../onboarding.ts";
import { FIRST_RESULT_STORAGE_KEY } from "../activation.ts";
import {
  featureFlagsFromList,
  NO_FEATURE_FLAGS,
  writeFeatureFlags,
} from "../featureFlags.ts";
import type {
  ConversationSummary,
  StoredMessage,
  TerminalAttach,
  TerminalSession,
} from "../types";
import { installUxAuditHarness } from "./mock-ipc.ts";
import { briefSharingGate } from "../briefSharingGate.ts";
import {
  AGENT_STATUS_SESSION_IDS,
  prepareAgentStatusStorage,
} from "./agent-status.ts";
import { marketingGhosttyIsReady } from "./marketing-exports.ts";
import {
  MARKETING_PRO_SPLIT_AGENT_SESSIONS,
  marketingProSplitCoachMarkIsAbsent,
  prepareMarketingProSplitStorage,
} from "./marketing-pro-split.ts";
import {
  MARKETING_CRUX_AGENT_SESSIONS,
  MARKETING_PRO_SPLIT_AGENT_SESSIONS as MARKETING_SIX_AGENT_SESSIONS,
} from "./marketing-terminal-scenes.ts";
import {
  marketingExpandedSidebarIsReady,
  marketingForbiddenChromeIsAbsent,
  marketingLiveSavingsIsReady,
} from "./marketing-realism.ts";
import { marketingSavingsDashboardIsReady } from "./marketing-savings.ts";
import { MARKETING_SUBAGENT_THREADS } from "./marketing-subagents.ts";
import {
  MARKETING_WORKSPACE_SESSION_IDS,
  prepareMarketingWorkspaceStorage,
} from "./marketing-workspace.ts";
import {
  readHarnessSelection,
  type HarnessBootstrapAction,
  type HarnessSelection,
} from "./states.ts";

const BOOTSTRAP_TIMEOUT_MS = 10_000;
const MARKETING_ACTIVE_SURFACE_GRACE_MS = 1_000;

type HarnessStorePreparationDependencies = {
  store: Pick<AgentSessionStore, "connect" | "getSnapshot">;
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
};

type HarnessMarketingChatPreparationDependencies = {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
};

type HarnessMarketingChatPreparation = {
  conversationId: string;
  conversationTitle: string;
  modelId: string;
  modelLabel: string;
  modelBadge: string;
  savingsTokens: number;
};

if (typeof window !== "undefined" && typeof document !== "undefined") {
  void startHarness().catch(showHarnessError);
}

async function startHarness() {
  const selection = readHarnessSelection(window.location.search);
  prepareHarnessStorage(selection);
  installUxAuditHarness(selection);
  const marketingChat = await prepareHarnessMarketingChatState(selection);
  await prepareHarnessStoreState(selection);

  // Dedicated marketing surfaces do not mount the production app shell.
  if (selection.definition.bootstrap !== "open-marketing-ghostty") {
    // The real entry imports App and styles only after Tauri v2 globals exist.
    await import("../main");
  }
  await bootstrapState(selection.definition.bootstrap, marketingChat);
  if (selection.definition.readyText) {
    await waitForText(selection.definition.readyText);
  } else {
    await waitFor(() => document.querySelector("#root")?.childElementCount !== 0);
  }
  await document.fonts.ready;
  await nextFrame();
  document.documentElement.dataset.visualReady = selection.name;
}

export async function prepareHarnessStoreState(
  selection: HarnessSelection,
  dependencies?: HarnessStorePreparationDependencies,
) {
  if (selection.definition.bootstrap !== "open-agent-sessions") {
    return null;
  }

  const resolvedDependencies = dependencies ?? {
    store: (await import("../agentSessions.ts")).agentSessionStore,
    invoke,
  };

  await resolvedDependencies.store.connect();
  const state = resolvedDependencies.store.getSnapshot();
  if (state.error) {
    throw new Error(`[ux-audit] Agent sessions did not load: ${state.error}`);
  }
  const entry = state.entries.find(
    (candidate) =>
      candidate.session.mode === "terminal_embedded" &&
      candidate.session.status === "running",
  );
  if (!entry) {
    throw new Error("[ux-audit] Embedded agent session did not load");
  }

  const sessions = await resolvedDependencies.invoke<TerminalSession[]>(
    "terminal_list",
  );
  if (!sessions.some((session) => session.id === entry.session.id)) {
    throw new Error("[ux-audit] Embedded terminal session did not load");
  }
  const attached = await resolvedDependencies.invoke<TerminalAttach>(
    "terminal_attach",
    { sessionId: entry.session.id },
  );
  const readyText = selection.definition.readyText;
  if (readyText && !attached.scrollback.includes(readyText)) {
    throw new Error("[ux-audit] Embedded terminal replay is not ready");
  }

  return { entry, attached };
}

export async function prepareHarnessMarketingChatState(
  selection: HarnessSelection,
  dependencies: HarnessMarketingChatPreparationDependencies = { invoke },
): Promise<HarnessMarketingChatPreparation | null> {
  if (selection.definition.bootstrap !== "open-marketing-chat") {
    return null;
  }

  const [conversations, laneOptions] = await Promise.all([
    dependencies.invoke<ConversationSummary[]>("storage_list_conversations"),
    dependencies.invoke<AgentSessionLaneOptions>("agent_session_lane_options"),
  ]);
  const conversation = conversations.find(
    (candidate) => candidate.agentBackend === "claude_session",
  );
  if (!conversation) {
    throw new Error("[ux-audit] Marketing Claude conversation did not load");
  }

  const modelId = conversation.agentSessionModel ?? conversation.model;
  const model = composerModelsWithClaudeCode([], laneOptions).find(
    (candidate) => candidate.id === modelId,
  );
  if (!model?.modelBadge) {
    throw new Error(
      `[ux-audit] Marketing Claude model is unavailable: ${modelId}`,
    );
  }

  const messages = await dependencies.invoke<StoredMessage[]>(
    "storage_get_messages",
    { conversationId: conversation.id },
  );
  const savingsTokens = messages.reduce(
    (total, message) => total + (message.tokensSaved ?? 0),
    0,
  );
  if (savingsTokens <= 0) {
    throw new Error("[ux-audit] Marketing chat savings did not load");
  }
  const readyText = selection.definition.readyText;
  if (readyText && !messages.some((message) => message.content.includes(readyText))) {
    throw new Error("[ux-audit] Marketing chat transcript is not ready");
  }

  return {
    conversationId: conversation.id,
    conversationTitle: conversation.title,
    modelId: model.id,
    modelLabel: model.label,
    modelBadge: model.modelBadge,
    savingsTokens,
  };
}

export function prepareHarnessStorage(
  selection: HarnessSelection,
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> =
    window.localStorage,
) {
  prepareHarnessViewStorage(selection, storage);
  if (!selection.visual) {
    return;
  }

  prepareMarketingProSplitStorage(selection.name, storage);
  prepareAgentStatusStorage(selection.name, storage);
  prepareMarketingWorkspaceStorage(selection.name, storage);
  if (
    selection.name.startsWith("workspace-tabs-") ||
    selection.name.startsWith("agent-glyphs-")
  ) {
    // The strip specs look at the strip, not at the Pro mode hint.
    storage.setItem("halv.agent-session-pro-mode-hint-seen", "true");
  }
  storage.setItem(
    "halv.theme",
    selection.definition.theme ?? "dark",
  );
  if (selection.definition.agentSessionProMode === undefined) {
    // Pro defaults ON in the app; visual states without an explicit choice
    // pin the classic chat so their snapshots stay stable.
    storage.setItem("halv.agent-session-pro-mode", "false");
  } else {
    storage.setItem(
      "halv.agent-session-pro-mode",
      String(selection.definition.agentSessionProMode),
    );
  }
  storage.removeItem("halv.selectedProject");
  if (selection.definition.welcome === "unseen") {
    storage.removeItem(WELCOME_SEEN_STORAGE_KEY);
  } else {
    storage.setItem(WELCOME_SEEN_STORAGE_KEY, "1");
  }
  if (selection.definition.onboarding === "available") {
    storage.removeItem(ONBOARDING_STORAGE_KEY);
  } else if (selection.definition.onboarding === "dismissed") {
    storage.setItem(ONBOARDING_STORAGE_KEY, "dismissed");
  }
  if (selection.definition.onboarding !== "preserve") {
    // A fresh viewer has no first result yet (ADR 0018).
    storage.removeItem(FIRST_RESULT_STORAGE_KEY);
  }
}

// ?flags= sets the feature flags. A harness page without it starts with
// every flag off, also the flags that are on by default, so the specs and
// baselines keep the classic sidebar. The tree state stays as it is, so a
// reload keeps the collapsed rows of the Projects tree. The sidebar has no
// Recent view any more (ADR 0020), so an old ?sidebar= does nothing.
export function prepareHarnessViewStorage(
  selection: HarnessSelection,
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem">,
) {
  writeFeatureFlags(
    selection.flags != null ? featureFlagsFromList(selection.flags) : NO_FEATURE_FLAGS,
    storage,
  );
}

async function bootstrapState(
  action: HarnessBootstrapAction,
  marketingChat: HarnessMarketingChatPreparation | null,
) {
  switch (action) {
    case "none":
      return;
    case "connect-pending":
    case "connect-failed":
      await clickElement("Sign in with ChatGPT");
      return;
    case "connect-keys":
      await clickElement("Prefer an API key? Add an OpenAI or Anthropic key instead");
      return;
    case "disconnect-api-key":
      await openSettings();
      await invoke("auth_clear_api_key");
      window.dispatchEvent(new Event("focus"));
      await waitForText("Not connected");
      await clickElement("Close");
      return;
    case "open-chat":
      await clickElement("Streaming markdown polish", "starts-with");
      return;
    case "open-marketing-chat": {
      if (!marketingChat) {
        throw new Error("[ux-audit] Marketing chat preparation is missing");
      }
      await clickElement(marketingChat.conversationTitle, "starts-with");
      (
        await waitForElement(
          `[aria-label^="Response options: ${marketingChat.modelLabel}"]`,
        )
      ).click();
      await waitForElement('[role="dialog"][aria-label="Response options"]');
      await waitFor(() => {
        const selects = document.querySelectorAll<HTMLSelectElement>("select");
        return [...selects].find((select) => {
          const selectedText = normalizedText(
            select.selectedOptions.item(0)?.textContent ?? null,
          );
          return (
            isVisible(select) &&
            select.value === marketingChat.modelId &&
            selectedText ===
              `${marketingChat.modelLabel} · ${marketingChat.modelBadge}`
          );
        });
      });
      await waitForElement('[aria-label="Session savings meter"]');
      return;
    }
    case "open-marketing-ghostty": {
      const { renderMarketingGhosttyHarness } = await import(
        "./marketing-ghostty.ts"
      );
      await renderMarketingGhosttyHarness();
      await waitFor(() => marketingGhosttyIsReady(document));
      return;
    }
    case "open-marketing-app": {
      if (!(await marketingActiveSurfaceAppearsWithinGracePeriod())) {
        const openingTitle =
          MARKETING_PRO_SPLIT_AGENT_SESSIONS[0]?.defaultTitle;
        if (!openingTitle) {
          throw new Error(
            "[ux-audit] Marketing app opening terminal is missing",
          );
        }
        await clickTerminalRow(openingTitle);
      }
      await waitForMarketingAppPanes();
      return;
    }
    case "open-marketing-pro-split":
      await waitForMarketingTerminalPanes(
        MARKETING_SIX_AGENT_SESSIONS,
        true,
        "31,204",
      );
      return;
    case "open-marketing-workspace":
      await waitForMarketingWorkspace();
      return;
    case "open-marketing-crux":
      await waitForMarketingTerminalPanes(
        MARKETING_CRUX_AGENT_SESSIONS,
        false,
        "44,876",
      );
      return;
    case "open-marketing-subagents":
      await waitForMarketingSubagentPanel();
      return;
    case "open-marketing-savings": {
      await openSettings();
      await clickElement("System", "starts-with");
      await waitForMarketingSavingsDashboard();
      return;
    }
    case "open-agent-sessions":
      (
        await waitForElement(
          '[aria-label="Workspace and application menu"]',
        )
      ).click();
      await clickElement("Agent sessions");
      await waitForElement('[aria-label="Claude embedded terminal"]');
      return;
    case "open-savings":
      await openSettings();
      await clickElement("System", "starts-with");
      return;
    case "open-agent-status": {
      if (!(await marketingActiveSurfaceAppearsWithinGracePeriod())) {
        const openingTitle =
          MARKETING_PRO_SPLIT_AGENT_SESSIONS[0]?.defaultTitle;
        if (!openingTitle) {
          throw new Error("[ux-audit] Agent status opening terminal is missing");
        }
        await clickTerminalRow(openingTitle);
      }
      await waitForAgentStatusPanes();
      return;
    }
    case "open-jev-settings":
      await openSettings();
      await clickElement("JEV", "starts-with");
      return;
    case "open-brief-sharing-prompt":
    case "open-brief-sharing-card": {
      // The gate asks only after the account status loaded.
      await waitFor(() => briefSharingGate.accessKnown());
      // A recent click makes the launch one that the user started. An old
      // one makes it a launch without a user action, like a restore.
      briefSharingGate.noteUserInput(
        action === "open-brief-sharing-prompt"
          ? Date.now()
          : Number.NEGATIVE_INFINITY,
      );
      void briefSharingGate.beforeLaunch("agent_terminal_create", {
        agentKind: "claude",
      });
      await waitForElement(
        action === "open-brief-sharing-prompt"
          ? '[data-testid="brief-sharing-prompt"]'
          : '[data-testid="brief-sharing-card"]',
      );
      return;
    }
    case "open-status-bar-wallet": {
      // Claude's plan windows load after the first render.
      const pill = await waitFor(() => {
        const element = document.querySelector<HTMLElement>(
          '[data-status-provider="claude"]',
        );
        return element &&
          isVisible(element) &&
          element.textContent?.includes("% used")
          ? element
          : undefined;
      });
      pill.click();
      await waitForElement(
        '[data-status-popover="claude"][data-wallet-usage="loaded"] [data-account-usage]',
      );
      return;
    }
  }
}

async function marketingActiveSurfaceAppearsWithinGracePeriod() {
  const startedAt = performance.now();
  while (performance.now() - startedAt < MARKETING_ACTIVE_SURFACE_GRACE_MS) {
    const surface = document.querySelector<HTMLElement>(
      '.conversation-agent-mode-surface[data-active="true"]',
    );
    if (surface && isVisible(surface)) return true;
    await new Promise<void>((done) => window.setTimeout(done, 20));
  }
  return false;
}

async function clickTerminalRow(title: string) {
  const row = await waitFor(() =>
    [...document.querySelectorAll<HTMLButtonElement>("button[title]")].find(
      (element) =>
        isVisible(element) &&
        normalizedText(element.getAttribute("title")) ===
          `Open terminal: ${title}`,
    ),
  );
  row.click();
}

export function terminalSceneCompletionIsReady(
  expectedSessionIds: readonly string[],
  mountedScenes: readonly {
    sessionId: string | null;
    complete: boolean;
  }[],
) {
  if (mountedScenes.length !== expectedSessionIds.length) return false;
  const completeSessionIds = new Set(
    mountedScenes
      .filter((scene) => scene.complete && scene.sessionId !== null)
      .map((scene) => scene.sessionId),
  );
  return (
    completeSessionIds.size === expectedSessionIds.length &&
    expectedSessionIds.every((sessionId) => completeSessionIds.has(sessionId))
  );
}

// Each split pane shows its glyph, and the sidebar shows the question.
async function waitForAgentStatusPanes() {
  const splitIds = [
    AGENT_STATUS_SESSION_IDS.approval,
    AGENT_STATUS_SESSION_IDS.working,
    AGENT_STATUS_SESSION_IDS.done,
  ];
  await waitFor(() => {
    if (!marketingProSplitCoachMarkIsAbsent(document)) return false;
    const surface = document.querySelector<HTMLElement>(
      '.conversation-agent-mode-surface[data-active="true"]',
    );
    if (!surface || !isVisible(surface)) return false;
    const panes = [...surface.querySelectorAll<HTMLElement>(".terminal-pane")];
    const mountedScenes = panes.map((pane) => ({
      sessionId: pane.dataset.terminalSessionId ?? null,
      complete: pane.dataset.terminalSceneComplete === "true",
    }));
    const navigation = document.querySelector<HTMLElement>(
      '[aria-label="Conversation navigation"]',
    );
    return (
      terminalSceneCompletionIsReady(splitIds, mountedScenes) &&
      panes.every(
        (pane) =>
          pane.querySelector(".terminal-pane-title [data-agent-glyph]") !== null,
      ) &&
      navigation?.querySelector('[data-agent-glyph="attention"]') != null
    );
  });
}

// The hero of halv.ai (ADR 0010, ADR 0019): the strip, the two panes of the
// active tab with their scenes, and the savings meter of the tab.
async function waitForMarketingWorkspace() {
  await waitFor(() => {
    const strip = document.querySelector<HTMLElement>("[data-workspace-tab-strip]");
    if (!strip || !isVisible(strip)) return false;
    const expected: string[] = [
      MARKETING_WORKSPACE_SESSION_IDS.claude,
      MARKETING_WORKSPACE_SESSION_IDS.codex,
    ];
    const panes = [...document.querySelectorAll<HTMLElement>(".terminal-pane")].filter(
      (pane) => expected.includes(pane.dataset.terminalSessionId ?? "") && isVisible(pane),
    );
    const mounted = panes.map((pane) => ({
      sessionId: pane.dataset.terminalSessionId ?? null,
      complete: pane.dataset.terminalSceneComplete === "true",
    }));
    if (!terminalSceneCompletionIsReady(expected, mounted)) return false;
    const meter = document.querySelector<HTMLElement>('[aria-label="Session savings meter"]');
    return (
      meter !== null &&
      /\$[1-9]/.test(normalizedText(meter.innerText)) &&
      normalizedText(document.body.innerText).includes("saved")
    );
  });
}

async function waitForMarketingAppPanes() {
  const expectedTitles = MARKETING_PRO_SPLIT_AGENT_SESSIONS.flatMap((session) =>
    session.defaultTitle ? [session.defaultTitle] : [],
  );
  await waitFor(() => {
    if (!marketingProSplitCoachMarkIsAbsent(document)) return false;
    const surface = document.querySelector<HTMLElement>(
      '.conversation-agent-mode-surface[data-active="true"]',
    );
    if (!surface || !isVisible(surface)) return false;
    const panes = [...surface.querySelectorAll<HTMLElement>(".terminal-pane")];
    const mountedTitles = panes.map((pane) =>
      normalizedText(
        pane.querySelector<HTMLElement>(".terminal-pane-title")?.textContent ??
          null,
      ),
    );
    const mountedScenes = panes.map((pane) => ({
      sessionId: pane.dataset.terminalSessionId ?? null,
      complete: pane.dataset.terminalSceneComplete === "true",
    }));
    const savings = document.querySelector<HTMLElement>(
      "#live-savings-details",
    );
    const sessionMeter = surface.querySelector<HTMLElement>(
      '.terminal-header [aria-label="Session savings meter"]',
    );
    const sessionStatus = surface.querySelector<HTMLElement>(
      ".terminal-header .terminal-status-label",
    );
    return (
      panes.length === expectedTitles.length &&
      panes.every(
        (pane) =>
          pane.querySelector(".xterm-screen") !== null && isVisible(pane),
      ) &&
      expectedTitles.every((title) => mountedTitles.includes(title)) &&
      terminalSceneCompletionIsReady(
        MARKETING_PRO_SPLIT_AGENT_SESSIONS.map((session) => session.id),
        mountedScenes,
      ) &&
      sessionMeter !== null &&
      isVisible(sessionMeter) &&
      sessionMeter.querySelector(".agent-session-savings-chip-measured") !==
        null &&
      normalizedText(sessionMeter.innerText).includes(
        "measured ~44,876 tokens saved this session",
      ) &&
      terminalSurfaceIsSettled(sessionStatus) &&
      savings !== null &&
      isVisible(savings) &&
      marketingWeeklyChipIsReady() &&
      marketingLiveSavingsIsReady(document) &&
      marketingExpandedSidebarIsReady(document) &&
      marketingForbiddenChromeIsAbsent(document)
    );
  });
}

async function waitForMarketingTerminalPanes(
  sessions: readonly AgentSession[],
  sidebarCollapsed: boolean,
  expectedSessionTokensSaved: string,
) {
  const expectedTitles = sessions.flatMap((session) =>
    session.defaultTitle ? [session.defaultTitle] : [],
  );
  await waitFor(() => {
    const surface = document.querySelector<HTMLElement>(
      '.conversation-agent-mode-surface[data-active="true"]',
    );
    if (!surface || !isVisible(surface)) return false;
    if (!marketingProSplitCoachMarkIsAbsent(document)) return false;
    const panes = [...surface.querySelectorAll<HTMLElement>(".terminal-pane")];
    const mountedTitles = panes.map((pane) =>
      normalizedText(
        pane.querySelector<HTMLElement>(".terminal-pane-title")?.textContent ??
          null,
      ),
    );
    const mountedScenes = panes.map((pane) => ({
      sessionId: pane.dataset.terminalSessionId ?? null,
      complete: pane.dataset.terminalSceneComplete === "true",
    }));
    const sessionMeter = surface.querySelector<HTMLElement>(
      '.terminal-header [aria-label="Session savings meter"]',
    );
    const sessionStatus = surface.querySelector<HTMLElement>(
      ".terminal-header .terminal-status-label",
    );
    const panesAreReady =
      panes.length === expectedTitles.length &&
      panes.every(
        (pane) =>
          pane.querySelector(".xterm-screen") !== null && isVisible(pane),
      ) &&
      expectedTitles.every((title) => mountedTitles.includes(title)) &&
      terminalSceneCompletionIsReady(
        sessions.map((session) => session.id),
        mountedScenes,
      ) &&
      sessionMeter !== null &&
      isVisible(sessionMeter) &&
      sessionMeter.querySelector(".agent-session-savings-chip-measured") !==
        null &&
      normalizedText(sessionMeter.innerText).includes(
        `measured ~${expectedSessionTokensSaved} tokens saved this session`,
      ) &&
      terminalSurfaceIsSettled(sessionStatus) &&
      marketingWeeklyChipIsReady() &&
      marketingForbiddenChromeIsAbsent(document);

    if (!panesAreReady) return false;
    if (sidebarCollapsed) {
      const collapsedControl = document.querySelector<HTMLElement>(
        '[aria-label="Expand sidebar"]',
      );
      return collapsedControl !== null && isVisible(collapsedControl);
    }

    return (
      marketingExpandedSidebarIsReady(document) &&
      marketingLiveSavingsIsReady(document)
    );
  });
}

function marketingWeeklyChipIsReady() {
  return [...document.querySelectorAll<HTMLElement>("div")].some(
    (element) =>
      isVisible(element) &&
      normalizedText(element.innerText) === "↓ 21.7M last 7 days",
  );
}

async function waitForMarketingSubagentPanel() {
  await waitFor(() => {
    const panel = document.querySelector<HTMLElement>(
      'section[aria-label="Subagent activity"]',
    );
    if (!panel || !isVisible(panel)) return false;
    const rows = [...panel.querySelectorAll<HTMLElement>("article")];
    const panelText = panel.innerText;
    return (
      rows.length === MARKETING_SUBAGENT_THREADS.length &&
      rows.every(isVisible) &&
      ["Queued", "Running", "Completed", "Failed"].every((status) =>
        panelText.includes(status),
      ) &&
      marketingProSplitCoachMarkIsAbsent(document) &&
      marketingLiveSavingsIsReady(document) &&
      marketingExpandedSidebarIsReady(document, false) &&
      marketingForbiddenChromeIsAbsent(document)
    );
  });
}

async function waitForMarketingSavingsDashboard() {
  await waitFor(() => {
    const advancedDiagnostics = [...document.querySelectorAll("details")].find(
      (details) =>
        details.querySelector("summary")?.textContent?.trim() ===
        "Advanced diagnostics",
    );
    return (
      advancedDiagnostics !== undefined &&
      isVisible(advancedDiagnostics) &&
      marketingProSplitCoachMarkIsAbsent(document) &&
      marketingSavingsDashboardIsReady(document) &&
      marketingLiveSavingsIsReady(document) &&
      marketingExpandedSidebarIsReady(document, false) &&
      marketingForbiddenChromeIsAbsent(document)
    );
  });
}

async function openSettings() {
  const menu = await waitForElement(
    '[aria-label="Workspace and application menu"]',
  );
  menu.click();
  await clickElement("Settings");
  await waitForText("Manage your account, preferences, and local tools.");
}

async function clickElement(
  text: string,
  match: "exact" | "starts-with" = "exact",
) {
  const element = await waitFor(() => findClickable(text, match));
  element.click();
}

function findClickable(text: string, match: "exact" | "starts-with") {
  const candidates = document.querySelectorAll<HTMLElement>(
    "button, summary, [role='button']",
  );
  return [...candidates].find((element) => {
    if (!isVisible(element)) {
      return false;
    }
    const content = normalizedText(element.textContent);
    return match === "exact" ? content === text : content.startsWith(text);
  });
}

async function waitForElement(selector: string) {
  return waitFor(() => {
    const element = document.querySelector<HTMLElement>(selector);
    return element && isVisible(element) ? element : undefined;
  });
}

async function waitForText(text: string) {
  await waitFor(
    () =>
      harnessReadyTextMatches(
        text,
        document.body.innerText,
        [...document.querySelectorAll<HTMLElement>("[aria-label]")].flatMap(
          (element) =>
            isVisible(element) ? [element.getAttribute("aria-label") ?? ""] : [],
        ),
      ),
  );
}

export function harnessReadyTextMatches(
  text: string,
  bodyText: string,
  visibleAriaLabels: readonly string[],
) {
  return (
    bodyText.includes(text) ||
    visibleAriaLabels.some((label) => label.includes(text))
  );
}

async function waitFor<T>(resolve: () => T | undefined | false): Promise<T> {
  const startedAt = performance.now();
  while (performance.now() - startedAt < BOOTSTRAP_TIMEOUT_MS) {
    const value = resolve();
    if (value) {
      return value;
    }
    await new Promise<void>((done) => window.setTimeout(done, 20));
  }
  throw new Error("[ux-audit] Timed out while preparing the requested state");
}

function normalizedText(value: string | null) {
  return (value ?? "").replace(/\s+/g, " ").trim();
}

// An agent pane at rest: the header names any state but work.
function terminalSurfaceIsSettled(status: HTMLElement | null) {
  const state = status?.dataset.agentStatus;
  return state !== undefined && state !== "working";
}

function isVisible(element: HTMLElement) {
  return element.getClientRects().length > 0;
}

function nextFrame() {
  return new Promise<void>((resolve) =>
    window.requestAnimationFrame(() => resolve()),
  );
}

function showHarnessError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  document.documentElement.dataset.visualError = message;
  const output = document.createElement("pre");
  output.style.cssText =
    "margin:24px;white-space:pre-wrap;color:#ff8b8b;font:13px/1.5 monospace";
  output.textContent = message;
  document.body.append(output);
  console.error(error);
}

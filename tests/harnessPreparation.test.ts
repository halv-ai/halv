import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs source extensions.
import {
  AgentSessionProModeStore,
  AgentSessionStore,
  type AgentSession,
} from "../src/agentSessions.ts";
// @ts-expect-error Node's test runner needs source extensions.
import {
  harnessReadyTextMatches,
  prepareHarnessMarketingChatState,
  prepareHarnessStorage,
  prepareHarnessStoreState,
  terminalSceneCompletionIsReady,
} from "../src/harness/entry.ts";
// @ts-expect-error Node's test runner needs source extensions.
import { readFeatureFlags } from "../src/featureFlags.ts";
// @ts-expect-error Node's test runner needs source extensions.
import { createUxAuditHarnessMock } from "../src/harness/mock-ipc.ts";
// @ts-expect-error Node's test runner needs source extensions.
import { MARKETING_EXPORTS } from "../src/harness/marketing-exports.ts";
// @ts-expect-error Node's test runner needs source extensions.
import {
  MARKETING_PRO_SPLIT_PRO_MODE_HINT_SEEN_STORAGE_KEY,
  marketingProSplitCoachMarkIsAbsent,
} from "../src/harness/marketing-pro-split.ts";
// @ts-expect-error Node's test runner needs source extensions.
import {
  MARKETING_SAVINGS_ALL_TIME_TOKENS,
  MARKETING_SAVINGS_ALL_TIME_USD,
  MARKETING_SAVINGS_SESSION_TOKENS,
  MARKETING_SAVINGS_STATE_NAMES,
  MARKETING_SAVINGS_WEEK_TOKENS,
} from "../src/harness/marketing-savings.ts";
// @ts-expect-error Node's test runner needs source extensions.
import {
  MARKETING_CLAUDE_USAGE_WINDOWS,
  MARKETING_CODEX_USAGE_WINDOWS,
  MARKETING_ENGINE_SAVINGS,
  MARKETING_RECENT_SAVINGS,
  MARKETING_REALISM_STATE_NAMES,
  marketingExpandedSidebarIsReady,
} from "../src/harness/marketing-realism.ts";
// @ts-expect-error Node's test runner needs source extensions.
import {
  MARKETING_CRUX_READY_MARKERS,
  MARKETING_PRO_SPLIT_READY_MARKERS,
  marketingTerminalSceneStartsCollapsed,
} from "../src/harness/marketing-terminal-scenes.ts";
// @ts-expect-error Node's test runner needs source extensions.
import {
  MARKETING_WORKSPACE_CONVERSATION_IDS,
  MARKETING_WORKSPACE_FLAGS,
  MARKETING_WORKSPACE_FOLDER,
  MARKETING_WORKSPACE_SESSION_IDS,
  marketingWorkspaceTreeState,
} from "../src/harness/marketing-workspace.ts";
// @ts-expect-error Node's test runner needs source extensions.
import {
  MARKETING_SUBAGENT_MESSAGES,
  MARKETING_SUBAGENT_THREADS,
} from "../src/harness/marketing-subagents.ts";
// @ts-expect-error Node's test runner needs source extensions.
import {
  MARKETING_STATE_NAMES,
  VISUAL_STATE_NAMES,
  readHarnessSelection,
} from "../src/harness/states.ts";
// @ts-expect-error Node's test runner needs source extensions.
import type {
  AppPreferences,
  SavingsToday,
  SavingsTotals,
  SidecarStats,
  StoredMessage,
  SubagentThread,
  TerminalAttach,
  TerminalSession,
  TerminalSupervisorStatus,
} from "../src/types.ts";

test("harness reports its terminal supervisor as available", async () => {
  const mock = createUxAuditHarnessMock(
    readHarnessSelection("?scenario=normal"),
  );
  const sessions = await mock.invoke<TerminalSession[]>("terminal_list");
  const status = await mock.invoke<TerminalSupervisorStatus>(
    "terminal_supervisor_status",
  );

  assert.equal(status.available, true);
  assert.equal(status.sessions, sessions.length);
  assert.equal(status.message, null);
});

test("harness preferences persist Claude subscription proxy opt-out", async () => {
  const mock = createUxAuditHarnessMock(
    readHarnessSelection("?state=marketing-app-dark"),
  );
  const initial = await mock.invoke<AppPreferences>("preferences_get");
  assert.equal(initial.claudeSubscriptionProxy, true);

  const saved = await mock.invoke<AppPreferences>(
    "preferences_set_claude_subscription_proxy",
    { claudeSubscriptionProxy: false },
  );
  assert.equal(saved.claudeSubscriptionProxy, false);
  assert.equal(
    (await mock.invoke<AppPreferences>("preferences_get"))
      .claudeSubscriptionProxy,
    false,
  );
});

test("harness preferences persist JEV tier agents", async () => {
  const mock = createUxAuditHarnessMock(
    readHarnessSelection("?state=marketing-app-dark"),
  );
  const routingTierAgents = {
    cheapest: { lane: "claude" as const, model: "haiku", effort: null },
    cheap: { lane: "codex" as const, model: "gpt-6-luna", effort: "high" },
    medium: { lane: "claude" as const, model: "opus", effort: "medium" },
    strong: { lane: "codex" as const, model: "gpt-6.1-sol", effort: "xhigh" },
  };

  const saved = await mock.invoke<AppPreferences>(
    "preferences_set_routing_tier_agents",
    { routingTierAgents },
  );
  assert.deepEqual(saved.routingTierAgents, routingTierAgents);
  assert.equal(saved.workerLane, "auto");
});

test("ready text matches visible accessible labels", () => {
  assert.equal(
    harnessReadyTextMatches("Anthropic API key", "Anthropic", [
      "Anthropic API key",
    ]),
    true,
  );
  assert.equal(
    harnessReadyTextMatches("Anthropic API key", "Anthropic", []),
    false,
  );
});

test("expanded marketing sidebar can omit terminal badges", () => {
  const navigation = {} as HTMLElement;
  const newConversation = { textContent: "+ New chat" } as HTMLElement;
  const search = {} as HTMLElement;
  const conversations = {} as HTMLElement;
  const root = {
    querySelector: (selector: string) => {
      if (selector === '[aria-label="Conversation navigation"]') {
        return navigation;
      }
      if (selector === '[aria-label="Search conversations"]') return search;
      if (selector === '[aria-label="Conversations"]') return conversations;
      return null;
    },
    querySelectorAll: (selector: string) => {
      if (selector === "button") return [newConversation];
      return [];
    },
  } as unknown as ParentNode;

  assert.equal(marketingExpandedSidebarIsReady(root), false);
  assert.equal(marketingExpandedSidebarIsReady(root, false), true);
});

test("marketing app preserves the three-pane content", async () => {
  const selection = readHarnessSelection("?state=marketing-app-dark");
  assert.equal(selection.definition.bootstrap, "open-marketing-app");

  const mock = createUxAuditHarnessMock(selection);
  const sessions = await mock.invoke<AgentSession[]>("agent_session_list");
  const terminals = await mock.invoke<TerminalSession[]>("terminal_list");
  assert.deepEqual(
    sessions.map((session) => session.defaultTitle),
    ["Claude 4", "Claude 5", "Codex 1"],
  );
  assert.ok(
    sessions.every((session) => session.conversationId === "audit-marketing"),
  );
  assert.deepEqual(
    terminals.map((terminal) => terminal.id),
    sessions.map((session) => session.id),
  );
  const attached = await Promise.all(
    terminals.map((terminal) =>
      mock.invoke<TerminalAttach>("terminal_attach", {
        sessionId: terminal.id,
      }),
    ),
  );
  const content = stripTerminalAnsi(
    attached.map((item) => item.scrollback).join("\n"),
  );
  assert.match(content, /Plan a safe refactor/);
  assert.match(content, /3,104 lines → 41 lines/);
  assert.match(content, /Trace the flaky refresh-token test/);
  assert.match(content, /⏸ plan mode on \(shift\+tab to cycle\)/);
  assert.match(
    content,
    /⏵⏵ bypass permissions on · 2 shells, 1 monitor/,
  );
  assert.match(content, /› Ask Codex to do anything/);
  assert.match(content, /gpt-5\.6-sol xhigh fast · ~\/programming\/halv/);
  assert.match(content, /codex:codex-rescue/);
  assert.doesNotMatch(content, /BYPASSING PERMISSIONS ON/);
});

test("marketing terminal readiness waits for every completed scene", () => {
  const expectedSessionIds = ["terminal-a", "terminal-b"];
  assert.equal(
    terminalSceneCompletionIsReady(expectedSessionIds, [
      { sessionId: "terminal-a", complete: true },
      { sessionId: "terminal-b", complete: false },
    ]),
    false,
  );
  assert.equal(
    terminalSceneCompletionIsReady(expectedSessionIds, [
      { sessionId: "terminal-b", complete: true },
      { sessionId: "terminal-a", complete: true },
    ]),
    true,
  );
  assert.equal(
    terminalSceneCompletionIsReady(expectedSessionIds, [
      { sessionId: "terminal-a", complete: true },
      { sessionId: "terminal-a", complete: true },
    ]),
    false,
  );
});

test("marketing shots stage the Pro mode hint as acknowledged", () => {
  for (const stateName of MARKETING_REALISM_STATE_NAMES) {
    const values = new Map<string, string>();
    prepareHarnessStorage(
      readHarnessSelection(`?state=${stateName}`),
      mapStorage(values),
    );
    assert.equal(
      values.get(MARKETING_PRO_SPLIT_PRO_MODE_HINT_SEEN_STORAGE_KEY),
      "true",
      stateName,
    );
  }

  const siblingValues = new Map<string, string>();
  prepareHarnessStorage(
    readHarnessSelection("?state=marketing-pro-dark"),
    mapStorage(siblingValues),
  );
  assert.equal(
    siblingValues.has(MARKETING_PRO_SPLIT_PRO_MODE_HINT_SEEN_STORAGE_KEY),
    false,
  );
});

test("marketing split coach mark readiness requires the hint to be absent", () => {
  assert.equal(
    marketingProSplitCoachMarkIsAbsent({ querySelector: () => null }),
    true,
  );
  assert.equal(
    marketingProSplitCoachMarkIsAbsent({
      querySelector: () => ({}) as Element,
    }),
    false,
  );
});

test("marketing savings base state passes every state-name guard", () => {
  assert.ok(MARKETING_STATE_NAMES.includes("marketing-savings"));
  assert.ok(MARKETING_SAVINGS_STATE_NAMES.includes("marketing-savings"));
  assert.ok(MARKETING_REALISM_STATE_NAMES.includes("marketing-savings"));
  const selection = readHarnessSelection("?state=marketing-savings");
  assert.equal(selection.name, "marketing-savings");
  assert.equal(selection.definition.bootstrap, "open-marketing-savings");
});

test("marketing savings stages the released app version", async () => {
  for (const stateName of ["marketing-savings-dark", "marketing-savings-light"]) {
    const mock = createUxAuditHarnessMock(
      readHarnessSelection(`?state=${stateName}`),
    );
    assert.equal(await mock.invoke<string>("plugin:app|version"), "0.4.4");
  }
});

test("marketing exports match the fourteen requested PNGs", () => {
  assert.equal(VISUAL_STATE_NAMES.length, 13);
  assert.deepEqual(
    MARKETING_EXPORTS.map((entry) => ({
      state: entry.state,
      viewport: entry.viewport,
      deviceScaleFactor: entry.deviceScaleFactor,
      colorScheme: entry.colorScheme,
    })),
    [
      marketingExport("marketing-app-dark", 1600, 1000, "dark"),
      marketingExport("marketing-app-light", 1600, 1000, "light"),
      marketingExport("marketing-pro-split-dark", 1600, 900, "dark"),
      marketingExport("marketing-pro-split-light", 1600, 900, "light"),
      marketingExport("marketing-subagents-dark", 1600, 1000, "dark"),
      marketingExport("marketing-subagents-light", 1600, 1000, "light"),
      marketingExport("marketing-crux-dark", 1600, 1000, "dark"),
      marketingExport("marketing-crux-light", 1600, 1000, "light"),
      marketingExport("marketing-savings-dark", 1600, 1000, "dark"),
      marketingExport("marketing-savings-light", 1600, 1000, "light"),
      marketingExport("marketing-ghostty-dark", 1200, 900, "dark"),
      marketingExport("marketing-ghostty-light", 1200, 900, "light"),
      marketingExport("marketing-workspace-dark", 1512, 947, "dark"),
      marketingExport("marketing-workspace-light", 1512, 947, "light"),
    ],
  );

  for (const entry of MARKETING_EXPORTS) {
    const selection = readHarnessSelection(`?state=${entry.state}`);
    assert.equal(selection.definition.theme, entry.colorScheme);
    assert.equal(selection.definition.agentSessionProMode, true);
    const values = new Map<string, string>();
    prepareHarnessStorage(selection, mapStorage(values));
    assert.equal(values.get("halv.theme"), entry.colorScheme);
  }
});

test("the hero workspace stages a folder group, tabs and a split with live savings", async () => {
  for (const state of ["marketing-workspace-dark", "marketing-workspace-light"]) {
    const selection = readHarnessSelection(`?state=${state}`);
    // A plain ?state= has the flags of a release build; ?flags= still wins.
    assert.equal(selection.flags, MARKETING_WORKSPACE_FLAGS);
    assert.equal(readHarnessSelection(`?state=${state}&flags=projectTree`).flags, "projectTree");
    assert.equal(selection.definition.bootstrap, "open-marketing-workspace");

    const values = new Map<string, string>();
    prepareHarnessStorage(selection, mapStorage(values));
    assert.deepEqual(
      JSON.parse(values.get("halv.sidebar.project-tree.v1") ?? "null"),
      marketingWorkspaceTreeState(),
    );
    assert.equal(
      values.get("halv.workspace.last-active-conversation.v1"),
      MARKETING_WORKSPACE_CONVERSATION_IDS.checkout,
    );
    const flags = readFeatureFlags(mapStorage(values));
    assert.equal(flags.projectTree && flags.workspaceTabs && flags.shellTabs, true);

    const mock = createUxAuditHarnessMock(selection);
    const projects = await mock.invoke<Array<{ id: number; name: string; rootPath: string }>>(
      "project_list",
    );
    assert.deepEqual(
      projects.map((project) => project.name).sort(),
      ["app", "halv", "web"],
    );
    const overview = await mock.invoke<Array<{ projectId: number; isGitRepo: boolean }>>(
      "workspace_overview",
    );
    const folder = projects.find((project) => project.rootPath === MARKETING_WORKSPACE_FOLDER);
    assert.ok(folder);
    assert.equal(overview.find((item) => item.projectId === folder.id)?.isGitRepo, false);

    const terminals = await mock.invoke<TerminalSession[]>("terminal_list");
    const checkoutPanes = terminals.filter(
      (terminal) => terminal.conversationId === MARKETING_WORKSPACE_CONVERSATION_IDS.checkout,
    );
    assert.deepEqual(
      checkoutPanes.map((terminal) => terminal.agentKind).sort(),
      ["claude", "codex"],
    );
    assert.ok(terminals.some((terminal) => terminal.agentKind === null && !terminal.conversationId));
    for (const sessionId of [MARKETING_WORKSPACE_SESSION_IDS.claude, MARKETING_WORKSPACE_SESSION_IDS.codex]) {
      const attach = await mock.invoke<TerminalAttach>("terminal_attach", { sessionId });
      assert.match(stripTerminalAnsi(attach.scrollback), /checkout/);
    }
    const statuses = await mock.invoke<Array<{ status: string }>>("agent_status_snapshot");
    assert.deepEqual(
      [...new Set(statuses.map((entry) => entry.status))].sort(),
      ["done", "needsInput", "working"],
    );
  }
});

test("marketing Pro split stages six distinct terminal panes", async () => {
  const selection = readHarnessSelection("?state=marketing-pro-split-dark");
  assert.equal(selection.definition.bootstrap, "open-marketing-pro-split");
  assert.equal(marketingTerminalSceneStartsCollapsed(selection.name), true);

  const mock = createUxAuditHarnessMock(selection);
  const sessions = await mock.invoke<AgentSession[]>("agent_session_list");
  const terminals = await mock.invoke<TerminalSession[]>("terminal_list");
  assert.deepEqual(
    sessions.map((session) => session.defaultTitle),
    ["Claude 2", "Grok 1", "Codex 2", "Kimi 1", "GLM 1", "Claude 4"],
  );
  assert.deepEqual(
    sessions.map((session) => session.status),
    ["running", "running", "running", "starting", "exited", "running"],
  );
  assert.deepEqual(
    terminals.map((terminal) => terminal.id),
    sessions.map((session) => session.id),
  );

  const glmTerminal = terminals.find(
    (terminal) => terminal.id === "marketing-six-glm",
  );
  assert.equal(glmTerminal?.status, "running");
  assert.equal(glmTerminal?.exitCode, undefined);
  assert.equal(glmTerminal?.exitedAt, undefined);
  const savedTotals = await mock.invoke<
    Array<{
      sessionId: string;
      conversationId: string;
      savingsTokens: number;
    }>
  >("agent_session_saved_totals");
  assert.deepEqual(
    savedTotals.map(({ sessionId, conversationId, savingsTokens }) => ({
      sessionId,
      conversationId,
      savingsTokens,
    })),
    [
      {
        sessionId: "marketing-six-claude-plan",
        conversationId: "audit-marketing",
        savingsTokens: 31_204,
      },
    ],
  );

  const scrollback = await Promise.all(
    terminals.map((terminal) =>
      mock.invoke<TerminalAttach>("terminal_attach", {
        sessionId: terminal.id,
      }),
    ),
  );
  const combined = stripTerminalAnsi(
    scrollback.map((attached) => attached.scrollback).join("\n"),
  );
  for (const marker of MARKETING_PRO_SPLIT_READY_MARKERS) {
    assert.match(combined, new RegExp(escapeRegExp(marker)));
  }
  assert.match(combined, /› Ask Codex to do anything/);
  assert.match(combined, /✳ Churned 4m 9s/);
  assert.match(combined, /✳ Gusting…/);
  assert.match(combined, /✳ Worked 1m 4s/);
  assert.match(combined, /codex:codex-rescue/);
  assert.doesNotMatch(combined, /BYPASSING PERMISSIONS ON/);
});

test("marketing subagents stages the real activity data model", async () => {
  const selection = readHarnessSelection("?state=marketing-subagents-dark");
  assert.equal(selection.definition.bootstrap, "open-marketing-subagents");
  const mock = createUxAuditHarnessMock(selection);
  const messages = await mock.invoke<StoredMessage[]>("storage_get_messages", {
    conversationId: "audit-marketing",
  });
  const threads = await mock.invoke<SubagentThread[]>("chat_list_subagents", {
    conversationId: "audit-marketing",
  });

  assert.deepEqual(
    messages.map((message) => ({
      id: message.id,
      content: message.content,
      mentions: message.mentions,
    })),
    MARKETING_SUBAGENT_MESSAGES.map((message) => ({
      id: message.id,
      content: message.content,
      mentions: message.mentions,
    })),
  );
  assert.deepEqual(threads, MARKETING_SUBAGENT_THREADS);
  assert.deepEqual(
    threads.map((thread) => thread.status),
    ["running", "completed", "pending", "errored"],
  );
  assert.ok(threads.every((thread) => thread.turnId === messages[0]?.id));
  assert.ok(
    threads.every((thread) =>
      Boolean(thread.result?.trim() || thread.error?.trim()),
    ),
  );
});

test("marketing crux stages callers, references, and impact", async () => {
  const selection = readHarnessSelection("?state=marketing-crux-dark");
  assert.equal(selection.definition.bootstrap, "open-marketing-crux");
  assert.equal(marketingTerminalSceneStartsCollapsed(selection.name), false);
  const mock = createUxAuditHarnessMock(selection);
  const sessions = await mock.invoke<AgentSession[]>("agent_session_list");
  assert.equal(sessions.length, 1);
  assert.equal(sessions[0]?.defaultTitle, "Claude 6");
  const attached = await mock.invoke<TerminalAttach>("terminal_attach", {
    sessionId: sessions[0]?.id,
  });
  const content = stripTerminalAnsi(attached.scrollback);
  assert.match(content, /crux index/);
  assert.match(content, /crux map/);
  for (const marker of MARKETING_CRUX_READY_MARKERS) {
    assert.match(content, new RegExp(escapeRegExp(marker)));
  }
  assert.match(content, /What breaks if RefreshResult changes shape\?/);
  assert.match(
    content,
    /crux map RefreshResult --callers --references --impact/,
  );
  assert.match(content, /src\/api\/routes\/auth-refresh\.ts → RefreshResult/);
  assert.ok(content.split(/\r?\n/).length >= 50);
  assert.deepEqual(
    content
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .slice(-2),
    ["❯", "⏸ plan mode on (shift+tab to cycle)"],
  );
  assert.doesNotMatch(content, /BYPASSING PERMISSIONS ON/);
  assert.doesNotMatch(content, /Restart to update/);
});

test("all marketing states stage the founder savings data", async () => {
  assert.equal(MARKETING_SAVINGS_WEEK_TOKENS, 21_676_683);
  assert.equal(MARKETING_SAVINGS_ALL_TIME_TOKENS, 21_900_000);
  assert.equal(MARKETING_SAVINGS_ALL_TIME_USD, 101.16);
  assert.equal(MARKETING_SAVINGS_SESSION_TOKENS, 44_876);

  for (const entry of MARKETING_EXPORTS) {
    const selection = readHarnessSelection(`?state=${entry.state}`);
    const mock = createUxAuditHarnessMock(selection);
    const totals = await mock.invoke<SavingsTotals>("storage_total_savings");
    const week = await mock.invoke<SavingsToday>("storage_savings_week");
    const stats = await mock.invoke<SidecarStats>("sidecar_stats");
    const projects = await mock.invoke<Array<{ name: string; rootPath: string }>>(
      "project_list",
    );
    const terminals = await mock.invoke<TerminalSession[]>("terminal_list");
    const claudeUsage = await mock.invoke<{
      windows: Array<{ label: string; usedPercent: number; resetsAt: string }>;
      unavailableReason: string | null;
    }>("agent_claude_usage_windows");
    const codexUsage = await mock.invoke<{
      planType: string | null;
      windows: Array<{ label: string; usedPercent: number; resetsAt: string }>;
    }>("agent_codex_usage_windows");

    assert.equal(totals.tokensSaved, MARKETING_SAVINGS_ALL_TIME_TOKENS);
    assert.equal(totals.savingsUsd, MARKETING_SAVINGS_ALL_TIME_USD);
    assert.equal(week.tokensSaved, MARKETING_SAVINGS_WEEK_TOKENS);
    assert.deepEqual(week.recentSavings, MARKETING_RECENT_SAVINGS);
    assert.equal(totals.sessionTokensSaved, MARKETING_SAVINGS_SESSION_TOKENS);
    assert.equal(totals.cruxTokensSaved, MARKETING_ENGINE_SAVINGS.crux);
    assert.equal(stats.rtkTokensSaved, MARKETING_ENGINE_SAVINGS.rtk);
    assert.equal(stats.leanCtxTokensSaved, MARKETING_ENGINE_SAVINGS.leanContext);
    assert.equal(stats.toolSearchTokensSaved, MARKETING_ENGINE_SAVINGS.toolSearch);
    assert.equal(
      stats.compressionTokensSaved,
      MARKETING_ENGINE_SAVINGS.compression,
    );
    assert.deepEqual(claudeUsage, {
      windows: MARKETING_CLAUDE_USAGE_WINDOWS,
      unavailableReason: null,
    });
    assert.deepEqual(codexUsage, {
      planType: null,
      windows: MARKETING_CODEX_USAGE_WINDOWS,
    });
    assert.deepEqual(stats.planWindows, []);
    assert.ok(terminals.some((terminal) => Boolean(terminal.conversationId)));
    assert.ok(
      projects.some(
        (project) =>
          project.name === "halv" &&
          project.rootPath === "/Users/demo/programming/halv",
      ),
    );
  }
});

function mapStorage(values: Map<string, string>) {
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  };
}

function marketingExport(
  state: string,
  width: number,
  height: number,
  colorScheme: "dark" | "light",
) {
  return {
    state,
    viewport: { width, height },
    deviceScaleFactor: 2,
    colorScheme,
  };
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function stripTerminalAnsi(value: string) {
  return value.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "");
}

for (const stateName of ["marketing-chat-dark", "marketing-chat-light"]) {
  test(`${stateName} prepares its versioned Claude chat and savings`, async () => {
    const selection = readHarnessSelection(`?state=${stateName}`);
    const values = new Map<string, string>([
      ["halv.agent-session-pro-mode", "true"],
    ]);
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
      removeItem: (key: string) => values.delete(key),
    };

    prepareHarnessStorage(selection, storage);
    assert.equal(new AgentSessionProModeStore(storage).getSnapshot(), false);

    const mock = createUxAuditHarnessMock(selection);
    const prepared = await prepareHarnessMarketingChatState(selection, {
      invoke: mock.invoke,
    });

    assert.deepEqual(prepared, {
      conversationId: "audit-marketing",
      conversationTitle: "Vite multi-page design system",
      modelId: "sonnet",
      modelLabel: "Sonnet 5",
      modelBadge: "via Claude Code",
      savingsTokens: 3_830,
    });
  });
}

for (const stateName of ["marketing-pro-dark", "marketing-pro-light"]) {
  test(`${stateName} prepares its agent store and terminal replay`, async () => {
    const selection = readHarnessSelection(`?state=${stateName}`);
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
      removeItem: (key: string) => values.delete(key),
    };

    prepareHarnessStorage(selection, storage);
    assert.equal(new AgentSessionProModeStore(storage).getSnapshot(), true);

    const mock = createUxAuditHarnessMock(selection);
    const store = new AgentSessionStore({
      probe: () => mock.invoke("agent_session_probe"),
      list: () => mock.invoke("agent_session_list"),
      laneOptions: () => mock.invoke("agent_session_lane_options"),
      create: (input) => mock.invoke("agent_session_create", input),
      send: (sessionId, text) =>
        mock.invoke("agent_session_send", { sessionId, text }),
      kill: (sessionId) => mock.invoke("agent_session_kill", { sessionId }),
      rename: (sessionId, title) =>
        mock.invoke("agent_session_rename", { sessionId, title }),
      subscribe: async (handler) =>
        mock.subscribe("agent-session-event", handler),
    });

    const prepared = await prepareHarnessStoreState(selection, {
      store,
      invoke: mock.invoke,
    });

    assert.ok(prepared);
    assert.equal(prepared.entry.session.mode, "terminal_embedded");
    assert.equal(prepared.entry.session.status, "running");
    assert.equal(prepared.entry.totals.inputTokens, 4_860);
    assert.equal(prepared.entry.totals.savingsTokens, 8_920);
    assert.match(prepared.attached.scrollback, /Review the release checklist/);
    assert.match(prepared.attached.scrollback, /Read.*package\.json, RELEASING\.md/);
    store.disconnect();
  });
}

test("harness flags reach storage, and a reload keeps the stored tree state", () => {
  const stored = JSON.stringify({ version: 1, view: "recent", collapsed: ["project:2"] });
  const values = new Map<string, string>([
    ["halv.sidebar.project-tree.v1", stored],
    ["halv.feature-flags.v1", JSON.stringify({ shellTabs: true })],
  ]);
  // An old ?sidebar= does nothing: the sidebar has no Recent view (ADR 0020).
  const selection = readHarnessSelection(
    "?state=project-tree-dark&flags=projectTree,bogus&sidebar=projects",
  );
  assert.equal(selection.flags, "projectTree,bogus");
  assert.equal("sidebar" in selection, false);
  prepareHarnessStorage(selection, mapStorage(values));
  // projectTree is on by default, so the stored value lists only the tab
  // flags, which are on by default and off on this page.
  assert.equal(
    values.get("halv.feature-flags.v1"),
    JSON.stringify({ workspaceTabs: false, shellTabs: false }),
  );
  assert.equal(values.get("halv.sidebar.project-tree.v1"), stored);

  // A visual state without ?flags= starts with every flag off, also the
  // flags that are on by default.
  prepareHarnessStorage(readHarnessSelection("?state=project-tree-dark"), mapStorage(values));
  assert.equal(
    values.get("halv.feature-flags.v1"),
    JSON.stringify({ projectTree: false, workspaceTabs: false, shellTabs: false }),
  );
  // A scenario page without ?flags= also starts with every flag off.
  values.set("halv.feature-flags.v1", JSON.stringify({ workspaceTabs: true }));
  prepareHarnessStorage(readHarnessSelection("?scenario=normal"), mapStorage(values));
  assert.equal(
    values.get("halv.feature-flags.v1"),
    JSON.stringify({ projectTree: false, workspaceTabs: false, shellTabs: false }),
  );
  assert.doesNotThrow(() => readHarnessSelection("?state=project-tree-dark&sidebar=grid"));
});

test("the worktree fixture lists a primary checkout and two worktrees", async () => {
  const mock = createUxAuditHarnessMock(readHarnessSelection("?state=project-tree-light"));
  const workspaces = await mock.invoke<Array<{ id: string; kind: string }>>(
    "workspace_list",
    { projectId: 1 },
  );
  assert.deepEqual(
    workspaces.map((workspace) => `${workspace.kind}:${workspace.id}`),
    ["primary:primary-1", "worktree:audit-wt-login", "worktree:audit-wt-docs"],
  );
  const conversations = await mock.invoke<
    Array<{ id: string; projectId?: number | null; workspaceId?: string | null; projectRootPath?: string | null }>
  >("storage_list_conversations");
  const byId = new Map(conversations.map((conversation) => [conversation.id, conversation]));
  assert.equal(byId.get("audit-proxy")?.workspaceId, "audit-wt-login");
  assert.match(byId.get("audit-proxy")?.projectRootPath ?? "", /worktrees\/a1b2\/halv$/);
  assert.equal(byId.get("audit-docs-sweep")?.workspaceId, "audit-wt-docs");
  assert.equal(byId.get("audit-agent-refactor")?.workspaceId, "primary-1");
  assert.equal(byId.get("audit-markdown")?.workspaceId, null);
  for (const conversation of conversations) {
    assert.equal(
      conversation.workspaceId == null,
      conversation.projectId == null,
      conversation.id,
    );
  }
  const current = await mock.invoke<{ id: string } | null>("workspace_current", {
    conversationId: "audit-proxy",
  });
  assert.equal(current?.id, "audit-wt-login");
  assert.equal(
    await mock.invoke("workspace_current", { conversationId: "audit-agent-refactor" }),
    null,
  );

  const created = await mock.invoke<{ workspaceId?: string | null; projectRootPath?: string | null }>(
    "storage_set_conversation_goal",
    { conversationId: null, goal: null, model: null, projectId: 1, workspaceId: "audit-wt-docs" },
  );
  assert.equal(created.workspaceId, "audit-wt-docs");
  assert.match(created.projectRootPath ?? "", /worktrees\/c3d4\/halv$/);
});

test("the harness overview lists every workspace of every project", async () => {
  const mock = createUxAuditHarnessMock(readHarnessSelection("?state=project-tree-dark"));
  const overview = await mock.invoke<Array<Record<string, unknown>>>("workspace_overview");
  assert.deepEqual(
    overview.map((workspace) => `${workspace.projectId}:${workspace.id}:${workspace.currentBranch}`),
    [
      "1:primary-1:main",
      "1:audit-wt-login:halv/login-flow",
      "1:audit-wt-docs:halv/docs-refresh",
      "2:primary-2:main",
    ],
  );
  for (const workspace of overview) {
    assert.equal("dirty" in workspace, false);
    assert.equal("conversationCount" in workspace, false);
    assert.equal(workspace.available, true);
  }
});

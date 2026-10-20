import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  AGENT_SESSION_PRO_MODE_HINT_SEEN_STORAGE_KEY,
  AGENT_SESSION_PRO_MODE_STORAGE_KEY,
  AgentSessionProModeStore,
  AgentSessionStore,
  agentSessionAgentLabel,
  agentSessionAgentSubtitle,
  agentSupportsInitialPrompt,
  agentSessionLaneLabel,
  agentSessionModeDecision,
  agentSessionModeForProMode,
  agentSessionProModeCaption,
  buildAgentSessionLane,
  claimAgentSessionProModeHint,
  defaultLaneKindForAgent,
  defaultLaneSelectionForAgent,
  emptyAgentSessionsState,
  probeForAgent,
  readAgentSessionProMode,
  reduceAgentSessionEvent,
  replaceAgentSessions,
  restartLaneForAgentSession,
  resolvedAgentSessionModelForConversation,
  sessionSavingsDisplay,
  sessionSavingsMeasurementNote,
  writeAgentSessionProMode,
  type AgentSession,
  type AgentSessionEventPayload,
  type AgentSessionLaneOptions,
} from "../src/agentSessions.ts";

const session: AgentSession = {
  id: "session-1",
  agentKind: "claude",
  laneKind: "claudeSubscription",
  mode: "gui",
  projectId: 42,
  cwd: "/tmp/project",
  pid: 123,
  status: "running",
  cliSessionId: null,
  createdAt: "2026-08-16T12:00:00Z",
  lastActiveAt: "2026-08-16T12:00:00Z",
  exitCode: null,
};

const laneOptions: AgentSessionLaneOptions = [
  {
    agentKind: "claude",
    lanes: [
      { kind: "claudeSubscription", loggedIn: true, credentialProfiles: [] },
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
    agentKind: "zai",
    lanes: [{ kind: "zaiGlm", loggedIn: null, credentialProfiles: [] }],
  },
  {
    agentKind: "grok",
    lanes: [{ kind: "grokApiKey", loggedIn: null, credentialProfiles: [] }],
  },
];

test("folds consecutive text deltas within one turn", () => {
  let state = replaceAgentSessions(emptyAgentSessionsState(), [session]);
  state = reduceAgentSessionEvent(state, event({ type: "turnStarted" }));
  state = reduceAgentSessionEvent(
    state,
    event({ type: "textDelta", text: "Hello" }),
  );
  state = reduceAgentSessionEvent(
    state,
    event({ type: "textDelta", text: " world" }),
  );
  state = reduceAgentSessionEvent(
    state,
    event({
      type: "toolCallStarted",
      name: "Read",
      summaryJson: '{"path":"README.md"}',
    }),
  );
  state = reduceAgentSessionEvent(
    state,
    event({ type: "textDelta", text: "Done." }),
  );

  assert.deepEqual(
    state.entries[0].transcript.map((item) => item.kind),
    ["assistantText", "toolCall", "assistantText"],
  );
  assert.equal(state.entries[0].transcript[0].kind, "assistantText");
  assert.equal(
    state.entries[0].transcript[0].kind === "assistantText"
      ? state.entries[0].transcript[0].text
      : null,
    "Hello world",
  );
});

test("marks a resolved permission request completed", () => {
  let state = replaceAgentSessions(emptyAgentSessionsState(), [session]);
  state = reduceAgentSessionEvent(state, event({ type: "turnStarted" }));
  state = reduceAgentSessionEvent(
    state,
    event({
      type: "permissionRequest",
      requestId: "permission-1",
      description: "Run tests",
      payloadJson: '{"command":"pnpm test"}',
    }),
  );

  state = reduceAgentSessionEvent(
    state,
    event({
      type: "permissionResolved",
      requestId: "permission-1",
      approved: false,
    }),
  );

  const item = state.entries[0].transcript[0];
  assert.equal(item.kind, "toolCall");
  assert.equal(item.kind === "toolCall" ? item.completed : null, true);
  assert.equal(
    item.kind === "toolCall" ? item.permissionRequest?.approved : null,
    false,
  );
  assert.equal(state.entries[0].session.status, "running");
});

test("updates live savings source availability from the event stream", () => {
  let state = replaceAgentSessions(emptyAgentSessionsState(), [session]);
  state = reduceAgentSessionEvent(
    state,
    event({
      type: "savingsSourcesChanged",
      savingsSources: {
        rtk: "instruction",
        crux: "missing",
        proxy: true,
      },
    }),
  );

  assert.deepEqual(state.entries[0].session.savingsSources, {
    rtk: "instruction",
    crux: "missing",
    proxy: true,
  });
});

test("embedded turn lifecycle does not replace process status", () => {
  const embeddedSession: AgentSession = {
    ...session,
    agentKind: "codex",
    laneKind: "codexChatGpt",
    mode: "terminal_embedded",
  };
  let state = replaceAgentSessions(emptyAgentSessionsState(), [embeddedSession]);

  state = reduceAgentSessionEvent(
    state,
    event({ type: "embeddedTurnStarted", turnId: "turn-1" }),
  );
  state = reduceAgentSessionEvent(
    state,
    event({
      type: "embeddedTurnFinished",
      turnId: "turn-1",
      outcome: "completed",
    }),
  );

  assert.equal(state.entries[0].session.status, "running");
  assert.equal(state.entries[0].turnActive, false);
  assert.deepEqual(state.entries[0].turns, []);
  assert.deepEqual(state.entries[0].transcript, []);
});

test("tracks usage and savings from a mocked event stream", async () => {
  let handler: ((payload: AgentSessionEventPayload) => void) | null = null;
  let createdAgentKind: string | null = null;
  let sentText: string | null = null;
  let stopped = false;
  const store = new AgentSessionStore({
    probe: async () => ({
      claude: { found: true, version: "1.0.0" },
      codex: { found: true, version: "2.0.0" },
    }),
    list: async () => [],
    laneOptions: async () => laneOptions,
    create: async (input) => {
      createdAgentKind = input.agentKind;
      return session;
    },
    send: async (_sessionId, text) => {
      sentText = text;
    },
    kill: async () => ({ ...session, status: "exited", exitCode: 0 }),
    rename: async (_sessionId, customTitle) => ({ ...session, customTitle }),
    subscribe: async (nextHandler) => {
      handler = nextHandler;
      return () => {
        stopped = true;
      };
    },
  });

  await store.connect();
  assert.equal(store.getSnapshot().laneOptions[0]?.agentKind, "claude");
  await store.create({
    agentKind: "claude",
    lane: { kind: "claudeSubscription" },
    cwd: session.cwd,
    projectId: 42,
  });
  await store.send(session.id, "Build the feature");
  assert.equal(createdAgentKind, "claude");
  assert.equal(sentText, "Build the feature");
  assert.ok(handler);
  const emit = handler as (payload: AgentSessionEventPayload) => void;
  emit(event({ type: "turnStarted" }));
  emit(event({ type: "textDelta", text: "A" }));
  emit(event({ type: "textDelta", text: "gent" }));
  emit(
    event({
      type: "permissionRequest",
      requestId: "permission-1",
      description: "Run the tests",
      payloadJson: '{"command":"pnpm test"}',
    }),
  );
  emit(
    event({
      type: "turnCompleted",
      usage: {
        inputTokens: 1_000,
        outputTokens: 200,
        cacheReadTokens: 400,
        cacheCreationTokens: 50,
        totalCostUsd: 0.04,
        savingsTokens: 300,
        savingsSource: "headroom",
      },
    }),
  );

  const entry = store.getSnapshot().entries[0];
  assert.equal(entry.transcript[0].kind, "userText");
  assert.equal(entry.transcript[1].kind, "assistantText");
  assert.equal(
    entry.transcript[1].kind === "assistantText"
      ? entry.transcript[1].text
      : null,
    "Agent",
  );
  assert.equal(entry.transcript[2].kind, "toolCall");
  assert.ok(
    entry.transcript[2].kind === "toolCall" &&
      entry.transcript[2].permissionRequest,
  );
  assert.deepEqual(entry.totals, {
    inputTokens: 1_000,
    outputTokens: 200,
    cacheReadTokens: 400,
    cacheCreationTokens: 50,
    totalCostUsd: 0.04,
    savingsTokens: 300,
    savingsUsd: 0,
    dollarsUsd: 0,
    pricingSource: null,
    rtkTokensSaved: 0,
    rtkIncludedInTotal: false,
    cruxTokensSaved: 0,
    cruxIncludedInTotal: false,
  });
  assert.equal(entry.session.status, "waiting_input");

  await store.kill(session.id);
  assert.equal(store.getSnapshot().entries[0].session.status, "exited");

  await store.rename(session.id, "Release terminal");
  assert.equal(
    store.getSnapshot().entries[0].session.customTitle,
    "Release terminal",
  );

  store.disconnect();
  assert.equal(stopped, true);
});

test("keeps a renamed session title after a store refresh", async () => {
  let persisted = { ...session, customTitle: null as string | null };
  const store = new AgentSessionStore({
    probe: async () => ({ claude: { found: true, version: "1.0.0" } }),
    list: async () => [persisted],
    laneOptions: async () => laneOptions,
    create: async () => persisted,
    send: async () => undefined,
    kill: async () => persisted,
    rename: async (_sessionId, customTitle) => {
      persisted = { ...persisted, customTitle };
      return persisted;
    },
    subscribe: async () => () => undefined,
  });

  await store.connect();
  await store.rename(session.id, "Release terminal");
  await store.refreshSessions();

  assert.equal(
    store.getSnapshot().entries[0].session.customTitle,
    "Release terminal",
  );
  store.disconnect();
});

test("Kimi subscription note reports RTK instruction guidance", () => {
  assert.equal(
    sessionSavingsMeasurementNote("kimiSubscription", {
      rtk: "instruction",
      crux: "present",
      proxy: true,
    }),
    "crux + RTK (by instruction) · proxy on",
  );
});

test("uses measured savings when the backend supplies savings tokens", () => {
  assert.deepEqual(
    sessionSavingsDisplay({
      inputTokens: 100,
      outputTokens: 20,
      cacheReadTokens: 10,
      cacheCreationTokens: 0,
      totalCostUsd: 0.02,
      savingsTokens: 42,
      savingsSource: "headroom",
    }),
    {
      label: "measured",
      text: "~42 tokens saved",
      tokenCount: 42,
      source: "headroom",
      costUsd: 0.02,
    },
  );
});

test("shows a non-zero measured header after a terminal savings poll", () => {
  let state = replaceAgentSessions(emptyAgentSessionsState(), [
    { ...session, mode: "terminal_embedded" },
  ]);
  state = reduceAgentSessionEvent(
    state,
    event({
      type: "savingsSample",
      savingsTokens: 55,
      savingsSource: "measured",
    }),
  );

  assert.equal(state.entries[0].session.status, "running");
  assert.equal(state.entries[0].turns.at(-1)?.savingsTokens, 55);
  assert.equal(state.entries[0].totals.savingsTokens, 55);
  const header = sessionSavingsDisplay(state.entries[0].turns.at(-1));
  assert.equal(header.label, "measured");
  assert.ok(header.tokenCount > 0);
});

test("keeps absent savings unmeasured and estimates observed cache use", () => {
  assert.deepEqual(sessionSavingsDisplay(null), {
    label: "unmeasured",
    text: "Savings not measured",
    tokenCount: 0,
    source: null,
    costUsd: null,
  });
  assert.deepEqual(
    sessionSavingsDisplay({
      inputTokens: 100,
      outputTokens: 20,
      cacheReadTokens: 75,
      cacheCreationTokens: 0,
      totalCostUsd: 0.03,
    }),
    {
      label: "estimated",
      text: "75 tokens served from cache",
      tokenCount: 75,
      source: null,
      costUsd: 0.03,
    },
  );
});

test("selects the expected default lane for each agent card", () => {
  assert.equal(
    defaultLaneKindForAgent(laneOptions, "claude"),
    "claudeSubscription",
  );
  assert.equal(defaultLaneKindForAgent(laneOptions, "codex"), "codexChatGpt");
  assert.equal(defaultLaneKindForAgent(laneOptions, "gemini"), "custom");
  assert.equal(defaultLaneKindForAgent(laneOptions, "antigravity"), "custom");
  assert.equal(defaultLaneKindForAgent(laneOptions, "zai"), "zaiGlm");
  assert.equal(defaultLaneKindForAgent(laneOptions, "grok"), "grokApiKey");
  assert.equal(
    defaultLaneKindForAgent(laneOptions, "kimi"),
    "kimiSubscription",
  );
});

test("uses only documented interactive initial prompt arguments", () => {
  assert.equal(agentSupportsInitialPrompt("gemini"), true);
  assert.equal(agentSupportsInitialPrompt("antigravity"), false);
});

test("prefers a signed-in subscription, then an active stored profile", () => {
  const profile = {
    id: "anthropic-1",
    provider: "anthropic",
    name: "Work key",
    kind: "api_key",
    priority: 0,
    status: "active" as const,
    lastValidatedAt: null,
    metadataJson: null,
    createdAt: "2026-08-17T00:00:00Z",
    updatedAt: "2026-08-17T00:00:00Z",
  };
  const options: AgentSessionLaneOptions = [
    {
      agentKind: "claude",
      lanes: [
        {
          kind: "claudeSubscription",
          loggedIn: false,
          credentialProfiles: [],
        },
        {
          kind: "claudeApiKey",
          loggedIn: null,
          credentialProfiles: [profile],
        },
      ],
    },
  ];

  assert.deepEqual(defaultLaneSelectionForAgent(options, "claude"), {
    laneKind: "claudeApiKey",
    credentialProfileId: "anthropic-1",
  });
  options[0].lanes[0].loggedIn = true;
  assert.deepEqual(defaultLaneSelectionForAgent(options, "claude"), {
    laneKind: "claudeSubscription",
    credentialProfileId: null,
  });
});

test("builds profile lanes without raw secrets and proxies Codex by default", () => {
  assert.deepEqual(buildAgentSessionLane("claudeApiKey", "anthropic-1", ""), {
    kind: "claudeApiKey",
    credentialProfileId: "anthropic-1",
  });
  assert.deepEqual(buildAgentSessionLane("zaiGlm", "zai-1", ""), {
    kind: "zaiGlm",
    credentialProfileId: "zai-1",
  });
  assert.deepEqual(buildAgentSessionLane("grokApiKey", "xai-1", ""), {
    kind: "grokApiKey",
    credentialProfileId: "xai-1",
  });
  assert.deepEqual(buildAgentSessionLane("codexChatGpt", null, ""), {
    kind: "codexChatGpt",
    proxied: true,
  });
  assert.throws(
    () => buildAgentSessionLane("kimiApiKey", null, ""),
    /credential profile/,
  );
});

test("parses custom variables only when the custom lane is built", () => {
  assert.deepEqual(
    buildAgentSessionLane("custom", null, "MODEL=glm-5\nEMPTY="),
    {
      kind: "custom",
      env: [
        ["MODEL", "glm-5"],
        ["EMPTY", ""],
      ],
    },
  );
  assert.throws(
    () => buildAgentSessionLane("custom", null, "NOT VALID=value"),
    /Invalid environment entry/,
  );
});

test("maps Pro mode to Ghostty only when its automation is ready", () => {
  assert.equal(agentSessionModeForProMode(false, null), "gui");
  assert.equal(
    agentSessionModeForProMode(true, {
      found: true,
      version: "1.3.0",
      applescriptReady: true,
    }),
    "terminal_ghostty",
  );
  assert.equal(
    agentSessionModeForProMode(true, {
      found: true,
      version: "1.2.0",
      applescriptReady: false,
    }),
    "terminal_embedded",
  );
  assert.equal(
    agentSessionProModeCaption({
      found: true,
      version: "1.3.0",
      applescriptReady: true,
    }),
    "New conversations open in Ghostty",
  );
  assert.equal(
    agentSessionProModeCaption(null),
    "New conversations open in the built-in terminal",
  );
});

test("switches each resumable agent kind between GUI and terminal modes", () => {
  for (const agentKind of [
    "antigravity",
    "claude",
    "codex",
    "gemini",
    "kimi",
    "zai",
    "grok",
  ] as const) {
    const gui = {
      ...session,
      agentKind,
      status: "waiting_input" as const,
      cliSessionId: `${agentKind}-session`,
    };
    assert.deepEqual(agentSessionModeDecision(true, null, gui), {
      action: "replace",
      mode: "terminal_embedded",
    });
    assert.deepEqual(
      agentSessionModeDecision(false, null, {
        ...gui,
        mode: "terminal_embedded",
        status: "running",
      }),
      { action: "replace", mode: "gui" },
    );
  }
});

test("waits for a safe resumable boundary before switching session mode", () => {
  assert.deepEqual(agentSessionModeDecision(true, null, session), {
    action: "wait",
    mode: "terminal_embedded",
  });
  assert.deepEqual(
    agentSessionModeDecision(true, null, {
      ...session,
      cliSessionId: "claude-session",
    }),
    { action: "wait", mode: "terminal_embedded" },
  );
  assert.deepEqual(agentSessionModeDecision(false, null, session), {
    action: "keep",
    mode: "gui",
  });
});

test("restarts subscription composer sessions on their matching lane", () => {
  assert.deepEqual(restartLaneForAgentSession(session), {
    kind: "claudeSubscription",
  });
  assert.deepEqual(
    restartLaneForAgentSession({ laneKind: "codexChatGpt" }),
    { kind: "codexChatGpt", proxied: true },
  );
  assert.equal(
    restartLaneForAgentSession({ laneKind: "claudeApiKey" }),
    null,
  );
  assert.deepEqual(
    restartLaneForAgentSession({ laneKind: "zaiGlm" }, "zai-1"),
    { kind: "zaiGlm", credentialProfileId: "zai-1" },
  );
  assert.deepEqual(
    restartLaneForAgentSession({ laneKind: "grokApiKey" }, "xai-1"),
    { kind: "grokApiKey", credentialProfileId: "xai-1" },
  );
  assert.deepEqual(
    restartLaneForAgentSession({ laneKind: "kimiApiKey" }, "moonshot-1"),
    { kind: "kimiApiKey", credentialProfileId: "moonshot-1" },
  );
});

test("uses the Claude binary probe for proxied Anthropic-compatible agents", () => {
  const claudeProbe = { found: true, version: "2.1.0" };
  assert.deepEqual(
    probeForAgent({ claude: claudeProbe }, "zai"),
    claudeProbe,
  );
  assert.deepEqual(
    probeForAgent({ claude: claudeProbe }, "grok"),
    claudeProbe,
  );
});

test("uses distinct Gemini and Antigravity binary probes", () => {
  const gemini = { found: true, version: "0.50.0" };
  const antigravity = { found: true, version: "1.0.0" };
  const probe = { gemini, antigravity };

  assert.deepEqual(probeForAgent(probe, "gemini"), gemini);
  assert.deepEqual(probeForAgent(probe, "antigravity"), antigravity);
});

test("uses GLM-first labels for the Z.ai Coding Plan lane", () => {
  assert.equal(agentSessionAgentLabel("zai"), "GLM");
  assert.equal(agentSessionAgentSubtitle("zai"), "Z.ai Coding Plan");
  assert.equal(agentSessionLaneLabel("zaiGlm"), "GLM Coding Plan");
});

test("uses Grok and xAI labels for the Grok API lane", () => {
  assert.equal(agentSessionAgentLabel("grok"), "Grok");
  assert.equal(agentSessionAgentSubtitle("grok"), "xAI API");
  assert.equal(agentSessionLaneLabel("grokApiKey"), "Grok API key");
});

test("persists the Pro mode preference", () => {
  const values = new Map<string, string>();
  const storage = {
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };

  // Pro is the default; only an explicit stored choice changes it.
  assert.equal(readAgentSessionProMode(storage), true);
  assert.equal(
    AGENT_SESSION_PRO_MODE_STORAGE_KEY,
    "halv.agent-session-pro-mode",
  );
  writeAgentSessionProMode(false, storage);
  assert.equal(values.get(AGENT_SESSION_PRO_MODE_STORAGE_KEY), "false");
  assert.equal(readAgentSessionProMode(storage), false);
  writeAgentSessionProMode(true, storage);
  assert.equal(readAgentSessionProMode(storage), true);
});

test("claims the first Pro mode hint only once", () => {
  const values = new Map<string, string>();
  const storage = {
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };

  assert.equal(claimAgentSessionProModeHint(storage), true);
  assert.equal(claimAgentSessionProModeHint(storage), false);
  assert.equal(
    values.get(AGENT_SESSION_PRO_MODE_HINT_SEEN_STORAGE_KEY),
    "true",
  );
});

test("keeps composer and panel Pro mode subscribers synchronized", () => {
  const values = new Map<string, string>();
  const storage = {
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
  const store = new AgentSessionProModeStore(storage);
  let composerValue = store.getSnapshot();
  let panelValue = store.getSnapshot();
  const unsubscribeComposer = store.subscribe(() => {
    composerValue = store.getSnapshot();
  });
  const unsubscribePanel = store.subscribe(() => {
    panelValue = store.getSnapshot();
  });

  store.toggle();

  // Default starts true; the first toggle lands on false for everyone.
  assert.equal(composerValue, false);
  assert.equal(panelValue, false);
  assert.equal(values.get(AGENT_SESSION_PRO_MODE_STORAGE_KEY), "false");
  unsubscribeComposer();
  unsubscribePanel();
});

test("shows the resolved Claude model after the session init event", () => {
  let state = reduceAgentSessionEvent(
    emptyAgentSessionsState(),
    event({
      type: "sessionInit",
      cliSessionId: "claude-session-1",
      model: "claude-fable-5",
    }),
  );
  state = replaceAgentSessions(state, [
    { ...session, conversationId: "conversation-1", model: "fable" },
  ]);

  assert.equal(
    resolvedAgentSessionModelForConversation(state, "conversation-1"),
    "claude-fable-5",
  );
  assert.equal(resolvedAgentSessionModelForConversation(state, null), null);
});

function event(
  agentEvent: AgentSessionEventPayload["event"],
): AgentSessionEventPayload {
  return { sessionId: session.id, event: agentEvent };
}

import type { AgentSession, AgentSessionEvent } from "../agentSessions.ts";
import type { TerminalSession } from "../types.ts";
import { MARKETING_SESSION_TOKENS } from "./marketing-realism.ts";

export const MARKETING_TERMINAL_CONVERSATION_ID = "audit-marketing";

export const MARKETING_PRO_SPLIT_STATE_NAMES = [
  "marketing-pro-split-dark",
  "marketing-pro-split-light",
] as const;

export const MARKETING_CRUX_STATE_NAMES = [
  "marketing-crux-dark",
  "marketing-crux-light",
] as const;

export const MARKETING_PRO_SPLIT_SESSION_IDS = {
  plan: "marketing-six-claude-plan",
  grok: "marketing-six-grok",
  codex: "marketing-six-codex",
  kimi: "marketing-six-kimi",
  glm: "marketing-six-glm",
  editing: "marketing-six-claude-editing",
} as const;

export const MARKETING_CRUX_SESSION_ID = "marketing-crux-structural-map";

const MARKETING_PRO_SPLIT_CONVERSATION_TITLE = "Parallel release hardening";
const MARKETING_CRUX_CONVERSATION_TITLE = "Map authentication architecture";
const MARKETING_PROJECT_CWD = "/Users/demo/programming/halv";

export const MARKETING_PRO_SPLIT_AGENT_SESSIONS: AgentSession[] = [
  marketingSession({
    id: MARKETING_PRO_SPLIT_SESSION_IDS.plan,
    defaultTitle: "Claude 2",
    agentKind: "claude",
    laneKind: "claudeSubscription",
    model: "sonnet",
    pid: 4411,
    status: "running",
    conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
  }),
  marketingSession({
    id: MARKETING_PRO_SPLIT_SESSION_IDS.grok,
    defaultTitle: "Grok 1",
    agentKind: "grok",
    laneKind: "grokApiKey",
    model: "grok-4.6",
    pid: 4412,
    status: "running",
    conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
  }),
  marketingSession({
    id: MARKETING_PRO_SPLIT_SESSION_IDS.codex,
    defaultTitle: "Codex 2",
    agentKind: "codex",
    laneKind: "codexChatGpt",
    model: "gpt-5.6-sol",
    pid: 4413,
    status: "running",
    conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
  }),
  marketingSession({
    id: MARKETING_PRO_SPLIT_SESSION_IDS.kimi,
    defaultTitle: "Kimi 1",
    agentKind: "kimi",
    laneKind: "kimiSubscription",
    model: "kimi-k2.5",
    pid: 4414,
    status: "starting",
    conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
  }),
  marketingSession({
    id: MARKETING_PRO_SPLIT_SESSION_IDS.glm,
    defaultTitle: "GLM 1",
    agentKind: "zai",
    laneKind: "zaiGlm",
    model: "glm-5",
    pid: 4415,
    status: "exited",
    conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
  }),
  marketingSession({
    id: MARKETING_PRO_SPLIT_SESSION_IDS.editing,
    defaultTitle: "Claude 4",
    agentKind: "claude",
    laneKind: "claudeSubscription",
    model: "opus",
    pid: 4416,
    status: "running",
    conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
  }),
];

export const MARKETING_CRUX_AGENT_SESSIONS: AgentSession[] = [
  marketingSession({
    id: MARKETING_CRUX_SESSION_ID,
    defaultTitle: "Claude 6",
    agentKind: "claude",
    laneKind: "claudeSubscription",
    model: "sonnet",
    pid: 4511,
    status: "running",
    conversationTitle: MARKETING_CRUX_CONVERSATION_TITLE,
  }),
];

export const MARKETING_PRO_SPLIT_TERMINAL_SESSIONS =
  MARKETING_PRO_SPLIT_AGENT_SESSIONS.map(marketingTerminal);

export const MARKETING_CRUX_TERMINAL_SESSIONS =
  MARKETING_CRUX_AGENT_SESSIONS.map(marketingTerminal);

export const MARKETING_PRO_SPLIT_AGENT_EVENTS: Record<
  string,
  AgentSessionEvent[]
> = Object.fromEntries(
  MARKETING_PRO_SPLIT_AGENT_SESSIONS.filter(
    (session) => session.status === "running",
  ).map((session) => [
    session.id,
    [
        {
          type: "sessionInit",
          cliSessionId: session.id,
          model: session.model ?? "unknown",
        },
        { type: "turnStarted" },
        {
          type: "savingsSample",
          savingsTokens: 31_204,
          savingsSource: "measured",
          cacheReadTokens: 18_240,
          turnBoundary: false,
        },
    ] satisfies AgentSessionEvent[],
  ]),
);

export const MARKETING_CRUX_AGENT_EVENTS: Record<string, AgentSessionEvent[]> = {
  [MARKETING_CRUX_SESSION_ID]: [
    {
      type: "sessionInit",
      cliSessionId: MARKETING_CRUX_SESSION_ID,
      model: "sonnet",
    },
    { type: "turnStarted" },
    {
      type: "savingsSample",
      savingsTokens: MARKETING_SESSION_TOKENS,
      savingsSource: "measured",
      cacheReadTokens: 18_240,
      turnBoundary: false,
    },
  ],
};

export const MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK: Record<string, string> = {
  [MARKETING_PRO_SPLIT_SESSION_IDS.plan]: terminalBuffer([
    "\x1b[1;38;5;117mClaude Code\x1b[0m  \x1b[2mSonnet 5\x1b[0m",
    "",
    "\x1b[1;38;5;117m❯\x1b[0m Plan the safe session-store migration.",
    "",
    "\x1b[36m● crux map\x1b[0m rotateSession",
    "  \x1b[2m↳ 18 callers · 6 references · 3 test seams\x1b[0m",
    "\x1b[36m● Read\x1b[0m(src/auth/session-store.ts)",
    "\x1b[36m● Read\x1b[0m(tests/auth/session-store.test.ts)",
    "",
    "\x1b[1m● Plan ready\x1b[0m",
    "  1. Preserve the rotation boundary.",
    "  2. Move persistence behind one adapter.",
    "  3. Prove rollback with focused tests.",
    "",
    "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
  ]),
  [MARKETING_PRO_SPLIT_SESSION_IDS.grok]: terminalBuffer([
    "\x1b[1;38;5;117mGrok\x1b[0m  \x1b[2mgrok-4.6 via xAI\x1b[0m",
    "",
    "\x1b[1;38;5;117m❯\x1b[0m Run the auth regression suite.",
    "",
    "\x1b[36m● Bash\x1b[0m pnpm test --filter auth",
    "  \x1b[2m3,104 lines → 41 lines\x1b[0m",
    "  PASS token rotation (18)",
    "  PASS middleware guards (24)",
    "  PASS refresh recovery (11)",
    "",
    "\x1b[2mRunning integration tests…\x1b[0m",
    "\x1b[38;5;214m⏵⏵ bypass permissions on · 2 shells, 1 monitor\x1b[0m",
  ]),
  [MARKETING_PRO_SPLIT_SESSION_IDS.codex]: terminalBuffer([
    "\x1b[1;38;5;81mOpenAI Codex\x1b[0m  \x1b[2mgpt-5.6-sol\x1b[0m",
    "",
    "\x1b[1;38;5;81m›\x1b[0m Repair refresh-token replay handling.",
    "",
    "• Inspected 9 call sites",
    "• Editing src/auth/refresh-token.ts",
    "• Keeping the public response contract stable",
    "",
    "\x1b[2mRunning focused verification (23s)…\x1b[0m",
  ]),
  [MARKETING_PRO_SPLIT_SESSION_IDS.kimi]: terminalBuffer([
    "\x1b[1;38;5;141mKimi\x1b[0m  \x1b[2mK2.5\x1b[0m",
    "",
    "\x1b[1;38;5;141m❯\x1b[0m Review the migration after implementation.",
    "",
    "\x1b[38;5;214m● QUEUED\x1b[0m",
    "  waiting on the diff",
    "",
    "\x1b[2mWill check API compatibility and error paths.\x1b[0m",
  ]),
  [MARKETING_PRO_SPLIT_SESSION_IDS.glm]: terminalBuffer([
    "\x1b[1;38;5;120mGLM\x1b[0m  \x1b[2mCoding Plan\x1b[0m",
    "",
    "\x1b[1;38;5;120m❯\x1b[0m Refresh authentication docs.",
    "",
    "\x1b[36m● Update\x1b[0m(docs/authentication.md)",
    "\x1b[36m● Update\x1b[0m(docs/session-rotation.md)",
    "",
    "\x1b[38;5;120m✓ COMPLETED\x1b[0m",
    "  docs pass · saved 41.2K tokens",
  ]),
  [MARKETING_PRO_SPLIT_SESSION_IDS.editing]: terminalBuffer([
    "\x1b[1;38;5;117mClaude Code\x1b[0m  \x1b[2mOpus 5.5\x1b[0m",
    "",
    "\x1b[1;38;5;117m❯\x1b[0m Apply the approved adapter change.",
    "",
    "\x1b[36m● Edit\x1b[0m(src/auth/session-adapter.ts)",
    "\x1b[36m● Edit\x1b[0m(src/auth/session-store.ts)",
    "\x1b[36m● Edit\x1b[0m(tests/auth/session-store.test.ts)",
    "",
    "\x1b[2mEditing rollback coverage…\x1b[0m",
  ]),
};

export const MARKETING_CRUX_TERMINAL_SCROLLBACK: Record<string, string> = {
  [MARKETING_CRUX_SESSION_ID]: terminalBuffer([
    "\x1b[1;38;5;117mClaude Code\x1b[0m  \x1b[2mSonnet 5 · Plan Mode\x1b[0m",
    "",
    "\x1b[1;38;5;117m❯\x1b[0m Who calls refreshSession? What breaks if it changes?",
    "",
    "\x1b[36m● crux index\x1b[0m src/auth",
    "  \x1b[2m↳ ready · 2,418 symbols · 684 relationships\x1b[0m",
    "",
    "\x1b[36m● crux map\x1b[0m refreshSession --callers --references --impact",
    "",
    "\x1b[1mCALLERS  6\x1b[0m",
    "  api/auth/refresh.ts → refreshSession",
    "  middleware/require-session.ts → refreshSession",
    "  jobs/rotate-session.ts → refreshSession",
    "",
    "\x1b[1mREFERENCES  9\x1b[0m",
    "  SessionStore.refreshSession",
    "  RefreshResult · SessionRotationError",
    "  tests/auth/refresh-session.test.ts",
    "",
    "\x1b[1mIMPACT\x1b[0m",
    "  Changes affect HTTP refresh, middleware recovery,",
    "  background rotation, and three contract tests.",
    "",
    "\x1b[38;5;120m✓ Structural answer ready in 1.2s\x1b[0m",
    "",
    "\x1b[1;38;5;117m❯\x1b[0m What breaks if RefreshResult changes shape?",
    "",
    "\x1b[36m● crux map\x1b[0m RefreshResult --callers --references --impact",
    "  \x1b[2m↳ 11 callers · 7 references · 4 contract seams\x1b[0m",
    "",
    "\x1b[1mCALLERS  4\x1b[0m",
    "  src/auth/refresh-session.ts → RefreshResult",
    "  src/api/routes/auth-refresh.ts → RefreshResult",
    "  src/middleware/session-recovery.ts → RefreshResult",
    "  src/jobs/session-rotation.ts → RefreshResult",
    "",
    "\x1b[1mREFERENCES  7\x1b[0m",
    "  src/auth/types.ts: RefreshResult",
    "  src/auth/session-store.ts: rotateSession",
    "  tests/auth/refresh-result.test.ts",
    "",
    "\x1b[1mIMPACT\x1b[0m",
    "  Changing the shape touches response serialization,",
    "  retry recovery, stored-session migration, and four tests.",
    "",
    "\x1b[36m● Read\x1b[0m(src/auth/types.ts)",
    "  \x1b[2mRead 1 file\x1b[0m",
    "\x1b[36m● Read\x1b[0m(tests/auth/refresh-result.test.ts)",
    "  \x1b[2mRead 1 file\x1b[0m",
    "",
    "\x1b[1m●\x1b[0m Keep the adapter boundary until callers migrate.",
    "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
  ]),
};

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.plan] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.plan]
    .replace(
      "\r\n\r\n\x1b[1;38;5;117m❯",
      "\r\n\r\n\x1b[38;5;117m✳ Gusting… (2m 54s · ↓ 6.0k tokens)\x1b[0m\r\n\r\n\x1b[1;38;5;117m❯",
    )
    .replace(" \x1b[2m↳ 18 callers", "  \x1b[2m18 callers")
    .replace(
      "\x1b[36m● Read\x1b[0m(src/auth/session-store.ts)",
      "\x1b[36m● Read\x1b[0m(src/auth/session-store.ts)\r\n  \x1b[2mRead 1 file\x1b[0m",
    )
    .replace(
      "\x1b[36m● Read\x1b[0m(tests/auth/session-store.test.ts)",
      "\x1b[36m● Read\x1b[0m(tests/auth/session-store.test.ts)\r\n  \x1b[2mRead 1 file\x1b[0m",
    )
    .replace(
      "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
      "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
    )
    .replace(
      "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
      "\x1b[38;5;117m❯ ○ codex:codex-rescue Codex: fix shots export f… 21s · ↓ 12.6k tokens\x1b[0m" +
        "\r\n\x1b[1;38;5;117m❯\x1b[0m" +
        "\r\n\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
    );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.grok] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.grok]
    .replace(
      "\r\n\r\n\x1b[1;38;5;117m❯",
      "\r\n\r\n\x1b[38;5;117m✳ Churned 4m 9s · 1 shell still running\x1b[0m\r\n\r\n\x1b[1;38;5;117m❯",
    )
    .replace(" \x1b[2m3,104", "  \x1b[2m3,104")
    .replace(
      "\x1b[38;5;214m⏵⏵ bypass permissions on · 2 shells, 1 monitor\x1b[0m",
      "\x1b[1;38;5;117m❯\x1b[0m\r\n" +
        "\x1b[38;5;214m⏵⏵ bypass permissions on · 2 shells, 1 monitor\x1b[0m",
    );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.codex] +=
  "\r\n\r\n\x1b[38;5;81m›\x1b[0m Ask Codex to do anything" +
  "\r\n\x1b[38;5;229mgpt-5.6-sol xhigh fast\x1b[0m · " +
  "\x1b[38;5;120m~/programming/halv\x1b[0m";

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.editing] +=
  "\r\n  \x1b[2mRead 1 file\x1b[0m" +
  "\r\n\x1b[38;5;117m✳ Worked 1m 4s\x1b[0m" +
  "\r\n\x1b[1;38;5;117m❯\x1b[0m" +
  "\r\n\x1b[38;5;214m⏵⏵ auto mode on (shift+tab to cycle) · ⇐ 4 agents\x1b[0m";

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.plan] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[
    MARKETING_PRO_SPLIT_SESSION_IDS.plan
  ].replace(
    "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
    [
      "\x1b[36m● Read\x1b[0m(src/auth/session-adapter.ts)",
      " \x1b[2mRead 1 file\x1b[0m",
      "\x1b[36m● crux index\x1b[0m rotateSession",
      " \x1b[2mIndexed 18 callers and 6 references\x1b[0m",
      "\x1b[36m● Background command \"pnpm test --filter session-store\" completed (exit code 0)\x1b[0m",
      " \x1b[2mRan 3 shell commands\x1b[0m",
      "\x1b[1m●\x1b[0m Rollback remains behind the adapter boundary.",
      "",
      "\x1b[1;38;5;117m❯\x1b[0m",
      "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
    ].join("\r\n"),
  );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.grok] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[
    MARKETING_PRO_SPLIT_SESSION_IDS.grok
  ].replace(
    "\x1b[1;38;5;117m❯\x1b[0m\r\n\x1b[38;5;214m⏵⏵ bypass permissions on · 2 shells, 1 monitor\x1b[0m",
    [
      "\x1b[36m● Background command \"pnpm test --filter auth\" completed (exit code 0)\x1b[0m",
      " \x1b[2mRan 3 shell commands\x1b[0m",
      "\x1b[36m● Read\x1b[0m(tests/auth/refresh-recovery.test.ts)",
      " \x1b[2mRead 1 file\x1b[0m",
      "\x1b[38;5;117m✳ Churned 4m 9s · 1 shell still running\x1b[0m",
      "\x1b[1m●\x1b[0m Unit and integration groups remain green.",
      "",
      "\x1b[1;38;5;117m❯\x1b[0m",
      "\x1b[38;5;214m⏵⏵ bypass permissions on · 2 shells, 1 monitor\x1b[0m",
    ].join("\r\n"),
  );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.codex] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[
    MARKETING_PRO_SPLIT_SESSION_IDS.codex
  ].replace(
    "\r\n\r\n\x1b[38;5;81m›\x1b[0m Ask Codex to do anything",
    [
      "",
      "• Read tests/auth/refresh-token.test.ts",
      "• Ran pnpm test --filter refresh-token",
      "  12 passed · 0 failed",
      "• Checked replay and expiry branches",
      "• Kept the public response contract stable",
      "• Reviewed the rotation audit trail",
      "• Confirmed no schema changes are required",
      "\x1b[2mWorking (31s) · validating full auth suite…\x1b[0m",
      "",
      "\x1b[38;5;81m›\x1b[0m Ask Codex to do anything",
    ].join("\r\n"),
  );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.kimi] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[
    MARKETING_PRO_SPLIT_SESSION_IDS.kimi
  ].replace(
    "\x1b[38;5;214m● QUEUED\x1b[0m",
    [
      "\x1b[36m● Read\x1b[0m(docs/session-migration.md)",
      " \x1b[2mRead 1 file\x1b[0m",
      "\x1b[36m● Review\x1b[0m public API checklist",
      " \x1b[2mCompare response types and error paths\x1b[0m",
      "\x1b[2mThe implementation agent still owns the pending patch.\x1b[0m",
      "\x1b[36m● Queue dependency\x1b[0m implementation diff",
      " \x1b[2mWatch branch feat/session-rotation\x1b[0m",
      "\x1b[2mReview starts as soon as the patch is available.\x1b[0m",
      "",
      "\x1b[38;5;214m● QUEUED\x1b[0m",
    ].join("\r\n"),
  );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.glm] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[
    MARKETING_PRO_SPLIT_SESSION_IDS.glm
  ].replace(
    "\x1b[38;5;120m✓ COMPLETED\x1b[0m",
    [
      "\x1b[36m● Read\x1b[0m(src/auth/session-store.ts)",
      " \x1b[2mRead 1 file\x1b[0m",
      "\x1b[36m● Update\x1b[0m(docs/error-codes.md)",
      " \x1b[2mDocumented refresh and replay failures\x1b[0m",
      "\x1b[36m● Bash\x1b[0m pnpm lint:docs",
      " \x1b[2mRan 3 shell commands\x1b[0m",
      "\x1b[36m● Check\x1b[0m documentation links",
      " \x1b[2mValidated 14 references\x1b[0m",
      "\x1b[2mNo stale anchors remain in the authentication guide.\x1b[0m",
      "",
      "\x1b[38;5;120m✓ COMPLETED\x1b[0m",
    ].join("\r\n"),
  );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.editing] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[
    MARKETING_PRO_SPLIT_SESSION_IDS.editing
  ].replace(
    "\x1b[38;5;117m✳ Worked 1m 4s\x1b[0m",
    [
      "\x1b[36m● Read\x1b[0m(src/auth/rollback.ts)",
      " \x1b[2mRead 1 file\x1b[0m",
      "\x1b[36m● Edit\x1b[0m(tests/auth/rollback.test.ts)",
      " \x1b[2mAdded stale-write and retry coverage\x1b[0m",
      "\x1b[36m● Bash\x1b[0m pnpm test --filter rollback",
      " \x1b[2mRan 3 shell commands\x1b[0m",
      "\x1b[1m●\x1b[0m The retry path now restores the prior session record.",
      "\x1b[38;5;117m✳ Worked 1m 4s\x1b[0m",
    ].join("\r\n"),
  );

MARKETING_CRUX_TERMINAL_SCROLLBACK[MARKETING_CRUX_SESSION_ID] =
  MARKETING_CRUX_TERMINAL_SCROLLBACK[MARKETING_CRUX_SESSION_ID]
    .replace(
      "\r\n\r\n\x1b[1;38;5;117m❯",
      "\r\n\r\n\x1b[38;5;117m✳ Worked 1m 4s\x1b[0m\r\n\r\n\x1b[1;38;5;117m❯",
    )
    .replace(" \x1b[2m↳ indexed", "  \x1b[2mindexed")
    .replace(
      "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
      "\x1b[1;38;5;117m❯\x1b[0m\r\n" +
        "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
    );

export const MARKETING_PRO_SPLIT_READY_MARKERS = [
  "⏸ plan mode on (shift+tab to cycle)",
  "Grok  grok-4.6 via xAI",
  "⏵⏵ bypass permissions on · 2 shells, 1 monitor",
  "gpt-5.6-sol xhigh fast · ~/programming/halv",
  "waiting on the diff",
  "docs pass · saved 41.2K tokens",
  "⏵⏵ auto mode on (shift+tab to cycle) · ⇐ 4 agents",
] as const;

export const MARKETING_CRUX_READY_MARKERS = [
  "CALLERS",
  "REFERENCES",
  "Changes affect HTTP refresh",
  "⏸ plan mode on (shift+tab to cycle)",
] as const;

export function marketingTerminalSceneForState(stateName: string) {
  if (isStateName(MARKETING_PRO_SPLIT_STATE_NAMES, stateName)) {
    return {
      conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
      sessions: MARKETING_PRO_SPLIT_AGENT_SESSIONS,
      terminals: MARKETING_PRO_SPLIT_TERMINAL_SESSIONS,
      events: MARKETING_PRO_SPLIT_AGENT_EVENTS,
      scrollback: MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK,
      layout: marketingProSplitLayout(),
      sidebarCollapsed: true,
    } as const;
  }
  if (isStateName(MARKETING_CRUX_STATE_NAMES, stateName)) {
    return {
      conversationTitle: MARKETING_CRUX_CONVERSATION_TITLE,
      sessions: MARKETING_CRUX_AGENT_SESSIONS,
      terminals: MARKETING_CRUX_TERMINAL_SESSIONS,
      events: MARKETING_CRUX_AGENT_EVENTS,
      scrollback: MARKETING_CRUX_TERMINAL_SCROLLBACK,
      layout: marketingCruxLayout(),
      sidebarCollapsed: false,
    } as const;
  }
  return null;
}

export function prepareMarketingTerminalSceneHarness(stateName: string) {
  const scene = marketingTerminalSceneForState(stateName);
  if (!scene) return;

  window.localStorage.setItem(
    "halv.workspace.last-active-conversation.v1",
    MARKETING_TERMINAL_CONVERSATION_ID,
  );
  window.localStorage.setItem(
    `halv.terminal-layout.v1:conversation:${MARKETING_TERMINAL_CONVERSATION_ID}`,
    JSON.stringify(scene.layout),
  );
  if (scene.sidebarCollapsed) stageInitialSidebarCollapsed();
}

export function marketingTerminalSceneStartsCollapsed(stateName: string) {
  return marketingTerminalSceneForState(stateName)?.sidebarCollapsed ?? false;
}

function marketingSession(input: {
  id: string;
  defaultTitle: string;
  agentKind: AgentSession["agentKind"];
  laneKind: AgentSession["laneKind"];
  model: string;
  pid: number;
  status: AgentSession["status"];
  conversationTitle: string;
}): AgentSession {
  return {
    ...input,
    mode: "terminal_embedded",
    projectId: 3,
    conversationId: MARKETING_TERMINAL_CONVERSATION_ID,
    cwd: MARKETING_PROJECT_CWD,
    cliSessionId: input.id,
    createdAt: "2026-08-13T14:42:00.000Z",
    lastActiveAt: "2026-08-13T14:59:00.000Z",
  };
}

function marketingTerminal(session: AgentSession): TerminalSession {
  const exited = session.status === "exited";
  const finishedTranscriptStaysOpen =
    session.id === MARKETING_PRO_SPLIT_SESSION_IDS.glm;
  return {
    id: session.id,
    projectId: session.projectId,
    conversationId: session.conversationId,
    projectName: "halv",
    cwd: session.cwd,
    shell: "/bin/zsh",
    pid: session.pid,
    executionProfile: "full_access",
    agentKind: session.agentKind,
    status: finishedTranscriptStaysOpen
      ? "running"
      : session.status === "failed"
        ? "failed"
        : exited
          ? "exited"
          : "running",
    cols: 72,
    rows: 24,
    startedAt: session.createdAt,
    ...(exited && !finishedTranscriptStaysOpen
      ? {
          exitedAt: "2026-08-13T14:58:30.000Z",
          exitCode: 0,
        }
      : {}),
  };
}

function marketingProSplitLayout() {
  const leaf = (leafId: string) => ({ type: "leaf", leafId } as const);
  const row = (first: string, second: string, third: string) => ({
    type: "split" as const,
    direction: "vertical" as const,
    ratio: 0.333,
    first: leaf(first),
    second: {
      type: "split" as const,
      direction: "vertical" as const,
      ratio: 0.5,
      first: leaf(second),
      second: leaf(third),
    },
  });
  return {
    version: 1,
    activeTabId: "marketing-six-tab",
    activeLeafId: MARKETING_PRO_SPLIT_SESSION_IDS.plan,
    tabs: [
      {
        tabId: "marketing-six-tab",
        root: {
          type: "split" as const,
          direction: "horizontal" as const,
          ratio: 0.5,
          first: row(
            MARKETING_PRO_SPLIT_SESSION_IDS.plan,
            MARKETING_PRO_SPLIT_SESSION_IDS.grok,
            MARKETING_PRO_SPLIT_SESSION_IDS.codex,
          ),
          second: row(
            MARKETING_PRO_SPLIT_SESSION_IDS.kimi,
            MARKETING_PRO_SPLIT_SESSION_IDS.glm,
            MARKETING_PRO_SPLIT_SESSION_IDS.editing,
          ),
        },
      },
    ],
  };
}

function marketingCruxLayout() {
  return {
    version: 1,
    activeTabId: "marketing-crux-tab",
    activeLeafId: MARKETING_CRUX_SESSION_ID,
    tabs: [
      {
        tabId: "marketing-crux-tab",
        root: { type: "leaf" as const, leafId: MARKETING_CRUX_SESSION_ID },
      },
    ],
  };
}

function stageInitialSidebarCollapsed() {
  const nativeMatchMedia = window.matchMedia.bind(window);
  window.matchMedia = ((query: string) => {
    const result = nativeMatchMedia(query);
    if (query !== "(max-width: 767px)") return result;
    return new Proxy(result, {
      get(target, property) {
        if (property === "matches") return true;
        const value = Reflect.get(target, property, target) as unknown;
        return typeof value === "function" ? value.bind(target) : value;
      },
    });
  }) as typeof window.matchMedia;
}

function isStateName(
  stateNames: readonly string[],
  stateName: string,
): boolean {
  return stateNames.includes(stateName);
}

function terminalBuffer(lines: string[]) {
  return `\x1b[?25l${lines.join("\r\n")}`;
}

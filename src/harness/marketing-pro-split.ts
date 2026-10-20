import type { AgentSession, AgentSessionEvent } from "../agentSessions.ts";
import type { SidecarStats, TerminalSession } from "../types.ts";

export const MARKETING_PRO_SPLIT_STATE_NAME = "marketing-app-dark";
export const MARKETING_APP_STATE_NAMES = [
  "marketing-app-dark",
  "marketing-app-light",
] as const;
const MARKETING_TERMINAL_COACH_MARK_DISMISSED_STATE_NAMES = [
  ...MARKETING_APP_STATE_NAMES,
  "marketing-pro-split-dark",
  "marketing-pro-split-light",
  "marketing-subagents-dark",
  "marketing-subagents-light",
  "marketing-crux-dark",
  "marketing-crux-light",
  "marketing-savings",
  "marketing-savings-dark",
  "marketing-savings-light",
] as const;
export const MARKETING_PRO_SPLIT_PRO_MODE_HINT_SEEN_STORAGE_KEY =
  "halv.agent-session-pro-mode-hint-seen";
export const MARKETING_PRO_SPLIT_PRO_MODE_HINT_SELECTOR =
  ".agent-session-pro-hint";
export const MARKETING_PRO_SPLIT_CONVERSATION_ID = "audit-marketing";
export const MARKETING_PRO_SPLIT_CONVERSATION_TITLE =
  "Authentication middleware refactor";
export const MARKETING_PRO_SPLIT_VISIBLE_CONVERSATION_TITLE =
  "Authentication middleware refactor";
export const MARKETING_PRO_SPLIT_WEEK_TOKENS = 21_676_683;
export const MARKETING_PRO_SPLIT_SESSION_TOKENS = 44_876;
export const MARKETING_PRO_SPLIT_RECENT_SAVINGS = [
  2_103_882,
  2_481_774,
  2_767_901,
  3_038_114,
  3_251_409,
  3_496_225,
  4_537_378,
] as const;
export const MARKETING_PRO_SPLIT_PLAN_WINDOWS: SidecarStats["planWindows"] = [
  {
    label: "7d",
    usedPercent: 97,
    resetsAt: Date.parse("2026-08-13T17:32:00.000Z") / 1_000,
  },
];

export const MARKETING_PRO_SPLIT_SESSION_IDS = {
  plan: "marketing-pro-split-claude-plan",
  bypass: "marketing-pro-split-claude-bypass",
  codex: "marketing-pro-split-codex",
} as const;

const SESSION_TIMESTAMPS = {
  plan: {
    createdAt: "2026-08-13T14:42:00.000Z",
    lastActiveAt: "2026-08-13T14:59:00.000Z",
  },
  bypass: {
    createdAt: "2026-08-13T14:45:00.000Z",
    lastActiveAt: "2026-08-13T14:58:00.000Z",
  },
  codex: {
    createdAt: "2026-08-13T14:48:00.000Z",
    lastActiveAt: "2026-08-13T14:57:00.000Z",
  },
} as const;

export const MARKETING_PRO_SPLIT_AGENT_SESSIONS: AgentSession[] = [
  {
    id: MARKETING_PRO_SPLIT_SESSION_IDS.plan,
    defaultTitle: "Claude 4",
    agentKind: "claude",
    laneKind: "claudeSubscription",
    mode: "terminal_embedded",
    projectId: 3,
    conversationId: MARKETING_PRO_SPLIT_CONVERSATION_ID,
    conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
    model: "sonnet",
    cwd: "/Users/demo/programming/halv",
    pid: 4311,
    status: "running",
    cliSessionId: "claude-plan-api-service",
    createdAt: SESSION_TIMESTAMPS.plan.createdAt,
    lastActiveAt: SESSION_TIMESTAMPS.plan.lastActiveAt,
  },
  {
    id: MARKETING_PRO_SPLIT_SESSION_IDS.bypass,
    defaultTitle: "Claude 5",
    agentKind: "claude",
    laneKind: "claudeSubscription",
    mode: "terminal_embedded",
    projectId: 3,
    conversationId: MARKETING_PRO_SPLIT_CONVERSATION_ID,
    conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
    model: "sonnet",
    cwd: "/Users/demo/programming/halv",
    pid: 4312,
    status: "running",
    cliSessionId: "claude-bypass-api-service",
    createdAt: SESSION_TIMESTAMPS.bypass.createdAt,
    lastActiveAt: SESSION_TIMESTAMPS.bypass.lastActiveAt,
  },
  {
    id: MARKETING_PRO_SPLIT_SESSION_IDS.codex,
    defaultTitle: "Codex 1",
    agentKind: "codex",
    laneKind: "codexChatGpt",
    mode: "terminal_embedded",
    projectId: 3,
    conversationId: MARKETING_PRO_SPLIT_CONVERSATION_ID,
    conversationTitle: MARKETING_PRO_SPLIT_CONVERSATION_TITLE,
    model: "gpt-5.6-sol",
    cwd: "/Users/demo/programming/halv",
    pid: 4313,
    status: "running",
    cliSessionId: "codex-refresh-api-service",
    createdAt: SESSION_TIMESTAMPS.codex.createdAt,
    lastActiveAt: SESSION_TIMESTAMPS.codex.lastActiveAt,
  },
];

export const MARKETING_PRO_SPLIT_TERMINAL_SESSIONS: TerminalSession[] =
  MARKETING_PRO_SPLIT_AGENT_SESSIONS.map((session) => ({
    id: session.id,
    projectId: session.projectId,
    conversationId: session.conversationId,
    projectName: "halv",
    cwd: session.cwd,
    shell: "/bin/zsh",
    pid: session.pid,
    executionProfile: "full_access",
    agentKind: session.agentKind,
    status: "running",
    cols: session.id === MARKETING_PRO_SPLIT_SESSION_IDS.plan ? 108 : 62,
    rows: session.id === MARKETING_PRO_SPLIT_SESSION_IDS.plan ? 44 : 21,
    startedAt: session.createdAt,
  }));

export const MARKETING_PRO_SPLIT_AGENT_EVENTS: Record<
  string,
  AgentSessionEvent[]
> = {
  [MARKETING_PRO_SPLIT_SESSION_IDS.plan]: [
    {
      type: "sessionInit",
      cliSessionId: "claude-plan-api-service",
      model: "sonnet",
    },
    { type: "turnStarted" },
    {
      type: "savingsSample",
      savingsTokens: MARKETING_PRO_SPLIT_SESSION_TOKENS,
      savingsSource: "crux + RTK",
      cacheReadTokens: 18_240,
      turnBoundary: false,
    },
  ],
  [MARKETING_PRO_SPLIT_SESSION_IDS.bypass]: [
    {
      type: "sessionInit",
      cliSessionId: "claude-bypass-api-service",
      model: "sonnet",
    },
    { type: "turnStarted" },
    {
      type: "savingsSample",
      savingsTokens: MARKETING_PRO_SPLIT_SESSION_TOKENS,
      savingsSource: "measured",
      cacheReadTokens: 18_240,
      turnBoundary: false,
    },
  ],
  [MARKETING_PRO_SPLIT_SESSION_IDS.codex]: [
    {
      type: "sessionInit",
      cliSessionId: "codex-refresh-api-service",
      model: "gpt-5.6-sol",
    },
    { type: "turnStarted" },
    {
      type: "savingsSample",
      savingsTokens: MARKETING_PRO_SPLIT_SESSION_TOKENS,
      savingsSource: "measured",
      cacheReadTokens: 18_240,
      turnBoundary: false,
    },
  ],
};

export const MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK: Record<string, string> = {
  [MARKETING_PRO_SPLIT_SESSION_IDS.plan]: terminalBuffer([
    "\x1b[1;38;5;117mClaude Code\x1b[0m  \x1b[2mSonnet 5 · ~/programming/halv\x1b[0m",
    "",
    "\x1b[1;38;5;117m❯\x1b[0m Plan a safe refactor of the authentication middleware.",
    "",
    "\x1b[1m●\x1b[0m I’ll map the request path before proposing changes.",
    "",
    "\x1b[38;5;117m✳ Gusting… (2m 54s · ↓ 6.0k tokens)\x1b[0m",
    "",
    "\x1b[36m● crux index\x1b[0m  src/auth/middleware.ts",
    "  \x1b[2m↳ 42 callers · 7 references · indexed\x1b[0m",
    "\x1b[36m● crux map\x1b[0m  requireSession",
    "  \x1b[2m↳ router → auth middleware → session store\x1b[0m",
    "\x1b[36m● Read\x1b[0m(src/auth/middleware.ts)",
    "  \x1b[2mRead 1 file\x1b[0m",
    "\x1b[36m● Read\x1b[0m(src/auth/session-store.ts)",
    "  \x1b[2mRead 1 file\x1b[0m",
    "\x1b[36m● Read\x1b[0m(tests/auth/middleware.test.ts)",
    "  \x1b[2mRead 1 file\x1b[0m",
    "",
    "\x1b[1m● Proposed plan\x1b[0m",
    "  \x1b[38;5;117m1.\x1b[0m Extract token parsing into auth/token.ts.",
    "  \x1b[38;5;117m2.\x1b[0m Keep requireSession’s public contract unchanged.",
    "  \x1b[38;5;117m3.\x1b[0m Add expiry and malformed-header cases.",
    "  \x1b[38;5;117m4.\x1b[0m Run the focused auth suite, then full tests.",
    "",
    "\x1b[2mNo production files changed. Waiting for approval.\x1b[0m",
    "",
    "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
  ]),
  [MARKETING_PRO_SPLIT_SESSION_IDS.bypass]: terminalBuffer([
    "\x1b[1;38;5;117mClaude Code\x1b[0m  \x1b[2mSonnet 5\x1b[0m",
    "",
    "\x1b[1;38;5;117m❯\x1b[0m Apply the auth cleanup and verify it.",
    "",
    "\x1b[36m● Update\x1b[0m(src/auth/token.ts)",
    "  \x1b[32m↳ added parseBearerToken()\x1b[0m",
    "\x1b[36m● Update\x1b[0m(src/auth/middleware.ts)",
    "  \x1b[32m↳ removed duplicate parsing branch\x1b[0m",
    "\x1b[36m● Update\x1b[0m(tests/auth/middleware.test.ts)",
    "  \x1b[32m↳ added 4 regression cases\x1b[0m",
    "",
    "\x1b[36m● Bash\x1b[0m(pnpm test --filter auth)",
    "  \x1b[2m↳ 3,104 lines → 41 lines\x1b[0m",
    "  \x1b[32m✓ 38 passed\x1b[0m  \x1b[2m(2.8s)\x1b[0m",
    "",
    "\x1b[1m●\x1b[0m Running the full test suite now…",
    "",
    "\x1b[38;5;214m⏵⏵ bypass permissions on · 2 shells, 1 monitor\x1b[0m",
  ]),
  [MARKETING_PRO_SPLIT_SESSION_IDS.codex]: terminalBuffer([
    "\x1b[1;38;5;81mOpenAI Codex\x1b[0m  \x1b[2mgpt-5.6-sol\x1b[0m",
    "\x1b[2m~/programming/halv\x1b[0m",
    "",
    "\x1b[1m› Trace the flaky refresh-token test.\x1b[0m",
    "",
    "\x1b[36m• Read\x1b[0m tests/auth/refresh-token.test.ts",
    "\x1b[36m• Read\x1b[0m src/auth/refresh-token.ts",
    "\x1b[36m• Ran\x1b[0m pnpm test --filter refresh-token",
    "  \x1b[31m1 failed · 11 passed\x1b[0m",
    "",
    "\x1b[1m• Found the race\x1b[0m",
    "  The test advances the clock before the write settles.",
    "",
    "\x1b[1m• Proposed fix\x1b[0m",
    "  Await persistRotation() before advancing time.",
    "  Keep the production expiry path unchanged.",
    "",
    "\x1b[2mWorking (12s) · validating the focused fix…\x1b[0m",
  ]),
};

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.plan] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[
    MARKETING_PRO_SPLIT_SESSION_IDS.plan
  ].replace(
    "\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
    "\x1b[38;5;117m❯ ○ codex:codex-rescue Codex: fix shots export f… 21s · ↓ 12.6k tokens\x1b[0m" +
      "\r\n\x1b[1;38;5;117m❯\x1b[0m" +
      "\r\n\x1b[38;5;229m⏸ plan mode on (shift+tab to cycle)\x1b[0m",
  );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.bypass] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[
    MARKETING_PRO_SPLIT_SESSION_IDS.bypass
  ]
    .replace(
      "\r\n\r\n\x1b[1;38;5;117m❯",
      "\r\n\r\n\x1b[38;5;117m✳ Churned 4m 9s · 1 shell still running\x1b[0m\r\n\r\n\x1b[1;38;5;117m❯",
    )
    .replace(
      / \x1b\[32m↳ (?:added )?parseBearerToken\(\)\x1b\[0m/,
      "  \x1b[2mRead 1 file\x1b[0m",
    )
    .replace(
      " \x1b[32m↳ removed duplicate parsing branch\x1b[0m",
      "  \x1b[2mRead 1 file\x1b[0m",
    )
    .replace(
      " \x1b[32m↳ added 4 regression cases\x1b[0m",
      "  \x1b[2mRead 1 file\x1b[0m",
    )
    .replace(
      " \x1b[2m↳ 3,104 lines → 41 lines\x1b[0m",
      "  \x1b[2m3,104 lines → 41 lines · Ran 3 shell commands\x1b[0m",
    )
    .replace(
      "\x1b[38;5;214m⏵⏵ bypass permissions on · 2 shells, 1 monitor\x1b[0m",
      "\x1b[1;38;5;117m❯\x1b[0m\r\n" +
      "\x1b[38;5;214m⏵⏵ bypass permissions on · 2 shells, 1 monitor\x1b[0m",
    );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.plan] =
  MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[
    MARKETING_PRO_SPLIT_SESSION_IDS.plan
  ].replace(
    "\x1b[38;5;117m❯ ○ codex:codex-rescue Codex: fix shots export f… 21s · ↓ 12.6k tokens\x1b[0m",
    [
      "\x1b[36m● Search\x1b[0m(requireSession)",
      " \x1b[2mFound 18 matches in 6 files\x1b[0m",
      "\x1b[36m● Read\x1b[0m(src/auth/headers.ts)",
      " \x1b[2mRead 1 file\x1b[0m",
      "\x1b[36m● Read\x1b[0m(src/auth/errors.ts)",
      " \x1b[2mRead 1 file\x1b[0m",
      "",
      "\x1b[36m● Background command \"pnpm test --filter auth\" completed (exit code 0)\x1b[0m",
      " \x1b[2mRan 3 shell commands\x1b[0m",
      "",
      "\x1b[1m●\x1b[0m The public error contract stays unchanged.",
      " \x1b[2mThe plan limits edits to auth/token.ts and focused tests.\x1b[0m",
      "",
      "\x1b[38;5;117m❯ ○ codex:codex-rescue Codex: fix shots export f… 21s · ↓ 12.6k tokens\x1b[0m",
    ].join("\r\n"),
  );

MARKETING_PRO_SPLIT_TERMINAL_SCROLLBACK[MARKETING_PRO_SPLIT_SESSION_IDS.codex] +=
  "\r\n\r\n\x1b[38;5;81m›\x1b[0m Ask Codex to do anything" +
  "\r\n\x1b[38;5;229mgpt-5.6-sol xhigh fast\x1b[0m · " +
  "\x1b[38;5;120m~/programming/halv\x1b[0m";

export function prepareMarketingProSplitStorage(
  stateName: string,
  storage: Pick<Storage, "setItem">,
) {
  if (!isMarketingTerminalCoachMarkDismissedStateName(stateName)) return;
  storage.setItem(
    MARKETING_PRO_SPLIT_PRO_MODE_HINT_SEEN_STORAGE_KEY,
    "true",
  );
}

export function marketingProSplitCoachMarkIsAbsent(root: {
  querySelector(selectors: string): Element | null;
}) {
  return root.querySelector(MARKETING_PRO_SPLIT_PRO_MODE_HINT_SELECTOR) === null;
}

export function prepareMarketingProSplitHarness(stateName: string) {
  if (!isMarketingAppStateName(stateName)) return;
  seedMarketingProSplitLayout();
}

/** Opens the split conversation with its three panes on the next start. */
export function seedMarketingProSplitLayout() {
  window.localStorage.setItem(
    "halv.workspace.last-active-conversation.v1",
    MARKETING_PRO_SPLIT_CONVERSATION_ID,
  );

  window.localStorage.setItem(
    `halv.terminal-layout.v1:conversation:${MARKETING_PRO_SPLIT_CONVERSATION_ID}`,
    JSON.stringify({
      version: 1,
      activeTabId: "marketing-pro-split-tab",
      activeLeafId: MARKETING_PRO_SPLIT_SESSION_IDS.plan,
      tabs: [
        {
          tabId: "marketing-pro-split-tab",
          root: {
            type: "split",
            direction: "vertical",
            ratio: 0.61,
            first: {
              type: "leaf",
              leafId: MARKETING_PRO_SPLIT_SESSION_IDS.plan,
            },
            second: {
              type: "split",
              direction: "horizontal",
              ratio: 0.5,
              first: {
                type: "leaf",
                leafId: MARKETING_PRO_SPLIT_SESSION_IDS.bypass,
              },
              second: {
                type: "leaf",
                leafId: MARKETING_PRO_SPLIT_SESSION_IDS.codex,
              },
            },
          },
        },
      ],
    }),
  );
}

function terminalBuffer(lines: string[]) {
  return `\x1b[?25l${lines.join("\r\n")}`;
}

function isMarketingAppStateName(stateName: string) {
  return MARKETING_APP_STATE_NAMES.some((candidate) => candidate === stateName);
}

function isMarketingTerminalCoachMarkDismissedStateName(stateName: string) {
  return MARKETING_TERMINAL_COACH_MARK_DISMISSED_STATE_NAMES.some(
    (candidate) => candidate === stateName,
  );
}

import type { AgentStatusEntry } from "../agentPaneStatus.ts";
import type { AgentSession, AgentSessionEvent } from "../agentSessions.ts";
import type {
  ConversationSummary,
  Project,
  TerminalSession,
  WorkspaceStatus,
} from "../types.ts";

// The hero shot of halv.ai for 0.8.0: the Projects tree with a folder group
// (ADR 0019), the workspace strip with tabs (ADR 0010), and one tab with
// Claude Code and Codex side by side. Every name and path is fictional.

export const MARKETING_WORKSPACE_STATE_NAMES = [
  "marketing-workspace-dark",
  "marketing-workspace-light",
] as const;

/** The flags of a 0.8.0 release build. A plain ?state= turns them on. */
export const MARKETING_WORKSPACE_FLAGS = "projectTree,workspaceTabs,shellTabs";

export const MARKETING_WORKSPACE_FOLDER = "/Users/demo/programming/halv";
const APP_ROOT = `${MARKETING_WORKSPACE_FOLDER}/app`;
const WEB_ROOT = `${MARKETING_WORKSPACE_FOLDER}/web`;
const RETRIES_ROOT =
  "/Users/demo/Library/Application Support/Halv/worktrees/b7c2/app";

export const MARKETING_WORKSPACE_PROJECT_IDS = { app: 1, web: 2, folder: 3 } as const;
export const MARKETING_WORKSPACE_RETRIES_WORKTREE_ID = "marketing-wt-retries";
const GROUP_ID = "marketing-group-halv";

export const MARKETING_WORKSPACE_CONVERSATION_IDS = {
  checkout: "marketing-ws-1-checkout",
  webhook: "marketing-ws-2-webhook",
  review: "marketing-ws-3-review",
  retries: "marketing-ws-retries",
  pricing: "marketing-ws-pricing",
} as const;

export const MARKETING_WORKSPACE_SESSION_IDS = {
  claude: "marketing-ws-claude-checkout",
  codex: "marketing-ws-codex-checkout",
  webhook: "marketing-ws-claude-webhook",
  review: "marketing-ws-codex-review",
  retries: "marketing-ws-claude-retries",
  pricing: "marketing-ws-codex-pricing",
  shell: "marketing-ws-shell",
} as const;

export const MARKETING_WORKSPACE_ACTIVE_TITLE = "Ship checkout v2";

// The visual specs and the export fix the clock before the page loads.
function minutesAgo(minutes: number) {
  return new Date(Date.now() - minutes * 60_000).toISOString();
}

export function isMarketingWorkspaceStateName(stateName: string) {
  return (MARKETING_WORKSPACE_STATE_NAMES as readonly string[]).includes(stateName);
}

export function createMarketingWorkspaceProjects(): Project[] {
  const project = (
    id: number,
    name: string,
    rootPath: string,
    createdMinutes: number,
    openedMinutes: number,
  ): Project => ({
    id,
    name,
    rootPath,
    trustLevel: "trusted",
    approvalPolicy: "ask",
    createdAt: minutesAgo(createdMinutes),
    lastOpenedAt: minutesAgo(openedMinutes),
  });
  return [
    project(MARKETING_WORKSPACE_PROJECT_IDS.app, "app", APP_ROOT, 9_000, 1),
    project(MARKETING_WORKSPACE_PROJECT_IDS.web, "web", WEB_ROOT, 8_800, 6),
    project(MARKETING_WORKSPACE_PROJECT_IDS.folder, "halv", MARKETING_WORKSPACE_FOLDER, 9_100, 40),
  ];
}

export function createMarketingWorkspaceWorktrees(): WorkspaceStatus[] {
  return [
    {
      id: MARKETING_WORKSPACE_RETRIES_WORKTREE_ID,
      projectId: MARKETING_WORKSPACE_PROJECT_IDS.app,
      name: "halv/stripe-retries",
      kind: "worktree",
      rootPath: RETRIES_ROOT,
      gitCommonDir: `${APP_ROOT}/.git`,
      baseRef: "main",
      baseCommit: "8c41e2a",
      branch: "halv/stripe-retries",
      managed: true,
      pinned: false,
      state: "available",
      createdAt: minutesAgo(300),
      lastUsedAt: minutesAgo(4),
      dirty: true,
      conversationCount: 1,
      currentBranch: "halv/stripe-retries",
      available: true,
      isGitRepo: true,
    },
  ];
}

export function createMarketingWorkspaceConversations(
  projects: readonly Project[],
): ConversationSummary[] {
  const ids = MARKETING_WORKSPACE_CONVERSATION_IDS;
  const conversation = (input: {
    id: string;
    title: string;
    minutes: number;
    /** The strip orders the seeded tabs by age, the oldest first. */
    createdMinutes?: number;
    projectId: number | null;
    workspaceRoot?: string;
    workspaceId?: string;
    backend?: "claude_session" | "codex_session";
    model?: string;
  }): ConversationSummary => {
    const project = projects.find((item) => item.id === input.projectId);
    const agent = input.backend !== undefined;
    return {
      id: input.id,
      title: input.title,
      titleSource: "manual",
      mode: agent ? "agent" : "chat",
      model: input.model ?? (input.backend === "codex_session" ? "gpt-5.6-sol" : "sonnet"),
      createdAt: minutesAgo(input.createdMinutes ?? input.minutes + 120),
      updatedAt: minutesAgo(input.minutes),
      pinned: false,
      draft: "",
      draftOnly: false,
      archivedAt: null,
      memoriesMode: null,
      fastMode: false,
      goal: null,
      agentMode: agent,
      workflowMode: "default",
      credentialProfileId: null,
      agentBackend: input.backend ?? null,
      agentSessionModel: agent ? (input.backend === "codex_session" ? "gpt-5.6-sol" : "sonnet") : null,
      projectId: project?.id ?? null,
      projectName: project?.name ?? null,
      projectRootPath: input.workspaceRoot ?? project?.rootPath ?? null,
      projectTrustLevel: project?.trustLevel ?? null,
      approvalPolicyOverride: agent ? "ask" : null,
      messageCount: 2,
      workspaceId: input.workspaceId ?? (project ? `primary-${project.id}` : null),
    };
  };
  const app = MARKETING_WORKSPACE_PROJECT_IDS.app;
  return [
    conversation({ id: ids.checkout, title: MARKETING_WORKSPACE_ACTIVE_TITLE, minutes: 1, createdMinutes: 240, projectId: app, backend: "claude_session" }),
    conversation({ id: ids.webhook, title: "Fix flaky webhook test", minutes: 9, createdMinutes: 180, projectId: app, backend: "claude_session" }),
    conversation({ id: ids.review, title: "Review payment migration", minutes: 22, createdMinutes: 150, projectId: app, backend: "codex_session" }),
    conversation({
      id: ids.retries,
      title: "Stripe retry policy",
      minutes: 4,
      projectId: app,
      workspaceRoot: RETRIES_ROOT,
      workspaceId: MARKETING_WORKSPACE_RETRIES_WORKTREE_ID,
      backend: "claude_session",
    }),
    conversation({ id: ids.pricing, title: "Pricing page copy", minutes: 2, projectId: MARKETING_WORKSPACE_PROJECT_IDS.web, backend: "codex_session" }),
    conversation({ id: "marketing-chat-billing", title: "Explain the billing flow", minutes: 75, projectId: null, model: "gpt-5.6-terra" }),
    conversation({ id: "marketing-chat-queues", title: "Compare Postgres job queues", minutes: 260, projectId: null, model: "gpt-5.6-sol" }),
    conversation({ id: "marketing-chat-lifetimes", title: "Rust lifetimes question", minutes: 1_500, projectId: null, model: "gpt-5.5" }),
  ];
}

type SessionInput = {
  id: string;
  title: string;
  agentKind: AgentSession["agentKind"];
  conversationId: string | null;
  projectId: number;
  cwd: string;
  pid: number;
};

const SESSIONS: SessionInput[] = [
  { id: MARKETING_WORKSPACE_SESSION_IDS.claude, title: "Claude 1", agentKind: "claude", conversationId: MARKETING_WORKSPACE_CONVERSATION_IDS.checkout, projectId: 1, cwd: APP_ROOT, pid: 6101 },
  { id: MARKETING_WORKSPACE_SESSION_IDS.codex, title: "Codex 1", agentKind: "codex", conversationId: MARKETING_WORKSPACE_CONVERSATION_IDS.checkout, projectId: 1, cwd: APP_ROOT, pid: 6102 },
  { id: MARKETING_WORKSPACE_SESSION_IDS.webhook, title: "Claude 2", agentKind: "claude", conversationId: MARKETING_WORKSPACE_CONVERSATION_IDS.webhook, projectId: 1, cwd: APP_ROOT, pid: 6103 },
  { id: MARKETING_WORKSPACE_SESSION_IDS.review, title: "Codex 2", agentKind: "codex", conversationId: MARKETING_WORKSPACE_CONVERSATION_IDS.review, projectId: 1, cwd: APP_ROOT, pid: 6104 },
  { id: MARKETING_WORKSPACE_SESSION_IDS.retries, title: "Claude 3", agentKind: "claude", conversationId: MARKETING_WORKSPACE_CONVERSATION_IDS.retries, projectId: 1, cwd: RETRIES_ROOT, pid: 6105 },
  { id: MARKETING_WORKSPACE_SESSION_IDS.pricing, title: "Codex 3", agentKind: "codex", conversationId: MARKETING_WORKSPACE_CONVERSATION_IDS.pricing, projectId: 2, cwd: WEB_ROOT, pid: 6106 },
];

export function createMarketingWorkspaceAgentSessions(): AgentSession[] {
  return SESSIONS.map((session) => ({
    id: session.id,
    defaultTitle: session.title,
    agentKind: session.agentKind,
    laneKind: session.agentKind === "codex" ? "codexChatGpt" : "claudeSubscription",
    mode: "terminal_embedded",
    projectId: session.projectId,
    conversationId: session.conversationId,
    model: session.agentKind === "codex" ? "gpt-5.6-sol" : "sonnet",
    cwd: session.cwd,
    pid: session.pid,
    status: "running",
    cliSessionId: session.id,
    createdAt: minutesAgo(50),
    // The Claude pane of the active tab is the most recent session.
    lastActiveAt: minutesAgo(session.id === MARKETING_WORKSPACE_SESSION_IDS.claude ? 0.5 : 3),
  }));
}

export function createMarketingWorkspaceTerminals(): TerminalSession[] {
  const terminal = (
    input: Omit<SessionInput, "agentKind"> & { agentKind: TerminalSession["agentKind"] },
  ): TerminalSession => ({
    id: input.id,
    title: input.title,
    projectId: input.projectId,
    conversationId: input.conversationId,
    projectName: input.projectId === 2 ? "web" : "app",
    cwd: input.cwd,
    shell: "/bin/zsh",
    pid: input.pid,
    executionProfile: "full_access",
    agentKind: input.agentKind,
    status: "running",
    cols: 90,
    rows: 40,
    startedAt: minutesAgo(50),
  });
  return [
    ...SESSIONS.map((session) => terminal(session)),
    terminal({
      id: MARKETING_WORKSPACE_SESSION_IDS.shell,
      title: "zsh",
      agentKind: null,
      conversationId: null,
      projectId: 1,
      cwd: APP_ROOT,
      pid: 6107,
    }),
  ];
}

/** The savings of the two visible panes. The dollars use the input price
 *  of each model (ADR 0015). The header meter shows the most recent
 *  session of the tab: the Claude pane. */
const PANE_SAVINGS = [
  { sessionId: MARKETING_WORKSPACE_SESSION_IDS.codex, model: "gpt-5.6-sol", tokens: 512_840, dollars: 1.28 },
  { sessionId: MARKETING_WORKSPACE_SESSION_IDS.claude, model: "sonnet", tokens: 1_284_310, dollars: 6.42 },
] as const;

export const MARKETING_WORKSPACE_AGENT_EVENTS: Record<string, AgentSessionEvent[]> =
  Object.fromEntries(
    PANE_SAVINGS.map((pane) => [
      pane.sessionId,
      [
        { type: "sessionInit", cliSessionId: pane.sessionId, model: pane.model },
        { type: "turnStarted" },
        {
          type: "savingsSample",
          savingsTokens: pane.tokens,
          savingsSource: "measured",
          dollarsUsd: pane.dollars,
          cacheReadTokens: 18_240,
          turnBoundary: false,
        },
      ] satisfies AgentSessionEvent[],
    ]),
  );

export function createMarketingWorkspaceAgentStatuses(): AgentStatusEntry[] {
  const now = Date.now();
  const ids = MARKETING_WORKSPACE_SESSION_IDS;
  return [
    { sessionId: ids.claude, status: "working", updatedAt: now - 20_000, unread: false, waitSeq: 0, turnSeq: 4, source: "hook" },
    { sessionId: ids.codex, status: "working", updatedAt: now - 15_000, unread: false, waitSeq: 0, turnSeq: 2, source: "rollout" },
    { sessionId: ids.webhook, status: "done", updatedAt: now - 9 * 60_000, unread: true, waitSeq: 0, turnSeq: 3, source: "hook" },
    { sessionId: ids.review, status: "done", updatedAt: now - 22 * 60_000, unread: false, waitSeq: 0, turnSeq: 1, source: "rollout" },
    {
      sessionId: ids.retries,
      status: "needsInput",
      reason: "question",
      updatedAt: now - 4 * 60_000,
      unread: false,
      waitSeq: 1,
      turnSeq: 2,
      source: "hook",
      prompt: { kind: "question", text: "Retry a declined card three times?" },
    },
    { sessionId: ids.pricing, status: "working", updatedAt: now - 60_000, unread: false, waitSeq: 0, turnSeq: 2, source: "rollout" },
  ];
}

const CLAUDE = "\x1b[1;38;5;209m";
const DIM = "\x1b[2m";
const RESET = "\x1b[0m";
const TOOL = "\x1b[36m";
const OK = "\x1b[38;5;114m";
const ADD = "\x1b[38;5;114m";
const DEL = "\x1b[38;5;210m";
const CODEX = "\x1b[1;38;5;81m";
const YELLOW = "\x1b[38;5;222m";

function buffer(lines: string[]) {
  return `\x1b[?25l${lines.join("\r\n")}`;
}

export const MARKETING_WORKSPACE_SCROLLBACK: Record<string, string> = {
  [MARKETING_WORKSPACE_SESSION_IDS.claude]: buffer([
    `${CLAUDE}✻ Claude Code${RESET} ${DIM}v2.2.14${RESET}`,
    `${DIM}  Opus 5.5 · Claude Max · ~/programming/halv/app${RESET}`,
    "",
    `${CLAUDE}❯${RESET} Where does checkout create the payment intent?`,
    "",
    `${TOOL}●${RESET} crux map ${DIM}createIntent --callers${RESET}`,
    `  ${DIM}⎿ 3 callers · src/checkout/session.ts:41${RESET}`,
    `${TOOL}●${RESET} Read(src/routes/stripe.ts)`,
    `  ${DIM}⎿ Read 96 lines${RESET}`,
    `${TOOL}●${RESET} Checkout creates one intent per cart in session.ts:41.`,
    "  The Stripe webhook confirms it in routes/stripe.ts:88.",
    `${DIM}✻ Worked for 38s${RESET}`,
    "",
    `${CLAUDE}❯${RESET} Ship checkout v2 behind the flag. Keep the old flow`,
    "  as the fallback and prove it with the checkout tests.",
    "",
    `${TOOL}●${RESET} crux map ${DIM}CheckoutSession --callers --impact${RESET}`,
    `  ${DIM}⎿ 14 callers · 5 references · 2 API routes${RESET}`,
    `${TOOL}●${RESET} Read(src/checkout/session.ts)`,
    `  ${DIM}⎿ Read 212 lines${RESET}`,
    `${TOOL}●${RESET} Update(src/checkout/session.ts)`,
    `  ${DIM}⎿ Updated with 18 additions and 6 removals${RESET}`,
    `     ${DIM}41${RESET} ${DEL}- const intent = await createIntent(cart)${RESET}`,
    `     ${DIM}41${RESET} ${ADD}+ if (flags.checkoutV2) {${RESET}`,
    `     ${DIM}42${RESET} ${ADD}+   return createCheckoutV2(cart, customer)${RESET}`,
    `     ${DIM}43${RESET} ${ADD}+ }${RESET}`,
    `${TOOL}●${RESET} Bash(pnpm test --filter checkout)`,
    `  ${DIM}⎿ 3,812 lines → 41 lines (rtk)${RESET}`,
    `     ${OK}PASS${RESET} checkout/session.test.ts ${DIM}(24)${RESET}`,
    `     ${OK}PASS${RESET} checkout/fallback.test.ts ${DIM}(9)${RESET}`,
    `     ${OK}PASS${RESET} checkout/flags.test.ts ${DIM}(6)${RESET}`,
    `${TOOL}●${RESET} Update(src/routes/checkout.ts)`,
    `  ${DIM}⎿ Updated with 7 additions and 2 removals${RESET}`,
    "",
    `${TOOL}●${RESET} The v2 flow is behind ${YELLOW}checkoutV2${RESET}. The old flow stays`,
    "  the fallback when the flag is off or Stripe refuses.",
    "",
    `${CLAUDE}✻ Wiring the fallback route… ${DIM}(1m 12s · ↓ 4.1k tokens)${RESET}`,
    "",
    `${CLAUDE}❯${RESET} `,
    `${DIM}  ⏵⏵ auto mode on (shift+tab to cycle)${RESET}`,
  ]),
  [MARKETING_WORKSPACE_SESSION_IDS.codex]: buffer([
    `${DIM}╭─────────────────────────────────────────────╮${RESET}`,
    `${DIM}│${RESET} ${CODEX}>_ OpenAI Codex${RESET} ${DIM}(v0.163.0)${RESET}                  ${DIM}│${RESET}`,
    `${DIM}│${RESET}                                             ${DIM}│${RESET}`,
    `${DIM}│${RESET} model:     gpt-5.6-sol high                 ${DIM}│${RESET}`,
    `${DIM}│${RESET} directory: ~/programming/halv/app           ${DIM}│${RESET}`,
    `${DIM}╰─────────────────────────────────────────────╯${RESET}`,
    "",
    `${CODEX}›${RESET} List the open TODOs in src/checkout.`,
    "",
    `${OK}•${RESET} Ran rg -n "TODO" src/checkout`,
    `  ${DIM}└ 4 matches in 3 files${RESET}`,
    `${OK}•${RESET} Two TODOs block v2: idempotency keys on the intent,`,
    "  and a retry for the 3D Secure redirect. Both are small.",
    "",
    `${CODEX}›${RESET} Review the checkout v2 diff and run the e2e suite.`,
    "",
    `${OK}•${RESET} Explored`,
    `  ${DIM}└ Read session.ts, flags.ts, routes/checkout.ts${RESET}`,
    `${OK}•${RESET} Ran pnpm e2e --grep checkout`,
    `  ${DIM}└ 3,104 lines → 38 lines (rtk)${RESET}`,
    `    ${OK}✓${RESET} guest checkout ${DIM}(4.2s)${RESET}`,
    `    ${OK}✓${RESET} saved card checkout ${DIM}(3.8s)${RESET}`,
    `    ${OK}✓${RESET} declined card falls back to v1 ${DIM}(2.9s)${RESET}`,
    `    ${OK}✓${RESET} 3D Secure challenge ${DIM}(5.1s)${RESET}`,
    `${OK}•${RESET} Edited src/routes/checkout.ts ${ADD}(+6${RESET} ${DEL}-2)${RESET}`,
    `    ${ADD}+ if (!flags.checkoutV2) return legacyCheckout(req)${RESET}`,
    `${OK}•${RESET} Ran pnpm typecheck`,
    `  ${DIM}└ no errors${RESET}`,
    "",
    `${OK}•${RESET} The diff is safe to merge. One note: log the`,
    "  fallback reason, so a refused v2 call is visible.",
    "",
    `${DIM}◦ Working (48s • esc to interrupt)${RESET}`,
    "",
    `${CODEX}›${RESET} ${DIM}Ask Codex to do anything${RESET}`,
    "",
    `  ${DIM}gpt-5.6-sol high · ~/programming/halv/app${RESET}`,
  ]),
};

/** The sidebar tree: the folder group "halv" holds the folder and its two
 *  repositories. The one-time scan saw every root, so it adds nothing. */
export function marketingWorkspaceTreeState() {
  const { app, web, folder } = MARKETING_WORKSPACE_PROJECT_IDS;
  return {
    version: 1,
    collapsed: [],
    // The agents of the app checkout show as rows, not as a folded pill.
    expanded: [`agents:primary-${app}`],
    groups: [
      {
        id: GROUP_ID,
        name: "halv",
        createdFrom: "folder-scan",
        folderProjectId: folder,
        parentPath: MARKETING_WORKSPACE_FOLDER,
      },
    ],
    projectGroups: { [String(folder)]: GROUP_ID, [String(app)]: GROUP_ID, [String(web)]: GROUP_ID },
    projectOrder: [folder, app, web],
    scannedFolders: [MARKETING_WORKSPACE_FOLDER, APP_ROOT, WEB_ROOT],
  };
}

/** Claude Code left, Codex right, in the tab of the checkout conversation. */
export function marketingWorkspaceLayout() {
  return {
    version: 1,
    activeTabId: "marketing-ws-tab",
    activeLeafId: MARKETING_WORKSPACE_SESSION_IDS.claude,
    tabs: [
      {
        tabId: "marketing-ws-tab",
        root: {
          type: "split",
          direction: "vertical",
          ratio: 0.5,
          first: { type: "leaf", leafId: MARKETING_WORKSPACE_SESSION_IDS.claude },
          second: { type: "leaf", leafId: MARKETING_WORKSPACE_SESSION_IDS.codex },
        },
      },
    ],
  };
}

export function prepareMarketingWorkspaceStorage(
  stateName: string,
  storage: Pick<Storage, "setItem">,
) {
  if (!isMarketingWorkspaceStateName(stateName)) return;
  storage.setItem("halv.sidebar.project-tree.v1", JSON.stringify(marketingWorkspaceTreeState()));
  storage.setItem(
    "halv.workspace.last-active-conversation.v1",
    MARKETING_WORKSPACE_CONVERSATION_IDS.checkout,
  );
  storage.setItem(
    `halv.terminal-layout.v1:conversation:${MARKETING_WORKSPACE_CONVERSATION_IDS.checkout}`,
    JSON.stringify(marketingWorkspaceLayout()),
  );
  storage.setItem("halv.agent-session-pro-mode-hint-seen", "true");
}

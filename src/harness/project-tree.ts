import type { AgentStatusEntry } from "../agentPaneStatus.ts";
import type {
  ConversationSummary,
  Project,
  TerminalSession,
  WorkspaceStatus,
} from "../types.ts";

// Worktree fixtures for the Projects tree (ADR 0010). Project 1 has its
// primary checkout and two Halv-managed worktrees. Conversations name
// their workspace, and agent panes in three workspaces have a state.

export function isProjectTreeStateName(stateName: string) {
  return stateName === "project-tree-dark" || stateName === "project-tree-light";
}

export const PROJECT_TREE_WORKSPACE_IDS = {
  login: "audit-wt-login",
  docs: "audit-wt-docs",
} as const;

export const PROJECT_TREE_DOCS_CONVERSATION_ID = "audit-docs-sweep";

export const PROJECT_TREE_SESSION_IDS = {
  login: "tree-claude-login",
  docs: "tree-codex-docs",
  primary: "tree-claude-primary",
  loose: "tree-codex-loose",
  shell: "tree-shell",
} as const;

const WORKTREE_ROOT =
  "/Users/auditor/Library/Application Support/Halv/worktrees";

// The visual specs fix the clock before the page loads, so these times
// are stable there.
function minutesAgo(minutes: number) {
  return new Date(Date.now() - minutes * 60_000).toISOString();
}

function worktree(
  project: Project,
  id: string,
  folder: string,
  branch: string,
  lastUsedMinutes: number,
): WorkspaceStatus {
  return {
    id,
    projectId: project.id,
    name: branch,
    kind: "worktree",
    rootPath: `${WORKTREE_ROOT}/${folder}/halv`,
    gitCommonDir: `${project.rootPath}/.git`,
    baseRef: "main",
    baseCommit: "4a487d0",
    branch,
    managed: true,
    pinned: false,
    state: "available",
    createdAt: minutesAgo(lastUsedMinutes + 600),
    lastUsedAt: minutesAgo(lastUsedMinutes),
    dirty: false,
    conversationCount: 1,
    currentBranch: branch,
    available: true,
    isGitRepo: true,
  };
}

export function createProjectTreeWorktrees(projects: Project[]): WorkspaceStatus[] {
  const project = projects.find((item) => item.id === 1);
  if (!project) return [];
  return [
    worktree(project, PROJECT_TREE_WORKSPACE_IDS.login, "a1b2", "halv/login-flow", 10),
    worktree(project, PROJECT_TREE_WORKSPACE_IDS.docs, "c3d4", "halv/docs-refresh", 90),
  ];
}

export const PROJECT_TREE_SPIKE_ID = "audit-wt-spike";

/** A worktree that no conversation uses. Only a removal spec adds it. */
export function createSpikeWorktree(projects: Project[], dirty: boolean): WorkspaceStatus {
  const project = projects.find((item) => item.id === 1) ?? projects[0];
  return {
    ...worktree(project, PROJECT_TREE_SPIKE_ID, "e5f6", "halv/old-spike", 300),
    dirty,
    conversationCount: 0,
  };
}

/** Binds conversations to the worktrees and adds one docs conversation. */
export function applyProjectTreeConversations(
  conversations: ConversationSummary[],
  worktrees: WorkspaceStatus[],
) {
  const byId = new Map(worktrees.map((workspace) => [workspace.id, workspace]));
  const bind = (conversation: ConversationSummary, workspaceId: string) => {
    const workspace = byId.get(workspaceId);
    if (!workspace) return;
    conversation.workspaceId = workspace.id;
    conversation.projectRootPath = workspace.rootPath;
  };
  // Manual titles keep the conversation title over the pane title.
  const proxy = conversations.find((item) => item.id === "audit-proxy");
  if (proxy) {
    proxy.mode = "agent";
    proxy.titleSource = "manual";
    bind(proxy, PROJECT_TREE_WORKSPACE_IDS.login);
  }
  const refactor = conversations.find((item) => item.id === "audit-agent-refactor");
  if (refactor) {
    refactor.mode = "agent";
    refactor.titleSource = "manual";
  }
  const template = refactor ?? proxy;
  if (!template) return;
  const docs: ConversationSummary = {
    ...template,
    id: PROJECT_TREE_DOCS_CONVERSATION_ID,
    title: "Docs refresh sweep",
    titleSource: "manual",
    mode: "agent",
    goal: null,
    createdAt: minutesAgo(40),
    updatedAt: minutesAgo(3),
    messageCount: 2,
  };
  bind(docs, PROJECT_TREE_WORKSPACE_IDS.docs);
  conversations.unshift(docs);
}

function terminal(
  id: string,
  title: string,
  cwd: string,
  agentKind: TerminalSession["agentKind"],
  conversationId: string | null,
  pid: number,
): TerminalSession {
  return {
    id,
    title,
    projectId: 1,
    conversationId,
    projectName: "Halv Desktop",
    cwd,
    shell: "/bin/zsh",
    pid,
    executionProfile: "full_access",
    agentKind,
    status: "running",
    cols: 100,
    rows: 30,
    startedAt: minutesAgo(30),
  };
}

export function createProjectTreeTerminals(
  projects: Project[],
  worktrees: WorkspaceStatus[],
): TerminalSession[] {
  const primaryRoot =
    projects.find((item) => item.id === 1)?.rootPath ?? "/Users/auditor/Projects/halv";
  const root = (id: string) =>
    worktrees.find((workspace) => workspace.id === id)?.rootPath ?? primaryRoot;
  const ids = PROJECT_TREE_SESSION_IDS;
  return [
    terminal(ids.login, "Claude 2", root(PROJECT_TREE_WORKSPACE_IDS.login), "claude", "audit-proxy", 5101),
    terminal(ids.docs, "Codex 1", root(PROJECT_TREE_WORKSPACE_IDS.docs), "codex", PROJECT_TREE_DOCS_CONVERSATION_ID, 5102),
    terminal(ids.primary, "Claude 1", primaryRoot, "claude", "audit-agent-refactor", 5103),
    terminal(ids.loose, "Codex 2", `${root(PROJECT_TREE_WORKSPACE_IDS.docs)}/src`, "codex", null, 5104),
    terminal(ids.shell, "zsh", primaryRoot, null, null, 5105),
  ];
}

/** The terminal ids of `?adoption=1` (ADR 0020). */
export const ADOPTION_SESSION_IDS = {
  repo: "adoption-shell-repo",
  home: "adoption-shell-home",
} as const;

/** Two shells outside every project: one in a git repository that is no
 *  project yet, and one in the home folder, which has no repository. */
export function createAdoptionTerminals(): TerminalSession[] {
  return [
    {
      ...terminal(ADOPTION_SESSION_IDS.repo, "notes shell", "/Users/auditor/Projects/orca-notes/src", null, null, 5201),
      projectId: null,
      projectName: "",
    },
    {
      ...terminal(ADOPTION_SESSION_IDS.home, "home shell", "/Users/auditor", null, null, 5202),
      projectId: null,
      projectName: "",
    },
  ];
}

export function createProjectTreeAgentStatuses(): AgentStatusEntry[] {
  const now = Date.now();
  return [
    {
      sessionId: PROJECT_TREE_SESSION_IDS.login,
      status: "needsInput",
      reason: "question",
      updatedAt: now - 4 * 60_000,
      unread: false,
      waitSeq: 1,
      turnSeq: 2,
      source: "presence",
      prompt: { kind: "question", text: "Keep the old login route as a redirect?" },
    },
    {
      sessionId: PROJECT_TREE_SESSION_IDS.docs,
      status: "working",
      updatedAt: now - 60_000,
      unread: false,
      waitSeq: 0,
      turnSeq: 3,
      source: "rollout",
    },
    {
      sessionId: PROJECT_TREE_SESSION_IDS.primary,
      status: "done",
      updatedAt: now - 18 * 60_000,
      unread: true,
      waitSeq: 0,
      turnSeq: 5,
      source: "presence",
    },
    {
      sessionId: PROJECT_TREE_SESSION_IDS.loose,
      status: "idle",
      updatedAt: now - 3 * 60 * 60_000,
      unread: false,
      waitSeq: 0,
      turnSeq: 1,
      source: "rollout",
    },
  ];
}

export const PARKED_CONVERSATION_ID = "audit-parked";

/** An agent conversation of the primary workspace with no live session.
 *  A strip spec gives it a saved pane, so its tab stays dormant. */
export function addParkedConversation(conversations: ConversationSummary[]) {
  const template = conversations.find((item) => item.id === "audit-agent-refactor");
  if (!template) return;
  conversations.push({
    ...template,
    id: PARKED_CONVERSATION_ID,
    title: "Parked login spike",
    titleSource: "manual",
    mode: "agent",
    agentBackend: "claude_session",
    goal: null,
    createdAt: minutesAgo(600),
    updatedAt: minutesAgo(300),
    messageCount: 0,
  });
}

// Three more agents of the primary workspace for the glyph specs: an
// unread finished turn, a pane without any signal, and a failed turn. With the
// fixture above, the primary workspace shows every chip of Orca's summary.
export const GLYPH_SHOWCASE = [
  { id: "glyph-review", title: "Review flaky tests", agentKind: "claude", sessionId: "glyph-claude-review", pid: 5111 },
  { id: "glyph-deps", title: "Bump dependencies", agentKind: "claude", sessionId: "glyph-claude-deps", pid: 5112 },
  { id: "glyph-config", title: "Migrate config loader", agentKind: "codex", sessionId: "glyph-codex-config", pid: 5113 },
] as const;

export function addGlyphShowcaseConversations(conversations: ConversationSummary[]) {
  const template = conversations.find((item) => item.id === "audit-agent-refactor");
  if (!template) return;
  GLYPH_SHOWCASE.forEach((item, index) => {
    conversations.push({
      ...template,
      id: item.id,
      title: item.title,
      titleSource: "manual",
      mode: "agent",
      agentBackend: item.agentKind === "codex" ? "codex_session" : "claude_session",
      goal: null,
      createdAt: minutesAgo(120 + index),
      updatedAt: minutesAgo(20 + index * 7),
      messageCount: 2,
    });
  });
}

export function createGlyphShowcaseTerminals(projects: Project[]): TerminalSession[] {
  const primaryRoot =
    projects.find((item) => item.id === 1)?.rootPath ?? "/Users/auditor/Projects/halv";
  return GLYPH_SHOWCASE.map((item) =>
    terminal(item.sessionId, item.title, primaryRoot, item.agentKind, item.id, item.pid),
  );
}

/** No entry for the "deps" pane: it reports nothing. */
export function createGlyphShowcaseStatuses(): AgentStatusEntry[] {
  const now = Date.now();
  return [
    {
      sessionId: GLYPH_SHOWCASE[0].sessionId,
      status: "done",
      updatedAt: now - 25 * 60_000,
      unread: true,
      waitSeq: 0,
      turnSeq: 4,
      source: "hook",
    },
    {
      sessionId: GLYPH_SHOWCASE[2].sessionId,
      status: "failed",
      reason: "exit 1",
      updatedAt: now - 41 * 60_000,
      unread: false,
      waitSeq: 0,
      turnSeq: 2,
      source: "rollout",
    },
  ];
}

export const SECOND_DOCS_PANE_ID = "tree-claude-docs-review";

/** A second agent pane of the docs conversation (a split). */
export function createSecondDocsPane(terminals: TerminalSession[]): TerminalSession[] {
  const docs = terminals.find((item) => item.id === PROJECT_TREE_SESSION_IDS.docs);
  if (!docs) return [];
  return [{ ...docs, id: SECOND_DOCS_PANE_ID, title: "Claude 3", agentKind: "claude", pid: 5106 }];
}

export function createSecondDocsPaneStatus(): AgentStatusEntry {
  return {
    sessionId: SECOND_DOCS_PANE_ID,
    status: "needsInput",
    reason: "question",
    updatedAt: Date.now() - 2 * 60_000,
    unread: false,
    waitSeq: 1,
    turnSeq: 1,
    source: "hook",
    prompt: { kind: "question", text: "Split the docs into two pages?" },
  };
}

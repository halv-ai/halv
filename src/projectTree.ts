import {
  agentPaneStateGlyph,
  GLYPH_RANK,
  strongestAgentPaneView,
  strongestGlyph,
  type AgentPaneGlyph,
  type AgentPaneView,
} from "./agentPaneStatus.ts";
import {
  conversationDisplayTitle,
  conversationNamingTitle,
  conversationTerminalStatus,
  type ConversationTerminalStatus,
} from "./conversationMode.ts";
import { effectiveWorkspaceId, primaryWorkspaceId } from "./conversationWorkspace.ts";
import { pinnedConversationColor, pinnedConversationInitial } from "./pinnedConversations.ts";
import { standaloneSidebarTerminals } from "./sidebarTerminals.ts";
import type { ProjectGroup } from "./projectTreeState.ts";
import type { ConversationSummary, Project, WorkspaceOverview } from "./types";

// The pure model of the Projects tree in the sidebar (ADR 0010, stage S1):
// projects, their workspaces, and the agent rows of each workspace. A
// conversation goes to its workspace through `effectiveWorkspaceId`, the
// same rule that places its tab. An agent terminal without a visible
// conversation goes to the workspace that holds its cwd. Plain shells are
// not workspace rows.
//
// With workspace tabs (stage S4b), the rows of a workspace are its open
// tabs in strip order, then a dimmed row for each live conversation
// without a tab. A closed conversation goes to the "History" of its
// workspace (stage S7): Orca has no history list, so this is Halv's
// quiet answer. A terminal without a tab and without a conversation shows
// in the Chats area below the tree (ADR 0020). Adoption moves most of
// them into tabs; the rest, for example a shell in the home folder, stay.

export type TreeTerminal = {
  id: string;
  title: string;
  customTitle: string | null;
  cwd: string;
  agentKind: string | null;
  conversationId: string | null;
  status: "starting" | "running" | "waiting_input" | "exited" | "failed";
  pane?: AgentPaneView | null;
  continuedFrom?: string | null;
};

/** The state that a row shows. A chat reply that the user did not read is
 * an unread finished turn, as in the agent-status ladder (ADR 0011). */
export type TreeRowState = {
  glyph: AgentPaneGlyph | null;
  /** An unread finished turn or an unread chat reply. The row is bold. */
  unread: boolean;
  /** The strongest agent pane, when an agent pane drives the state. */
  paneView: AgentPaneView | null;
  /** The status of agent sessions without a pane view, for example Ghostty. */
  terminalStatus: ConversationTerminalStatus | null;
  chatRunning: boolean;
  chatUnviewed: boolean;
};

/** One agent pane of a conversation with several agent panes (stage S7),
 *  as Orca shows one row per agent pane. */
export type TreePaneRow<T extends TreeTerminal = TreeTerminal> = {
  key: string;
  terminal: T;
  title: string;
  agentKind: string | null;
  state: TreeRowState;
  ageAt: number | null;
};

export type TreeConversationRow<T extends TreeTerminal = TreeTerminal> = {
  kind: "conversation";
  key: string;
  conversation: ConversationSummary;
  title: string;
  agentKind: string | null;
  state: TreeRowState;
  /** The time that the short age counts from. */
  ageAt: number;
  /** The pane that a click brings to the front, when one needs it. */
  focusSessionId: string | null;
  terminals: T[];
  /** A live conversation without a tab (workspace tabs only). */
  untabbed?: boolean;
  /** One child row per live agent pane, when there are two or more. */
  panes: TreePaneRow<T>[];
};

export type TreeTerminalRow<T extends TreeTerminal = TreeTerminal> = {
  kind: "terminal";
  key: string;
  terminal: T;
  title: string;
  agentKind: string | null;
  state: TreeRowState;
  ageAt: number | null;
};

export type TreeAgentRow<T extends TreeTerminal = TreeTerminal> =
  | TreeConversationRow<T>
  | TreeTerminalRow<T>;

export type TreeWorkspaceNode<T extends TreeTerminal = TreeTerminal> = {
  key: string;
  workspace: WorkspaceOverview;
  label: string;
  primary: boolean;
  glyph: AgentPaneGlyph | null;
  /** A row of the workspace has an unread finished turn. */
  unread: boolean;
  rows: TreeAgentRow<T>[];
  /** Quiet rows past the cap. "Show N more" lists them. */
  hiddenRows: TreeAgentRow<T>[];
  /** Every row in time order, for the expanded list. */
  allRows: TreeAgentRow<T>[];
  /** Workspace tabs: two or more rows fold into the "N agents" pill. */
  fold: boolean;
  /** Workspace tabs: the closed conversations of the workspace, newest
   *  first. The "History" disclosure lists them (stage S7). */
  history: TreeConversationRow<T>[];
};

export type TreeProjectNode<T extends TreeTerminal = TreeTerminal> = {
  key: string;
  project: Project;
  avatar: { initial: string; color: string };
  glyph: AgentPaneGlyph | null;
  workspaces: TreeWorkspaceNode<T>[];
  /** The folder project of its folder-scan group (ADR 0019). Its row comes
   *  first in the group and shows a folder icon, as Orca's folder workspace. */
  groupFolder?: boolean;
};

/** A project group (Orca's "project group"), with its projects in order:
 *  the folder project of a folder-scan group first, then the rest. */
export type TreeGroupNode<T extends TreeTerminal = TreeTerminal> = {
  key: string;
  group: ProjectGroup;
  glyph: AgentPaneGlyph | null;
  projects: TreeProjectNode<T>[];
};

export type ProjectTree<T extends TreeTerminal = TreeTerminal> = {
  /** Every project, in sidebar order: the groups first, then the rest. */
  projects: TreeProjectNode<T>[];
  /** The groups, in their order. A group can be empty. */
  groups: TreeGroupNode<T>[];
  /** The projects without a group. */
  ungrouped: TreeProjectNode<T>[];
  /** Conversations without a project, newest first. */
  looseChats: TreeConversationRow<T>[];
  /** Plain shells, and agent terminals that no workspace holds. */
  otherTerminals: T[];
  /** The rows of `otherTerminals` in the Chats area (ADR 0020). */
  chatTerminals: TreeTerminalRow<T>[];
};

/** The open tabs of each strip, from the workspace tab store. */
export type TreeTabsInput = {
  /** The conversation ids of each strip, in strip order. */
  stripsByWorkspace: ReadonlyMap<string, readonly string[]>;
  /** Conversations with a running agent or a running owned terminal. */
  liveConversationIds: ReadonlySet<string>;
};

/** One chip of the folded agent summary: a state, and the agents in it. */
export type TreeChipGroup = {
  glyph: AgentPaneGlyph;
  count: number;
  /** One icon per agent kind, the most frequent kind first. */
  kinds: (string | null)[];
  /** The agents of the chip that have no icon of their own. */
  extra: number;
};

/**
 * Orca's summary chips: one chip per state, in rank order. A row without a
 * state counts as idle. The chips after `maxGroups` add to `hiddenCount`.
 */
export function agentChipGroups(
  items: readonly { glyph: AgentPaneGlyph | null; kind: string | null }[],
  maxGroups = 3,
  maxIcons = 3,
): { groups: TreeChipGroup[]; hiddenCount: number } {
  const byGlyph = new Map<AgentPaneGlyph, (string | null)[]>();
  for (const item of items) {
    const glyph = item.glyph ?? "idle";
    const kinds = byGlyph.get(glyph);
    if (kinds) kinds.push(item.kind);
    else byGlyph.set(glyph, [item.kind]);
  }
  const all = [...byGlyph.entries()]
    .sort(([left], [right]) => GLYPH_RANK[left] - GLYPH_RANK[right])
    .map(([glyph, kinds]): TreeChipGroup => {
      const counts = new Map<string | null, { count: number; first: number }>();
      kinds.forEach((kind, index) => {
        const entry = counts.get(kind);
        if (entry) entry.count += 1;
        else counts.set(kind, { count: 1, first: index });
      });
      const iconKinds = [...counts.entries()]
        .sort(([, a], [, b]) => b.count - a.count || a.first - b.first)
        .slice(0, maxIcons)
        .map(([kind]) => kind);
      return {
        glyph,
        count: kinds.length,
        kinds: iconKinds,
        extra: kinds.length - iconKinds.length,
      };
    });
  return {
    groups: all.slice(0, maxGroups),
    hiddenCount: all.slice(maxGroups).reduce((sum, group) => sum + group.count, 0),
  };
}

/** Rows of one workspace that fold into the "N agents" pill. */
export const TREE_FOLD_MIN_ROWS = 2;

export type ProjectTreeInput<T extends TreeTerminal> = {
  projects: readonly Project[];
  workspaces: readonly WorkspaceOverview[];
  conversations: readonly ConversationSummary[];
  terminals: readonly T[];
  runningConversationIds: ReadonlySet<string>;
  unviewedConversationIds: ReadonlySet<string>;
  /** Rows that a workspace shows before "Show N more". */
  maxRowsPerWorkspace: number;
  /** Workspace tabs: rows come from the strips. */
  tabs?: TreeTabsInput | null;
  /** Project groups and the manual project order (per viewer). */
  organization?: TreeOrganization | null;
};

export type TreeOrganization = {
  groups: readonly ProjectGroup[];
  /** The group of each grouped project, by project id. */
  projectGroups: Readonly<Record<string, string>>;
  /** The manual order of the projects, by id. */
  projectOrder: readonly number[];
};

export const MAX_ROWS_PER_WORKSPACE = 5;

export function groupTreeKey(groupId: string) {
  return `group:${groupId}`;
}

export function projectTreeKey(projectId: number) {
  return `project:${projectId}`;
}

export function workspaceTreeKey(workspaceId: string) {
  return `workspace:${workspaceId}`;
}

const TRAILING_SEPARATORS = /[\\/]+$/;

function normalizeRoot(path: string) {
  return path.replace(TRAILING_SEPARATORS, "");
}

/** True when `path` is `root` or inside it. `/a/application` is not in `/a/app`. */
export function pathIsInside(path: string, root: string) {
  const normalizedPath = normalizeRoot(path);
  const normalizedRoot = normalizeRoot(root);
  if (!normalizedRoot) return false;
  return (
    normalizedPath === normalizedRoot ||
    normalizedPath.startsWith(`${normalizedRoot}/`) ||
    normalizedPath.startsWith(`${normalizedRoot}\\`)
  );
}

// The workspace whose root is the longest root that holds `path`. A known
// project limits the candidates to its workspaces.
export function workspaceForPath<W extends Pick<WorkspaceOverview, "projectId" | "rootPath">>(
  path: string,
  workspaces: readonly W[],
  projectId?: number | null,
): W | null {
  let best: W | null = null;
  for (const workspace of workspaces) {
    if (projectId != null && workspace.projectId !== projectId) continue;
    if (!pathIsInside(path, workspace.rootPath)) continue;
    if (
      !best ||
      normalizeRoot(workspace.rootPath).length > normalizeRoot(best.rootPath).length
    ) {
      best = workspace;
    }
  }
  return best;
}

export { strongestGlyph };

// The glyph of an agent session without a pane view, for example Ghostty.
function terminalStatusGlyph(
  status: ConversationTerminalStatus | null,
): AgentPaneGlyph | null {
  switch (status) {
    case "running":
    case "starting":
      return "working";
    case "waiting_input":
      return "attention";
    case "failed":
      return "failed";
    default:
      return null;
  }
}

// Every signal as one state, ranked as in Orca: needs input > failed >
// working > stopped > done > not reporting > idle. An agent pane gives
// its state glyph. A running chat turn is work, and an unread chat reply
// is a finished turn.
export function treeRowState(input: {
  paneViews: readonly (AgentPaneView | null | undefined)[];
  terminalStatus: ConversationTerminalStatus | null;
  chatRunning: boolean;
  chatUnviewed: boolean;
}): TreeRowState {
  const paneView = strongestAgentPaneView(input.paneViews, agentPaneStateGlyph);
  const { terminalStatus, chatRunning, chatUnviewed } = input;
  const glyph = strongestGlyph([
    agentPaneStateGlyph(paneView),
    terminalStatusGlyph(terminalStatus),
    chatRunning ? "working" : null,
    chatUnviewed ? "done" : null,
  ]);
  const unread =
    chatUnviewed ||
    input.paneViews.some(
      (view) => view?.unread && (view.status === "done" || view.status === "failed"),
    );
  return { glyph, unread, paneView, terminalStatus, chatRunning, chatUnviewed };
}

export function projectAvatar(project: Pick<Project, "name" | "rootPath">) {
  return {
    initial: pinnedConversationInitial(project.name),
    color: pinnedConversationColor(project.rootPath),
  };
}

function compareWorkspaces(left: WorkspaceOverview, right: WorkspaceOverview) {
  return (
    Number(right.kind === "primary") - Number(left.kind === "primary") ||
    Number(right.pinned) - Number(left.pinned) ||
    right.lastUsedAt.localeCompare(left.lastUsedAt) ||
    left.id.localeCompare(right.id)
  );
}

function compareProjects(left: Project, right: Project) {
  return left.createdAt.localeCompare(right.createdAt) || left.id - right.id;
}

function timeOf(value: string) {
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : 0;
}

function rowSortTime<T extends TreeTerminal>(row: TreeAgentRow<T>) {
  return row.kind === "conversation" ? timeOf(row.conversation.updatedAt) : (row.ageAt ?? 0);
}

function compareRows<T extends TreeTerminal>(left: TreeAgentRow<T>, right: TreeAgentRow<T>) {
  const leftPinned = left.kind === "conversation" && left.conversation.pinned;
  const rightPinned = right.kind === "conversation" && right.conversation.pinned;
  return (
    Number(rightPinned) - Number(leftPinned) ||
    rowSortTime(right) - rowSortTime(left) ||
    left.key.localeCompare(right.key)
  );
}

// A project without a listed workspace (the overview is still loading, or
// it failed) gets its primary workspace from the project row, so its
// conversations still have a home.
function fallbackPrimary(project: Project): WorkspaceOverview {
  return {
    id: primaryWorkspaceId(project.id),
    projectId: project.id,
    name: project.name,
    kind: "primary",
    rootPath: project.rootPath,
    managed: false,
    pinned: true,
    state: "available",
    createdAt: project.createdAt,
    lastUsedAt: project.lastOpenedAt ?? project.createdAt,
    available: true,
    isGitRepo: false,
    currentBranch: null,
  };
}

/** Agent panes that run now. A conversation with two or more of them
 *  shows one child row per pane. */
export const TREE_PANE_ROWS_MIN = 2;

function livePane<T extends TreeTerminal>(terminal: T) {
  return (
    Boolean(terminal.agentKind) &&
    (terminal.status === "starting" ||
      terminal.status === "running" ||
      terminal.status === "waiting_input")
  );
}

function paneRow<T extends TreeTerminal>(terminal: T, conversationId: string): TreePaneRow<T> {
  const state = treeRowState({
    paneViews: [terminal.pane],
    terminalStatus: terminal.pane ? null : conversationTerminalStatus([terminal], conversationId),
    chatRunning: false,
    chatUnviewed: false,
  });
  return {
    key: `pane:${terminal.id}`,
    terminal,
    title: terminal.customTitle?.trim() || terminal.title,
    agentKind: terminal.agentKind,
    state,
    ageAt: terminal.pane?.updatedAt ?? null,
  };
}

function conversationRow<T extends TreeTerminal>(
  conversation: ConversationSummary,
  bound: T[],
  input: ProjectTreeInput<T>,
): TreeConversationRow<T> {
  const agentPanes = bound.filter((terminal) => terminal.pane);
  const state = treeRowState({
    paneViews: agentPanes.map((terminal) => terminal.pane),
    terminalStatus: conversationTerminalStatus(
      bound.filter((terminal) => !terminal.pane),
      conversation.id,
    ),
    chatRunning: input.runningConversationIds.has(conversation.id),
    chatUnviewed: input.unviewedConversationIds.has(conversation.id),
  });
  const focusTerminal = state.paneView
    ? agentPanes.find((terminal) => terminal.pane === state.paneView)
    : undefined;
  const settled =
    state.paneView &&
    (state.paneView.status === "done" || state.paneView.status === "failed");
  const agentKind =
    (focusTerminal ?? bound.find((terminal) => terminal.agentKind))?.agentKind ?? null;
  return {
    kind: "conversation",
    key: `conversation:${conversation.id}`,
    conversation,
    title: conversationDisplayTitle(
      conversation,
      conversationNamingTitle(bound, conversation.id),
    ),
    agentKind,
    state,
    ageAt: settled ? state.paneView!.updatedAt : timeOf(conversation.updatedAt),
    focusSessionId: focusTerminal?.id ?? null,
    terminals: bound,
    panes: (() => {
      const live = bound.filter(livePane);
      return live.length >= TREE_PANE_ROWS_MIN
        ? live.map((terminal) => paneRow(terminal, conversation.id))
        : [];
    })(),
  };
}

function terminalRow<T extends TreeTerminal>(terminal: T): TreeTerminalRow<T> {
  const state = treeRowState({
    paneViews: [terminal.pane],
    terminalStatus: terminal.pane
      ? null
      : conversationTerminalStatus([{ ...terminal, conversationId: "" }], ""),
    chatRunning: false,
    chatUnviewed: false,
  });
  return {
    kind: "terminal",
    key: `terminal:${terminal.id}`,
    terminal,
    title: terminal.title,
    agentKind: terminal.agentKind,
    state,
    ageAt: terminal.pane?.updatedAt ?? null,
  };
}

// Rows that need the user, or show work, always show. Quiet rows fill the
// rest of the cap. The shown rows keep the time order, so a row never
// jumps when its state changes.
function capRows<T extends TreeTerminal>(rows: TreeAgentRow<T>[], max: number) {
  const sorted = [...rows].sort(compareRows);
  const room = Math.max(0, max - sorted.filter((row) => row.state.glyph).length);
  const shown = new Set<TreeAgentRow<T>>();
  let quiet = 0;
  for (const row of sorted) {
    if (row.state.glyph) shown.add(row);
    else if (quiet < room) {
      shown.add(row);
      quiet += 1;
    }
  }
  return {
    rows: sorted.filter((row) => shown.has(row)),
    hiddenRows: sorted.filter((row) => !shown.has(row)),
    allRows: sorted,
  };
}

export function buildProjectTree<T extends TreeTerminal>(
  input: ProjectTreeInput<T>,
): ProjectTree<T> {
  // The manual order first (Orca's "Manual" project order), then the rest
  // in the order they were added.
  const manual = new Map(
    (input.organization?.projectOrder ?? []).map((id, index) => [id, index]),
  );
  const projects = [...input.projects].sort(
    (left, right) =>
      (manual.get(left.id) ?? Number.MAX_SAFE_INTEGER) -
        (manual.get(right.id) ?? Number.MAX_SAFE_INTEGER) || compareProjects(left, right),
  );
  const projectIds = new Set(projects.map((project) => project.id));
  const workspacesByProject = new Map<number, WorkspaceOverview[]>();
  for (const workspace of input.workspaces) {
    if (!projectIds.has(workspace.projectId)) continue;
    const list = workspacesByProject.get(workspace.projectId) ?? [];
    list.push(workspace);
    workspacesByProject.set(workspace.projectId, list);
  }
  for (const project of projects) {
    const list = workspacesByProject.get(project.id) ?? [];
    if (!list.some((workspace) => workspace.kind === "primary")) {
      list.push(fallbackPrimary(project));
    }
    workspacesByProject.set(project.id, list.sort(compareWorkspaces));
  }
  const workspaces = [...workspacesByProject.values()].flat();
  const knownWorkspaceIds = new Set(workspaces.map((workspace) => workspace.id));

  const activeConversations = input.conversations.filter(
    (conversation) => conversation.archivedAt == null,
  );
  const visibleConversationIds = new Set(
    activeConversations.map((conversation) => conversation.id),
  );
  const rowsByWorkspace = new Map<string, TreeAgentRow<T>[]>();
  const addRow = (workspaceId: string, row: TreeAgentRow<T>) => {
    const list = rowsByWorkspace.get(workspaceId) ?? [];
    list.push(row);
    rowsByWorkspace.set(workspaceId, list);
  };

  const looseChats: TreeConversationRow<T>[] = [];
  const tabs = input.tabs ?? null;
  const tabbedIds = new Set(tabs ? [...tabs.stripsByWorkspace.values()].flat() : []);
  const tabRows = new Map<string, TreeConversationRow<T>>();
  const historyByWorkspace = new Map<string, TreeConversationRow<T>[]>();
  for (const conversation of activeConversations) {
    const workspaceId =
      conversation.projectId != null && projectIds.has(conversation.projectId)
        ? effectiveWorkspaceId(conversation, knownWorkspaceIds)
        : null;
    const bound = input.terminals.filter(
      (terminal) => terminal.conversationId === conversation.id,
    );
    const row = conversationRow(conversation, bound, input);
    if (!workspaceId) looseChats.push(row);
    else if (!tabs) addRow(workspaceId, row);
    else if (tabbedIds.has(conversation.id)) tabRows.set(conversation.id, row);
    // A live agent never becomes invisible. Adoption opens its tab
    // (ADR 0020). Until then, and after the user closed it, it is a dimmed
    // row.
    else if (tabs.liveConversationIds.has(conversation.id)) {
      addRow(workspaceId, { ...row, untabbed: true });
    } else {
      // A history row has no tab, like a dimmed live row.
      const list = historyByWorkspace.get(workspaceId) ?? [];
      list.push({ ...row, untabbed: true });
      historyByWorkspace.set(workspaceId, list);
    }
  }
  looseChats.sort(compareRows);

  // The standalone list already drops the naming terminal of a visible
  // conversation and the ended agent that a continuation shell replaced.
  const otherTerminals: T[] = [];
  for (const terminal of standaloneSidebarTerminals(
    input.terminals,
    activeConversations,
  )) {
    const bound =
      terminal.conversationId !== null &&
      visibleConversationIds.has(terminal.conversationId);
    const workspace =
      terminal.agentKind && !bound && !tabs
        ? workspaceForPath(terminal.cwd, workspaces)
        : null;
    if (workspace) addRow(workspace.id, terminalRow(terminal));
    else if (!bound) otherTerminals.push(terminal);
  }

  // The folder project of a folder-scan group, while it is in that group.
  const organization = input.organization ?? null;
  const groupFolderIds = new Set(
    (organization?.groups ?? []).flatMap((group) =>
      group.folderProjectId !== undefined &&
      organization?.projectGroups[String(group.folderProjectId)] === group.id
        ? [group.folderProjectId]
        : [],
    ),
  );
  const treeProjects = projects.map((project): TreeProjectNode<T> => {
    const nodes = (workspacesByProject.get(project.id) ?? []).map(
      (workspace): TreeWorkspaceNode<T> => {
        const untabbed = rowsByWorkspace.get(workspace.id) ?? [];
        if (tabs) {
          const stripRows = (tabs.stripsByWorkspace.get(workspace.id) ?? []).flatMap(
            (id) => {
              const row = tabRows.get(id);
              return row ? [row] : [];
            },
          );
          const rows = [...stripRows, ...[...untabbed].sort(compareRows)];
          return {
            key: workspaceTreeKey(workspace.id),
            workspace,
            label:
              workspace.currentBranch?.trim() ||
              workspace.branch?.trim() ||
              workspace.name,
            primary: workspace.kind === "primary",
            glyph: strongestGlyph(rows.map((row) => row.state.glyph)),
            unread: rows.some((row) => row.state.unread),
            rows,
            hiddenRows: [],
            allRows: rows,
            fold: rows.length >= TREE_FOLD_MIN_ROWS,
            history: (historyByWorkspace.get(workspace.id) ?? []).sort(
              (left, right) =>
                timeOf(right.conversation.updatedAt) - timeOf(left.conversation.updatedAt) ||
                left.key.localeCompare(right.key),
            ),
          };
        }
        const allRows = untabbed;
        const capped = capRows(allRows, input.maxRowsPerWorkspace);
        return {
          key: workspaceTreeKey(workspace.id),
          workspace,
          label:
            workspace.currentBranch?.trim() ||
            workspace.branch?.trim() ||
            workspace.name,
          primary: workspace.kind === "primary",
          glyph: strongestGlyph(allRows.map((row) => row.state.glyph)),
          unread: allRows.some((row) => row.state.unread),
          ...capped,
          fold: false,
          history: [],
        };
      },
    );
    return {
      key: projectTreeKey(project.id),
      project,
      avatar: projectAvatar(project),
      glyph: strongestGlyph(nodes.map((node) => node.glyph)),
      workspaces: nodes,
      ...(groupFolderIds.has(project.id) ? { groupFolder: true } : {}),
    };
  });

  // Groups come first, then the projects without a group (Orca). A project
  // that names a missing group stays ungrouped, so it never disappears.
  const groupIds = new Set((organization?.groups ?? []).map((group) => group.id));
  const groupOf = (node: TreeProjectNode<T>) => {
    const id = organization?.projectGroups[String(node.project.id)];
    return id && groupIds.has(id) ? id : null;
  };
  const groups = (organization?.groups ?? []).map((group): TreeGroupNode<T> => {
    // Orca's order under a group header: the folder first, then the repos.
    const members = treeProjects
      .filter((node) => groupOf(node) === group.id)
      .sort((left, right) => Number(right.groupFolder ?? false) - Number(left.groupFolder ?? false));
    return {
      key: groupTreeKey(group.id),
      group,
      glyph: strongestGlyph(members.map((node) => node.glyph)),
      projects: members,
    };
  });
  const ungrouped = treeProjects.filter((node) => groupOf(node) === null);
  return {
    projects: [...groups.flatMap((group) => group.projects), ...ungrouped],
    groups,
    ungrouped,
    looseChats,
    otherTerminals,
    chatTerminals: otherTerminals.map(terminalRow),
  };
}

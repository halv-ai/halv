import { workspaceForPath } from "./projectTree.ts";

// Terminal adoption (ADR 0020). Each live terminal and each live agent
// conversation belongs to a tab. The app adopts the ones without a tab:
//
// - A live terminal without an owner (a plain shell, or an agent terminal
//   without a conversation) whose folder is in a project workspace becomes
//   a shell tab of that workspace.
// - A terminal whose folder is in a git repository that is no project yet:
//   Halv adds the repository root as a project, then adopts the terminal.
//   Rust refuses the home folder root, the app data folder and the
//   temporary folders (`project_adoption_root`).
// - Any other terminal, for example one in the home folder, stays in the
//   Chats area of the sidebar.
// - A live conversation of a project without a tab opens as a tab of its
//   home workspace.
//
// Adoption never steals focus: a new tab does not become active, and the
// visible workspace and tab stay. Each terminal and each conversation is
// adopted at most once per viewer. A tab that the user closes does not
// come back by itself. This module only decides. The App runs the steps.

export type AdoptionTerminal = {
  id: string;
  cwd: string;
  agentKind?: string | null;
  conversationId?: string | null;
  shellTabId?: string | null;
  status: string;
  projectId?: number | null;
};

export type AdoptionWorkspace = {
  id: string;
  projectId: number;
  rootPath: string;
  available: boolean;
};

export type AdoptionStep =
  /** Move the terminal into a new shell tab of the workspace. */
  | { kind: "shell"; sessionId: string; workspaceId: string }
  /** Ask Rust for an adoptable git root of the terminal folder. */
  | { kind: "resolve"; sessionId: string; cwd: string }
  /** Open the tab of the conversation in its home workspace. */
  | { kind: "conversation"; conversationId: string; workspaceId: string };

export type AdoptionMemory = {
  version: 1;
  /** Terminal ids that adoption handled, newest last. */
  terminals: string[];
  /** Conversation ids that adoption handled or that the user closed. */
  conversations: string[];
};

export const ADOPTION_STORAGE_KEY = "halv.tab-adoption.v1";
/** The memory keeps this many ids of each kind. */
export const ADOPTION_MEMORY_LIMIT = 500;

export const EMPTY_ADOPTION_MEMORY: AdoptionMemory = Object.freeze({
  version: 1,
  terminals: [],
  conversations: [],
}) as AdoptionMemory;

type AdoptionStorage = Pick<Storage, "getItem" | "setItem">;

function idList(value: unknown) {
  return Array.isArray(value)
    ? [...new Set(value.filter((item): item is string => typeof item === "string" && item !== ""))]
    : [];
}

export function readAdoptionMemory(storage: AdoptionStorage | null): AdoptionMemory {
  if (!storage) return EMPTY_ADOPTION_MEMORY;
  try {
    const parsed = JSON.parse(storage.getItem(ADOPTION_STORAGE_KEY) ?? "null") as
      | Partial<AdoptionMemory>
      | null;
    if (!parsed || typeof parsed !== "object" || parsed.version !== 1) {
      return EMPTY_ADOPTION_MEMORY;
    }
    return {
      version: 1,
      terminals: idList(parsed.terminals).slice(-ADOPTION_MEMORY_LIMIT),
      conversations: idList(parsed.conversations).slice(-ADOPTION_MEMORY_LIMIT),
    };
  } catch {
    return EMPTY_ADOPTION_MEMORY;
  }
}

export function writeAdoptionMemory(memory: AdoptionMemory, storage: AdoptionStorage | null) {
  if (!storage) return false;
  try {
    storage.setItem(ADOPTION_STORAGE_KEY, JSON.stringify(memory));
    return true;
  } catch {
    return false;
  }
}

/** Adds ids to the memory. The oldest ids go when a list is full. */
export function withAdopted(
  memory: AdoptionMemory,
  ids: { terminals?: readonly string[]; conversations?: readonly string[] },
): AdoptionMemory {
  const add = (list: string[], extra: readonly string[] = []) => {
    const fresh = extra.filter((id) => id && !list.includes(id));
    if (fresh.length === 0) return list;
    return [...list, ...new Set(fresh)].slice(-ADOPTION_MEMORY_LIMIT);
  };
  const terminals = add(memory.terminals, ids.terminals);
  const conversations = add(memory.conversations, ids.conversations);
  return terminals === memory.terminals && conversations === memory.conversations
    ? memory
    : { version: 1, terminals, conversations };
}

/** A terminal that adoption may move: it runs and has no owner. Rust has
 *  the same rule (`check_shell_adoption`). A terminal with a shell tab id
 *  belongs to a shell tab, also when the tab is gone. */
export function adoptableTerminal(terminal: AdoptionTerminal) {
  return (
    terminal.status === "running" &&
    !terminal.conversationId?.trim() &&
    !terminal.shellTabId?.trim()
  );
}

/**
 * The workspace of a terminal folder: the workspace of its project first,
 * else any workspace that holds the folder. Only workspaces on disk count.
 */
export function adoptionWorkspace<W extends AdoptionWorkspace>(
  terminal: Pick<AdoptionTerminal, "cwd" | "projectId">,
  workspaces: readonly W[],
): W | null {
  const available = workspaces.filter((workspace) => workspace.available);
  return (
    (terminal.projectId != null
      ? workspaceForPath(terminal.cwd, available, terminal.projectId)
      : null) ?? workspaceForPath(terminal.cwd, available)
  );
}

/**
 * The next adoption steps. A step names each terminal or conversation at
 * most once. The caller marks each step in the memory before it runs it.
 */
export function adoptionPlan(input: {
  terminals: readonly AdoptionTerminal[];
  workspaces: readonly AdoptionWorkspace[];
  /** Shell tabs on: terminals can move into tabs. */
  shellTabs: boolean;
  /** The home workspace of each conversation with a project, else null. */
  conversationHomes: ReadonlyMap<string, string | null>;
  /** Conversations with a running agent or a running owned terminal. */
  liveConversationIds: ReadonlySet<string>;
  /** Conversations that have a tab now. */
  tabbedConversationIds: ReadonlySet<string>;
  memory: AdoptionMemory;
  /** Steps that run now. */
  pending: ReadonlySet<string>;
  /** Terminals whose folder has no adoptable git root in this run. */
  unresolvable: ReadonlySet<string>;
  /**
   * The adoptable git roots that Rust gave in this run, by terminal id. A
   * known root skips a second git call. Its workspace can differ in the
   * path form from the terminal folder, for example by a symlink.
   */
  resolvedRoots?: ReadonlyMap<string, string>;
  /**
   * The visible workspace while its strip shows no tab (the launcher, or
   * the composer with Pro mode off). A first tab would replace that view,
   * so adoption waits until the user leaves or opens a tab there.
   */
  waitWorkspaceId: string | null;
}): AdoptionStep[] {
  const steps: AdoptionStep[] = [];
  const doneTerminals = new Set(input.memory.terminals);
  const doneConversations = new Set(input.memory.conversations);
  if (input.shellTabs) {
    for (const terminal of input.terminals) {
      if (!adoptableTerminal(terminal)) continue;
      if (doneTerminals.has(terminal.id) || input.pending.has(terminal.id)) continue;
      const root = input.resolvedRoots?.get(terminal.id);
      const workspace =
        adoptionWorkspace(terminal, input.workspaces) ??
        (root ? workspaceForAdoptionRoot(root, input.workspaces) : null);
      if (workspace) {
        if (workspace.id === input.waitWorkspaceId) continue;
        steps.push({ kind: "shell", sessionId: terminal.id, workspaceId: workspace.id });
      } else if (!input.unresolvable.has(terminal.id)) {
        steps.push({ kind: "resolve", sessionId: terminal.id, cwd: terminal.cwd });
      }
    }
  }
  for (const conversationId of [...input.liveConversationIds].sort()) {
    if (input.tabbedConversationIds.has(conversationId)) continue;
    if (doneConversations.has(conversationId) || input.pending.has(conversationId)) continue;
    const workspaceId = input.conversationHomes.get(conversationId) ?? null;
    if (!workspaceId || workspaceId === input.waitWorkspaceId) continue;
    steps.push({ kind: "conversation", conversationId, workspaceId });
  }
  return steps;
}

/**
 * The workspace of an adoptable git root: an existing project with this
 * root gives its primary workspace. Null means that Halv adds the root as
 * a new project first.
 */
export function workspaceForAdoptionRoot<W extends AdoptionWorkspace>(
  root: string,
  workspaces: readonly W[],
): W | null {
  const normalized = root.replace(/[\\/]+$/, "");
  return (
    workspaces.find(
      (workspace) =>
        workspace.available && workspace.rootPath.replace(/[\\/]+$/, "") === normalized,
    ) ?? null
  );
}

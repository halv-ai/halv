import type { ConversationSummary, Project } from "./types";

type WorkspaceStorage = Pick<Storage, "getItem" | "setItem">;

export const LAST_ACTIVE_CONVERSATION_STORAGE_KEY =
  "halv.workspace.last-active-conversation.v1";
export const TRANSCRIPT_SCROLL_STORAGE_PREFIX =
  "halv.workspace.transcript-scroll.v1:";
export const TERMINAL_PANEL_STORAGE_PREFIX = "halv.terminal-layout.v1:";

export type TranscriptScrollState = {
  version: 1;
  top: number;
  pinnedToBottom: boolean;
};

// The scope of a panel. A conversation wins over a shell tab, as in the
// owner rule of a session (ADR 0010, P1).
export function terminalPanelStorageKey(input: {
  conversationId: string | null;
  shellTabId?: string | null;
  agentSessionId?: string | null;
  workspaceId: string | null;
  projectId: number | null;
}) {
  const scope = input.conversationId
    ? `conversation:${input.conversationId}`
    : input.shellTabId
      ? `shell:${input.shellTabId}`
      : input.agentSessionId
        ? `agent:${input.agentSessionId}`
        : input.workspaceId
          ? `workspace:${input.workspaceId}`
          : input.projectId !== null
            ? `project:${input.projectId}`
            : "panel:global";
  return `${TERMINAL_PANEL_STORAGE_PREFIX}${scope}`;
}

/** The id of the workspace that is the project root. */
export function primaryWorkspaceId(projectId: number) {
  return `primary-${projectId}`;
}

// The home workspace of a conversation: its bound workspace, else the
// primary workspace of its project. The project tree (and later the tab
// strip) uses only this rule, so the two cannot disagree. A bound id that
// is not in `knownWorkspaceIds` also falls back, so a row never vanishes.
// A conversation without a project has no workspace.
export function effectiveWorkspaceId(
  conversation: Pick<ConversationSummary, "projectId" | "workspaceId">,
  knownWorkspaceIds?: Pick<ReadonlySet<string>, "has"> | null,
): string | null {
  if (conversation.projectId == null) return null;
  const bound = conversation.workspaceId?.trim();
  if (bound && (!knownWorkspaceIds || knownWorkspaceIds.has(bound))) {
    return bound;
  }
  return primaryWorkspaceId(conversation.projectId);
}

// The cwd of a fresh agent for a conversation. Storage gives the summary
// the root of the bound workspace, else the project root. So an agent of
// a worktree conversation starts in the worktree, not in the project root.
export function agentLaunchCwd(input: {
  conversation:
    | Pick<ConversationSummary, "projectId" | "projectRootPath">
    | null
    | undefined;
  project: Pick<Project, "id" | "rootPath">;
}) {
  const { conversation, project } = input;
  const root = conversation?.projectRootPath?.trim();
  return conversation?.projectId === project.id && root
    ? root
    : project.rootPath;
}

export function readLastActiveConversationId(storage: WorkspaceStorage) {
  try {
    const value = storage.getItem(LAST_ACTIVE_CONVERSATION_STORAGE_KEY)?.trim();
    return value || null;
  } catch {
    return null;
  }
}

export function writeLastActiveConversationId(
  conversationId: string,
  storage: WorkspaceStorage,
) {
  const value = conversationId.trim();
  if (!value) return;
  try {
    storage.setItem(LAST_ACTIVE_CONVERSATION_STORAGE_KEY, value);
  } catch {
    // Storage can be unavailable in restricted webviews.
  }
}

export function resolveLastActiveConversationId(
  savedConversationId: string | null,
  conversationIds: readonly string[],
) {
  if (!savedConversationId) return null;
  return conversationIds.includes(savedConversationId)
    ? savedConversationId
    : null;
}

export function transcriptScrollStorageKey(conversationId: string) {
  return `${TRANSCRIPT_SCROLL_STORAGE_PREFIX}${conversationId}`;
}

export function readTranscriptScrollState(
  conversationId: string,
  storage: WorkspaceStorage,
): TranscriptScrollState | null {
  try {
    const parsed = JSON.parse(
      storage.getItem(transcriptScrollStorageKey(conversationId)) ?? "null",
    ) as Partial<TranscriptScrollState> | null;
    if (
      !parsed ||
      parsed.version !== 1 ||
      typeof parsed.top !== "number" ||
      !Number.isFinite(parsed.top) ||
      parsed.top < 0 ||
      typeof parsed.pinnedToBottom !== "boolean"
    ) {
      return null;
    }
    return {
      version: 1,
      top: parsed.top,
      pinnedToBottom: parsed.pinnedToBottom,
    };
  } catch {
    return null;
  }
}

export function writeTranscriptScrollState(
  conversationId: string,
  state: Omit<TranscriptScrollState, "version">,
  storage: WorkspaceStorage,
) {
  if (!Number.isFinite(state.top) || state.top < 0) return;
  try {
    storage.setItem(
      transcriptScrollStorageKey(conversationId),
      JSON.stringify({ version: 1, ...state } satisfies TranscriptScrollState),
    );
  } catch {
    // Storage can be unavailable in restricted webviews.
  }
}

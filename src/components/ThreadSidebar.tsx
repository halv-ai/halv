import { terminalActivityStore } from "../terminalActivityStore";
import { ipc } from "../ipc.ts";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { formatRelativeTime } from "../format";
import {
  conversationDisplayTitle,
  conversationNamingTerminal,
  conversationNamingTitle,
  conversationTerminalStatus,
} from "../conversationMode";
import { conversationBuckets } from "../conversationOrdering";
import {
  pinnedConversations,
  pinnedConversationColor,
  pinnedConversationInitial,
} from "../pinnedConversations";
import { useT, type Translate } from "../i18n/runtime";
import { inlineRenameCommitValue } from "../sidebarRename";
import { standaloneSidebarTerminals } from "../sidebarTerminals";
import type {
  ConversationExportFormat,
  ConversationSummary,
  MessageSearchResult,
  Project,
  WorkspaceOverview,
} from "../types";
import { DotShortcut } from "./DotShortcut";
import {
  ProjectTree,
  setTreeRowCollapsed,
  useProjectTreeState,
  type ProjectTreeActions,
} from "./ProjectTree";
import {
  buildProjectTree,
  MAX_ROWS_PER_WORKSPACE,
  type TreeTabsInput,
} from "../projectTree";
import { withRowsCollapsed } from "../projectTreeState";
import { terminalFocusBridge } from "../terminalFocusBridge";
import { AgentStateGlyph } from "./AgentStateGlyph";
import {
  agentPaneGlyph,
  strongestAgentPaneView,
  type AgentPaneView,
} from "../agentPaneStatus";

const SEARCH_HIGHLIGHT_START = "[[HALV_HIT]]";
const SEARCH_HIGHLIGHT_END = "[[/HALV_HIT]]";
const CONFIRMATION_TIMEOUT_MS = 4_000;

type InlineRenameTarget = {
  kind: "conversation" | "terminal";
  id: string;
  currentTitle: string;
  draft: string;
};

export type SidebarTerminalSession = {
  id: string;
  title: string;
  customTitle: string | null;
  /** The agent state of a running agent pane. */
  pane?: AgentPaneView | null;
  cwd: string;
  agentKind:
    | "antigravity"
    | "claude"
    | "codex"
    | "gemini"
    | "kimi"
    | "zai"
    | "grok"
    | null;
  conversationId: string | null;
  status: "starting" | "running" | "waiting_input" | "exited" | "failed";
  // The ended agent session that this shell continues in the same pane.
  continuedFrom?: string | null;
};

/** The Projects tree input. Null while the projectTree flag is off. */
export type ThreadSidebarProjectTree = {
  workspaces: WorkspaceOverview[];
  currentWorkspaceId: string | null;
  /** With workspace tabs, the rows come from the strips. */
  tabs?: TreeTabsInput | null;
  /** False until the first project list loads. The empty state waits. */
  projectsLoaded?: boolean;
  onSelectWorkspace: (projectId: number, workspaceId: string) => void;
  /** The actions of the row menus (stage S7). The sidebar adds the two
   *  renames and "Open in tab", which it already owns. */
  actions?: Omit<
    ProjectTreeActions,
    "renameConversation" | "renameTerminal" | "openTerminalInTab"
  > | null;
};

type ThreadSidebarProps = {
  projectTree?: ThreadSidebarProjectTree | null;
  conversations: ConversationSummary[];
  projects: Project[];
  currentConversationId: string | null;
  selectedProjectId: number | null;
  projectSwitcherOpen: boolean;
  collapsed: boolean;
  searchFocusSignal: number;
  runningConversationIds: Set<string>;
  unviewedConversationIds: Set<string>;
  terminals: SidebarTerminalSession[];
  onNewConversation: () => void;
  onOpenDot: () => void;
  dotOpen: boolean;
  onOpenSettings: () => void;
  onLoadConversation: (conversationId: string) => void;
  onFocusTerminal: (sessionId: string) => void;
  onRenameTerminal: (sessionId: string, title: string | null) => void;
  /** "Open in tab" for a plain shell without an owner. Null without shell
   *  tabs. */
  onOpenShellInTab?: ((sessionId: string) => void) | null;
  onDeleteConversation: (conversationId: string) => void;
  onRenameConversation: (conversationId: string, title: string) => void;
  onSetConversationPinned: (conversationId: string, pinned: boolean) => void;
  onExportConversation: (
    conversationId: string,
    format: ConversationExportFormat,
  ) => void;
  onUnarchiveConversation: (conversationId: string) => void;
  onSelectProject: (projectId: number | null) => void;
  onRemoveProject: (projectId: number) => void;
  onAddFolder: () => void;
  onCloneGithub: () => void;
  onProjectSwitcherOpenChange: (open: boolean) => void;
  onToggleCollapsed: () => void;
};

export function ThreadSidebar({
  projectTree = null,
  conversations,
  projects,
  currentConversationId,
  selectedProjectId,
  projectSwitcherOpen,
  collapsed,
  searchFocusSignal,
  runningConversationIds,
  unviewedConversationIds,
  terminals,
  onNewConversation,
  onOpenDot,
  dotOpen,
  onOpenSettings,
  onLoadConversation,
  onFocusTerminal,
  onRenameTerminal,
  onOpenShellInTab = null,
  onDeleteConversation,
  onRenameConversation,
  onSetConversationPinned,
  onExportConversation,
  onUnarchiveConversation,
  onSelectProject,
  onRemoveProject,
  onAddFolder,
  onCloneGithub,
  onProjectSwitcherOpenChange,
  onToggleCollapsed,
}: ThreadSidebarProps) {
  const t = useT();
  const selectedProject =
    projects.find((project) => project.id === selectedProjectId) ?? null;
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<MessageSearchResult[]>([]);
  const [searchPending, setSearchPending] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [archivedOpen, setArchivedOpen] = useState(false);
  const [inlineRename, setInlineRename] =
    useState<InlineRenameTarget | null>(null);
  const [pendingDeleteConversationId, setPendingDeleteConversationId] =
    useTimedConfirmation<string>();
  const projectSwitcherContainerRef = useRef<HTMLDivElement>(null);
  const projectSwitcherTriggerRef = useRef<HTMLButtonElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const renameInputRef = useRef<HTMLInputElement>(null);
  const inlineRenameKey = inlineRename
    ? `${inlineRename.kind}:${inlineRename.id}`
    : null;
  const activeSearch = searchQuery.trim().length > 0;
  const groupedSearchResults = useMemo(
    () => groupSearchResults(searchResults),
    [searchResults],
  );
  const activeConversations = useMemo(
    () => conversations.filter((conversation) => conversation.archivedAt == null),
    [conversations],
  );
  const archivedConversations = useMemo(
    () => conversations.filter((conversation) => conversation.archivedAt != null),
    [conversations],
  );
  const [treeState, updateTreeState] = useProjectTreeState();
  const noProjects =
    projectTree != null && projectTree.projectsLoaded !== false && projects.length === 0;
  // The Projects tree is the sidebar (ADR 0020). The flat list below
  // shows only while the projectTree flag is off.
  const projectTreeModel = useMemo(
    () =>
      projectTree
        ? buildProjectTree({
            projects,
            workspaces: projectTree.workspaces,
            conversations,
            terminals,
            runningConversationIds,
            unviewedConversationIds,
            maxRowsPerWorkspace: MAX_ROWS_PER_WORKSPACE,
            tabs: projectTree.tabs ?? null,
            organization: {
              groups: treeState.groups,
              projectGroups: treeState.projectGroups,
              projectOrder: treeState.projectOrder,
            },
          })
        : null,
    [
      conversations,
      projectTree,
      projects,
      runningConversationIds,
      terminals,
      treeState.groups,
      treeState.projectGroups,
      treeState.projectOrder,
      unviewedConversationIds,
    ],
  );
  const projectTreeActions = useMemo<ProjectTreeActions | null>(
    () =>
      projectTree?.actions
        ? {
            ...projectTree.actions,
            renameConversation: onRenameConversation,
            renameTerminal: onRenameTerminal,
            openTerminalInTab: onOpenShellInTab,
          }
        : null,
    [onOpenShellInTab, onRenameConversation, onRenameTerminal, projectTree?.actions],
  );
  // With the Projects tree, the terminals without a tab show in its Chats
  // area (ADR 0020). The flat list keeps its own terminal section.
  const standaloneTerminals = useMemo(
    () =>
      projectTreeModel ? [] : standaloneSidebarTerminals(terminals, activeConversations),
    [activeConversations, projectTreeModel, terminals],
  );
  const runningTerminalCount = standaloneTerminals.filter((terminal) =>
    terminal.pane
      ? terminal.pane.status === "working"
      : ["starting", "running"].includes(terminal.status),
  ).length;

  useEffect(() => {
    if (searchFocusSignal > 0 && !collapsed) {
      window.requestAnimationFrame(() => searchInputRef.current?.focus());
    }
  }, [collapsed, searchFocusSignal]);

  useEffect(() => {
    if (!inlineRenameKey || collapsed) {
      return undefined;
    }
    const frame = window.requestAnimationFrame(() => {
      renameInputRef.current?.focus();
      renameInputRef.current?.select();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [collapsed, inlineRenameKey]);

  useEffect(() => {
    const query = searchQuery.trim();
    if (!query) {
      setSearchResults([]);
      setSearchError(null);
      setSearchPending(false);
      return undefined;
    }

    let cancelled = false;
    setSearchPending(true);
    const timeout = window.setTimeout(() => {
      void ipc.search.messages({ query })
        .then((results) => {
          if (cancelled) {
            return;
          }
          setSearchResults(results);
          setSearchError(null);
        })
        .catch((error) => {
          if (cancelled) {
            return;
          }
          setSearchResults([]);
          setSearchError(sidebarErrorMessage(error));
        })
        .finally(() => {
          if (!cancelled) {
            setSearchPending(false);
          }
        });
    }, 160);

    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
    };
  }, [searchQuery]);

  useEffect(() => {
    if (!projectSwitcherOpen) {
      return undefined;
    }

    const handlePointerDown = (event: PointerEvent) => {
      const container = projectSwitcherContainerRef.current;
      const trigger = projectSwitcherTriggerRef.current;
      const target = event.target as Node;
      if (
        !container?.contains(target) &&
        !trigger?.contains(target)
      ) {
        onProjectSwitcherOpenChange(false);
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onProjectSwitcherOpenChange(false);
      }
    };

    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [onProjectSwitcherOpenChange, projectSwitcherOpen]);

  function loadConversation(conversationId: string) {
    for (const terminal of terminals) {
      if (terminal.conversationId === conversationId) {
        terminalActivityStore.markViewed(terminal.id);
      }
    }
    onLoadConversation(conversationId);
  }

  function startInlineRename(
    kind: InlineRenameTarget["kind"],
    id: string,
    currentTitle: string,
  ) {
    setInlineRename({ kind, id, currentTitle, draft: currentTitle });
  }

  function commitInlineRename() {
    if (!inlineRename) {
      return;
    }
    const title = inlineRenameCommitValue(
      inlineRename.currentTitle,
      inlineRename.draft,
    );
    setInlineRename(null);
    if (!title) {
      return;
    }
    if (inlineRename.kind === "conversation") {
      onRenameConversation(inlineRename.id, title);
    } else {
      onRenameTerminal(inlineRename.id, title);
    }
  }

  if (collapsed) {
    return (
      <aside
        aria-label={t("Conversation navigation")}
        className="hidden h-full w-14 shrink-0 flex-col items-center overflow-hidden border-r border-surface-border/20 bg-surface-panel md:flex"
      >
        <div className="flex flex-none flex-col items-center gap-2 px-2 py-3">
          <button
            aria-label={t("New conversation")}
            className="flex h-9 w-9 items-center justify-center rounded-lg bg-accent text-lg font-semibold text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40"
            onClick={onNewConversation}
            title={t("New conversation")}
            type="button"
          >
            +
          </button>
          <button
            aria-label={t("Expand sidebar")}
            className="flex h-9 w-9 items-center justify-center rounded-lg text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={onToggleCollapsed}
            title={t("Expand sidebar")}
            type="button"
          >
            <span aria-hidden className="text-base">{"\u2630"}</span>
          </button>
          <button
            aria-label={t("Search conversations")}
            className="flex h-9 w-9 items-center justify-center rounded-lg text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={() => {
              onToggleCollapsed();
              window.requestAnimationFrame(() =>
                window.requestAnimationFrame(() => searchInputRef.current?.focus()),
              );
            }}
            title={t("Search conversations")}
            type="button"
          >
            <span aria-hidden className="text-base">{"\u2315"}</span>
          </button>
        </div>
        <DotShortcut compact onOpen={onOpenDot} active={dotOpen} />
        <nav className="sidebar-pinned-rail" aria-label={t("Pinned")}>
          {pinnedConversations(activeConversations).map((conversation) => {
            const title = conversationDisplayTitle(
              conversation,
              conversationNamingTitle(terminals, conversation.id),
            );
            const { paneView, paneGlyph, terminalStatus } =
              conversationActivity(terminals, conversation.id);
            const working = runningConversationIds.has(conversation.id) ||
              paneGlyph === "working" ||
              terminalStatus === "running" || terminalStatus === "starting";
            const unviewed = unviewedConversationIds.has(conversation.id) ||
              terminalStatus === "waiting_input";
            return (
              <button
                key={conversation.id}
                className="sidebar-pinned-conversation"
                data-color={pinnedConversationColor(conversation.id)}
                aria-current={conversation.id === currentConversationId ? "page" : undefined}
                aria-label={title}
                title={title}
                onClick={() => loadConversation(conversation.id)}
                type="button"
              >
                <span aria-hidden>{pinnedConversationInitial(title)}</span>
                {paneGlyph === "attention" || paneGlyph === "failed" ? (
                  <AgentStateGlyph className="sidebar-pin-activity" size="sm" view={paneView} />
                ) : working ? (
                  <span className="sidebar-pin-activity conversation-spinner" role="status" aria-label={t("Working")} />
                ) : paneGlyph === "done" ? (
                  <AgentStateGlyph className="sidebar-pin-activity" size="sm" view={paneView} />
                ) : unviewed ? (
                  <span className="sidebar-pin-activity sidebar-pin-unviewed" role="status" aria-label={t("Unviewed response")} />
                ) : null}
              </button>
            );
          })}
        </nav>
        <button
          aria-label={t("Open Settings")}
          className="mb-3 flex h-9 w-9 items-center justify-center rounded-lg text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
          onClick={onOpenSettings}
          title={t("Settings")}
          type="button"
        >
          <span aria-hidden className="text-base">{"\u2699"}</span>
        </button>
      </aside>
    );
  }

  return (
    <aside
      aria-label={t("Conversation navigation")}
      className="absolute inset-y-0 left-0 z-40 flex h-full w-[min(20rem,calc(100vw-3.5rem))] shrink-0 flex-col overflow-hidden border-r border-surface-border/25 bg-surface-panel shadow-2xl shadow-black/25 md:relative md:w-80 md:shadow-none"
    >
      <div className="relative flex-none px-4 pb-3 pt-4">
        <div className="flex gap-2">
          <button
            className="min-w-0 flex-1 rounded-lg bg-accent px-3 py-2.5 text-sm font-semibold text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40"
            onClick={onNewConversation}
            type="button"
          >
            <span className="mr-1" aria-hidden>+</span> {t("New chat")}
          </button>
          <button
            aria-label={t("Collapse sidebar")}
            className="flex w-10 shrink-0 items-center justify-center rounded-lg border border-surface-border/25 text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={onToggleCollapsed}
            title={t("Collapse sidebar")}
            type="button"
          >
            <span aria-hidden>{"\u2039"}</span>
          </button>
        </div>

        <div className="mt-3"><DotShortcut onOpen={onOpenDot} active={dotOpen} /></div>
        <div className="mt-3">
          <input
            aria-label={t("Search conversations")}
            className="w-full rounded-lg border border-surface-border/25 bg-transparent px-3 py-2 text-sm text-content-primary outline-none transition-colors placeholder:text-content-muted focus:border-accent focus:ring-2 focus:ring-accent/30"
            onChange={(event) => setSearchQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                setSearchQuery("");
              }
            }}
            placeholder={t("Search")}
            ref={searchInputRef}
            type="search"
            value={searchQuery}
          />
        </div>

        {/* A viewer with no project sees the empty state of the tree
            instead, with the same two actions. */}
        {noProjects ? null : (
          <div className="mt-3">
            <button
              aria-controls="halv-project-switcher"
              aria-expanded={projectSwitcherOpen}
              aria-haspopup="dialog"
              className="flex w-full items-center justify-between gap-3 rounded-lg border border-surface-border/20 bg-surface-raised/30 px-2 py-2 text-left transition-colors hover:border-accent/40 hover:bg-surface-raised/70 focus:outline-none focus:ring-2 focus:ring-accent/30"
              onClick={() => onProjectSwitcherOpenChange(!projectSwitcherOpen)}
              ref={projectSwitcherTriggerRef}
              title={selectedProject?.rootPath ?? t("No project selected")}
              type="button"
            >
              <span className="flex min-w-0 items-center gap-2.5">
                <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-accent/10 font-mono text-xs text-accent">
                  {selectedProject ? projectInitials(selectedProject.name) : "\u2014"}
                </span>
                <span className="min-w-0">
                <span className="block font-mono text-[9px] uppercase tracking-[0.14em] text-content-muted">
                  {t("Project")}
                </span>
                <span className="block truncate text-sm font-semibold text-content-primary">
                  {selectedProject?.name ?? t("No project")}
                </span>
                <span className="mt-0.5 block truncate font-mono text-[10px] text-content-muted">
                  {selectedProject?.rootPath ?? t("Chat without workspace tools")}
                </span>
                </span>
              </span>
              <span className="shrink-0 font-mono text-content-muted" aria-hidden>
                {projectSwitcherOpen ? "^" : "v"}
              </span>
            </button>
          </div>
        )}

        {projectSwitcherOpen ? (
          <ProjectSwitcher
            containerRef={projectSwitcherContainerRef}
            onAddFolder={onAddFolder}
            onCloneGithub={onCloneGithub}
            onClose={() => onProjectSwitcherOpenChange(false)}
            onRemoveProject={onRemoveProject}
            onSelectProject={onSelectProject}
            projects={projects}
            selectedProjectId={selectedProjectId}
          />
        ) : null}
      </div>

      <div
        className={`flex flex-1 flex-col ${
          conversations.length > 0 ? "min-h-[260px]" : "min-h-0"
        }`}
      >
        <nav
          aria-label={t("Conversations")}
          className="min-h-0 flex-1 overflow-y-auto px-3 pb-3 [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:border-0 [&::-webkit-scrollbar-thumb]:bg-accent/50"
          style={{
            scrollbarColor: "rgb(var(--accent) / 0.55) transparent",
            scrollbarWidth: "thin",
          }}
        >
        <h2 className="px-1 pt-1 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
          {projectTree ? t("Projects") : t("Conversations")}
        </h2>
        {activeSearch ? (
          <SearchResultsList
            error={searchError}
            groups={groupedSearchResults}
            onLoadConversation={onLoadConversation}
            pending={searchPending}
          />
        ) : projectTree && projectTreeModel ? (
          <ProjectTree
            collapsed={treeState.collapsed}
            expanded={treeState.expanded}
            currentConversationId={currentConversationId}
            currentWorkspaceId={projectTree.currentWorkspaceId}
            onFocusTerminal={(sessionId) => {
              terminalActivityStore.markViewed(sessionId);
              onFocusTerminal(sessionId);
            }}
            onOpenConversation={(conversationId, focusSessionId) => {
              loadConversation(conversationId);
              if (focusSessionId) terminalFocusBridge.request(focusSessionId);
            }}
            onAddFolder={onAddFolder}
            onCloneGithub={onCloneGithub}
            projectsLoaded={projectTree.projectsLoaded !== false}
            onSelectWorkspace={projectTree.onSelectWorkspace}
            onToggleRow={(key, rowCollapsed) =>
              updateTreeState(setTreeRowCollapsed(key, rowCollapsed))
            }
            onSetRowsCollapsed={(keys, rowsCollapsed) =>
              updateTreeState((state) => withRowsCollapsed(state, keys, rowsCollapsed))
            }
            actions={projectTreeActions}
            onUpdateState={updateTreeState}
            tabsOn={Boolean(projectTree.tabs)}
            tree={projectTreeModel}
          />
        ) : activeConversations.length === 0 ? (
          <p className="px-1 py-3 text-[13px] text-content-muted">
            {archivedConversations.length > 0
              ? t("Nothing active — restore one from Archived.")
              : t("Nothing here yet.")}
          </p>
        ) : (
          conversationBuckets(activeConversations).map((bucket) => (
            <section className="pt-4" key={bucket.label}>
              <div className="px-1 pb-2 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
                {conversationBucketLabel(bucket.label, t)}
              </div>
              <div className="space-y-0.5">
                {bucket.conversations.map((conversation) => {
                  const active = conversation.id === currentConversationId;
                  const running = runningConversationIds.has(conversation.id);
                  const unviewed = unviewedConversationIds.has(conversation.id);
                  const deleteConfirmationPending =
                    conversation.id === pendingDeleteConversationId;
                  const boundTerminal = conversationNamingTerminal(
                    terminals,
                    conversation.id,
                  );
                  const { paneView, paneGlyph, terminalStatus } =
                    conversationActivity(terminals, conversation.id);
                  const working =
                    running ||
                    paneGlyph === "working" ||
                    terminalStatus === "starting" ||
                    terminalStatus === "running";
                  const displayTitle = conversationDisplayTitle(
                    conversation,
                    conversationNamingTitle(terminals, conversation.id),
                  );
                  const isRenaming =
                    inlineRename?.kind === "conversation" &&
                    inlineRename.id === conversation.id;
                  return (
                    <div
                      className={`group relative rounded-md transition-colors ${
                        active
                          ? "bg-accent/10 text-content-primary"
                          : "text-content-primary hover:bg-surface-raised/65"
                      }`}
                      key={conversation.id}
                    >
                      {isRenaming ? (
                        <div className="flex w-full items-center px-1 py-1.5">
                          <input
                            aria-label={t("Rename conversation")}
                            className="min-w-0 flex-1 rounded-md border border-accent/45 bg-surface-raised px-2 py-1 text-sm font-medium text-content-primary outline-none focus:ring-2 focus:ring-accent/25"
                            onBlur={commitInlineRename}
                            onChange={(event) =>
                              setInlineRename((current) =>
                                current
                                  ? { ...current, draft: event.target.value }
                                  : current,
                              )
                            }
                            onKeyDown={(event) => {
                              if (event.key === "Enter") {
                                event.preventDefault();
                                commitInlineRename();
                              } else if (event.key === "Escape") {
                                event.preventDefault();
                                event.stopPropagation();
                                setInlineRename(null);
                              }
                            }}
                            ref={renameInputRef}
                            value={inlineRename?.draft ?? ""}
                          />
                        </div>
                      ) : (
                        <button
                          className="flex w-full items-center gap-2 px-1 py-2 text-left focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
                          onClick={() => loadConversation(conversation.id)}
                          title={t(
                            "{title} - {messageCount} messages - {model}",
                            {
                              title: displayTitle,
                              messageCount: conversation.messageCount,
                              model: conversation.model,
                            },
                          )}
                          type="button"
                        >
                          {paneGlyph === "attention" || paneGlyph === "failed" ? (
                            <AgentStateGlyph view={paneView} />
                          ) : working ? (
                            <span
                              aria-label={t("Working")}
                              className="conversation-spinner h-3 w-3 shrink-0"
                              role="status"
                              title={t("Working")}
                            />
                          ) : paneGlyph === "done" ? (
                            <AgentStateGlyph view={paneView} />
                          ) : terminalStatus ? (
                            <span
                              aria-label={terminalStatusLabel(terminalStatus, t)}
                              className={`agent-session-status-dot agent-session-status-dot-${terminalStatus}`}
                              role="status"
                              title={terminalStatusLabel(terminalStatus, t)}
                            />
                          ) : unviewed ? (
                            <span
                              aria-label={t("Unviewed response")}
                              className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent"
                              role="status"
                              title={t("Unviewed response")}
                            />
                          ) : null}
                          {conversation.pinned ? <PinnedMark /> : null}
                          {conversation.mode === "agent" ? (
                            <span
                              aria-label={t("Agent conversation")}
                              className="conversation-agent-badge"
                              title={t("Agent terminal conversation")}
                            >
                              {">_"}
                            </span>
                          ) : null}
                          {conversation.draft.trim() ? (
                            <span
                              className="shrink-0 rounded border border-warning/35 bg-warning/10 px-1 py-0.5 font-mono text-[9px] uppercase tracking-wide text-warning"
                              title={t("Unsent draft")}
                            >
                              {t("Draft")}
                            </span>
                          ) : null}
                          <span className="min-w-0 flex-1 truncate text-sm font-medium">
                            {displayTitle}
                          </span>
                          <span
                            className={`${boundTerminal ? "mr-16" : "mr-8"} shrink-0 font-mono text-[10px] text-content-muted`}
                          >
                            {relativeTimeLabel(conversation.updatedAt, t)}
                          </span>
                        </button>
                      )}
                      {!isRenaming && boundTerminal ? (
                        <button
                          aria-label={t("Focus terminal for {title}", {
                            title: displayTitle,
                          })}
                          className="absolute right-9 top-1/2 z-20 -translate-y-1/2 rounded border border-accent/30 bg-accent/10 px-1 py-0.5 font-mono text-[9px] text-accent hover:bg-accent/20 focus:outline-none focus:ring-2 focus:ring-accent/30"
                          onClick={() => {
                            terminalActivityStore.markViewed(boundTerminal.id);
                            onFocusTerminal(boundTerminal.id);
                          }}
                          title={t("Open terminal: {title}", {
                            title: boundTerminal.title,
                          })}
                          type="button"
                        >
                          {">_"}
                        </button>
                      ) : null}
                      {!isRenaming ? (
                        <ConversationActions
                          conversation={conversation}
                          deleteConfirmationPending={deleteConfirmationPending}
                          onDelete={() => {
                            if (deleteConfirmationPending) {
                              setPendingDeleteConversationId(null);
                              onDeleteConversation(conversation.id);
                              return true;
                            }
                            setPendingDeleteConversationId(conversation.id);
                            return false;
                          }}
                          onExport={onExportConversation}
                          onTogglePinned={() =>
                            onSetConversationPinned(
                              conversation.id,
                              !conversation.pinned,
                            )
                          }
                          onRename={() =>
                            startInlineRename(
                              "conversation",
                              conversation.id,
                              conversation.title,
                            )
                          }
                        />
                      ) : null}
                    </div>
                  );
                })}
              </div>
            </section>
          ))
        )}
        {!activeSearch && standaloneTerminals.length > 0 ? (
          <section className="pt-4">
            <div className="flex items-center justify-between px-1 pb-2 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
              <span>{t("Terminals")}</span>
              <span>{t("{count} running", { count: runningTerminalCount })}</span>
            </div>
            {standaloneTerminals.length === 0 ? (
              <div className="px-1 py-2 text-xs text-content-muted">
                {t("No terminals open")}
              </div>
            ) : (
              <div className="space-y-0.5">
                {standaloneTerminals.map((terminal) => {
                  const isRenaming =
                    inlineRename?.kind === "terminal" &&
                    inlineRename.id === terminal.id;
                  return (
                    <div
                      className="group relative flex items-center rounded-md text-content-primary transition-colors hover:bg-surface-raised/65"
                      key={terminal.id}
                    >
                      {isRenaming ? (
                        <div className="flex min-w-0 flex-1 items-center gap-2 px-1 py-1.5">
                          <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded bg-accent/10 font-mono text-[9px] font-bold text-accent">
                            {terminalAgentBadge(terminal.agentKind)}
                          </span>
                          <TerminalRowStatus terminal={terminal} />
                          <input
                            aria-label={t("Rename terminal")}
                            className="min-w-0 flex-1 rounded-md border border-accent/45 bg-surface-raised px-2 py-1 text-sm font-medium text-content-primary outline-none focus:ring-2 focus:ring-accent/25"
                            onBlur={commitInlineRename}
                            onChange={(event) =>
                              setInlineRename((current) =>
                                current
                                  ? { ...current, draft: event.target.value }
                                  : current,
                              )
                            }
                            onKeyDown={(event) => {
                              if (event.key === "Enter") {
                                event.preventDefault();
                                commitInlineRename();
                              } else if (event.key === "Escape") {
                                event.preventDefault();
                                event.stopPropagation();
                                setInlineRename(null);
                              }
                            }}
                            ref={renameInputRef}
                            value={inlineRename?.draft ?? ""}
                          />
                        </div>
                      ) : (
                        <>
                          <button
                            className="flex min-w-0 flex-1 items-center gap-2 px-1 py-2 pr-9 text-left focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
                            onClick={() => {
                              terminalActivityStore.markViewed(terminal.id);
                              onFocusTerminal(terminal.id);
                            }}
                            title={`${terminal.title} · ${terminal.cwd}`}
                            type="button"
                          >
                            <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded bg-accent/10 font-mono text-[9px] font-bold text-accent">
                              {terminalAgentBadge(terminal.agentKind)}
                            </span>
                            <TerminalRowStatus terminal={terminal} />
                            <span className="min-w-0 flex-1 truncate text-sm font-medium">
                              {terminal.title}
                            </span>
                          </button>
                          <details className="absolute right-1 top-1/2 z-20 -translate-y-1/2 open:z-40">
                            <summary
                              aria-label={t("Actions for {title}", {
                                title: terminal.title,
                              })}
                              className="flex h-7 w-7 cursor-pointer list-none items-center justify-center rounded-md bg-surface-panel/90 text-content-muted hover:bg-surface-sunken hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
                              title={t("Terminal actions")}
                            >
                              •••
                            </summary>
                            <div className="absolute right-0 top-[calc(100%+0.25rem)] z-30 w-44 rounded-lg border border-surface-border/30 bg-surface-glass/95 p-1 shadow-xl shadow-black/25 backdrop-blur-xl">
                              {onOpenShellInTab &&
                              !terminal.agentKind &&
                              terminal.status === "running" ? (
                                <button
                                  className="block w-full rounded-md px-2.5 py-2 text-left text-xs text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                                  onClick={(event) => {
                                    event.currentTarget
                                      .closest("details")
                                      ?.removeAttribute("open");
                                    onOpenShellInTab(terminal.id);
                                  }}
                                  type="button"
                                >
                                  {t("Open in tab")}
                                </button>
                              ) : null}
                              <button
                                className="block w-full rounded-md px-2.5 py-2 text-left text-xs text-content-secondary hover:bg-surface-raised hover:text-content-primary"
                                onClick={(event) => {
                                  event.currentTarget
                                    .closest("details")
                                    ?.removeAttribute("open");
                                  startInlineRename(
                                    "terminal",
                                    terminal.id,
                                    terminal.customTitle ?? terminal.title,
                                  );
                                }}
                                type="button"
                              >
                                {t("Rename")}
                              </button>
                            </div>
                          </details>
                        </>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </section>
        ) : null}
        {!activeSearch && archivedConversations.length > 0 ? (
          <section className="pt-4">
            <button
              aria-expanded={archivedOpen}
              className="flex w-full items-center justify-between rounded-md px-1 pb-2 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted transition-colors hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
              onClick={() => setArchivedOpen((current) => !current)}
              type="button"
            >
              <span>
                {t("Archived ({count})", {
                  count: archivedConversations.length,
                })}
              </span>
              <span aria-hidden>{archivedOpen ? "−" : "+"}</span>
            </button>
            {archivedOpen ? (
              <div className="space-y-0.5">
                {archivedConversations.map((conversation) => {
                  const active = conversation.id === currentConversationId;
                  const deleteConfirmationPending =
                    conversation.id === pendingDeleteConversationId;
                  return (
                    <div
                      className={`group relative rounded-md transition-colors ${
                        active
                          ? "bg-accent/10 text-content-primary"
                          : "text-content-primary hover:bg-surface-raised/65"
                      }`}
                      key={conversation.id}
                    >
                      <button
                        className="flex w-full items-center gap-2 px-1 py-2 text-left focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
                        onClick={() => loadConversation(conversation.id)}
                        title={t("{title} - archived conversation", {
                          title: conversation.title,
                        })}
                        type="button"
                      >
                        <span className="min-w-0 flex-1 truncate text-sm font-medium">
                          {conversation.title}
                        </span>
                        <span className="mr-8 shrink-0 font-mono text-[10px] text-content-muted">
                          {relativeTimeLabel(conversation.updatedAt, t)}
                        </span>
                      </button>
                      <ConversationActions
                        archived
                        conversation={conversation}
                        deleteConfirmationPending={deleteConfirmationPending}
                        onDelete={() => {
                          if (deleteConfirmationPending) {
                            setPendingDeleteConversationId(null);
                            onDeleteConversation(conversation.id);
                            return true;
                          }
                          setPendingDeleteConversationId(conversation.id);
                          return false;
                        }}
                        onExport={onExportConversation}
                        onRestore={() => onUnarchiveConversation(conversation.id)}
                      />
                    </div>
                  );
                })}
              </div>
            ) : null}
          </section>
        ) : null}
        </nav>
      </div>
    </aside>
  );
}

function terminalAgentBadge(
  agentKind: SidebarTerminalSession["agentKind"],
) {
  if (agentKind === "claude") return "C";
  if (agentKind === "codex") return "X";
  if (agentKind === "gemini") return "Ge";
  if (agentKind === "antigravity") return "A";
  if (agentKind === "kimi") return "K";
  if (agentKind === "zai") return "G";
  if (agentKind === "grok") return "Gr";
  return ">_";
}

// Agent panes rank by their state; other terminals keep the process status.
function conversationActivity(
  terminals: readonly SidebarTerminalSession[],
  conversationId: string,
) {
  const rows = terminals.filter(
    (terminal) => terminal.conversationId === conversationId,
  );
  const paneView = strongestAgentPaneView(rows.map((terminal) => terminal.pane));
  return {
    paneView,
    paneGlyph: agentPaneGlyph(paneView),
    terminalStatus: conversationTerminalStatus(
      rows.filter((terminal) => !terminal.pane),
      conversationId,
    ),
  };
}

function TerminalRowStatus({ terminal }: { terminal: SidebarTerminalSession }) {
  const t = useT();
  return (
    <span className="flex h-3 w-3 shrink-0 items-center justify-center">
      {terminal.pane ? (
        <AgentStateGlyph state view={terminal.pane} />
      ) : (
        <span
          aria-label={terminalStatusLabel(terminal.status, t)}
          className={`agent-session-status-dot agent-session-status-dot-${terminal.status}`}
          role="status"
        />
      )}
    </span>
  );
}

function terminalStatusLabel(
  status: SidebarTerminalSession["status"],
  t: Translate,
) {
  if (status === "waiting_input") return t("Waiting for input");
  if (status === "exited") return t("Finished");
  if (status === "failed") return t("Failed");
  return t("Working");
}

function PinnedMark() {
  const t = useT();
  return (
    <span
      aria-label={t("Pinned")}
      className="flex h-3 w-3 shrink-0 items-center justify-center text-accent"
      role="status"
      title={t("Pinned")}
    >
      <svg
        aria-hidden="true"
        fill="none"
        height="12"
        viewBox="0 0 12 12"
        width="12"
      >
        <path
          d="M4.2 1.5h3.6l-.55 2.45 1.5 1.5v.8h-2.1V10L6 11l-.65-1V6.25h-2.1v-.8l1.5-1.5L4.2 1.5Z"
          fill="currentColor"
        />
      </svg>
    </span>
  );
}

function ConversationActions({
  archived = false,
  conversation,
  deleteConfirmationPending,
  onDelete,
  onExport,
  onRename,
  onRestore,
  onTogglePinned,
}: {
  archived?: boolean;
  conversation: ConversationSummary;
  deleteConfirmationPending: boolean;
  onDelete: () => boolean;
  onExport: (
    conversationId: string,
    format: ConversationExportFormat,
  ) => void;
  onRename?: () => void;
  onRestore?: () => void;
  onTogglePinned?: () => void;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const firstActionRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) {
      return undefined;
    }
    const closeOnPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") {
        return;
      }
      event.preventDefault();
      setOpen(false);
      window.requestAnimationFrame(() => triggerRef.current?.focus());
    };
    document.addEventListener("pointerdown", closeOnPointerDown);
    document.addEventListener("keydown", closeOnEscape);
    window.requestAnimationFrame(() => firstActionRef.current?.focus());
    return () => {
      document.removeEventListener("pointerdown", closeOnPointerDown);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  function finishAction(action: () => void) {
    action();
    setOpen(false);
    window.requestAnimationFrame(() => triggerRef.current?.focus());
  }

  return (
    <div
      className={`absolute right-1 top-1/2 -translate-y-1/2 ${
        open ? "z-40" : "z-20"
      }`}
      ref={rootRef}
    >
      <button
        aria-controls={menuId}
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label={t("Actions for {title}", { title: conversation.title })}
        className="flex h-7 w-7 items-center justify-center rounded-md bg-surface-panel/90 text-content-muted transition-colors hover:bg-surface-sunken hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            setOpen(true);
          }
        }}
        ref={triggerRef}
        title={t("Conversation actions")}
        type="button"
      >
        <span aria-hidden>•••</span>
      </button>
      {open ? (
        <div
          aria-label={t("Actions for {title}", { title: conversation.title })}
          className="absolute right-0 top-[calc(100%+0.25rem)] z-30 w-44 overflow-hidden rounded-lg border border-surface-border/30 bg-surface-glass/95 p-1 shadow-xl shadow-black/25 backdrop-blur-xl"
          id={menuId}
          role="menu"
        >
          {archived && onRestore ? (
            <button
              className="block w-full rounded-md px-2.5 py-2 text-left text-xs text-content-secondary hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
              onClick={() => finishAction(onRestore)}
              ref={firstActionRef}
              role="menuitem"
              type="button"
            >
              {t("Restore conversation")}
            </button>
          ) : null}
          {onTogglePinned ? (
            <button
              className="block w-full rounded-md px-2.5 py-2 text-left text-xs text-content-secondary hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
              onClick={() => finishAction(onTogglePinned)}
              ref={archived ? undefined : firstActionRef}
              role="menuitem"
              type="button"
            >
              {conversation.pinned
                ? t("Unpin conversation")
                : t("Pin conversation")}
            </button>
          ) : null}
          {onRename ? (
            <button
              className="block w-full rounded-md px-2.5 py-2 text-left text-xs text-content-secondary hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
              onClick={() => {
                setOpen(false);
                onRename();
              }}
              ref={archived || onTogglePinned ? undefined : firstActionRef}
              role="menuitem"
              type="button"
            >
              {t("Rename conversation")}
            </button>
          ) : null}
          <button
            className="block w-full rounded-md px-2.5 py-2 text-left text-xs text-content-secondary hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
            onClick={() =>
              finishAction(() => onExport(conversation.id, "markdown"))
            }
            ref={archived || onTogglePinned || onRename ? undefined : firstActionRef}
            role="menuitem"
            type="button"
          >
            {t("Export Markdown")}
          </button>
          <button
            className="block w-full rounded-md px-2.5 py-2 text-left text-xs text-content-secondary hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
            onClick={() => finishAction(() => onExport(conversation.id, "json"))}
            role="menuitem"
            type="button"
          >
            {t("Export JSON")}
          </button>
          <div className="my-1 h-px bg-surface-border/20" />
          <button
            aria-label={
              deleteConfirmationPending
                ? t("Confirm delete {title}", { title: conversation.title })
                : t("Delete {title}", { title: conversation.title })
            }
            className={`block w-full rounded-md px-2.5 py-2 text-left text-xs focus:outline-none focus:ring-2 focus:ring-inset focus:ring-danger/30 ${
              deleteConfirmationPending
                ? "bg-danger/10 font-medium text-danger"
                : "text-content-secondary hover:bg-danger/10 hover:text-danger"
            }`}
            onClick={() => {
              const deleted = onDelete();
              if (deleted) {
                setOpen(false);
              }
            }}
            role="menuitem"
            type="button"
          >
            {deleteConfirmationPending
              ? t("Confirm delete")
              : t("Delete conversation")}
          </button>
        </div>
      ) : null}
    </div>
  );
}

function SearchResultsList({
  groups,
  pending,
  error,
  onLoadConversation,
}: {
  groups: SearchResultGroup[];
  pending: boolean;
  error: string | null;
  onLoadConversation: (conversationId: string) => void;
}) {
  const t = useT();
  if (pending && groups.length === 0) {
    return (
      <div
        aria-live="polite"
        className="mt-3 rounded-lg border border-surface-border/25 bg-surface-raised/45 px-3 py-4 font-mono text-[11px] text-content-muted"
        role="status"
      >
        {t("Searching...")}
      </div>
    );
  }

  if (error) {
    return (
      <div
        className="mt-3 rounded-lg border border-danger/30 bg-danger/10 px-3 py-4 text-sm text-danger"
        role="alert"
      >
        {error}
      </div>
    );
  }

  if (groups.length === 0) {
    return (
      <div
        aria-live="polite"
        className="mt-3 rounded-lg border border-dashed border-surface-border/30 bg-surface-raised/45 px-3 py-4 text-sm text-content-muted"
        role="status"
      >
        {t("No matches")}
      </div>
    );
  }

  return (
    <>
      <div className="sr-only" aria-live="polite" role="status">
        {t("{count} search results", {
          count: groups.reduce(
            (total, group) => total + group.results.length,
            0,
          ),
        })}
      </div>
      {groups.map((group) => (
        <div className="mt-3 rounded-lg border border-surface-border/20 bg-surface-raised/35" key={group.conversationId}>
          <button
            className="block w-full border-b border-surface-border/15 px-3 py-2 text-left text-sm font-medium text-content-primary transition-colors duration-150 hover:bg-surface-raised focus:outline-none focus:ring-2 focus:ring-inset focus:ring-accent/30"
            onClick={() => onLoadConversation(group.conversationId)}
            title={group.title}
            type="button"
          >
            <span className="block truncate">{group.title}</span>
          </button>
          <div className="space-y-1 p-1.5">
            {group.results.map((result) => (
              <button
                className="block w-full rounded-md px-2 py-2 text-left transition-colors duration-150 hover:bg-surface-sunken/45 focus:outline-none focus:ring-2 focus:ring-accent/30"
                key={result.messageId}
                onClick={() => onLoadConversation(result.conversationId)}
                type="button"
              >
                <span className="font-mono text-[10px] uppercase tracking-[0.14em] text-accent">
                  {messageRoleLabel(result.role, t)}
                </span>
                <span className="mt-1 block text-xs leading-5 text-content-secondary">
                  <HighlightedSnippet snippet={result.snippet} />
                </span>
              </button>
            ))}
          </div>
        </div>
      ))}
    </>
  );
}

type SearchResultGroup = {
  conversationId: string;
  title: string;
  results: MessageSearchResult[];
};

function groupSearchResults(results: MessageSearchResult[]): SearchResultGroup[] {
  const groups: SearchResultGroup[] = [];
  for (const result of results) {
    let group = groups.find(
      (item) => item.conversationId === result.conversationId,
    );
    if (!group) {
      group = {
        conversationId: result.conversationId,
        title: result.conversationTitle,
        results: [],
      };
      groups.push(group);
    }
    group.results.push(result);
  }
  return groups;
}

function HighlightedSnippet({ snippet }: { snippet: string }) {
  return (
    <>
      {splitHighlightedSnippet(snippet).map((part, index) =>
        part.highlight ? (
          <mark
            className="rounded bg-accent/20 px-0.5 text-accent-strong"
            key={index}
          >
            {part.text}
          </mark>
        ) : (
          <span key={index}>{part.text}</span>
        ),
      )}
    </>
  );
}

function splitHighlightedSnippet(snippet: string) {
  const parts: { text: string; highlight: boolean }[] = [];
  let cursor = 0;
  while (cursor < snippet.length) {
    const start = snippet.indexOf(SEARCH_HIGHLIGHT_START, cursor);
    if (start === -1) {
      parts.push({ text: snippet.slice(cursor), highlight: false });
      break;
    }
    if (start > cursor) {
      parts.push({ text: snippet.slice(cursor, start), highlight: false });
    }
    const highlightStart = start + SEARCH_HIGHLIGHT_START.length;
    const end = snippet.indexOf(SEARCH_HIGHLIGHT_END, highlightStart);
    if (end === -1) {
      parts.push({ text: snippet.slice(highlightStart), highlight: true });
      break;
    }
    parts.push({
      text: snippet.slice(highlightStart, end),
      highlight: true,
    });
    cursor = end + SEARCH_HIGHLIGHT_END.length;
  }
  return parts.length > 0 ? parts : [{ text: snippet, highlight: false }];
}

function sidebarErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function ProjectSwitcher({
  compact = false,
  containerRef,
  projects,
  selectedProjectId,
  onSelectProject,
  onRemoveProject,
  onAddFolder,
  onCloneGithub,
  onClose,
}: {
  compact?: boolean;
  containerRef: { current: HTMLDivElement | null };
  projects: Project[];
  selectedProjectId: number | null;
  onSelectProject: (projectId: number | null) => void;
  onRemoveProject: (projectId: number) => void;
  onAddFolder: () => void;
  onCloneGithub: () => void;
  onClose: () => void;
}) {
  const t = useT();
  const [pendingRemoveProjectId, setPendingRemoveProjectId] =
    useTimedConfirmation<number>();

  return (
    <div
      aria-label={t("Project switcher")}
      className={`absolute z-30 overflow-hidden rounded-xl border border-surface-border/30 bg-surface-glass/95 shadow-2xl shadow-[rgb(var(--shadow-color)/0.22)] backdrop-blur-xl ${
        compact ? "left-14 top-12 w-72" : "-mt-1 left-4 right-4 top-full"
      }`}
      id="halv-project-switcher"
      ref={containerRef}
      role="dialog"
    >
      <div className="max-h-72 overflow-y-auto p-1.5">
        <button
          className={`mb-1 flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left transition-colors ${
            selectedProjectId === null
              ? "bg-accent/10 text-content-primary"
              : "text-content-secondary hover:bg-surface-raised/70"
          }`}
          onClick={() => {
            onSelectProject(null);
            onClose();
          }}
          type="button"
        >
          <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-surface-sunken font-mono text-xs text-content-muted">
            {"\u2014"}
          </span>
          <span className="min-w-0">
            <span className="block text-sm font-medium">{t("No project")}</span>
            <span className="mt-0.5 block text-[10px] text-content-muted">
              {t("Plain chat, without workspace tools")}
            </span>
          </span>
        </button>
        {projects.length === 0 ? (
          <div className="px-2 py-3 text-sm text-content-muted">
            {t("No projects yet")}
          </div>
        ) : (
          projects.map((project) => {
            const selected = project.id === selectedProjectId;
            const removeConfirmationPending =
              project.id === pendingRemoveProjectId;
            return (
              <div
                className={`group flex items-center gap-2 rounded-lg px-2 py-2 transition-colors duration-150 ${
                  selected ? "bg-accent/10" : "hover:bg-surface-raised/70"
                }`}
                key={project.id}
              >
                <button
                  className="min-w-0 flex-1 text-left focus:outline-none"
                  onClick={() => {
                    onSelectProject(project.id);
                    onClose();
                  }}
                  title={project.rootPath}
                  type="button"
                >
                  <span className="block truncate text-sm font-medium text-content-primary">
                    {project.name}
                  </span>
                  <span className="mt-0.5 block truncate font-mono text-[11px] text-content-muted">
                    {project.rootPath}
                  </span>
                </button>
                <button
                  aria-label={
                    removeConfirmationPending
                      ? t("Confirm remove {name}", { name: project.name })
                      : t("Remove {name}", { name: project.name })
                  }
                  className={`shrink-0 whitespace-nowrap rounded px-2 py-1 font-mono text-[11px] transition-colors duration-150 hover:bg-danger/10 hover:text-danger focus:outline-none focus:ring-2 focus:ring-danger/30 ${
                    removeConfirmationPending
                      ? "bg-danger/10 text-danger opacity-100"
                      : "text-content-muted opacity-70 group-hover:opacity-100"
                  }`}
                  onClick={(event) => {
                    event.stopPropagation();
                    if (removeConfirmationPending) {
                      setPendingRemoveProjectId(null);
                      onRemoveProject(project.id);
                      return;
                    }
                    setPendingRemoveProjectId(project.id);
                  }}
                  title={
                    removeConfirmationPending
                      ? t("Confirm project removal")
                      : t("Deregister project")
                  }
                  type="button"
                >
                  {removeConfirmationPending
                    ? t("Confirm remove?")
                    : t("Remove")}
                </button>
              </div>
            );
          })
        )}
      </div>
      <div className="grid grid-cols-2 gap-2 border-t border-surface-border/25 p-2">
        <button
          className="rounded-lg border border-surface-border/30 px-2 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
          onClick={onAddFolder}
          type="button"
        >
          {t("Add folder...")}
        </button>
        <button
          className="rounded-lg border border-surface-border/30 px-2 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
          onClick={onCloneGithub}
          type="button"
        >
          {t("Clone from GitHub...")}
        </button>
      </div>
    </div>
  );
}

function conversationBucketLabel(label: string, t: Translate) {
  switch (label) {
    case "TODAY":
      return t("TODAY");
    case "YESTERDAY":
      return t("YESTERDAY");
    case "PREVIOUS 7 DAYS":
      return t("PREVIOUS 7 DAYS");
    case "OLDER":
      return t("OLDER");
    default:
      return label;
  }
}

function messageRoleLabel(role: MessageSearchResult["role"], t: Translate) {
  switch (role) {
    case "user":
      return t("user");
    case "assistant":
      return t("assistant");
    case "developer":
      return t("developer");
    case "system":
      return t("system");
  }
}

function relativeTimeLabel(value: string, t: Translate) {
  const relativeTime = formatRelativeTime(value);
  if (relativeTime === "unknown") {
    return t("unknown");
  }
  if (relativeTime === "just now") {
    return t("just now");
  }
  const past = /^(\d+)(y|mo|w|d|h|m) ago$/.exec(relativeTime);
  if (past) {
    const amount = past[1];
    switch (past[2]) {
      case "y":
        return t("{amount}y ago", { amount });
      case "mo":
        return t("{amount}mo ago", { amount });
      case "w":
        return t("{amount}w ago", { amount });
      case "d":
        return t("{amount}d ago", { amount });
      case "h":
        return t("{amount}h ago", { amount });
      case "m":
        return t("{amount}m ago", { amount });
    }
  }
  const future = /^in (\d+)(y|mo|w|d|h|m)$/.exec(relativeTime);
  if (future) {
    const amount = future[1];
    switch (future[2]) {
      case "y":
        return t("in {amount}y", { amount });
      case "mo":
        return t("in {amount}mo", { amount });
      case "w":
        return t("in {amount}w", { amount });
      case "d":
        return t("in {amount}d", { amount });
      case "h":
        return t("in {amount}h", { amount });
      case "m":
        return t("in {amount}m", { amount });
    }
  }
  return relativeTime;
}

function conversationInitials(title: string) {
  const words = title.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) {
    return "?";
  }
  return words
    .slice(0, 2)
    .map((word) => word[0])
    .join("")
    .toUpperCase();
}

function projectInitials(name: string) {
  return conversationInitials(name).slice(0, 2) || "P";
}

function useTimedConfirmation<T>() {
  const [pendingId, setPendingId] = useState<T | null>(null);

  useEffect(() => {
    if (pendingId === null) {
      return undefined;
    }
    const timeout = window.setTimeout(
      () => setPendingId(null),
      CONFIRMATION_TIMEOUT_MS,
    );
    return () => window.clearTimeout(timeout);
  }, [pendingId]);

  return [pendingId, setPendingId] as const;
}

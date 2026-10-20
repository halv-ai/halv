import {
  memo,
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent,
  type MouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { copyText } from "../clipboard";
import {
  agentGlyphTitle,
  agentPaneNeedsUser,
  agentPaneStateGlyph,
  agentPaneTitle,
  agentPromptSummary,
} from "../agentPaneStatus";
import { agentSessionAgentLabel, type AgentKind } from "../agentSessions";
import { agentKindForBackend } from "../conversationMode";
import { useT, type Translate } from "../i18n/runtime";
import {
  agentChipGroups,
  type ProjectTree as ProjectTreeModel,
  type TreeAgentRow,
  type TreeGroupNode,
  type TreeProjectNode,
  type TreeRowState,
  type TreeTerminal,
  type TreeWorkspaceNode,
} from "../projectTree";
import {
  projectTreeStateStore,
  withGroupDeleted,
  withGroupRenamed,
  withNewGroup,
  withProjectInGroup,
  withProjectMoved,
  withRowCollapsed,
  withRowExpanded,
  type ProjectTreeState,
} from "../projectTreeState";
import {
  agentRowMenu,
  groupRowMenu,
  menuPlatform,
  projectRowMenu,
  workspaceRowMenu,
  type TreeMenuItem,
} from "../projectTreeMenus";
import { treeOperationsStore } from "../projectTreeOperations";
import { shortAge, shortAgeLabel } from "../shortAge";
import type { WorkspaceStatus } from "../types";
import { AgentGlyph } from "./AgentGlyph";
import { AgentStateGlyph } from "./AgentStateGlyph";
import {
  DeleteProjectGroupDialog,
  NewWorktreeDialog,
  ProjectGroupNameDialog,
  RemoveProjectDialog,
  RemoveWorktreeDialog,
} from "./ProjectTreeDialogs";
import { TreeContextMenu, type TreeMenuRequest } from "./TreeContextMenu";

// The Projects tree of the sidebar (ADR 0010, stage S1). It follows the
// WAI-ARIA tree pattern: Up and Down move, Right expands or enters, Left
// collapses or leaves, Home and End jump, and Enter or Space activates.
// The tree only reads state. Its clicks reuse the handlers of today's list.
// A right click, the "…" button, Shift+F10 or the context menu key opens
// the row menu (stage S7), as in Orca.

const AGE_TICK_MS = 30_000;

/** The collapsed rows and the groups, kept in localStorage. */
export function useProjectTreeState() {
  const state = useSyncExternalStore(
    projectTreeStateStore.subscribe,
    projectTreeStateStore.getSnapshot,
  );
  return [state, projectTreeStateStore.update] as const;
}

export function setTreeRowCollapsed(key: string, collapsed: boolean) {
  // An "N agents" pill and a History start folded, so they store the open
  // state.
  if (key.startsWith("agents:") || key.startsWith("history:")) {
    return (state: ProjectTreeState) => withRowExpanded(state, key, !collapsed);
  }
  return (state: ProjectTreeState) => withRowCollapsed(state, key, collapsed);
}

function useNow(active: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return undefined;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), AGE_TICK_MS);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

function treeItems(root: HTMLElement) {
  return [...root.querySelectorAll<HTMLElement>('[role="treeitem"]')];
}

function activateItem(item: HTMLElement) {
  item.querySelector<HTMLElement>(":scope > .project-tree-row")?.click();
}

// One handler for every tree of the sidebar. Collapsed groups are not in
// the DOM, so the query lists exactly the visible items.
function handleTreeKeyDown(
  event: KeyboardEvent<HTMLElement>,
  onToggle: (key: string, collapsed: boolean) => void,
) {
  const target = event.target as HTMLElement;
  // The rename field keeps its keys.
  if (target.tagName === "INPUT") return;
  const item = target.closest<HTMLElement>('[role="treeitem"]');
  if (!item || event.altKey || event.metaKey || event.ctrlKey) return;
  if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
    event.preventDefault();
    const row = item.querySelector<HTMLElement>(":scope > .project-tree-row");
    const rect = row?.getBoundingClientRect();
    row?.dispatchEvent(
      new window.MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        clientX: (rect?.left ?? 0) + 24,
        clientY: rect?.bottom ?? 0,
      }),
    );
    return;
  }
  const items = treeItems(event.currentTarget);
  const index = items.indexOf(item);
  const expanded = item.getAttribute("aria-expanded");
  const key = item.dataset.treeKey ?? "";
  const move = (target: HTMLElement | undefined | null) => {
    event.preventDefault();
    target?.focus();
  };
  switch (event.key) {
    case "ArrowDown":
      return move(items[index + 1]);
    case "ArrowUp":
      return move(items[index - 1]);
    case "Home":
      return move(items[0]);
    case "End":
      return move(items.at(-1));
    case "ArrowRight":
      if (expanded === "false") {
        event.preventDefault();
        onToggle(key, false);
      } else if (expanded === "true") {
        move(item.querySelector<HTMLElement>('[role="group"] [role="treeitem"]'));
      }
      return;
    case "ArrowLeft":
      if (expanded === "true") {
        event.preventDefault();
        onToggle(key, true);
      } else {
        move(item.parentElement?.closest<HTMLElement>('[role="treeitem"]'));
      }
      return;
    case "Enter":
    case " ":
      event.preventDefault();
      activateItem(item);
      return;
  }
}

type TreeItemProps = {
  treeKey: string;
  level: number;
  focusKey: string;
  expanded?: boolean;
  current?: boolean;
  className?: string;
  row: ReactNode;
  rowTitle?: string;
  onActivate: () => void;
  onContextMenu?: (event: MouseEvent<HTMLDivElement>) => void;
  onPointerDown?: (event: ReactPointerEvent<HTMLDivElement>) => void;
  children?: ReactNode;
};

function TreeItem({
  treeKey,
  level,
  focusKey,
  expanded,
  current,
  className,
  row,
  rowTitle,
  onActivate,
  onContextMenu,
  onPointerDown,
  children,
}: TreeItemProps) {
  const labelId = useId();
  return (
    <div
      aria-current={current ? "page" : undefined}
      aria-expanded={expanded}
      aria-labelledby={labelId}
      aria-level={level}
      aria-selected={current ? true : undefined}
      className={`project-tree-item ${className ?? ""}`}
      data-tree-key={treeKey}
      role="treeitem"
      tabIndex={treeKey === focusKey ? 0 : -1}
    >
      <div
        className="project-tree-row"
        id={labelId}
        onClick={onActivate}
        onContextMenu={onContextMenu}
        onPointerDown={onPointerDown}
        style={{ paddingLeft: `${4 + (level - 1) * 14}px` }}
        title={rowTitle}
      >
        {row}
      </div>
      {expanded && children ? <div role="group">{children}</div> : null}
    </div>
  );
}

function Chevron({ expanded }: { expanded: boolean }) {
  return (
    <span aria-hidden className="project-tree-chevron" data-expanded={expanded}>
      {"\u203A"}
    </span>
  );
}

// The History disclosure icon (lucide History).
function HistoryIcon() {
  return (
    <svg
      aria-hidden
      className="project-tree-branch-icon"
      fill="none"
      height={12}
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
      viewBox="0 0 24 24"
      width={12}
    >
      <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
      <path d="M3 3v5h5" />
      <path d="M12 7v5l4 2" />
    </svg>
  );
}

// Orca's group header icon (lucide FolderTree).
function FolderTreeIcon() {
  return (
    <svg
      aria-hidden
      className="project-tree-branch-icon"
      fill="none"
      height={13}
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
      viewBox="0 0 24 24"
      width={13}
    >
      <path d="M20 10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1h-2.5a1 1 0 0 1-.8-.4l-.9-1.2A1 1 0 0 0 15 3h-2a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z" />
      <path d="M20 21a1 1 0 0 0 1-1v-3a1 1 0 0 0-1-1h-2.9a1 1 0 0 1-.88-.55l-.42-.85a1 1 0 0 0-.92-.6H13a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z" />
      <path d="M3 5a2 2 0 0 0 2 2h3" />
      <path d="M3 3v13a2 2 0 0 0 2 2h3" />
    </svg>
  );
}

// The folder project row of a folder-scan group (lucide Folder).
function FolderIcon() {
  return (
    <svg
      aria-hidden
      fill="none"
      height={12}
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
      viewBox="0 0 24 24"
      width={12}
    >
      <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
    </svg>
  );
}

function BranchIcon() {
  return (
    <svg
      aria-hidden
      className="project-tree-branch-icon"
      fill="none"
      height={12}
      stroke="currentColor"
      strokeLinecap="round"
      strokeWidth="2"
      viewBox="0 0 24 24"
      width={12}
    >
      <circle cx="6" cy="5" r="2.2" />
      <circle cx="6" cy="19" r="2.2" />
      <circle cx="18" cy="7" r="2.2" />
      <path d="M6 7.2v9.6M18 9.2c0 5-6 3.5-11 7.6" />
    </svg>
  );
}

function rowGlyph(state: TreeRowState, t: Translate) {
  if (!state.glyph) return null;
  // The pane names its own state, for example "Needs approval".
  const label =
    state.paneView && agentPaneStateGlyph(state.paneView) === state.glyph
      ? state.glyph === "unverifiable"
        ? undefined
        : agentPaneTitle(state.paneView, t)
      : state.glyph === "done" && state.chatUnviewed
        ? t("Unviewed response")
        : undefined;
  return <AgentStateGlyph glyph={state.glyph} label={label} size="md" />;
}

function agentIconKind<T extends TreeTerminal>(row: TreeAgentRow<T>) {
  if (row.agentKind) return row.agentKind;
  if (row.kind === "terminal") return null;
  return (
    agentKindForBackend(row.conversation.agentBackend) ??
    (row.conversation.mode === "chat" ? "chat" : null)
  );
}

function rowDetail<T extends TreeTerminal>(row: TreeAgentRow<T>, t: Translate) {
  const view = row.state.paneView;
  if (!view || !agentPaneNeedsUser(view)) return null;
  const agent = row.agentKind
    ? agentSessionAgentLabel(row.agentKind as AgentKind)
    : t("Agent");
  return agentPromptSummary(view.prompt, agent, t);
}

/** The inline rename of a row. */
export type TreeRename = { key: string; draft: string };

type AgentRowProps<T extends TreeTerminal> = {
  row: TreeAgentRow<T>;
  level: number;
  focusKey: string;
  current: boolean;
  now: number;
  onActivate: (row: TreeAgentRow<T>) => void;
  onMenu?: (row: TreeAgentRow<T>, event: MouseEvent<HTMLDivElement>) => void;
  /** The draft title while this row is renamed. */
  renameDraft?: string | null;
  onRename?: (change: { draft: string } | { commit: true } | { cancel: true }) => void;
  /** The pane child rows show (stage S7). They start open, as in Orca. */
  panesOpen?: boolean;
  onTogglePanes?: (key: string, collapsed: boolean) => void;
  onActivatePane?: (row: TreeAgentRow<T>, sessionId: string) => void;
};

function AgentRowItem<T extends TreeTerminal>({
  row,
  level,
  focusKey,
  current,
  now,
  onActivate,
  onMenu,
  renameDraft = null,
  onRename,
  panesOpen = true,
  onTogglePanes,
  onActivatePane,
}: AgentRowProps<T>) {
  const t = useT();
  const renaming = renameDraft !== null;
  const panes = row.kind === "conversation" ? row.panes : [];
  const detail = rowDetail(row, t);
  const stateName = row.state.paneView
    ? agentPaneTitle(row.state.paneView, t)
    : null;
  // A finished turn keeps its check after a view. Only news is bold.
  const emphasized = row.state.glyph === "attention" || row.state.unread;
  const untabbed = row.kind === "conversation" && row.untabbed === true;
  return (
    <TreeItem
      className={untabbed ? "project-tree-untabbed" : undefined}
      current={current}
      expanded={panes.length > 0 ? panesOpen : undefined}
      focusKey={focusKey}
      level={level}
      onActivate={() => {
        if (!renaming) onActivate(row);
      }}
      onContextMenu={onMenu ? (event) => onMenu(row, event) : undefined}
      row={
        <>
          <span className="project-tree-lane">{rowGlyph(row.state, t)}</span>
          <AgentGlyph kind={agentIconKind(row)} />
          {renaming ? (
            <input
              aria-label={row.kind === "conversation" ? t("Rename conversation") : t("Rename terminal")}
              autoFocus
              className="project-tree-rename"
              onBlur={() => onRename?.({ commit: true })}
              onChange={(event) => onRename?.({ draft: event.target.value })}
              onClick={(event) => event.stopPropagation()}
              onFocus={(event) => event.currentTarget.select()}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  onRename?.({ commit: true });
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  event.stopPropagation();
                  onRename?.({ cancel: true });
                }
              }}
              value={renameDraft}
            />
          ) : (
            <span className="min-w-0 flex-1">
              <span
                className={`block truncate text-[13px] ${emphasized ? "font-semibold" : "font-normal"}`}
              >
                {row.title}
              </span>
              {detail ? (
                <span className="project-tree-detail block truncate">{detail}</span>
              ) : null}
            </span>
          )}
          {row.ageAt !== null && !renaming ? (
            <span className="project-tree-age">
              {shortAgeLabel(shortAge(row.ageAt, now), t)}
            </span>
          ) : null}
          {panes.length > 0 ? (
            <button
              aria-hidden
              className="project-tree-disclosure"
              onClick={(event) => {
                event.stopPropagation();
                onTogglePanes?.(row.key, panesOpen);
              }}
              tabIndex={-1}
              type="button"
            >
              <Chevron expanded={panesOpen} />
            </button>
          ) : null}
        </>
      }
      rowTitle={[row.title, stateName, detail, untabbed ? t("No tab") : null]
        .filter(Boolean)
        .join(" · ")}
      treeKey={row.key}
    >
      {panes.map((pane) => (
        <TreeItem
          className="project-tree-pane"
          focusKey={focusKey}
          key={pane.key}
          level={level + 1}
          onActivate={() => onActivatePane?.(row, pane.terminal.id)}
          row={
            <>
              <span
                aria-hidden
                className="project-tree-pane-guide"
                style={{ left: `${4 + (level - 1) * 14 + 5}px` }}
              />
              <span className="project-tree-lane">{rowGlyph(pane.state, t)}</span>
              <AgentGlyph kind={pane.agentKind} />
              <span className="min-w-0 flex-1 truncate text-[12.5px] text-content-secondary">
                {pane.title}
              </span>
              {pane.ageAt !== null ? (
                <span className="project-tree-age">
                  {shortAgeLabel(shortAge(pane.ageAt, now), t)}
                </span>
              ) : null}
            </>
          }
          rowTitle={[
            pane.title,
            pane.state.paneView ? agentPaneTitle(pane.state.paneView, t) : null,
          ]
            .filter(Boolean)
            .join(" · ")}
          treeKey={pane.key}
        />
      ))}
    </TreeItem>
  );
}

const MemoAgentRow = memo(AgentRowItem) as typeof AgentRowItem;

// The folded agents of a workspace with two or more rows, as Orca's agent
// summary: one chip per state (glyph, agent icons, "+N"), then a chevron.
// It starts folded; the sidebar keeps the choice per workspace. Open, it
// is a header with the agent count.
function AgentsPill<T extends TreeTerminal>({
  treeKey,
  rows,
  open,
  focusKey,
  level = 3,
  onToggle,
  children,
}: {
  treeKey: string;
  rows: readonly TreeAgentRow<T>[];
  open: boolean;
  focusKey: string;
  level?: number;
  onToggle: (open: boolean) => void;
  children: ReactNode;
}) {
  const t = useT();
  const { groups, hiddenCount } = agentChipGroups(
    rows.map((row) => ({ glyph: row.state.glyph, kind: agentIconKind(row) })),
  );
  const countLabel = t("{count} agents", { count: rows.length });
  const summary = [
    countLabel,
    ...groups.map((group) => `${agentGlyphTitle(group.glyph, t)} ${group.count}`),
    ...(hiddenCount > 0 ? [`+${hiddenCount}`] : []),
  ].join(", ");
  return (
    <TreeItem
      className="project-tree-agents-pill"
      expanded={open}
      focusKey={focusKey}
      level={level}
      onActivate={() => onToggle(open)}
      row={
        <>
          <span className="project-tree-lane" />
          {open ? (
            <span className="project-tree-pill-label">{countLabel}</span>
          ) : (
            <span className="project-tree-chips">
              <span className="sr-only">{summary}</span>
              {groups.map((group) => (
                <span
                  aria-hidden
                  className="project-tree-chip"
                  data-chip-glyph={group.glyph}
                  key={group.glyph}
                >
                  <AgentStateGlyph glyph={group.glyph} size="sm" />
                  <span className="project-tree-chip-icons">
                    {group.kinds.map((kind, index) => (
                      <span className="project-tree-chip-icon" key={`${kind}:${index}`}>
                        <AgentGlyph kind={kind} size={11} />
                      </span>
                    ))}
                  </span>
                  {group.extra > 0 ? (
                    <span className="project-tree-chip-extra">+{group.extra}</span>
                  ) : null}
                </span>
              ))}
              {hiddenCount > 0 ? (
                <span aria-hidden className="project-tree-chip-extra">
                  +{hiddenCount}
                </span>
              ) : null}
            </span>
          )}
          <Chevron expanded={open} />
        </>
      }
      rowTitle={summary}
      treeKey={treeKey}
    >
      {children}
    </TreeItem>
  );
}

type ProjectTreeProps<T extends TreeTerminal> = {
  tree: ProjectTreeModel<T>;
  collapsed: readonly string[];
  /** The open "N agents" pills (workspace tabs). */
  expanded?: readonly string[];
  currentConversationId: string | null;
  currentWorkspaceId: string | null;
  onToggleRow: (key: string, collapsed: boolean) => void;
  onSelectWorkspace: (projectId: number, workspaceId: string) => void;
  onOpenConversation: (conversationId: string, focusSessionId: string | null) => void;
  onFocusTerminal: (sessionId: string) => void;
  /** False until the first project list loads. The empty state waits. */
  projectsLoaded?: boolean;
  /** The actions of the empty state, when the viewer has no project. */
  onAddFolder?: () => void;
  onCloneGithub?: () => void;
  /** Collapses or expands many rows in one change ("Collapse all"). */
  onSetRowsCollapsed?: (keys: string[], collapsed: boolean) => void;
  /** The row menus and their actions. Null: the rows have no menu. */
  actions?: ProjectTreeActions | null;
  /** Workspace tabs are on: a row can close its tab. */
  tabsOn?: boolean;
  /** Changes the per-viewer tree state: groups and the project order. */
  onUpdateState?: (change: (state: ProjectTreeState) => ProjectTreeState) => void;
};

/** What the row menus do (ADR 0010, stage S7). The App owns each action.
 *  An action that fails rejects, and the tree shows the error. */
export type ProjectTreeActions = {
  renameConversation: (conversationId: string, title: string) => void;
  renameTerminal: (sessionId: string, title: string | null) => void;
  /** "Open in tab" of a terminal without a tab (ADR 0020). Null without
   *  shell tabs. */
  openTerminalInTab: ((sessionId: string) => void) | null;
  setConversationUnread: (conversationId: string, unread: boolean) => void;
  closeConversationTab: (conversationId: string) => Promise<void>;
  archiveConversation: (conversationId: string) => Promise<void>;
  /** Workspace tabs only. */
  newAgentHere: ((workspaceId: string) => Promise<void>) | null;
  newTerminalHere: (projectId: number, workspaceId: string) => Promise<void>;
  openWorkspaceIn: (workspaceId: string, target: "fileManager" | "editor") => Promise<void>;
  setWorkspacePinned: (workspaceId: string, pinned: boolean) => Promise<void>;
  /** The status of one workspace, with its changes and conversations. */
  inspectWorkspace: (projectId: number, workspaceId: string) => Promise<WorkspaceStatus | null>;
  /** Running terminals that a removal stops. */
  openTerminalCount: (scope: { workspaceId: string } | { projectId: number }) => number;
  removeWorktree: (workspaceId: string) => Promise<void>;
  removeProject: (projectId: number) => Promise<void>;
  listBaseRefs: (projectId: number) => Promise<string[]>;
  /** Starts a creation. The tree shows its pending row. */
  createWorktree: (
    projectId: number,
    request: { branch: string | null; baseRef: string },
  ) => Promise<void>;
};

type TreeDialogState =
  | { kind: "new-worktree"; projectId: number; name: string }
  | { kind: "new-group"; projectId: number; name: string }
  | { kind: "rename-group"; groupId: string; name: string }
  | { kind: "delete-group"; groupId: string; name: string; projectNames: string[] }
  | { kind: "remove-worktree"; projectId: number; workspaceId: string; name: string; rootPath: string }
  | { kind: "remove-project"; projectId: number; name: string; worktreeCount: number };

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** A new viewer starts here: no project yet, and the two ways to add one. */
function ProjectTreeEmpty({
  onAddFolder,
  onCloneGithub,
}: {
  onAddFolder?: () => void;
  onCloneGithub?: () => void;
}) {
  const t = useT();
  return (
    <div className="project-tree-empty">
      <p className="project-tree-empty-title">{t("No projects yet")}</p>
      <p className="project-tree-empty-body">
        {t("Add a folder or clone a repository. Its worktrees and agents show here.")}
      </p>
      {onAddFolder || onCloneGithub ? (
        <div className="project-tree-empty-actions">
          {onAddFolder ? (
            <button
              className="project-tree-empty-action"
              data-primary=""
              onClick={onAddFolder}
              type="button"
            >
              {t("Add folder...")}
            </button>
          ) : null}
          {onCloneGithub ? (
            <button className="project-tree-empty-action" onClick={onCloneGithub} type="button">
              {t("Clone from GitHub...")}
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export function agentsPillKey(workspaceId: string) {
  return `agents:${workspaceId}`;
}

export function historyKey(workspaceId: string) {
  return `history:${workspaceId}`;
}

/** History rows that show before "Show N more". */
const HISTORY_SHOWN = 8;

export function ProjectTree<T extends TreeTerminal>({
  tree,
  collapsed,
  expanded = [],
  currentConversationId,
  currentWorkspaceId,
  onToggleRow,
  onSelectWorkspace,
  onOpenConversation,
  onFocusTerminal,
  projectsLoaded = true,
  onAddFolder,
  onCloneGithub,
  onSetRowsCollapsed,
  actions = null,
  tabsOn = false,
  onUpdateState,
}: ProjectTreeProps<T>) {
  const t = useT();
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const [showAll, setShowAll] = useState<ReadonlySet<string>>(() => new Set());
  const [menu, setMenu] = useState<TreeMenuRequest | null>(null);
  const menuTriggerRef = useRef<HTMLElement | null>(null);
  const [rename, setRename] = useState<TreeRename | null>(null);
  const renameRef = useRef<TreeRename | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [dialog, setDialog] = useState<TreeDialogState | null>(null);
  const operations = useSyncExternalStore(
    treeOperationsStore.subscribe,
    treeOperationsStore.getSnapshot,
  );
  const platform = menuPlatform(typeof navigator === "undefined" ? "" : navigator.platform);
  const collapsedKeys = new Set(collapsed);
  const expandedKeys = new Set(expanded);
  const hasRows =
    tree.looseChats.length > 0 ||
    tree.chatTerminals.length > 0 ||
    tree.projects.some((project) =>
      project.workspaces.some((node) => node.rows.length > 0),
    );
  const now = useNow(hasRows);

  // Roving tab stop: the focused row, else the current row, else the first.
  const visibleKeys: string[] = [];
  const visibleProjects = [
    ...tree.groups.flatMap((group) => {
      visibleKeys.push(group.key);
      return collapsedKeys.has(group.key) ? [] : group.projects;
    }),
    ...tree.ungrouped,
  ];
  for (const project of visibleProjects) {
    visibleKeys.push(project.key);
    if (collapsedKeys.has(project.key)) continue;
    for (const node of project.workspaces) {
      visibleKeys.push(node.key);
      if (collapsedKeys.has(node.key)) continue;
      const pillKey = agentsPillKey(node.workspace.id);
      if (node.fold) visibleKeys.push(pillKey);
      const expandedRows =
        node.fold && !expandedKeys.has(pillKey)
          ? []
          : showAll.has(node.key)
            ? node.allRows
            : node.rows;
      for (const row of expandedRows) {
        visibleKeys.push(row.key);
        if (row.kind === "conversation" && !collapsedKeys.has(row.key)) {
          visibleKeys.push(...row.panes.map((pane) => pane.key));
        }
      }
      if (node.hiddenRows.length > 0) visibleKeys.push(`more:${node.key}`);
      const history = historyKey(node.workspace.id);
      if (node.history.length > 0 && expandedKeys.has(history)) {
        visibleKeys.push(history);
        const shown = showAll.has(history) ? node.history : node.history.slice(0, HISTORY_SHOWN);
        visibleKeys.push(...shown.map((row) => row.key));
        if (node.history.length > HISTORY_SHOWN) visibleKeys.push(`more:${history}`);
      }
    }
  }
  const currentRowKey = currentConversationId
    ? `conversation:${currentConversationId}`
    : currentWorkspaceId
      ? `workspace:${currentWorkspaceId}`
      : null;
  const focusKey =
    (focusedKey && visibleKeys.includes(focusedKey) && focusedKey) ||
    (currentRowKey && visibleKeys.includes(currentRowKey) && currentRowKey) ||
    visibleKeys[0] ||
    "";

  const chatsFocusKey =
    tree.looseChats.find((row) => row.conversation.id === currentConversationId)?.key ??
    tree.looseChats[0]?.key ??
    tree.chatTerminals[0]?.key ??
    "";

  const activateRow = useCallback(
    (row: TreeAgentRow<T>) => {
      if (row.kind === "conversation") {
        onOpenConversation(row.conversation.id, row.focusSessionId);
      } else {
        onFocusTerminal(row.terminal.id);
      }
    },
    [onFocusTerminal, onOpenConversation],
  );

  // An action that fails shows its error above the tree.
  const run = (action: () => void | Promise<void>) => {
    setNotice(null);
    try {
      void Promise.resolve(action()).catch((error) => setNotice(errorText(error)));
    } catch (error) {
      setNotice(errorText(error));
    }
  };

  const copy = async (text: string) => {
    if (!(await copyText(text))) throw new Error(t("Could not copy to the clipboard."));
  };

  const openMenu = (
    event: MouseEvent<HTMLElement>,
    label: string,
    items: TreeMenuItem[],
    onSelect: (id: string) => void,
    anchor?: DOMRect,
  ) => {
    event.preventDefault();
    event.stopPropagation();
    delete menuTriggerRef.current?.dataset.menuOpen;
    menuTriggerRef.current = event.currentTarget.closest<HTMLElement>('[role="treeitem"]');
    // The row keeps its highlight while its menu is open.
    if (menuTriggerRef.current) menuTriggerRef.current.dataset.menuOpen = "";
    setMenu({
      x: anchor ? anchor.left : event.clientX,
      y: anchor ? anchor.bottom + 2 : event.clientY,
      label,
      items,
      onSelect,
    });
  };

  const closeMenu = useCallback(() => {
    setMenu(null);
    delete menuTriggerRef.current?.dataset.menuOpen;
    // The focus goes back to the row, unless the action took it.
    window.requestAnimationFrame(() => {
      const active = document.activeElement;
      if (!active || active === document.body) menuTriggerRef.current?.focus();
    });
  }, []);

  const startRename = (key: string, title: string) => {
    renameRef.current = { key, draft: title };
    setRename(renameRef.current);
  };

  // The latest menu and rename handlers, behind stable callbacks, so the
  // memoized rows do not render again on each status change.
  const agentMenuRef = useRef<(row: TreeAgentRow<T>, event: MouseEvent<HTMLDivElement>) => void>();
  agentMenuRef.current = (row, event) => {
    if (!actions) return;
    const conversation = row.kind === "conversation" ? row.conversation : null;
    const terminal = row.kind === "terminal" ? row.terminal : null;
    const path = conversation ? (conversation.projectRootPath ?? null) : (terminal?.cwd ?? null);
    const items = agentRowMenu(
      {
        kind: row.kind,
        unread: row.state.unread,
        tabbed: tabsOn && row.kind === "conversation" && row.untabbed !== true && conversation?.projectId != null,
        path,
        openInTab:
          actions.openTerminalInTab !== null &&
          terminal !== null &&
          terminal.status === "running" &&
          !terminal.conversationId,
      },
      t,
    );
    openMenu(event, t("Actions for {title}", { title: row.title }), items, (id) => {
      switch (id) {
        case "open":
          return activateRow(row);
        case "rename":
          return startRename(
            row.key,
            conversation ? conversation.title : (terminal?.customTitle ?? row.title),
          );
        case "copy-title":
          return run(() => copy(row.title));
        case "copy-path":
          return path ? run(() => copy(path)) : undefined;
        case "mark-read":
          return conversation && run(() => actions.setConversationUnread(conversation.id, false));
        case "mark-unread":
          return conversation && run(() => actions.setConversationUnread(conversation.id, true));
        case "close-tab":
          return conversation && run(() => actions.closeConversationTab(conversation.id));
        case "archive":
          return conversation && run(() => actions.archiveConversation(conversation.id));
        case "open-in-tab":
          return terminal && actions.openTerminalInTab?.(terminal.id);
      }
    });
  };
  const onAgentMenu = useCallback(
    (row: TreeAgentRow<T>, event: MouseEvent<HTMLDivElement>) => agentMenuRef.current?.(row, event),
    [],
  );

  // Stable callbacks for the pane child rows of the memoized agent rows.
  const toggleRowRef = useRef(onToggleRow);
  toggleRowRef.current = onToggleRow;
  const onTogglePanes = useCallback(
    (key: string, collapsed: boolean) => toggleRowRef.current(key, collapsed),
    [],
  );
  const openConversationRef = useRef(onOpenConversation);
  openConversationRef.current = onOpenConversation;
  const onActivatePane = useCallback((row: TreeAgentRow<T>, sessionId: string) => {
    if (row.kind === "conversation") openConversationRef.current(row.conversation.id, sessionId);
  }, []);

  const renameRowsRef = useRef(new Map<string, TreeAgentRow<T>>());
  const actionsRef = useRef(actions);
  actionsRef.current = actions;
  const onRename = useCallback(
    (change: { draft: string } | { commit: true } | { cancel: true }) => {
      const current = renameRef.current;
      if (!current) return;
      if ("draft" in change) {
        renameRef.current = { ...current, draft: change.draft };
        setRename(renameRef.current);
        return;
      }
      renameRef.current = null;
      setRename(null);
      if ("cancel" in change) return;
      const row = renameRowsRef.current.get(current.key);
      const title = current.draft.trim();
      const handlers = actionsRef.current;
      if (!row || !handlers) return;
      if (row.kind === "conversation") {
        if (title && title !== row.conversation.title) {
          handlers.renameConversation(row.conversation.id, title);
        }
      } else if (title !== (row.terminal.customTitle ?? row.title)) {
        handlers.renameTerminal(row.terminal.id, title || null);
      }
    },
    [],
  );

  const workspaceMenu = (
    event: MouseEvent<HTMLElement>,
    project: TreeProjectNode<T>,
    node: TreeWorkspaceNode<T>,
    anchor?: DOMRect,
  ) => {
    if (!actions) return;
    const workspace = node.workspace;
    const removing = operations.removals[workspace.id]?.status === "removing";
    const items = workspaceRowMenu(
      {
        primary: node.primary,
        managed: workspace.managed,
        pinned: workspace.pinned,
        available: workspace.available,
        agentTabs: actions.newAgentHere !== null,
      },
      platform,
      t,
    ).map((entry) => (removing && entry.kind === "item" ? { ...entry, disabled: true } : entry));
    if (node.history.length > 0) {
      const open = expandedKeys.has(historyKey(workspace.id));
      const index = items.findIndex((entry) => entry.kind === "item" && entry.id === "copy-branch");
      items.splice(index + 1, 0, {
        kind: "item",
        id: "toggle-history",
        label: open
          ? t("Hide history")
          : t("Show history ({count})", { count: node.history.length }),
      });
    }
    openMenu(event, t("Actions for {title}", { title: node.label }), items, (id) => {
      switch (id) {
        case "new-agent":
          return run(() => actions.newAgentHere?.(workspace.id));
        case "new-terminal":
          return run(() => actions.newTerminalHere(project.project.id, workspace.id));
        case "open-file-manager":
          return run(() => actions.openWorkspaceIn(workspace.id, "fileManager"));
        case "open-editor":
          return run(() => actions.openWorkspaceIn(workspace.id, "editor"));
        case "copy-path":
          return run(() => copy(workspace.rootPath));
        case "copy-branch":
          return run(() => copy(node.label));
        case "toggle-history":
          return toggleHistory(node, expandedKeys.has(historyKey(workspace.id)));
        case "pin":
          return run(() => actions.setWorkspacePinned(workspace.id, true));
        case "unpin":
          return run(() => actions.setWorkspacePinned(workspace.id, false));
        case "remove-worktree":
          return setDialog({
            kind: "remove-worktree",
            projectId: project.project.id,
            workspaceId: workspace.id,
            name: node.label,
            rootPath: workspace.rootPath,
          });
        case "remove-project":
          return openRemoveProject(project);
      }
    }, anchor);
  };

  const projectIsGitRepo = (project: TreeProjectNode<T>) =>
    project.workspaces.find((node) => node.primary)?.workspace.isGitRepo ?? false;

  const openNewWorktree = (project: TreeProjectNode<T>) =>
    setDialog({ kind: "new-worktree", projectId: project.project.id, name: project.project.name });

  const openRemoveProject = (project: TreeProjectNode<T>) =>
    setDialog({
      kind: "remove-project",
      projectId: project.project.id,
      name: project.project.name,
      worktreeCount: project.workspaces.filter((node) => node.workspace.managed).length,
    });

  const projectMenu = (
    event: MouseEvent<HTMLElement>,
    project: TreeProjectNode<T>,
    anchor?: DOMRect,
  ) => {
    if (!actions) return;
    const workspaceKeys = project.workspaces.map((node) => node.key);
    const allCollapsed =
      workspaceKeys.length > 0 && workspaceKeys.every((key) => collapsedKeys.has(key));
    const groupId =
      tree.groups.find((group) => group.projects.includes(project))?.group.id ?? null;
    const items = projectRowMenu(
      {
        allCollapsed,
        gitRepo: projectIsGitRepo(project),
        groups: onUpdateState ? tree.groups.map((group) => group.group) : [],
        groupId,
      },
      platform,
      t,
    ).filter(
      // The group items need the tree state.
      (entry) =>
        onUpdateState ||
        entry.kind === "separator" ||
        !["new-group", "remove-from-group"].includes(entry.id),
    );
    openMenu(event, t("Actions for {title}", { title: project.project.name }), items, (id) => {
      if (id.startsWith("move-to-group:")) {
        const target = id.slice("move-to-group:".length);
        return onUpdateState?.((state) => withProjectInGroup(state, project.project.id, target));
      }
      switch (id) {
        case "new-worktree":
          return openNewWorktree(project);
        case "new-group":
          return setDialog({
            kind: "new-group",
            projectId: project.project.id,
            name: t("{name} group", { name: project.project.name }),
          });
        case "remove-from-group":
          return onUpdateState?.((state) => withProjectInGroup(state, project.project.id, null));
        case "open-file-manager":
          return run(() =>
            actions.openWorkspaceIn(
              project.workspaces.find((node) => node.primary)?.workspace.id ??
                `primary-${project.project.id}`,
              "fileManager",
            ),
          );
        case "copy-path":
          return run(() => copy(project.project.rootPath));
        case "collapse-all":
          if (collapsedKeys.has(project.key)) onToggleRow(project.key, false);
          return onSetRowsCollapsed?.(workspaceKeys, true);
        case "expand-all":
          if (collapsedKeys.has(project.key)) onToggleRow(project.key, false);
          return onSetRowsCollapsed?.(workspaceKeys, false);
        case "remove-project":
          return openRemoveProject(project);
      }
    }, anchor);
  };

  const groupMenu = (event: MouseEvent<HTMLElement>, node: TreeGroupNode<T>, anchor?: DOMRect) => {
    if (!onUpdateState) return;
    openMenu(event, t("Actions for {title}", { title: node.group.name }), groupRowMenu(t), (id) => {
      if (id === "rename-group") {
        setDialog({ kind: "rename-group", groupId: node.group.id, name: node.group.name });
      } else if (id === "delete-group") {
        setDialog({
          kind: "delete-group",
          groupId: node.group.id,
          name: node.group.name,
          projectNames: node.projects.map((project) => project.project.name),
        });
      }
    }, anchor);
  };

  // Drag a project row with the pointer (the divider-drag pattern, no
  // library). A drop on a project row places the project before or after
  // it, in the group of that row. A drop on a group row moves the project
  // to the end of the group. Orca reorders only inside one group; Halv
  // also drags a project into a group.
  const [drag, setDrag] = useState<{
    projectId: number;
    target: { key: string; edge: "before" | "after" | "inside"; top: number; left: number; width: number } | null;
  } | null>(null);
  const dragRef = useRef<{
    projectId: number;
    pointerId: number;
    x: number;
    y: number;
    active: boolean;
  } | null>(null);
  const suppressClickRef = useRef(false);

  const dropTargetAt = (x: number, y: number, projectId: number) => {
    const element = document.elementFromPoint(x, y);
    const target = element?.closest<HTMLElement>(
      '.project-tree-root [role="treeitem"][data-tree-key^="project:"], .project-tree-root [role="treeitem"][data-tree-key^="group:"]',
    );
    const key = target?.dataset.treeKey;
    const row = target?.querySelector<HTMLElement>(":scope > .project-tree-row");
    if (!key || !row || key === `project:${projectId}`) return null;
    const rect = row.getBoundingClientRect();
    if (key.startsWith("group:")) {
      return { key, edge: "inside" as const, top: rect.top, left: rect.left, width: rect.width };
    }
    const before = y < rect.top + rect.height / 2;
    // The lower edge of an open project is after its last row.
    const box = target!.getBoundingClientRect();
    return {
      key,
      edge: before ? ("before" as const) : ("after" as const),
      top: before ? rect.top : box.bottom,
      left: rect.left,
      width: rect.width,
    };
  };

  const commitDrop = (projectId: number, target: NonNullable<NonNullable<typeof drag>["target"]>) => {
    if (!onUpdateState) return;
    const order = tree.projects.map((node) => node.project.id);
    if (target.edge === "inside") {
      const groupId = target.key.slice("group:".length);
      onUpdateState((state) =>
        withProjectMoved(state, { projectId, groupId, beforeProjectId: null, order }),
      );
      return;
    }
    const targetId = Number(target.key.slice("project:".length));
    const bucket =
      tree.groups.find((group) => group.projects.some((node) => node.project.id === targetId)) ??
      null;
    const members = (bucket ? bucket.projects : tree.ungrouped)
      .map((node) => node.project.id)
      .filter((id) => id !== projectId);
    const index = members.indexOf(targetId);
    const beforeProjectId =
      target.edge === "before" ? targetId : (members[index + 1] ?? null);
    onUpdateState((state) =>
      withProjectMoved(state, {
        projectId,
        groupId: bucket?.group.id ?? null,
        beforeProjectId,
        order,
      }),
    );
  };

  const startProjectDrag = (event: ReactPointerEvent<HTMLDivElement>, projectId: number) => {
    if (!onUpdateState || event.button !== 0 || (event.target as HTMLElement).closest("button")) {
      return;
    }
    dragRef.current = {
      projectId,
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      active: false,
    };
    const move = (pointer: PointerEvent) => {
      const current = dragRef.current;
      if (!current || pointer.pointerId !== current.pointerId) return;
      if (!current.active && Math.hypot(pointer.clientX - current.x, pointer.clientY - current.y) < 5) {
        return;
      }
      current.active = true;
      setDrag({ projectId, target: dropTargetAt(pointer.clientX, pointer.clientY, projectId) });
    };
    const finish = (pointer: PointerEvent) => {
      const current = dragRef.current;
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      dragRef.current = null;
      if (!current?.active) return;
      suppressClickRef.current = true;
      window.setTimeout(() => {
        suppressClickRef.current = false;
      }, 0);
      const target =
        pointer.type === "pointerup" ? dropTargetAt(pointer.clientX, pointer.clientY, projectId) : null;
      setDrag(null);
      if (target) commitDrop(projectId, target);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
  };

  // Opening a History also opens its workspace row.
  const toggleHistory = (node: TreeWorkspaceNode<T>, open: boolean) => {
    if (!open && collapsedKeys.has(node.key)) onToggleRow(node.key, false);
    onToggleRow(historyKey(node.workspace.id), open);
  };

  const toggleShowAll = (key: string) =>
    setShowAll((current) => {
      const next = new Set(current);
      if (!next.delete(key)) next.add(key);
      return next;
    });

  renameRowsRef.current = new Map(
    [
      ...tree.projects.flatMap((project) =>
        project.workspaces.flatMap((node) => [...node.allRows, ...node.history]),
      ),
      ...tree.looseChats,
      ...tree.chatTerminals,
    ].map((row) => [row.key, row]),
  );

  const agentRow = (row: TreeAgentRow<T>, level: number, rowFocusKey: string) => (
    <MemoAgentRow
      current={row.kind === "conversation" && row.conversation.id === currentConversationId}
      focusKey={rowFocusKey}
      key={row.key}
      level={level}
      now={now}
      onActivate={activateRow}
      onActivatePane={onActivatePane}
      onMenu={actions ? onAgentMenu : undefined}
      onRename={onRename}
      onTogglePanes={onTogglePanes}
      panesOpen={!collapsedKeys.has(row.key)}
      renameDraft={rename?.key === row.key ? rename.draft : null}
      row={row}
    />
  );

  // `depth` is 1 inside a project group, else 0.
  const renderWorkspace = (
    project: TreeProjectNode<T>,
    node: TreeWorkspaceNode<T>,
    depth: number,
  ) => {
    const open = !collapsedKeys.has(node.key);
    const allShown = showAll.has(node.key);
    const rows = allShown ? node.allRows : node.rows;
    const unavailable = !node.workspace.available;
    const removal = operations.removals[node.workspace.id] ?? null;
    const removing = removal?.status === "removing";
    const failedRemoval = removal?.status === "failed" ? removal : null;
    const history = historyKey(node.workspace.id);
    // The History shows only when the user opens it, so the tree stays
    // quiet (stage S7).
    const historyOpen = node.history.length > 0 && expandedKeys.has(history);
    const historyAll = showAll.has(history);
    const hasChildren =
      rows.length > 0 || node.hiddenRows.length > 0 || historyOpen || failedRemoval !== null;
    const emphasized = node.glyph === "attention" || node.unread;
    return (
      <TreeItem
        className={
          removing
            ? "project-tree-removing"
            : unavailable
              ? "project-tree-unavailable"
              : undefined
        }
        current={node.workspace.id === currentWorkspaceId}
        expanded={hasChildren ? open : undefined}
        focusKey={focusKey}
        key={node.key}
        level={depth + 2}
        onActivate={() => {
          if (!removing) onSelectWorkspace(project.project.id, node.workspace.id);
        }}
        onContextMenu={actions ? (event) => workspaceMenu(event, project, node) : undefined}
        row={
          <>
            <span className="project-tree-lane">
              <AgentStateGlyph glyph={node.glyph} size="md" summary />
            </span>
            <BranchIcon />
            <span
              className={`min-w-0 flex-1 truncate font-mono text-[12px] ${emphasized ? "font-semibold text-content-primary" : "text-content-secondary"}`}
            >
              {node.label}
            </span>
            {removing ? (
              <span className="project-tree-progress" role="status">
                <span aria-hidden className="conversation-spinner h-2.5 w-2.5" />
                {t("Removing…")}
              </span>
            ) : node.primary ? (
              <span
                className="project-tree-primary-badge"
                title={t("Primary workspace (project root)")}
              >
                {t("primary")}
              </span>
            ) : null}
            {hasChildren ? (
              <button
                aria-hidden
                className="project-tree-disclosure"
                onClick={(event) => {
                  event.stopPropagation();
                  onToggleRow(node.key, open);
                }}
                tabIndex={-1}
                type="button"
              >
                <Chevron expanded={open} />
              </button>
            ) : null}
            {node.history.length > 0 && !removing ? (
              // It floats over the row end on hover, so the row layout
              // never moves.
              <button
                aria-hidden
                className="project-tree-history-button"
                data-open={historyOpen ? "" : undefined}
                onClick={(event) => {
                  event.stopPropagation();
                  toggleHistory(node, historyOpen);
                }}
                tabIndex={-1}
                title={historyOpen ? t("Hide history") : t("Show history ({count})", { count: node.history.length })}
                type="button"
              >
                <HistoryIcon />
              </button>
            ) : null}
          </>
        }
        rowTitle={
          unavailable
            ? t("Workspace is no longer available on disk.")
            : node.workspace.rootPath
        }
        treeKey={node.key}
      >
        {failedRemoval ? (
          <TreeItem
            className="project-tree-failed"
            focusKey={focusKey}
            level={depth + 3}
            onActivate={() => undefined}
            row={
              <>
                <span className="project-tree-lane">
                  <AgentStateGlyph glyph="failed" size="md" />
                </span>
                <span className="project-tree-failed-text">
                  {t("Remove failed")}
                  {failedRemoval.error ? (
                    <span className="project-tree-failed-detail">{failedRemoval.error}</span>
                  ) : null}
                </span>
                <button
                  aria-label={t("Dismiss")}
                  className="project-tree-row-button"
                  data-visible=""
                  onClick={(event) => {
                    event.stopPropagation();
                    treeOperationsStore.dismissRemoval(node.workspace.id);
                  }}
                  title={t("Dismiss")}
                  type="button"
                >
                  {"\u00D7"}
                </button>
              </>
            }
            rowTitle={failedRemoval.error ?? undefined}
            treeKey={`remove-failed:${node.workspace.id}`}
          />
        ) : null}
        {node.fold ? (
          <AgentsPill
            rows={rows}
            focusKey={focusKey}
            level={depth + 3}
            open={expandedKeys.has(agentsPillKey(node.workspace.id))}
            onToggle={(open) => onToggleRow(agentsPillKey(node.workspace.id), open)}
            treeKey={agentsPillKey(node.workspace.id)}
          >
            {rows.map((row) => agentRow(row, depth + 4, focusKey))}
          </AgentsPill>
        ) : (
          rows.map((row) => agentRow(row, depth + 3, focusKey))
        )}
        {node.hiddenRows.length > 0 ? (
          <TreeItem
            className="project-tree-more"
            focusKey={focusKey}
            level={depth + 3}
            onActivate={() => toggleShowAll(node.key)}
            row={
              <>
                <span className="project-tree-lane" />
                <span className="truncate text-[12px] text-content-muted">
                  {allShown
                    ? t("Show less")
                    : t("Show {count} more", { count: node.hiddenRows.length })}
                </span>
              </>
            }
            treeKey={`more:${node.key}`}
          />
        ) : null}
        {historyOpen ? (
          // Orca has no history list. Halv keeps the closed conversations
          // of a workspace one click away: the clock button of the row or
          // "Show history" in its menu opens this list.
          <TreeItem
            className="project-tree-history"
            expanded
            focusKey={focusKey}
            level={depth + 3}
            onActivate={() => onToggleRow(history, true)}
            row={
              <>
                <span className="project-tree-lane">
                  <HistoryIcon />
                </span>
                <span className="min-w-0 flex-1 truncate text-[12px] text-content-muted">
                  {t("History")}
                </span>
                <span className="project-tree-age">{node.history.length}</span>
                <Chevron expanded />
              </>
            }
            rowTitle={t("Closed conversations of this workspace")}
            treeKey={history}
          >
            {(historyAll ? node.history : node.history.slice(0, HISTORY_SHOWN)).map((row) =>
              agentRow(row, depth + 4, focusKey),
            )}
            {node.history.length > HISTORY_SHOWN ? (
              <TreeItem
                className="project-tree-more"
                focusKey={focusKey}
                level={depth + 4}
                onActivate={() => toggleShowAll(history)}
                row={
                  <>
                    <span className="project-tree-lane" />
                    <span className="truncate text-[12px] text-content-muted">
                      {historyAll
                        ? t("Show less")
                        : t("Show {count} more", {
                            count: node.history.length - HISTORY_SHOWN,
                          })}
                    </span>
                  </>
                }
                treeKey={`more:${history}`}
              />
            ) : null}
          </TreeItem>
        ) : null}
      </TreeItem>
    );
  };

  const renderProject = (project: TreeProjectNode<T>, depth: number) => {
    const open = !collapsedKeys.has(project.key);
    const menuOpen = menu !== null && menuTriggerRef.current?.dataset.treeKey === project.key;
    return (
      <TreeItem
        className={drag?.projectId === project.project.id ? "project-tree-dragging" : undefined}
        expanded={open}
        focusKey={focusKey}
        key={project.key}
        level={depth + 1}
        onActivate={() => {
          if (suppressClickRef.current) return;
          // One click opens the project (ADR 0020): its primary workspace
          // shows, with the launcher when its strip is empty. The chevron
          // and the arrow keys still collapse and expand.
          const primary = project.workspaces.find((node) => node.primary);
          if (!open) onToggleRow(project.key, false);
          if (primary) onSelectWorkspace(project.project.id, primary.workspace.id);
        }}
        onContextMenu={actions ? (event) => projectMenu(event, project) : undefined}
        onPointerDown={(event) => startProjectDrag(event, project.project.id)}
        row={
          <>
            <button
              aria-hidden
              className="project-tree-disclosure project-tree-project-chevron"
              onClick={(event) => {
                event.stopPropagation();
                if (!suppressClickRef.current) onToggleRow(project.key, open);
              }}
              tabIndex={-1}
              type="button"
            >
              <Chevron expanded={open} />
            </button>
            {project.groupFolder ? (
              // The folder of a folder-scan group (ADR 0019): Orca's folder
              // workspace. Its terminals open in the folder.
              <span aria-hidden className="project-tree-folder-avatar">
                <FolderIcon />
              </span>
            ) : (
              <span
                aria-hidden
                className="project-tree-avatar"
                data-color={project.avatar.color}
              >
                {project.avatar.initial}
              </span>
            )}
            <span className="min-w-0 flex-1 truncate text-[13px] font-semibold text-content-primary">
              {project.project.name}
            </span>
            {project.groupFolder ? (
              <span
                className="project-tree-primary-badge"
                title={t("The folder that holds the repositories of this group")}
              >
                {t("folder")}
              </span>
            ) : null}
            {!open ? (
              <AgentStateGlyph glyph={project.glyph} size="md" summary />
            ) : null}
            {actions && projectIsGitRepo(project) ? (
              // Orca's "+" on a project header. The menu has the same
              // action for keyboard users.
              <button
                aria-hidden
                className="project-tree-row-button"
                onClick={(event) => {
                  event.stopPropagation();
                  openNewWorktree(project);
                }}
                tabIndex={-1}
                title={t("New worktree in {project}", { project: project.project.name })}
                type="button"
              >
                +
              </button>
            ) : null}
            {actions ? (
              // The row keeps its accessible name. Keyboard users open the
              // same menu with Shift+F10 or the context menu key.
              <button
                aria-expanded={menuOpen}
                aria-hidden
                className="project-tree-row-button"
                onClick={(event) =>
                  projectMenu(event, project, event.currentTarget.getBoundingClientRect())
                }
                tabIndex={-1}
                title={t("Project actions")}
                type="button"
              >
                {"\u2026"}
              </button>
            ) : null}
          </>
        }
        rowTitle={project.project.rootPath}
        treeKey={project.key}
      >
        {project.workspaces.map((node) => renderWorkspace(project, node, depth))}
        {operations.creations
          .filter((creation) => creation.projectId === project.project.id)
          .map((creation) => (
            <TreeItem
              className={creation.status === "failed" ? "project-tree-failed" : "project-tree-pending"}
              focusKey={focusKey}
              key={creation.id}
              level={depth + 2}
              onActivate={() => undefined}
              row={
                <>
                  <span className="project-tree-lane">
                    {creation.status === "failed" ? (
                      <AgentStateGlyph glyph="failed" size="md" />
                    ) : (
                      <span aria-hidden className="conversation-spinner h-2.5 w-2.5" />
                    )}
                  </span>
                  <BranchIcon />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-mono text-[12px] text-content-secondary">
                      {creation.branch ?? t("New worktree")}
                    </span>
                    <span
                      className={
                        creation.status === "failed"
                          ? "project-tree-failed-detail"
                          : "block truncate text-[11px] text-content-muted"
                      }
                      role="status"
                    >
                      {creation.status === "failed"
                        ? (creation.error ?? t("Creation failed"))
                        : t("Creating worktree…")}
                    </span>
                  </span>
                  {creation.status === "failed" ? (
                    <button
                      aria-label={t("Dismiss")}
                      className="project-tree-row-button"
                      data-visible=""
                      onClick={(event) => {
                        event.stopPropagation();
                        treeOperationsStore.dismissCreation(creation.id);
                      }}
                      title={t("Dismiss")}
                      type="button"
                    >
                      {"\u00D7"}
                    </button>
                  ) : null}
                </>
              }
              rowTitle={creation.error ?? undefined}
              treeKey={creation.id}
            />
          ))}
      </TreeItem>
    );
  };

  const renderGroup = (node: TreeGroupNode<T>) => {
    const open = !collapsedKeys.has(node.key);
    const menuOpen = menu !== null && menuTriggerRef.current?.dataset.treeKey === node.key;
    const dropInside = drag?.target?.key === node.key;
    return (
      <TreeItem
        className={`project-tree-group${dropInside ? " project-tree-drop-inside" : ""}`}
        expanded={open}
        focusKey={focusKey}
        key={node.key}
        level={1}
        onActivate={() => onToggleRow(node.key, open)}
        onContextMenu={onUpdateState ? (event) => groupMenu(event, node) : undefined}
        row={
          <>
            <Chevron expanded={open} />
            <FolderTreeIcon />
            <span className="min-w-0 flex-1 truncate text-[12px] font-semibold uppercase tracking-[0.08em] text-content-secondary">
              {node.group.name}
            </span>
            {!open ? <AgentStateGlyph glyph={node.glyph} size="md" summary /> : null}
            {onUpdateState ? (
              <button
                aria-expanded={menuOpen}
                aria-hidden
                className="project-tree-row-button"
                onClick={(event) =>
                  groupMenu(event, node, event.currentTarget.getBoundingClientRect())
                }
                tabIndex={-1}
                title={t("Group actions")}
                type="button"
              >
                {"\u2026"}
              </button>
            ) : null}
          </>
        }
        rowTitle={node.group.name}
        treeKey={node.key}
      >
        {node.projects.map((project) => renderProject(project, 1))}
      </TreeItem>
    );
  };

  const dialogView = (() => {
    if (!dialog) return null;
    const close = () => setDialog(null);
    if (dialog.kind === "new-group" || dialog.kind === "rename-group") {
      return (
        <ProjectGroupNameDialog
          initialName={dialog.name}
          mode={dialog.kind === "new-group" ? "create" : "rename"}
          onCancel={close}
          onSubmit={(name) => {
            close();
            if (dialog.kind === "new-group") {
              const id = window.crypto.randomUUID();
              onUpdateState?.((state) => withNewGroup(state, { id, name }, dialog.projectId));
            } else {
              onUpdateState?.((state) => withGroupRenamed(state, dialog.groupId, name));
            }
          }}
        />
      );
    }
    if (dialog.kind === "delete-group") {
      return (
        <DeleteProjectGroupDialog
          name={dialog.name}
          onCancel={close}
          onConfirm={() => {
            close();
            onUpdateState?.((state) => withGroupDeleted(state, dialog.groupId));
          }}
          projectNames={dialog.projectNames}
        />
      );
    }
    if (!actions) return null;
    if (dialog.kind === "new-worktree") {
      return (
        <NewWorktreeDialog
          loadBaseRefs={() => actions.listBaseRefs(dialog.projectId)}
          onCancel={close}
          onCreate={(request) => {
            close();
            // A collapsed project opens, so its pending row shows.
            const key = `project:${dialog.projectId}`;
            if (collapsedKeys.has(key)) onToggleRow(key, false);
            run(() => actions.createWorktree(dialog.projectId, request));
          }}
          projectName={dialog.name}
        />
      );
    }
    if (dialog.kind === "remove-worktree") {
      return (
        <RemoveWorktreeDialog
          inspect={() => actions.inspectWorkspace(dialog.projectId, dialog.workspaceId)}
          name={dialog.name}
          onCancel={close}
          onConfirm={() => {
            close();
            run(() => actions.removeWorktree(dialog.workspaceId));
          }}
          openTerminalCount={actions.openTerminalCount({ workspaceId: dialog.workspaceId })}
          rootPath={dialog.rootPath}
        />
      );
    }
    return (
      <RemoveProjectDialog
        name={dialog.name}
        onCancel={close}
        onConfirm={() => {
          close();
          run(() => actions.removeProject(dialog.projectId));
        }}
        openTerminalCount={actions.openTerminalCount({ projectId: dialog.projectId })}
        worktreeCount={dialog.worktreeCount}
      />
    );
  })();

  return (
    <div className="project-tree">
      {notice ? (
        <div className="project-tree-notice" role="alert">
          <span className="min-w-0 flex-1">{notice}</span>
          <button
            aria-label={t("Dismiss")}
            onClick={() => setNotice(null)}
            title={t("Dismiss")}
            type="button"
          >
            {"\u00D7"}
          </button>
        </div>
      ) : null}
      <div
        aria-label={t("Projects")}
        className="project-tree-root"
        onFocus={(event) => {
          const key = (event.target as HTMLElement).closest<HTMLElement>(
            '[role="treeitem"]',
          )?.dataset.treeKey;
          if (key) setFocusedKey(key);
        }}
        onKeyDown={(event) => handleTreeKeyDown(event, onToggleRow)}
        role="tree"
      >
        {tree.groups.map(renderGroup)}
        {tree.ungrouped.map((project) => renderProject(project, 0))}
      </div>
      {drag?.target && drag.target.edge !== "inside" ? (
        <div
          aria-hidden
          className="project-tree-drop-line"
          style={{ top: drag.target.top - 1, left: drag.target.left, width: drag.target.width }}
        />
      ) : null}
      {projectsLoaded && tree.projects.length === 0 ? (
        <ProjectTreeEmpty onAddFolder={onAddFolder} onCloneGithub={onCloneGithub} />
      ) : null}
      {tree.looseChats.length > 0 || tree.chatTerminals.length > 0 ? (
        // Chats without a project, then the terminals without a tab that
        // adoption left here, for example a shell in the home folder
        // (ADR 0020).
        <section className="pt-4">
          <h3 className="project-tree-section-title">{t("Chats")}</h3>
          <div
            aria-label={t("Chats")}
            onKeyDown={(event) => handleTreeKeyDown(event, onToggleRow)}
            role="tree"
          >
            {tree.looseChats.map((row) => agentRow(row, 1, chatsFocusKey))}
            {tree.chatTerminals.map((row) => agentRow(row, 1, chatsFocusKey))}
          </div>
        </section>
      ) : null}
      {menu ? <TreeContextMenu onClose={closeMenu} request={menu} /> : null}
      {dialogView}
    </div>
  );
}

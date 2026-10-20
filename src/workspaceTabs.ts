// The pure model of workspace tabs (ADR 0010).
//
// Terms:
// - workspace: the primary checkout of a project or a Halv worktree.
// - owner: the record that owns terminal sessions: a conversation, or a
//   shell tab row.
// - tab key: `conversation:<conversationId>` or `shell:<uuid>`. The tab key
//   is also the owner key and the TerminalPanel scope.
// - home: the workspace whose strip shows a tab. A conversation tab never
//   stores its home: the home is the workspace of its conversation.
// - strip: the ordered tabs of one workspace. A strip is a view. It is never
//   stored.
//
// Safety rules:
// - The rows are a record keyed by tab key. At rest, a primary key holds
//   them. A key cannot repeat.
// - Only the intent functions return insert operations. Views, refreshes and
//   session events never write.
// - Nothing deletes a row because panes or sessions are absent.
// - This module never imports the panel layout modules. Panes are never tabs.

import type { AgentPaneGlyph, AgentPaneView } from "./agentPaneStatus.ts";
import type { ConversationTerminalStatus } from "./conversationMode.ts";
import { effectiveWorkspaceId } from "./conversationWorkspace.ts";
import { strongestGlyph, treeRowState } from "./projectTree.ts";
import type {
  ConversationSummary,
  WorkspaceTabKey,
  WorkspaceTabRow,
  WorkspaceTabsOp,
  WorkspaceTabsSnapshot,
} from "./types";

// ---------------------------------------------------------------- keys

export type TabKey = WorkspaceTabKey;
export type ConversationTabKey = `conversation:${string}`;
export type ShellTabKey = `shell:${string}`;
export type TabRow = WorkspaceTabRow;
export type TabsOp = WorkspaceTabsOp;

const SHELL_TAB_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function conversationTabKey(conversationId: string): ConversationTabKey {
  if (!conversationId.trim()) throw new Error("A conversation id cannot be empty.");
  return `conversation:${conversationId}`;
}

export function shellTabKey(shellTabId: string): ShellTabKey {
  if (!shellTabId.trim()) throw new Error("A shell tab id cannot be empty.");
  return `shell:${shellTabId}`;
}

export type ParsedTabKey =
  | { kind: "conversation"; id: string; key: ConversationTabKey }
  | { kind: "shell"; id: string; key: ShellTabKey };

export function parseTabKey(value: unknown): ParsedTabKey | null {
  if (typeof value !== "string") return null;
  if (value.startsWith("conversation:")) {
    const id = value.slice("conversation:".length);
    return id.trim() ? { kind: "conversation", id, key: value as ConversationTabKey } : null;
  }
  if (value.startsWith("shell:")) {
    const id = value.slice("shell:".length);
    return id.trim() ? { kind: "shell", id, key: value as ShellTabKey } : null;
  }
  return null;
}

/** The key rule of `workspace_tabs_apply`: a conversation key, or a shell key
 *  with a UUID. Rust rejects every other key before the insert. */
export function isStorableTabKey(key: string) {
  if (key.startsWith("conversation:")) return true;
  return key.startsWith("shell:") && SHELL_TAB_ID.test(key.slice("shell:".length));
}

/** The tab key is the TerminalPanel scope. Every conversation panel already
 *  uses `conversation:<id>`, so a saved layout becomes the layout of its tab. */
export function panelScopeForTab(key: TabKey): string {
  return key;
}

/** `terminal_restore_shell` gets the scope. A shell tab scope names its owner. */
export function shellTabIdFromScope(scope: string): string | null {
  const parsed = parseTabKey(scope);
  return parsed?.kind === "shell" ? parsed.id : null;
}

// ---------------------------------------------------------------- state and facts

export type TabsState = {
  rows: Readonly<Record<string, TabRow>>;
  activeByWorkspace: Readonly<Record<string, TabKey | null>>;
};

export const EMPTY_TABS_STATE: TabsState = { rows: {}, activeByWorkspace: {} };

export function tabsStateFromSnapshot(snapshot: WorkspaceTabsSnapshot): TabsState {
  return {
    rows: Object.fromEntries(snapshot.rows.map((row) => [row.key, row])),
    activeByWorkspace: { ...snapshot.activeByWorkspace },
  };
}

export type ConversationFact = {
  id: string;
  projectId: number | null;
  /** The stored binding. Null means the primary workspace of the project. */
  workspaceId: string | null;
  /** The body renderer only. It never decides membership. */
  mode: "chat" | "agent";
  archived: boolean;
  createdAt: number;
  updatedAt: number;
};

export type WorkspaceFact = {
  id: string;
  projectId: number;
  kind: "primary" | "worktree";
};

export type TabFacts = {
  /** False until the conversation and workspace lists loaded. Views are
   *  empty then, and no intent writes. */
  complete: boolean;
  conversations: ReadonlyMap<string, ConversationFact>;
  workspaces: ReadonlyMap<string, WorkspaceFact>;
};

export const INCOMPLETE_TAB_FACTS: TabFacts = {
  complete: false,
  conversations: new Map(),
  workspaces: new Map(),
};

function timeOf(value: string) {
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : 0;
}

/** Facts from the App lists. Null lists mean "not loaded yet". */
export function tabFacts(input: {
  conversations: readonly ConversationSummary[] | null;
  workspaces: readonly WorkspaceFact[] | null;
}): TabFacts {
  if (!input.conversations || !input.workspaces) return INCOMPLETE_TAB_FACTS;
  return {
    complete: true,
    conversations: new Map(
      input.conversations.map((conversation) => [
        conversation.id,
        {
          id: conversation.id,
          projectId: conversation.projectId ?? null,
          workspaceId: conversation.workspaceId ?? null,
          mode: conversation.mode,
          archived: conversation.archivedAt != null,
          createdAt: timeOf(conversation.createdAt),
          updatedAt: timeOf(conversation.updatedAt),
        },
      ]),
    ),
    workspaces: new Map(
      input.workspaces.map((workspace) => [
        workspace.id,
        { id: workspace.id, projectId: workspace.projectId, kind: workspace.kind },
      ]),
    ),
  };
}

// ---------------------------------------------------------------- home

/** The strip of a conversation, or null when it has no strip now (archived,
 *  no project, or the workspace is not known). The project tree places a
 *  conversation with the same `effectiveWorkspaceId`, so the two agree. */
export function conversationHome(
  conversation: ConversationFact,
  workspaces: ReadonlyMap<string, WorkspaceFact>,
): string | null {
  if (conversation.archived) return null;
  const home = effectiveWorkspaceId(conversation, workspaces);
  return home && workspaces.has(home) ? home : null;
}

/** The strip that shows a row. Null is a view decision: it never deletes the
 *  row. The conversation mode is not an input, so a Pro toggle never moves a
 *  tab. */
export function homeWorkspaceId(row: TabRow, facts: TabFacts): string | null {
  if (!facts.complete) return null;
  if (row.conversationId !== null) {
    const conversation = facts.conversations.get(row.conversationId);
    return conversation ? conversationHome(conversation, facts.workspaces) : null;
  }
  return row.shellWorkspaceId && facts.workspaces.has(row.shellWorkspaceId)
    ? row.shellWorkspaceId
    : null;
}

// ---------------------------------------------------------------- strip view

export function compareRows(left: TabRow, right: TabRow) {
  return (
    left.position - right.position ||
    left.createdAt - right.createdAt ||
    (left.key < right.key ? -1 : left.key > right.key ? 1 : 0)
  );
}

export type StripView = {
  workspaceId: string;
  keys: TabKey[];
  rows: TabRow[];
  activeKey: TabKey | null;
};

function mostRecent(rows: readonly TabRow[]): TabKey | null {
  let best: TabRow | null = null;
  for (const row of rows) {
    if (row.activatedAt === null) continue;
    if (
      !best ||
      best.activatedAt === null ||
      row.activatedAt > best.activatedAt ||
      (row.activatedAt === best.activatedAt && compareRows(row, best) > 0)
    ) {
      best = row;
    }
  }
  return best?.key ?? null;
}

/** A pure view: same input, same output. The stored active pointer wins when
 *  it is in the strip, else the most recent tab, else the first tab. */
export function stripView(state: TabsState, facts: TabFacts, workspaceId: string): StripView {
  const rows = Object.values(state.rows)
    .filter((row) => homeWorkspaceId(row, facts) === workspaceId)
    .sort(compareRows);
  const keys = rows.map((row) => row.key);
  const stored = state.activeByWorkspace[workspaceId] ?? null;
  const activeKey =
    stored && keys.includes(stored) ? stored : mostRecent(rows) ?? keys[0] ?? null;
  return { workspaceId, keys, rows, activeKey };
}

/** The next active tab when `closingKey` closes: the most recently activated
 *  survivor, else the right neighbor, else the left neighbor (Orca). */
export function closeSuccessor(view: StripView, closingKey: TabKey): TabKey | null {
  const index = view.keys.indexOf(closingKey);
  const survivors = view.rows.filter((row) => row.key !== closingKey);
  return (
    mostRecent(survivors) ??
    (index >= 0 ? view.keys[index + 1] ?? view.keys[index - 1] : survivors[0]?.key) ??
    null
  );
}

/** Cmd+1 to Cmd+8 select tab N. Cmd+9 selects the last tab. */
export function tabKeyForDigit(keys: readonly TabKey[], digit: number): TabKey | null {
  if (!Number.isInteger(digit) || digit < 1 || digit > 9 || keys.length === 0) return null;
  if (digit === 9) return keys[keys.length - 1];
  return keys[digit - 1] ?? null;
}

/** The title of a shell tab: its custom title, else the live title of its
 *  active pane (OSC), else its default title "Terminal N". The default
 *  title is stored in English, so the caller translates it by its number. */
export function shellTabTitle(
  row: Pick<TabRow, "customTitle" | "defaultTitle">,
  liveTitle: string | null | undefined,
): { title: string } | { terminalNumber: number } {
  const custom = row.customTitle?.trim();
  if (custom) return { title: custom };
  const live = liveTitle?.trim();
  if (live) return { title: live };
  const match = /^Terminal (\d+)$/.exec(row.defaultTitle ?? "");
  if (match) return { terminalNumber: Number(match[1]) };
  return row.defaultTitle?.trim() ? { title: row.defaultTitle.trim() } : { terminalNumber: 1 };
}

/** "Terminal N": one more than the highest default number of the strip. */
export function defaultShellTitle(state: TabsState, workspaceId: string) {
  let highest = 0;
  for (const row of Object.values(state.rows)) {
    if (row.shellWorkspaceId !== workspaceId) continue;
    const match = /^Terminal (\d+)$/.exec(row.defaultTitle ?? "");
    if (match) highest = Math.max(highest, Number(match[1]));
  }
  return `Terminal ${highest + 1}`;
}

// ---------------------------------------------------------------- intents to op batches

/** One intent gives one batch. Rust applies it in one transaction. */
export type IntentResult = { state: TabsState; ops: TabsOp[]; rejected?: string };

function withRow(state: TabsState, row: TabRow): TabsState {
  return { ...state, rows: { ...state.rows, [row.key]: row } };
}

function withoutRow(state: TabsState, key: TabKey): TabsState {
  const rows = { ...state.rows };
  delete rows[key];
  const activeByWorkspace = Object.fromEntries(
    Object.entries(state.activeByWorkspace).map(([workspaceId, active]) => [
      workspaceId,
      active === key ? null : active,
    ]),
  );
  return { rows, activeByWorkspace };
}

function renumber(
  state: TabsState,
  orderedKeys: readonly TabKey[],
): { state: TabsState; op: TabsOp | null } {
  const positions: Array<[TabKey, number]> = [];
  let rows = state.rows;
  orderedKeys.forEach((key, index) => {
    const row = rows[key];
    if (row && row.position !== index) {
      rows = { ...rows, [key]: { ...row, position: index } };
      positions.push([key, index]);
    }
  });
  return {
    state: { ...state, rows },
    op: positions.length > 0 ? { type: "setPositions", positions } : null,
  };
}

export type OpenTabIntent = {
  key: TabKey;
  now: number;
  /** The new tab goes after this tab. Default: the end of the strip. */
  afterKey?: TabKey | null;
  /** Shell tabs only. */
  shellWorkspaceId?: string | null;
  /** Shell tabs only: a custom title. */
  title?: string | null;
  activate?: boolean;
};

/** Shows an owner as a tab and activates it. It is idempotent: an existing
 *  key only activates. With the one-time seed, this is the only producer of
 *  insert operations. */
export function openTab(state: TabsState, facts: TabFacts, intent: OpenTabIntent): IntentResult {
  const parsed = parseTabKey(intent.key);
  if (!parsed || !isStorableTabKey(parsed.key)) {
    return { state, ops: [], rejected: "invalid tab key" };
  }
  if (state.rows[parsed.key]) {
    if (intent.activate === false) return { state, ops: [] };
    return activateTab(state, facts, parsed.key, intent.now);
  }
  const shellWorkspaceId =
    parsed.kind === "shell" ? (intent.shellWorkspaceId ?? null) : null;
  const row: TabRow = {
    key: parsed.key,
    conversationId: parsed.kind === "conversation" ? parsed.id : null,
    shellWorkspaceId,
    customTitle: parsed.kind === "shell" ? intent.title?.trim() || null : null,
    defaultTitle:
      parsed.kind === "shell" && shellWorkspaceId
        ? defaultShellTitle(state, shellWorkspaceId)
        : null,
    position: 0,
    createdAt: intent.now,
    activatedAt: null,
  };
  const home = homeWorkspaceId(row, facts);
  if (!home) {
    return {
      state,
      ops: [],
      rejected:
        parsed.kind === "conversation"
          ? "the conversation is archived, unknown, or has no project"
          : "the shell tab needs an existing workspace",
    };
  }
  const before = stripView(state, facts, home);
  const anchor = intent.afterKey ? before.keys.indexOf(intent.afterKey) : -1;
  const at = anchor >= 0 ? anchor + 1 : before.keys.length;
  const ordered = [...before.keys.slice(0, at), row.key, ...before.keys.slice(at)];
  let next = withRow(state, { ...row, position: at });
  const ops: TabsOp[] = [{ type: "insertTab", row: next.rows[row.key] }];
  const renumbered = renumber(next, ordered);
  next = renumbered.state;
  if (renumbered.op) ops.push(renumbered.op);
  if (intent.activate === false) return { state: next, ops };
  const activated = activateTab(next, facts, row.key, intent.now);
  return { state: activated.state, ops: [...ops, ...activated.ops] };
}

export function activateTab(
  state: TabsState,
  facts: TabFacts,
  key: TabKey,
  now: number,
): IntentResult {
  const row = state.rows[key];
  if (!row) return { state, ops: [], rejected: "unknown tab" };
  const home = homeWorkspaceId(row, facts);
  if (!home) return { state, ops: [], rejected: "the tab has no workspace now" };
  return {
    state: {
      rows: { ...state.rows, [key]: { ...row, activatedAt: now } },
      activeByWorkspace: { ...state.activeByWorkspace, [home]: key },
    },
    ops: [
      { type: "touchTab", key, activatedAt: now },
      { type: "setActive", workspaceId: home, key },
    ],
  };
}

/** Removes the tab. The caller stops the processes of the tab and parks its
 *  agents first (ADR 0010). The conversation itself stays. */
export function closeTab(state: TabsState, facts: TabFacts, key: TabKey): IntentResult {
  const row = state.rows[key];
  if (!row) return { state, ops: [] };
  const home = homeWorkspaceId(row, facts);
  const before = home ? stripView(state, facts, home) : null;
  let next = withoutRow(state, key);
  const ops: TabsOp[] = [{ type: "deleteTab", key }];
  if (home && before && before.activeKey === key) {
    const successor = closeSuccessor(before, key);
    next = { ...next, activeByWorkspace: { ...next.activeByWorkspace, [home]: successor } };
    ops.push({ type: "setActive", workspaceId: home, key: successor });
  }
  return { state: next, ops };
}

export function moveTab(
  state: TabsState,
  facts: TabFacts,
  key: TabKey,
  toIndex: number,
): IntentResult {
  const row = state.rows[key];
  const home = row ? homeWorkspaceId(row, facts) : null;
  if (!row || !home) return { state, ops: [] };
  const keys = stripView(state, facts, home).keys.filter((candidate) => candidate !== key);
  const at = Math.min(Math.max(0, Math.trunc(toIndex)), keys.length);
  keys.splice(at, 0, key);
  const renumbered = renumber(state, keys);
  return { state: renumbered.state, ops: renumbered.op ? [renumbered.op] : [] };
}

/** Cmd+Shift+[ and Cmd+Shift+]. The cycle wraps. */
export function cycleTab(
  state: TabsState,
  facts: TabFacts,
  workspaceId: string,
  offset: -1 | 1,
  now: number,
): IntentResult {
  const view = stripView(state, facts, workspaceId);
  if (view.keys.length < 2 || !view.activeKey) return { state, ops: [] };
  const index = view.keys.indexOf(view.activeKey);
  const target = view.keys[(index + offset + view.keys.length) % view.keys.length];
  return activateTab(state, facts, target, now);
}

/** Cmd+1 to Cmd+9. A missing number is a no-op. */
export function selectTabByNumber(
  state: TabsState,
  facts: TabFacts,
  workspaceId: string,
  digit: number,
  now: number,
): IntentResult {
  const target = tabKeyForDigit(stripView(state, facts, workspaceId).keys, digit);
  return target ? activateTab(state, facts, target, now) : { state, ops: [] };
}

/** Only a shell tab keeps its own title. A conversation tab renames the
 *  conversation instead. An empty title clears the custom title. */
export function renameShellTab(state: TabsState, key: TabKey, title: string | null): IntentResult {
  const row = state.rows[key];
  if (!row || row.conversationId !== null) {
    return { state, ops: [], rejected: "only shell tabs keep their own title" };
  }
  const customTitle = title?.trim() || null;
  return {
    state: withRow(state, { ...row, customTitle }),
    ops: [{ type: "setTitle", key, title: customTitle }],
  };
}

/** True for a batch that only activates. The store may merge such batches. */
export function isActivationBatch(ops: readonly TabsOp[]) {
  return ops.length > 0 && ops.every((op) => op.type === "touchTab" || op.type === "setActive");
}

/** Merges activation batches into one batch with the same result: the last
 *  touch of each tab and the last pointer of each workspace. */
export function mergeActivationBatches(first: readonly TabsOp[], second: readonly TabsOp[]) {
  const touches = new Map<TabKey, number>();
  const pointers = new Map<string, TabKey | null>();
  for (const op of [...first, ...second]) {
    if (op.type === "touchTab") touches.set(op.key, op.activatedAt);
    else if (op.type === "setActive") pointers.set(op.workspaceId, op.key);
  }
  return [
    ...[...touches].map(([key, activatedAt]): TabsOp => ({ type: "touchTab", key, activatedAt })),
    ...[...pointers].map(([workspaceId, key]): TabsOp => ({ type: "setActive", workspaceId, key })),
  ];
}

// ---------------------------------------------------------------- disk model (SQL semantics)

/** What SQLite holds. The same shape as the visible state. */
export type DiskState = TabsState;

function rowShapeIsValid(row: TabRow) {
  const conversationShape =
    row.conversationId !== null &&
    row.shellWorkspaceId === null &&
    row.customTitle === null &&
    row.defaultTitle === null &&
    row.key === `conversation:${row.conversationId}`;
  const shellShape =
    row.conversationId === null &&
    row.shellWorkspaceId !== null &&
    row.defaultTitle !== null &&
    // SQLite LIKE ignores ASCII case.
    row.key.toLowerCase().startsWith("shell:") &&
    row.key.length > "shell:".length;
  return conversationShape || shellShape;
}

function applyOp(disk: DiskState, op: TabsOp, facts: TabFacts): { disk: DiskState; error?: string } {
  switch (op.type) {
    case "insertTab": {
      // Order of `workspace_tabs_apply`: the Rust key check, then the CHECK
      // constraint, then the conflict (a no-op), then the foreign keys.
      if (!isStorableTabKey(op.row.key)) return { disk, error: "not a tab key" };
      if (!rowShapeIsValid(op.row)) return { disk, error: "CHECK constraint failed" };
      if (disk.rows[op.row.key]) return { disk };
      if (op.row.conversationId !== null && !facts.conversations.has(op.row.conversationId)) {
        return { disk, error: "FOREIGN KEY constraint failed" };
      }
      if (op.row.shellWorkspaceId !== null && !facts.workspaces.has(op.row.shellWorkspaceId)) {
        return { disk, error: "FOREIGN KEY constraint failed" };
      }
      return { disk: withRow(disk, op.row) };
    }
    case "deleteTab":
      return { disk: disk.rows[op.key] ? withoutRow(disk, op.key) : disk };
    case "touchTab": {
      const row = disk.rows[op.key];
      return { disk: row ? withRow(disk, { ...row, activatedAt: op.activatedAt }) : disk };
    }
    case "setPositions": {
      let next = disk;
      for (const [key, position] of op.positions) {
        const row = next.rows[key];
        if (row) next = withRow(next, { ...row, position });
      }
      return { disk: next };
    }
    case "setTitle": {
      const row = disk.rows[op.key];
      return {
        disk:
          row && row.conversationId === null ? withRow(disk, { ...row, customTitle: op.title }) : disk,
      };
    }
    case "setActive": {
      // A guarded no-op: a pointer to a gone workspace or tab never fails.
      if (!facts.workspaces.has(op.workspaceId)) return { disk };
      if (op.key !== null && !disk.rows[op.key]) return { disk };
      return {
        disk: { ...disk, activeByWorkspace: { ...disk.activeByWorkspace, [op.workspaceId]: op.key } },
      };
    }
  }
}

/** `workspace_tabs_apply`: all operations or none. */
export function applyBatchToDisk(
  disk: DiskState,
  ops: readonly TabsOp[],
  facts: TabFacts,
): { disk: DiskState; error?: string } {
  let next = disk;
  for (const op of ops) {
    const outcome = applyOp(next, op, facts);
    if (outcome.error) return { disk, error: outcome.error };
    next = outcome.disk;
  }
  return { disk: next };
}

/** The visible state: the snapshot plus every queued batch in order. A
 *  queued batch that would fail now is left out. Rust rejects it too. */
export function rebase(
  snapshot: DiskState,
  queuedBatches: ReadonlyArray<readonly TabsOp[]>,
  facts: TabFacts,
): TabsState {
  let view = snapshot;
  for (const ops of queuedBatches) {
    const outcome = applyBatchToDisk(view, ops, facts);
    if (!outcome.error) view = outcome.disk;
  }
  return view;
}

/** The foreign key cascade of a conversation delete (any build). */
export function cascadeConversationDelete(state: TabsState, conversationId: string): TabsState {
  const key = conversationTabKey(conversationId);
  return state.rows[key] ? withoutRow(state, key) : state;
}

/** The foreign key cascades of a workspace delete (any build). */
export function cascadeWorkspaceDelete(state: TabsState, workspaceId: string): TabsState {
  let next = state;
  for (const row of Object.values(state.rows)) {
    if (row.shellWorkspaceId === workspaceId) next = withoutRow(next, row.key);
  }
  const activeByWorkspace = { ...next.activeByWorkspace };
  delete activeByWorkspace[workspaceId];
  return { ...next, activeByWorkspace };
}

/** The startup GC predicate of Rust: rows of archived and project-less
 *  conversations. It never looks at workspaces. */
export function garbageTabKeys(state: TabsState, facts: TabFacts): TabKey[] {
  if (!facts.complete) return [];
  return Object.values(state.rows)
    .filter((row) => {
      if (row.conversationId === null) return false;
      const conversation = facts.conversations.get(row.conversationId);
      return conversation !== undefined && (conversation.archived || conversation.projectId === null);
    })
    .map((row) => row.key);
}

// ---------------------------------------------------------------- session ownership

export type SessionOwnerFact = {
  id: string;
  conversationId?: string | null;
  /** `TerminalSession.shell_tab_id` (stage S5a). */
  shellTabId?: string | null;
  agentKind?: string | null;
};

/** Every terminal session has at most one owner. A conversation wins when
 *  corrupt data sets both fields. */
export function ownerKeyOfSession(
  session: Pick<SessionOwnerFact, "conversationId" | "shellTabId">,
): TabKey | null {
  if (session.conversationId) return conversationTabKey(session.conversationId);
  if (session.shellTabId) return shellTabKey(session.shellTabId);
  return null;
}

export type PanelScope =
  | { kind: "tab"; key: TabKey }
  | { kind: "loose-conversation"; conversationId: string }
  | { kind: "unowned"; scope: string };

/** The session filter of a panel. A tab panel shows exactly the sessions of
 *  its owner. A panel without an owner shows only sessions without an owner:
 *  plain shells, and agents of its saved layout. */
export function panelAcceptsSession(
  scope: PanelScope,
  session: SessionOwnerFact,
  inStoredLayout: (sessionId: string) => boolean = () => false,
): boolean {
  const owner = ownerKeyOfSession(session);
  switch (scope.kind) {
    case "tab":
      return owner === scope.key;
    case "loose-conversation":
      return owner === conversationTabKey(scope.conversationId);
    case "unowned":
      return owner === null && (!session.agentKind || inStoredLayout(session.id));
  }
}

// ---------------------------------------------------------------- navigation helpers

/** The tab kinds of App navigation (stage S4a). The `legacy` kind lives in
 *  `appNavigation.ts`. */
export type TabNavigation =
  | {
      kind: "workspace";
      workspaceId: string;
      composing: boolean;
      /** While composing: the conversation row that the first typed
       *  character creates. It becomes a tab when its row exists. */
      draftConversationId?: string | null;
    }
  | { kind: "loose"; conversationId: string | null };

/** Null while the user composes a new task, and in the loose view. */
export function visibleTabKey(nav: TabNavigation, state: TabsState, facts: TabFacts): TabKey | null {
  if (nav.kind !== "workspace" || nav.composing) return null;
  return stripView(state, facts, nav.workspaceId).activeKey;
}

/** The current conversation. It is derived, so it cannot drift. */
export function currentConversationOf(
  nav: TabNavigation,
  state: TabsState,
  facts: TabFacts,
): string | null {
  if (nav.kind === "loose") return nav.conversationId;
  if (nav.composing) return nav.draftConversationId ?? null;
  const parsed = parseTabKey(visibleTabKey(nav, state, facts));
  return parsed?.kind === "conversation" ? parsed.id : null;
}

/** "Show this conversation": a sidebar row, a search hit, a notification, or
 *  the first send of a new task. A conversation without a strip opens in the
 *  loose view and gets no tab. */
export function openConversationIntent(
  nav: TabNavigation,
  state: TabsState,
  facts: TabFacts,
  conversationId: string,
  now: number,
): { nav: TabNavigation; result: IntentResult } {
  const conversation = facts.conversations.get(conversationId);
  if (!conversation) {
    return { nav, result: { state, ops: [], rejected: "unknown conversation" } };
  }
  const home = conversationHome(conversation, facts.workspaces);
  if (!home) return { nav: { kind: "loose", conversationId }, result: { state, ops: [] } };
  const result = openTab(state, facts, { key: conversationTabKey(conversationId), now });
  if (result.rejected) return { nav, result };
  return { nav: { kind: "workspace", workspaceId: home, composing: false }, result };
}

/** A workspace row click: switch the strip. It writes nothing. */
export function selectWorkspaceNavigation(workspaceId: string): TabNavigation {
  return { kind: "workspace", workspaceId, composing: false };
}

/** A new task before its first send. No row exists yet. */
export function startComposingNavigation(workspaceId: string): TabNavigation {
  return { kind: "workspace", workspaceId, composing: true };
}

// ---------------------------------------------------------------- one-time seed

export const SEED_CAP_PER_WORKSPACE = 8;
export const SEED_SAVED_WITHIN_MS = 7 * 24 * 60 * 60 * 1000;

export type SeedInput = {
  facts: TabFacts;
  /** `conversation:<id>` scopes with saved panes. */
  savedConversationScopes: ReadonlyArray<{ conversationId: string; updatedAt: number }>;
  /** Conversations with a running non-GUI agent or a running owned terminal. */
  liveConversationIds: ReadonlySet<string>;
  lastActiveConversationId: string | null;
  now: number;
  capPerWorkspace?: number;
  savedWithinMs?: number;
};

/** The first strips, from the panels that Pro mode keeps today. Live and
 *  last active conversations always stay. Saved agent panels fill up to the
 *  cap, newest first. A strip is in conversation creation order. */
export function seedTabs(input: SeedInput): {
  rows: TabRow[];
  activeByWorkspace: Record<string, TabKey | null>;
} {
  const { facts } = input;
  const cap = input.capPerWorkspace ?? SEED_CAP_PER_WORKSPACE;
  const within = input.savedWithinMs ?? SEED_SAVED_WITHIN_MS;
  const savedAt = new Map(
    input.savedConversationScopes.map((scope) => [scope.conversationId, scope.updatedAt]),
  );
  const recentSaved = [...savedAt]
    .filter(
      ([id, at]) => input.now - at <= within && facts.conversations.get(id)?.mode === "agent",
    )
    .map(([id]) => id);
  const candidates = new Set<string>([
    ...input.liveConversationIds,
    ...recentSaved,
    ...(input.lastActiveConversationId ? [input.lastActiveConversationId] : []),
  ]);
  const byWorkspace = new Map<string, ConversationFact[]>();
  for (const id of candidates) {
    const conversation = facts.conversations.get(id);
    if (!conversation || conversation.projectId === null) continue;
    const workspaceId = conversationHome(conversation, facts.workspaces);
    if (!workspaceId) continue;
    byWorkspace.set(workspaceId, [...(byWorkspace.get(workspaceId) ?? []), conversation]);
  }
  const byId = (left: ConversationFact, right: ConversationFact) =>
    left.id < right.id ? -1 : left.id > right.id ? 1 : 0;
  const rows: TabRow[] = [];
  const activeByWorkspace: Record<string, TabKey | null> = {};
  const workspaceIds = [...byWorkspace.keys()].sort();
  for (const workspaceId of workspaceIds) {
    const conversations = byWorkspace.get(workspaceId) ?? [];
    const kept = conversations.filter(
      (c) => input.liveConversationIds.has(c.id) || c.id === input.lastActiveConversationId,
    );
    const rest = conversations
      .filter((c) => !kept.includes(c))
      .sort(
        (a, b) =>
          (savedAt.get(b.id) ?? b.updatedAt) - (savedAt.get(a.id) ?? a.updatedAt) || byId(a, b),
      );
    const chosen = [...kept, ...rest.slice(0, Math.max(0, cap - kept.length))].sort(
      (a, b) => a.createdAt - b.createdAt || byId(a, b),
    );
    chosen.forEach((conversation, position) => {
      rows.push({
        key: conversationTabKey(conversation.id),
        conversationId: conversation.id,
        shellWorkspaceId: null,
        customTitle: null,
        defaultTitle: null,
        position,
        createdAt: input.now,
        activatedAt: conversation.id === input.lastActiveConversationId ? input.now : null,
      });
    });
    const last = chosen.find((c) => c.id === input.lastActiveConversationId);
    const newest = [...chosen].sort((a, b) => b.updatedAt - a.updatedAt || byId(a, b))[0];
    const active = last ?? newest;
    activeByWorkspace[workspaceId] = active ? conversationTabKey(active.id) : null;
  }
  return { rows, activeByWorkspace };
}

// ---------------------------------------------------------------- close prompt

export type ClosePaneFact = {
  kind: "agent" | "shell";
  running: boolean;
  /** Agent panes: the agent-status state. Null: no status yet. */
  status: AgentPaneView["status"] | null;
  /** Shell panes: the foreground job probe (S5a). Null: unknown. */
  shellBusy: boolean | null;
};

function agentBlocksClose(pane: ClosePaneFact) {
  return (
    pane.kind === "agent" &&
    pane.running &&
    (pane.status === null ||
      pane.status === "working" ||
      pane.status === "needsInput" ||
      pane.status === "needsApproval")
  );
}

function shellBlocksClose(pane: ClosePaneFact) {
  // `terminal_foreground_busy` gives `shellBusy`. A shell that the probe
  // did not read (null, unknown) counts as busy.
  return pane.kind === "shell" && pane.running && pane.shellBusy !== false;
}

/** The reasons of the one close dialog of a tab. An idle agent adds no
 *  reason, because the close parks it. */
export function tabCloseReasons(panes: readonly ClosePaneFact[]) {
  return {
    busyAgents: panes.filter(agentBlocksClose).length,
    busyShells: panes.filter(shellBlocksClose).length,
  };
}

export function tabCloseNeedsConfirm(panes: readonly ClosePaneFact[]) {
  const reasons = tabCloseReasons(panes);
  return reasons.busyAgents + reasons.busyShells > 0;
}

/** A pane close inside a split loses that agent for good, so a live agent
 *  asks first. The last pane of a tab follows the tab rule. */
export function paneCloseNeedsConfirm(pane: ClosePaneFact, isLastPane: boolean) {
  if (isLastPane) return tabCloseNeedsConfirm([pane]);
  if (pane.kind === "agent") return pane.running;
  return shellBlocksClose(pane);
}

/** Close other tabs, or the tabs to one side: one prompt for the whole set. */
export function bulkCloseConfirm(tabs: ReadonlyArray<readonly ClosePaneFact[]>) {
  const guardedTabs = tabs.filter((panes) => tabCloseNeedsConfirm(panes)).length;
  return { confirm: guardedTabs > 0, guardedTabs };
}

// ---------------------------------------------------------------- "+" menu

export type MenuPlatform = "mac" | "windows" | "linux";

export type PlusMenuAgent = { kind: string; ready: boolean };

export type PlusMenuItem =
  | {
      type: "item";
      id: "new-terminal" | "no-agents" | "text-conversation" | "reopen" | "agent-settings";
      shortcut: string | null;
      disabled: boolean;
    }
  | { type: "agent"; agentKind: string; shortcut: string | null }
  | { type: "separator" };

/** The chord of a new shell tab (ADR 0010). */
export function newTerminalShortcut(platform: MenuPlatform) {
  return platform === "mac" ? "⌘T" : "Ctrl+Shift+T";
}

/** The chord of a new agent tab. Only macOS has one (ADR 0010). */
export function newAgentShortcut(platform: MenuPlatform) {
  return platform === "mac" ? "⌥⌘T" : null;
}

/** The agents that can start now, the default agent first. The "+" menu
 *  and the launcher of an empty workspace (ADR 0020) use the same list. */
export function readyAgentsInOrder(
  agents: readonly PlusMenuAgent[],
  defaultAgent: string | null,
): PlusMenuAgent[] {
  return agents
    .filter((agent) => agent.ready)
    .sort((a, b) => Number(b.kind === defaultAgent) - Number(a.kind === defaultAgent));
}

/** The "+" menu of a strip. The default agent comes first, and only macOS
 *  gets its Option+Cmd+T chord. Agents that are not ready stay out.
 *  `shellTabs` (S5b) and `reopen` (S7) show their items. "Text
 *  conversation" opens the composer for a chat conversation, also in Pro
 *  mode (ADR 0020). */
export function plusMenuItems(input: {
  platform: MenuPlatform;
  agents: readonly PlusMenuAgent[];
  defaultAgent: string | null;
  shellTabs: boolean;
  reopen: { shown: boolean; available: boolean };
}): PlusMenuItem[] {
  const mac = input.platform === "mac";
  const items: PlusMenuItem[] = [];
  if (input.shellTabs) {
    items.push(
      {
        type: "item",
        id: "new-terminal",
        shortcut: newTerminalShortcut(input.platform),
        disabled: false,
      },
      { type: "separator" },
    );
  }
  const ready = readyAgentsInOrder(input.agents, input.defaultAgent);
  if (ready.length === 0) {
    items.push({ type: "item", id: "no-agents", shortcut: null, disabled: true });
  }
  for (const agent of ready) {
    items.push({
      type: "agent",
      agentKind: agent.kind,
      shortcut: agent.kind === input.defaultAgent ? newAgentShortcut(input.platform) : null,
    });
  }
  items.push({ type: "separator" }, {
    type: "item",
    id: "text-conversation",
    shortcut: null,
    disabled: false,
  });
  if (input.reopen.shown) {
    items.push({
      type: "item",
      id: "reopen",
      shortcut: mac ? "⇧⌘T" : null,
      disabled: !input.reopen.available,
    });
  }
  items.push(
    { type: "separator" },
    { type: "item", id: "agent-settings", shortcut: null, disabled: false },
  );
  return items;
}

// ---------------------------------------------------------------- reveal

export type RevealTarget =
  | { kind: "tab"; workspaceId: string; key: TabKey; sessionId: string }
  | { kind: "open-conversation"; conversationId: string }
  | { kind: "background"; sessionId: string };

/** A row click or a notification click: the tab of the session, else open
 *  the conversation (an explicit open), else the session stays a background
 *  terminal. Reveal never creates a tab by itself. */
export function revealTarget(
  state: TabsState,
  facts: TabFacts,
  session: SessionOwnerFact,
): RevealTarget {
  const key = ownerKeyOfSession(session);
  const row = key ? state.rows[key] : undefined;
  const home = row ? homeWorkspaceId(row, facts) : null;
  if (row && home) return { kind: "tab", workspaceId: home, key: row.key, sessionId: session.id };
  const conversation = session.conversationId
    ? facts.conversations.get(session.conversationId)
    : undefined;
  if (conversation && conversationHome(conversation, facts.workspaces)) {
    return { kind: "open-conversation", conversationId: conversation.id };
  }
  return { kind: "background", sessionId: session.id };
}

// ---------------------------------------------------------------- glyph adapters

/** The glyph of a tab and its unread mark. They are the state of the tree
 *  row of the same owner, so the strip and the tree cannot disagree. Ladder:
 *  needs input > failed > working > stopped > done > not reporting > idle. */
export function tabState(input: {
  paneViews: readonly (AgentPaneView | null | undefined)[];
  terminalStatus: ConversationTerminalStatus | null;
  chatRunning: boolean;
  chatUnviewed: boolean;
}): { glyph: AgentPaneGlyph | null; unread: boolean } {
  const { glyph, unread } = treeRowState(input);
  return { glyph, unread };
}

/** The glyph of a workspace: the strongest glyph of its tabs and rows. */
export function workspaceGlyph(glyphs: readonly (AgentPaneGlyph | null)[]) {
  return strongestGlyph(glyphs);
}

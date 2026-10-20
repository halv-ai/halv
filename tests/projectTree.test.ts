import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  agentChipGroups,
  buildProjectTree,
  pathIsInside,
  projectAvatar,
  strongestGlyph,
  treeRowState,
  workspaceForPath,
  type TreeTerminal,
} from "../src/projectTree.ts";
// @ts-expect-error Node's test runner needs the source extension.
import type { AgentPaneView } from "../src/agentPaneStatus.ts";
// @ts-expect-error Node's test runner needs the source extension.
import type { ConversationSummary, Project, WorkspaceOverview } from "../src/types.ts";

// Ported from the project-sidebar prototype (12 checks). Project groups
// come in a later stage, so the two group checks became order checks. The
// glyph ladder follows ADR 0011: a question outranks work.

const NOW = Date.parse("2026-10-14T12:00:00.000Z");
const iso = (minutes: number) => new Date(NOW - minutes * 60_000).toISOString();

function project(id: number, name: string, rootPath: string, ageMinutes: number): Project {
  return { id, name, rootPath, createdAt: iso(ageMinutes), lastOpenedAt: iso(1) };
}

function workspace(
  fields: Partial<WorkspaceOverview> & Pick<WorkspaceOverview, "id" | "projectId" | "rootPath">,
): WorkspaceOverview {
  return {
    name: fields.id,
    kind: "worktree",
    managed: true,
    pinned: false,
    state: "available",
    createdAt: iso(2000),
    lastUsedAt: iso(10),
    available: true,
    isGitRepo: true,
    ...fields,
  };
}

function conversation(
  id: string,
  fields: Partial<ConversationSummary> = {},
): ConversationSummary {
  return {
    id,
    title: id,
    titleSource: "manual",
    mode: "agent",
    model: "sonnet",
    createdAt: iso(5000),
    updatedAt: iso(60),
    pinned: false,
    draft: "",
    draftOnly: false,
    archivedAt: null,
    memoriesMode: null,
    fastMode: false,
    agentMode: true,
    workflowMode: "default",
    credentialProfileId: null,
    agentBackend: null,
    agentSessionModel: null,
    messageCount: 1,
    ...fields,
  };
}

function pane(status: AgentPaneView["status"], unread = false, minutes = 1): AgentPaneView {
  return {
    status,
    reason: null,
    unread,
    waitSeq: 0,
    turnSeq: 1,
    prompt: null,
    source: "presence",
    updatedAt: NOW - minutes * 60_000,
  };
}

function terminal(
  id: string,
  cwd: string,
  fields: Partial<TreeTerminal> = {},
): TreeTerminal {
  return {
    id,
    title: id,
    customTitle: null,
    cwd,
    agentKind: "claude",
    conversationId: null,
    status: "running",
    pane: null,
    ...fields,
  };
}

const projects = [
  project(1, "app", "/u/halv/app", 900),
  project(2, "web", "/u/halv/web", 800),
  project(3, "estalo", "/u/estalo", 1000),
];
const workspaces = [
  workspace({ id: "primary-1", projectId: 1, name: "app", kind: "primary", rootPath: "/u/halv/app", currentBranch: "main", managed: false, pinned: true, lastUsedAt: iso(1) }),
  workspace({ id: "wt-a", projectId: 1, name: "Worktree 1a2b", rootPath: "/data/worktrees/1a2b/app", branch: "feat/device-capture", lastUsedAt: iso(5) }),
  workspace({ id: "primary-2", projectId: 2, name: "web", kind: "primary", rootPath: "/u/halv/web", currentBranch: "main", managed: false, pinned: true, lastUsedAt: iso(50) }),
  workspace({ id: "primary-3", projectId: 3, name: "estalo", kind: "primary", rootPath: "/u/estalo", currentBranch: "dev", managed: false, pinned: true, lastUsedAt: iso(500) }),
];
const conversations = [
  conversation("c1", { title: "Paridade do Halv com o ORCA", projectId: 1, workspaceId: "primary-1", updatedAt: iso(7200) }),
  conversation("c2", { title: "Device capture", projectId: 1, workspaceId: "wt-a", updatedAt: iso(18) }),
  conversation("c3", { title: "Plain chat", mode: "chat", projectId: null, updatedAt: iso(3) }),
  conversation("c4", { title: "Legacy no workspace", mode: "chat", projectId: 2, workspaceId: null, updatedAt: iso(30) }),
];
const terminals = [
  terminal("t1", "/u/halv/app", { conversationId: "c1", pane: pane("working") }),
  terminal("t2", "/data/worktrees/1a2b/app", { agentKind: "codex", conversationId: "c2", pane: pane("needsInput") }),
  terminal("t3", "/u/halv/app/src", { agentKind: "codex", pane: pane("idle") }),
  terminal("t4", "/tmp", { status: "exited" }),
  terminal("t5", "/u/halv/application", { status: "exited" }),
  terminal("t6", "/u/halv/app", { agentKind: null, title: "zsh" }),
];

function build(overrides: Partial<Parameters<typeof buildProjectTree>[0]> = {}) {
  return buildProjectTree({
    projects,
    workspaces,
    conversations,
    terminals,
    runningConversationIds: new Set<string>(),
    unviewedConversationIds: new Set<string>(),
    maxRowsPerWorkspace: 5,
    ...overrides,
  });
}

const tree = build();
const app = tree.projects.find((node) => node.project.name === "app")!;
const rowIds = (node: { rows: Array<{ key: string }> }) => node.rows.map((row) => row.key);

test("projects keep their creation order (was: group order)", () => {
  assert.deepEqual(
    tree.projects.map((node) => node.project.name),
    ["estalo", "app", "web"],
  );
});

test("a project with no listed workspace still gets its primary (was: ungrouped last)", () => {
  const lone = build({
    projects: [...projects, project(4, "lone", "/u/lone", 10)],
    conversations: [...conversations, conversation("c9", { projectId: 4, workspaceId: "primary-4" })],
  }).projects.at(-1)!;
  assert.equal(lone.project.name, "lone");
  assert.deepEqual(lone.workspaces.map((node) => node.workspace.id), ["primary-4"]);
  assert.equal(lone.workspaces[0].primary, true);
  assert.deepEqual(rowIds(lone.workspaces[0]), ["conversation:c9"]);
});

test("primary first", () => {
  assert.equal(app.workspaces[0].primary, true);
  assert.equal(app.workspaces[0].label, "main");
});

test("worktree label is branch", () => {
  assert.equal(app.workspaces[1].label, "feat/device-capture");
  assert.equal(app.workspaces[1].primary, false);
});

test("c1 under primary, working", () => {
  const row = app.workspaces[0].rows.find((item) => item.key === "conversation:c1");
  assert.equal(row?.state.glyph, "working");
  assert.equal(row?.title, "Paridade do Halv com o ORCA");
});

test("t3 (agent terminal, subdirectory cwd) maps to primary-1", () => {
  assert.ok(rowIds(app.workspaces[0]).includes("terminal:t3"));
});

test("c2 under worktree, attention", () => {
  const row = app.workspaces[1].rows[0];
  assert.equal(row.key, "conversation:c2");
  assert.equal(row.state.glyph, "attention");
  assert.equal(row.kind === "conversation" ? row.focusSessionId : null, "t2");
  assert.equal(row.agentKind, "codex");
});

test("project aggregate follows the ladder: a question outranks work", () => {
  assert.equal(app.workspaces[0].glyph, "working");
  assert.equal(app.workspaces[1].glyph, "attention");
  assert.equal(app.glyph, "attention");
});

test("legacy conversation falls back to primary", () => {
  const web = tree.projects.find((node) => node.project.name === "web")!;
  assert.deepEqual(rowIds(web.workspaces[0]), ["conversation:c4"]);
});

test("no-project chat stays loose", () => {
  assert.deepEqual(tree.looseChats.map((row) => row.conversation.id), ["c3"]);
  assert.equal(tree.looseChats[0].key, "conversation:c3");
});

test("unknown cwd is unassigned, and plain shells are never agent rows", () => {
  assert.deepEqual(tree.otherTerminals.map((item) => item.id), ["t4", "t5", "t6"]);
  // The Chats area lists them as terminal rows (ADR 0020).
  assert.deepEqual(
    tree.chatTerminals.map((row) => [row.kind, row.key]),
    [
      ["terminal", "terminal:t4"],
      ["terminal", "terminal:t5"],
      ["terminal", "terminal:t6"],
    ],
  );
  for (const node of tree.projects.flatMap((item) => item.workspaces)) {
    assert.ok(!rowIds(node).includes("terminal:t6"));
  }
});

test("prefix match is path-aware (/u/halv/application is not /u/halv/app)", () => {
  assert.ok(!rowIds(app.workspaces[0]).includes("terminal:t5"));
  assert.equal(pathIsInside("/u/halv/application", "/u/halv/app"), false);
  assert.equal(pathIsInside("/u/halv/app/", "/u/halv/app"), true);
});

test("workspaceForPath picks the longest root and honors the project", () => {
  const nested = [
    workspace({ id: "root", projectId: 1, rootPath: "/repo" }),
    workspace({ id: "inner", projectId: 1, rootPath: "/repo/.halv/worktrees/x/" }),
    workspace({ id: "other", projectId: 2, rootPath: "/repo/vendor" }),
  ];
  assert.equal(workspaceForPath("/repo/.halv/worktrees/x/src", nested)?.id, "inner");
  assert.equal(workspaceForPath("/repo/src", nested)?.id, "root");
  assert.equal(workspaceForPath("/repo/vendor/lib", nested)?.id, "other");
  assert.equal(workspaceForPath("/repo/vendor/lib", nested, 1)?.id, "root");
  assert.equal(workspaceForPath("/elsewhere", nested), null);
  assert.equal(
    workspaceForPath("C:\\code\\repo\\src", [workspace({ id: "win", projectId: 1, rootPath: "C:\\code\\repo" })])?.id,
    "win",
  );
});

test("an unknown bound workspace falls back to the primary workspace", () => {
  const result = build({
    conversations: [conversation("c5", { projectId: 1, workspaceId: "wt-removed" })],
    terminals: [],
  });
  const appNode = result.projects.find((node) => node.project.id === 1)!;
  assert.deepEqual(rowIds(appNode.workspaces[0]), ["conversation:c5"]);
});

test("the cap hides quiet rows only, and keeps the time order", () => {
  const many = Array.from({ length: 8 }, (_, index) =>
    conversation(`q${index}`, { projectId: 3, workspaceId: "primary-3", updatedAt: iso(index + 1) }),
  );
  const result = build({
    conversations: many,
    terminals: [
      terminal("busy", "/u/estalo", { conversationId: "q7", pane: pane("needsApproval") }),
      terminal("busy2", "/u/estalo", { conversationId: "q6", pane: pane("working") }),
    ],
    maxRowsPerWorkspace: 5,
  });
  const node = result.projects[0].workspaces[0];
  assert.deepEqual(rowIds(node), [
    "conversation:q0",
    "conversation:q1",
    "conversation:q2",
    "conversation:q6",
    "conversation:q7",
  ]);
  assert.deepEqual(node.hiddenRows.map((row) => row.key), [
    "conversation:q3",
    "conversation:q4",
    "conversation:q5",
  ]);
  assert.equal(node.glyph, "attention");
  assert.deepEqual(
    node.allRows.map((row) => row.key),
    many.map((item) => `conversation:${item.id}`),
    "the expanded list keeps the time order",
  );

  const crowded = build({
    conversations: many,
    terminals: many.map((item, index) =>
      terminal(`w${index}`, "/u/estalo", { conversationId: item.id, pane: pane("working") }),
    ),
  });
  assert.equal(crowded.projects[0].workspaces[0].rows.length, 8, "busy rows are never hidden");
});

test("pinned rows come first, and archived conversations stay out", () => {
  const result = build({
    conversations: [
      conversation("old-pinned", { projectId: 3, updatedAt: iso(900), pinned: true }),
      conversation("fresh", { projectId: 3, updatedAt: iso(1) }),
      conversation("archived", { projectId: 3, updatedAt: iso(1), archivedAt: NOW }),
    ],
    terminals: [],
  });
  assert.deepEqual(rowIds(result.projects[0].workspaces[0]), [
    "conversation:old-pinned",
    "conversation:fresh",
  ]);
});

test("bound terminals stay in their conversation row, also continuation shells", () => {
  const result = build({
    conversations: [
      conversation("live", { projectId: 3 }),
      conversation("gone", { projectId: 3, archivedAt: NOW }),
    ],
    terminals: [
      terminal("ended", "/u/estalo", { conversationId: "live", status: "exited" }),
      terminal("shell", "/u/estalo", { conversationId: "live", agentKind: null, continuedFrom: "ended" }),
      terminal("orphan", "/u/estalo/sub", { conversationId: "gone", pane: pane("idle") }),
    ],
  });
  const node = result.projects[0].workspaces[0];
  // The orphan pane changed a minute ago; the conversation an hour ago.
  assert.deepEqual(rowIds(node), ["terminal:orphan", "conversation:live"]);
  const live = node.rows[1];
  assert.equal(live.kind, "conversation");
  assert.deepEqual(
    live.kind === "conversation" ? live.terminals.map((item) => item.id) : [],
    ["ended", "shell"],
  );
  // A plain continuation shell never makes the row look busy.
  assert.equal(live.state.glyph, null);
  assert.deepEqual(result.otherTerminals, []);
});

test("chat signals map to the ladder: a running turn works, an unread reply is done", () => {
  const result = build({
    conversations: [
      conversation("running", { projectId: 3, mode: "chat" }),
      conversation("unread", { projectId: 3, mode: "chat" }),
    ],
    terminals: [],
    runningConversationIds: new Set(["running"]),
    unviewedConversationIds: new Set(["unread"]),
  });
  const [first, second] = result.projects[0].workspaces[0].rows;
  const glyphs = Object.fromEntries(
    [first, second].map((row) => [row.key, row.state.glyph]),
  );
  assert.deepEqual(glyphs, { "conversation:running": "working", "conversation:unread": "done" });
});

test("agent chips group by state in rank order, as Orca's summary", () => {
  const { groups, hiddenCount } = agentChipGroups([
    { glyph: "done", kind: "claude" },
    { glyph: "working", kind: "claude" },
    { glyph: "done", kind: "claude" },
    { glyph: "unverifiable", kind: "claude" },
    { glyph: null, kind: "chat" },
  ]);
  assert.deepEqual(
    groups.map((group) => [group.glyph, group.count, group.kinds, group.extra]),
    [
      ["working", 1, ["claude"], 0],
      ["done", 2, ["claude"], 1],
      ["unverifiable", 1, ["claude"], 0],
    ],
    "two finished Claude agents show one icon and +1",
  );
  assert.equal(hiddenCount, 1, "the idle chat is past the third chip");

  const mixed = agentChipGroups([
    { glyph: "working", kind: "codex" },
    { glyph: "working", kind: "claude" },
    { glyph: "working", kind: "claude" },
    { glyph: "working", kind: "gemini" },
    { glyph: "working", kind: "kimi" },
  ]);
  assert.deepEqual(mixed.groups[0].kinds, ["claude", "codex", "gemini"], "the most frequent kind first");
  assert.equal(mixed.groups[0].extra, 2);
  assert.equal(mixed.hiddenCount, 0);
});

test("a title-sourced pane drives the row like any agent signal", () => {
  const titled = (status: AgentPaneView["status"], unread = false): AgentPaneView => ({
    ...pane(status, unread),
    source: "title",
  });
  const row = (view: AgentPaneView) =>
    treeRowState({ paneViews: [view], terminalStatus: null, chatRunning: false, chatUnviewed: false });
  assert.equal(row(titled("working")).glyph, "working", "an old pane spins while it works");
  assert.equal(row(titled("needsApproval")).glyph, "attention");
  const finished = row(titled("done", true));
  assert.equal(finished.glyph, "done");
  assert.equal(finished.unread, true);
  assert.equal(row(titled("idle")).glyph, "idle", "a resting title is not the dashed ring");
  assert.equal(row({ ...titled("idle"), source: "none" }).glyph, "unverifiable");
  // A pane that reports through its title outranks one that reports nothing.
  assert.equal(
    treeRowState({
      paneViews: [{ ...titled("idle"), source: "none" }, titled("working")],
      terminalStatus: null,
      chatRunning: false,
      chatUnviewed: false,
    }).glyph,
    "working",
  );
});

test("row state ladder and ages", () => {
  assert.equal(
    treeRowState({ paneViews: [pane("working"), pane("failed", true)], terminalStatus: null, chatRunning: false, chatUnviewed: false }).glyph,
    "failed",
  );
  const readDone = treeRowState({ paneViews: [pane("done", false)], terminalStatus: null, chatRunning: false, chatUnviewed: false });
  assert.equal(readDone.glyph, "done", "a read finished turn keeps its check, as in Orca");
  assert.equal(readDone.unread, false, "only an unread turn is bold");
  assert.equal(
    treeRowState({ paneViews: [pane("done", true)], terminalStatus: null, chatRunning: false, chatUnviewed: false }).unread,
    true,
  );
  assert.equal(
    treeRowState({ paneViews: [pane("working")], terminalStatus: "waiting_input", chatRunning: false, chatUnviewed: false }).glyph,
    "attention",
    "a question outranks work",
  );
  assert.equal(
    treeRowState({ paneViews: [], terminalStatus: "running", chatRunning: false, chatUnviewed: false }).glyph,
    "working",
  );
  assert.equal(strongestGlyph([null, "done", "working", "failed"]), "failed");
  assert.equal(strongestGlyph([null]), null);
  assert.equal(strongestGlyph(["idle", "unverifiable", "done", "interrupted"]), "interrupted");
  assert.equal(strongestGlyph(["idle", "unverifiable"]), "unverifiable");

  const done = build({
    conversations: [conversation("finished", { projectId: 3, updatedAt: iso(300) })],
    terminals: [terminal("fin", "/u/estalo", { conversationId: "finished", pane: pane("done", true, 18) })],
  }).projects[0].workspaces[0].rows[0];
  assert.equal(done.ageAt, NOW - 18 * 60_000, "a finished turn counts from its status time");
  const quiet = build({
    conversations: [conversation("quiet", { projectId: 3, updatedAt: iso(300) })],
    terminals: [],
  }).projects[0].workspaces[0].rows[0];
  assert.equal(quiet.ageAt, NOW - 300 * 60_000);
});

test("avatar color is stable for one root path", () => {
  assert.deepEqual(projectAvatar(projects[0]), projectAvatar({ ...projects[0], name: "app" }));
  assert.equal(projectAvatar(projects[0]).initial, "A");
  assert.equal(projectAvatar({ name: "  ", rootPath: "/x" }).initial, "?");
});

// Stage S4b: with workspace tabs, the rows of a workspace are its open tabs
// in strip order, then dimmed rows for live conversations without a tab.

test("tab rows: open tabs in strip order, then live conversations without a tab", () => {
  const withTabs = build({
    conversations: [
      ...conversations,
      conversation("c5", { title: "Quiet tab", projectId: 1, workspaceId: "primary-1", updatedAt: iso(9000) }),
      conversation("c6", { title: "Closed and quiet", projectId: 1, workspaceId: "primary-1" }),
    ],
    tabs: {
      stripsByWorkspace: new Map([["primary-1", ["c5", "c1"]]]),
      liveConversationIds: new Set(["c1", "c2"]),
    },
  });
  const appNode = withTabs.projects.find((node) => node.project.name === "app")!;
  // Strip order, not time order. A quiet tab still has its row.
  assert.deepEqual(rowIds(appNode.workspaces[0]), ["conversation:c5", "conversation:c1"]);
  assert.deepEqual(appNode.workspaces[0].hiddenRows, []);
  // c2 runs an agent but has no tab: a dimmed row, never invisible.
  const worktreeRows = appNode.workspaces[1].rows;
  assert.deepEqual(rowIds(appNode.workspaces[1]), ["conversation:c2"]);
  assert.equal(worktreeRows[0].kind === "conversation" && worktreeRows[0].untabbed, true);
  assert.equal(appNode.workspaces[1].glyph, "attention");
  // A closed, quiet conversation has no row. It goes to the History of
  // its workspace, newest first.
  assert.ok(!rowIds(appNode.workspaces[0]).includes("conversation:c6"));
  assert.deepEqual(
    appNode.workspaces[0].history.map((row) => row.key),
    ["conversation:c6"],
  );
  assert.deepEqual(appNode.workspaces[1].history, []);
  // An agent terminal without a conversation and a plain shell are not
  // workspace rows. They wait in the Chats area until adoption moves them
  // into tabs (ADR 0020).
  assert.ok(!rowIds(appNode.workspaces[0]).includes("terminal:t3"));
  assert.deepEqual(
    withTabs.otherTerminals.map((item) => item.id).sort(),
    ["t3", "t4", "t5", "t6"],
  );
});

test("tab rows: a chat tab has one row, and a conversation with two panes has one row", () => {
  const withTabs = build({
    conversations: [
      ...conversations,
      conversation("c7", { title: "Chat in a project", mode: "chat", projectId: 2, workspaceId: null }),
    ],
    terminals: [
      ...terminals,
      terminal("t7", "/u/halv/app", { conversationId: "c1", pane: pane("needsInput") }),
    ],
    tabs: {
      stripsByWorkspace: new Map([
        ["primary-1", ["c1"]],
        ["primary-2", ["c7", "c4"]],
      ]),
      liveConversationIds: new Set(["c1"]),
    },
  });
  const appNode = withTabs.projects.find((node) => node.project.name === "app")!;
  const web = withTabs.projects.find((node) => node.project.name === "web")!;
  assert.deepEqual(rowIds(appNode.workspaces[0]), ["conversation:c1"]);
  const row = appNode.workspaces[0].rows[0];
  // The strongest pane gives the glyph and the focus target.
  assert.equal(row.state.glyph, "attention");
  assert.equal(row.kind === "conversation" ? row.focusSessionId : null, "t7");
  assert.deepEqual(rowIds(web.workspaces[0]), ["conversation:c7", "conversation:c4"]);
  assert.equal(web.workspaces[0].rows[0].agentKind, null);
});

test("groups come first, a project of a missing group stays, and the manual order wins", () => {
  const grouped = build({
    organization: {
      groups: [
        { id: "g-halv", name: "halv" },
        { id: "g-empty", name: "empty" },
      ],
      projectGroups: { "1": "g-halv", "3": "g-halv", "2": "g-gone" },
      projectOrder: [3, 1],
    },
  });
  assert.deepEqual(
    grouped.groups.map((group) => [group.key, group.projects.map((node) => node.project.name)]),
    [
      ["group:g-halv", ["estalo", "app"]],
      ["group:g-empty", []],
    ],
  );
  assert.deepEqual(grouped.ungrouped.map((node) => node.project.name), ["web"]);
  assert.deepEqual(grouped.projects.map((node) => node.project.name), ["estalo", "app", "web"]);
  // The group lane shows the strongest project, as a workspace lane does.
  assert.equal(grouped.groups[0].glyph, "attention");
  assert.equal(grouped.groups[1].glyph, null);
  // Without an organization, nothing is grouped.
  assert.deepEqual(tree.groups, []);
  assert.equal(tree.ungrouped.length, tree.projects.length);
});

test("a conversation with two or more live agent panes gets one child row per pane", () => {
  const split = build({
    terminals: [
      ...terminals,
      terminal("t7", "/u/halv/app", {
        agentKind: "codex",
        conversationId: "c1",
        customTitle: "Reviewer",
        pane: pane("needsInput"),
      }),
      terminal("t8", "/u/halv/app", { conversationId: "c1", status: "exited" }),
      terminal("t9", "/u/halv/app", { agentKind: null, conversationId: "c1", title: "zsh" }),
    ],
  });
  const row = split.projects
    .find((node) => node.project.id === 1)!
    .workspaces.find((node) => node.workspace.id === "primary-1")!
    .rows.find((item) => item.key === "conversation:c1");
  assert.ok(row && row.kind === "conversation");
  // An ended pane and a plain shell are not agent panes.
  assert.deepEqual(
    row.panes.map((item) => [item.key, item.title, item.agentKind, item.state.glyph]),
    [
      ["pane:t1", "t1", "claude", "working"],
      ["pane:t7", "Reviewer", "codex", "attention"],
    ],
  );
  // The conversation row keeps the strongest pane.
  assert.equal(row.state.glyph, "attention");
  assert.equal(row.focusSessionId, "t7");
  // One live agent pane: no child rows.
  const single = tree.projects
    .find((node) => node.project.id === 1)!
    .workspaces.find((node) => node.workspace.id === "primary-1")!
    .rows.find((item) => item.key === "conversation:c1");
  assert.ok(single && single.kind === "conversation");
  assert.deepEqual(single.panes, []);
});

test("the History of a workspace lists its closed conversations, newest first", () => {
  const withTabs = build({
    conversations: [
      ...conversations,
      conversation("h1", { title: "Old", projectId: 1, workspaceId: "primary-1", updatedAt: iso(500) }),
      conversation("h2", { title: "Newer", projectId: 1, workspaceId: "primary-1", updatedAt: iso(50) }),
      conversation("h3", {
        title: "Archived",
        projectId: 1,
        workspaceId: "primary-1",
        archivedAt: iso(10),
      }),
    ],
    tabs: {
      stripsByWorkspace: new Map([["primary-1", ["c1"]]]),
      liveConversationIds: new Set(["c1"]),
    },
  });
  const primary = withTabs.projects.find((node) => node.project.id === 1)!.workspaces[0];
  // An archived conversation stays in Archived. A tab is not history.
  assert.deepEqual(
    primary.history.map((row) => [row.title, row.untabbed]),
    [
      ["Newer", true],
      ["Old", true],
    ],
  );
  // Without workspace tabs, every conversation is a row: no History.
  assert.deepEqual(
    build().projects.flatMap((node) => node.workspaces.flatMap((item) => item.history)),
    [],
  );
});

import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  ADOPTION_MEMORY_LIMIT,
  ADOPTION_STORAGE_KEY,
  adoptableTerminal,
  adoptionPlan,
  adoptionWorkspace,
  EMPTY_ADOPTION_MEMORY,
  readAdoptionMemory,
  withAdopted,
  workspaceForAdoptionRoot,
  writeAdoptionMemory,
  type AdoptionTerminal,
} from "../src/terminalAdoption.ts";

const WORKSPACES = [
  { id: "primary-1", projectId: 1, rootPath: "/Users/ada/code/halv", available: true },
  {
    id: "wt-login",
    projectId: 1,
    rootPath: "/Users/ada/code/halv/.halv/worktrees/login",
    available: true,
  },
  { id: "primary-2", projectId: 2, rootPath: "/Users/ada/code/design", available: true },
  { id: "wt-gone", projectId: 2, rootPath: "/Users/ada/code/design-gone", available: false },
];

function shell(id: string, cwd: string, extra: Partial<AdoptionTerminal> = {}): AdoptionTerminal {
  return { id, cwd, status: "running", agentKind: null, conversationId: null, ...extra };
}

function plan(overrides: Partial<Parameters<typeof adoptionPlan>[0]> = {}) {
  return adoptionPlan({
    terminals: [],
    workspaces: WORKSPACES,
    shellTabs: true,
    conversationHomes: new Map(),
    liveConversationIds: new Set(),
    tabbedConversationIds: new Set(),
    memory: EMPTY_ADOPTION_MEMORY,
    pending: new Set(),
    unresolvable: new Set(),
    waitWorkspaceId: null,
    ...overrides,
  });
}

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    values,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
}

test("a terminal in a project workspace becomes a shell tab of the deepest workspace", () => {
  const steps = plan({
    terminals: [
      shell("t-primary", "/Users/ada/code/halv/src"),
      shell("t-login", "/Users/ada/code/halv/.halv/worktrees/login/app"),
      shell("t-design", "/Users/ada/code/design"),
    ],
  });
  assert.deepEqual(steps, [
    { kind: "shell", sessionId: "t-primary", workspaceId: "primary-1" },
    { kind: "shell", sessionId: "t-login", workspaceId: "wt-login" },
    { kind: "shell", sessionId: "t-design", workspaceId: "primary-2" },
  ]);
});

test("the project of a terminal picks its workspace first, and a workspace off disk never takes a tab", () => {
  // The folder of project 2 is inside project 1 here. The terminal names
  // project 2, so project 2 wins.
  const nested = [
    ...WORKSPACES,
    { id: "primary-3", projectId: 3, rootPath: "/Users/ada/code/halv/packages/ui", available: true },
  ];
  assert.equal(
    adoptionWorkspace({ cwd: "/Users/ada/code/halv/packages/ui/src", projectId: 1 }, nested)?.id,
    "primary-1",
  );
  assert.equal(
    adoptionWorkspace({ cwd: "/Users/ada/code/halv/packages/ui/src", projectId: null }, nested)?.id,
    "primary-3",
  );
  // A project that does not hold the folder falls back to any workspace.
  assert.equal(
    adoptionWorkspace({ cwd: "/Users/ada/code/design/x", projectId: 1 }, nested)?.id,
    "primary-2",
  );
  assert.equal(adoptionWorkspace({ cwd: "/Users/ada/code/design-gone/x" }, WORKSPACES), null);
});

test("a terminal outside every project asks for a git root; without one it stays in the Chats area", () => {
  const terminals = [shell("t-notes", "/Users/ada/code/notes"), shell("t-home", "/Users/ada")];
  assert.deepEqual(plan({ terminals }), [
    { kind: "resolve", sessionId: "t-notes", cwd: "/Users/ada/code/notes" },
    { kind: "resolve", sessionId: "t-home", cwd: "/Users/ada" },
  ]);
  // Rust found no adoptable root for the home shell: no second request.
  assert.deepEqual(plan({ terminals, unresolvable: new Set(["t-home"]) }), [
    { kind: "resolve", sessionId: "t-notes", cwd: "/Users/ada/code/notes" },
  ]);
});

test("a known git root picks its workspace without a second git call, and the launcher still waits", () => {
  // The terminal folder is a symlink of the project folder: the folder
  // test misses, but the root that Rust gave names project 1.
  const terminals = [shell("t-link", "/Volumes/work/halv/src")];
  const resolvedRoots = new Map([["t-link", "/Users/ada/code/halv/"]]);
  assert.deepEqual(plan({ terminals, resolvedRoots }), [
    { kind: "shell", sessionId: "t-link", workspaceId: "primary-1" },
  ]);
  // The visible empty workspace waits: no step, and no resolve step.
  assert.deepEqual(plan({ terminals, resolvedRoots, waitWorkspaceId: "primary-1" }), []);
  // A root that no workspace holds yet asks again: Halv adds the project.
  assert.deepEqual(
    plan({ terminals, resolvedRoots: new Map([["t-link", "/Users/ada/code/notes"]]) }),
    [{ kind: "resolve", sessionId: "t-link", cwd: "/Volumes/work/halv/src" }],
  );
});

test("only running terminals without an owner move, and a standalone agent moves like a shell", () => {
  assert.equal(adoptableTerminal(shell("a", "/x")), true);
  assert.equal(adoptableTerminal(shell("b", "/x", { agentKind: "codex" })), true);
  assert.equal(adoptableTerminal(shell("c", "/x", { conversationId: "conv-1" })), false);
  assert.equal(adoptableTerminal(shell("d", "/x", { conversationId: "  " })), true);
  assert.equal(adoptableTerminal(shell("e", "/x", { shellTabId: "tab-1" })), false);
  assert.equal(adoptableTerminal(shell("f", "/x", { status: "exited" })), false);
  const steps = plan({
    terminals: [
      shell("agent", "/Users/ada/code/halv", { agentKind: "claude" }),
      shell("owned", "/Users/ada/code/halv", { conversationId: "conv-1" }),
      shell("tabbed", "/Users/ada/code/halv", { shellTabId: "tab-1" }),
    ],
  });
  assert.deepEqual(steps, [{ kind: "shell", sessionId: "agent", workspaceId: "primary-1" }]);
  // Without shell tabs, terminals wait in the Chats area.
  assert.deepEqual(
    plan({ terminals: [shell("a", "/Users/ada/code/halv")], shellTabs: false }),
    [],
  );
});

test("a live conversation without a tab opens in its home workspace", () => {
  const steps = plan({
    liveConversationIds: new Set(["c-tabbed", "c-untabbed", "c-loose", "c-unknown"]),
    tabbedConversationIds: new Set(["c-tabbed"]),
    conversationHomes: new Map([
      ["c-tabbed", "primary-1"],
      ["c-untabbed", "wt-login"],
      ["c-loose", null],
    ]),
  });
  assert.deepEqual(steps, [
    { kind: "conversation", conversationId: "c-untabbed", workspaceId: "wt-login" },
  ]);
});

test("adoption runs once per terminal and conversation, also after a close", () => {
  const terminals = [shell("t1", "/Users/ada/code/halv"), shell("t2", "/Users/ada/code/design")];
  const live = {
    liveConversationIds: new Set(["c1"]),
    conversationHomes: new Map([["c1", "primary-1"]]),
  };
  let memory = EMPTY_ADOPTION_MEMORY;
  assert.equal(plan({ terminals, ...live, memory }).length, 3);
  memory = withAdopted(memory, { terminals: ["t1"], conversations: ["c1"] });
  assert.deepEqual(plan({ terminals, ...live, memory }), [
    { kind: "shell", sessionId: "t2", workspaceId: "primary-2" },
  ]);
  // A step that runs now is not planned twice.
  assert.deepEqual(plan({ terminals, ...live, memory, pending: new Set(["t2"]) }), []);
});

test("adoption waits for the visible workspace while its strip has no tab", () => {
  const steps = plan({
    terminals: [shell("t1", "/Users/ada/code/halv"), shell("t2", "/Users/ada/code/design")],
    liveConversationIds: new Set(["c1"]),
    conversationHomes: new Map([["c1", "primary-1"]]),
    waitWorkspaceId: "primary-1",
  });
  assert.deepEqual(steps, [{ kind: "shell", sessionId: "t2", workspaceId: "primary-2" }]);
});

test("the memory reads back, survives bad storage, drops bad ids and keeps the newest ids", () => {
  const storage = memoryStorage();
  assert.deepEqual(readAdoptionMemory(storage), EMPTY_ADOPTION_MEMORY);
  const memory = withAdopted(EMPTY_ADOPTION_MEMORY, { terminals: ["t1", "t1", ""], conversations: ["c1"] });
  assert.deepEqual(memory, { version: 1, terminals: ["t1"], conversations: ["c1"] });
  assert.equal(withAdopted(memory, { terminals: ["t1"] }), memory, "no change, same object");
  assert.equal(writeAdoptionMemory(memory, storage), true);
  assert.deepEqual(readAdoptionMemory(storage), memory);

  for (const value of ["{", "[]", "{\"version\":2}", "7"]) {
    assert.deepEqual(readAdoptionMemory(memoryStorage({ [ADOPTION_STORAGE_KEY]: value })), EMPTY_ADOPTION_MEMORY);
  }
  assert.deepEqual(
    readAdoptionMemory(
      memoryStorage({
        [ADOPTION_STORAGE_KEY]: JSON.stringify({ version: 1, terminals: ["a", 3, "a"], conversations: "x" }),
      }),
    ),
    { version: 1, terminals: ["a"], conversations: [] },
  );
  const throwing = {
    getItem(): string | null {
      throw new Error("blocked");
    },
    setItem() {
      throw new Error("blocked");
    },
  };
  assert.deepEqual(readAdoptionMemory(throwing), EMPTY_ADOPTION_MEMORY);
  assert.equal(writeAdoptionMemory(memory, throwing), false);
  assert.deepEqual(readAdoptionMemory(null), EMPTY_ADOPTION_MEMORY);

  const many = Array.from({ length: ADOPTION_MEMORY_LIMIT + 5 }, (_, index) => `t${index}`);
  const full = withAdopted(EMPTY_ADOPTION_MEMORY, { terminals: many });
  assert.equal(full.terminals.length, ADOPTION_MEMORY_LIMIT);
  assert.equal(full.terminals[0], "t5", "the oldest ids go first");
  assert.equal(full.terminals.at(-1), `t${ADOPTION_MEMORY_LIMIT + 4}`);
});

test("an adoptable root of a known project gives its workspace, else a new project", () => {
  assert.equal(workspaceForAdoptionRoot("/Users/ada/code/design/", WORKSPACES)?.id, "primary-2");
  assert.equal(workspaceForAdoptionRoot("/Users/ada/code/notes", WORKSPACES), null);
  assert.equal(workspaceForAdoptionRoot("/Users/ada/code/design-gone", WORKSPACES), null);
});

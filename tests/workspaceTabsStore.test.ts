import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { createWorkspaceTabsStore, type WorkspaceTabsBackend } from "../src/workspaceTabsStore.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { createFixtureWorkspaceTabs } from "../src/harness/workspace-tabs.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { conversationTabKey, stripView, type ConversationFact, type TabFacts, type TabsOp, type WorkspaceFact } from "../src/workspaceTabs.ts";
import type { WorkspaceTabsSnapshot } from "../src/types.ts";

const ck = conversationTabKey;
const WORKSPACES: WorkspaceFact[] = [
  { id: "primary-1", projectId: 1, kind: "primary" },
  { id: "wt-a", projectId: 1, kind: "worktree" },
];

function conv(id: string, extra: Partial<ConversationFact> = {}): ConversationFact {
  return { id, projectId: 1, workspaceId: "primary-1", mode: "agent", archived: false, createdAt: 1, updatedAt: 1, ...extra };
}

function facts(ids: string[]): TabFacts {
  return {
    complete: true,
    conversations: new Map(ids.map((id) => [id, conv(id)])),
    workspaces: new Map(WORKSPACES.map((workspace) => [workspace.id, workspace])),
  };
}

type Call = { kind: "read" | "apply" | "seed"; ops?: TabsOp[] };

/** A backend over the fixture tables. In manual mode each request waits for
 *  `release()`. It records the calls and the highest concurrency. */
function fakeBackend(backendFacts: () => TabFacts) {
  const tables = createFixtureWorkspaceTabs(backendFacts);
  const calls: Call[] = [];
  const waiting: Array<() => void> = [];
  let manual = false;
  let inFlight = 0;
  let maxInFlight = 0;
  function run<T>(call: Call, body: () => T): Promise<T> {
    calls.push(call);
    inFlight += 1;
    maxInFlight = Math.max(maxInFlight, inFlight);
    return new Promise<T>((resolve, reject) => {
      const go = () => {
        inFlight -= 1;
        try {
          resolve(body());
        } catch (error) {
          reject(error);
        }
      };
      if (manual) waiting.push(go);
      else queueMicrotask(go);
    });
  }
  const backend: WorkspaceTabsBackend = {
    snapshot: () => run({ kind: "read" }, () => tables.snapshot()),
    apply: (ops) => run({ kind: "apply", ops }, () => tables.apply(ops)),
    seed: (rows, active) => run({ kind: "seed" }, () => tables.seed(rows, active)),
  };
  return {
    backend,
    calls,
    maxInFlight: () => maxInFlight,
    setManual(value: boolean) {
      manual = value;
    },
    /** Lets the oldest waiting request finish. */
    async release() {
      const next = waiting.shift();
      assert.ok(next, "a request should wait");
      next();
      await new Promise((resolve) => setTimeout(resolve, 0));
    },
    pending: () => waiting.length,
  };
}

async function loadedStore(ids = ["a", "b", "c"]) {
  let clock = 100;
  const fake = fakeBackend(() => facts(ids));
  const store = createWorkspaceTabsStore({ backend: fake.backend, now: () => (clock += 1) });
  store.setFacts(facts(ids));
  store.refresh();
  await store.whenIdle();
  return { store, fake };
}

const keysOf = (store: ReturnType<typeof createWorkspaceTabsStore>, workspaceId = "primary-1") => {
  const state = store.getSnapshot();
  return stripView(state.tabs, state.facts, workspaceId).keys;
};

test("intents wait for the first snapshot and for complete facts", async () => {
  const fake = fakeBackend(() => facts(["a"]));
  const store = createWorkspaceTabsStore({ backend: fake.backend });
  assert.equal(store.open({ key: ck("a") }).queued, false, "nothing is loaded");
  store.setFacts(facts(["a"]));
  assert.equal(store.open({ key: ck("a") }).queued, false, "no snapshot yet");
  store.refresh();
  await store.whenIdle();
  assert.equal(store.getSnapshot().loaded, true);
  assert.equal(store.open({ key: ck("a") }).queued, true);
  await store.whenIdle();
  assert.deepEqual(keysOf(store), [ck("a")]);
  assert.deepEqual(fake.calls.map((call) => call.kind), ["read", "apply"], "reads go through the same queue");
});

test("optimistic overlay: an intent shows at once, before Rust answers", async () => {
  const { store, fake } = await loadedStore();
  fake.setManual(true);
  let notified = 0;
  const unsubscribe = store.subscribe(() => {
    notified += 1;
  });
  store.open({ key: ck("a") });
  assert.deepEqual(keysOf(store), [ck("a")], "visible before the batch commits");
  assert.equal(store.getSnapshot().rev, 0, "the snapshot is still the old one");
  assert.ok(notified > 0);
  await fake.release();
  assert.equal(store.getSnapshot().rev, 1);
  assert.deepEqual(keysOf(store), [ck("a")]);
  unsubscribe();
});

test("one serial queue: requests go one at a time, in intent order", async () => {
  const { store, fake } = await loadedStore();
  fake.setManual(true);
  store.open({ key: ck("a") });
  store.open({ key: ck("b") });
  store.close(ck("a"));
  store.refresh();
  assert.equal(fake.pending(), 1, "only the first request is in flight");
  while (fake.pending() > 0) await fake.release();
  await store.whenIdle();
  assert.equal(fake.maxInFlight(), 1);
  const kinds = fake.calls.slice(1).map((call) => call.ops?.[0]?.type ?? call.kind);
  assert.deepEqual(kinds, ["insertTab", "insertTab", "deleteTab", "read"]);
  assert.deepEqual(keysOf(store), [ck("b")], "a closed tab stays closed");
});

test("activation batches merge while they wait; nothing else merges", async () => {
  const { store, fake } = await loadedStore();
  store.open({ key: ck("a") });
  store.open({ key: ck("b") });
  store.open({ key: ck("c") });
  await store.whenIdle();
  const before = fake.calls.length;
  fake.setManual(true);
  store.open({ key: ck("a") }); // an existing tab: activation only, in flight
  store.cycle("primary-1", 1);
  store.cycle("primary-1", 1);
  store.selectByNumber("primary-1", 1);
  store.move(ck("c"), 0);
  store.cycle("primary-1", 1);
  while (fake.pending() > 0) await fake.release();
  await store.whenIdle();
  const sent = fake.calls.slice(before).map((call) => call.ops?.map((op) => op.type).join("+"));
  // b, c, then a (Cmd+1) merge into one batch: three touches, one pointer.
  assert.deepEqual(sent, ["touchTab+setActive", "touchTab+touchTab+touchTab+setActive", "setPositions", "touchTab+setActive"]);
  const state = store.getSnapshot();
  assert.deepEqual(keysOf(store), [ck("c"), ck("a"), ck("b")]);
  assert.equal(stripView(state.tabs, state.facts, "primary-1").activeKey, ck("b"), "the merged result equals the sequence");
});

test("a failed batch drops alone; later batches apply; a read follows the failure", async () => {
  let backendIds = ["a", "b", "gone"];
  let clock = 100;
  const fake = fakeBackend(() => facts(backendIds));
  const store = createWorkspaceTabsStore({ backend: fake.backend, now: () => (clock += 1) });
  store.setFacts(facts(["a", "b", "gone"]));
  store.refresh();
  await store.whenIdle();
  backendIds = ["a", "b"]; // Another writer deleted the conversation.
  fake.setManual(true);
  store.open({ key: ck("gone") });
  store.open({ key: ck("a") });
  store.activate(ck("gone"));
  assert.deepEqual(keysOf(store), [ck("gone"), ck("a")]);
  while (fake.pending() > 0) await fake.release();
  await store.whenIdle();
  const state = store.getSnapshot();
  assert.match(state.lastError ?? "", /FOREIGN KEY/);
  assert.deepEqual(Object.keys(state.tabs.rows), [ck("a")], "the unrelated open survived");
  assert.equal(fake.calls.at(-1)?.kind, "read", "the store reads after a failure");
  assert.equal(state.tabs.activeByWorkspace["primary-1"], ck("a"), "the activation of the failed tab became a no-op");
});

test("the store ignores a snapshot with a lower revision", async () => {
  const snapshots: WorkspaceTabsSnapshot[] = [
    { rev: 5, rows: [], activeByWorkspace: { "primary-1": null }, seeded: true },
    { rev: 3, rows: [{ key: ck("a"), conversationId: "a", shellWorkspaceId: null, customTitle: null, defaultTitle: null, position: 0, createdAt: 1, activatedAt: 1 }], activeByWorkspace: {}, seeded: true },
  ];
  const store = createWorkspaceTabsStore({
    backend: {
      snapshot: async () => snapshots.shift()!,
      apply: async () => {
        throw new Error("not used");
      },
      seed: async () => {
        throw new Error("not used");
      },
    },
  });
  store.setFacts(facts(["a"]));
  store.refresh();
  await store.whenIdle();
  store.refresh();
  await store.whenIdle();
  assert.equal(store.getSnapshot().rev, 5);
  assert.deepEqual(store.getSnapshot().tabs.rows, {}, "the stale rows did not land");
});

test("the seed runs once and shows the seeded strips", async () => {
  const { store, fake } = await loadedStore(["a", "b"]);
  const input = {
    savedConversationScopes: [],
    liveConversationIds: new Set(["a", "b"]),
    lastActiveConversationId: "b",
  };
  assert.equal(await store.seedOnce(input), true);
  assert.deepEqual(keysOf(store), [ck("a"), ck("b")]);
  assert.equal(store.getSnapshot().seeded, true);
  assert.equal(await store.seedOnce(input), false, "a second seed is a no-op");
  assert.equal(fake.calls.filter((call) => call.kind === "seed").length, 1);
});

test("the store exports intents only, and only allowlisted UI handlers call open and seedOnce", () => {
  const fake = fakeBackend(() => facts([]));
  const store = createWorkspaceTabsStore({ backend: fake.backend });
  assert.deepEqual(Object.keys(store).sort(), [
    "activate",
    "close",
    "cycle",
    "getSnapshot",
    "move",
    "open",
    "refresh",
    "renameShell",
    "seedOnce",
    "selectByNumber",
    "setFacts",
    "subscribe",
    "whenIdle",
  ]);
  // Stage S4a: only App.tsx opens tabs and seeds, each from one place.
  const allowlist = new Set<string>(["App.tsx"]);
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "src");
  const offenders: string[] = [];
  const walk = (directory: string) => {
    for (const name of readdirSync(directory)) {
      const file = path.join(directory, name);
      if (statSync(file).isDirectory()) {
        walk(file);
        continue;
      }
      if (!/\.tsx?$/.test(name) || name === "workspaceTabsStore.ts") continue;
      const source = readFileSync(file, "utf8");
      if (/workspaceTabsStore\s*\.\s*(open|seedOnce)\s*\(/.test(source)) {
        const relative = path.relative(root, file);
        if (!allowlist.has(relative)) offenders.push(relative);
      }
    }
  };
  walk(root);
  assert.deepEqual(offenders, []);
  const app = readFileSync(path.join(root, "App.tsx"), "utf8");
  // Stage S5b adds the one shell tab entry point (Cmd+T, "+", Run). Stage
  // S7 adds "Open in tab" for a shell without an owner; ADR 0020 moves it
  // into the shared terminal move, which adoption uses too. Adoption opens
  // the tab of a live conversation from one more place.
  const opens = [...app.matchAll(/workspaceTabsStore\s*\.\s*open\s*\(/g)];
  const entryPoints = [
    "function openConversationTab(",
    "function openShellTab(",
    "function moveTerminalIntoShellTab(",
    "function runAdoptionStep(",
  ];
  assert.equal(opens.length, entryPoints.length, "one open call per entry point");
  for (const entryPoint of entryPoints) {
    const opener = app.indexOf(entryPoint);
    const end = app.indexOf("\n  }\n", opener);
    const inside = opens.filter((open) => (open.index ?? 0) > opener && (open.index ?? 0) < end);
    assert.ok(opener >= 0 && inside.length === 1, `one open call sits in ${entryPoint}`);
  }
  const seeds = [...app.matchAll(/workspaceTabsStore\s*\.\s*seedOnce\s*\(/g)];
  assert.equal(seeds.length, 1, "one seed call");
});

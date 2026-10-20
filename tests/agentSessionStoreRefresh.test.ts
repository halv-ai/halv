import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  AgentSessionStore,
  type AgentSession,
  type AgentSessionLaneOptions,
} from "../src/agentSessions.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  resolveTerminalSplitAgentLane,
  terminalSplitAgentChoice,
} from "../src/terminalSplit.ts";

function session(id: string): AgentSession {
  return {
    id,
    agentKind: "codex",
    laneKind: "codexChatGpt",
    mode: "terminal_embedded",
    cwd: "/tmp/project",
    pid: 1,
    status: "running",
    createdAt: "2026-08-18T12:00:00Z",
    lastActiveAt: "2026-08-18T12:00:00Z",
  };
}

test("a slow saved-totals fetch never clobbers concurrent commits", async () => {
  // Regression guard: refreshSessions once captured the state before an
  // await and committed after it, wiping the resume flow's concurrent
  // upserts. The restore then reopened resumed panes as loose tabs.
  let releaseSavedTotals = () => {};
  const savedTotalsGate = new Promise<never[]>((resolve) => {
    releaseSavedTotals = () => resolve([]);
  });
  const store = new AgentSessionStore({
    probe: async () => ({ found: true }),
    list: async () => [session("existing")],
    laneOptions: async () => [],
    create: async () => session("created-during-refresh"),
    send: async () => undefined,
    kill: async () => session("existing"),
    rename: async () => session("existing"),
    subscribe: async () => () => undefined,
    savedTotals: () => savedTotalsGate,
  });

  const refresh = store.refreshSessions();
  await Promise.resolve();
  // The list commit must already be visible before savedTotals resolves.
  assert.ok(
    store
      .getSnapshot()
      .entries.some((entry) => entry.session.id === "existing"),
  );
  // A concurrent commit lands while savedTotals is still pending.
  await store.create({
    agentKind: "codex",
    lane: { kind: "codexChatGpt", proxied: true },
    cwd: "/tmp/project",
  });
  releaseSavedTotals();
  await refresh;

  const ids = store.getSnapshot().entries.map((entry) => entry.session.id);
  assert.ok(ids.includes("existing"));
  assert.ok(
    ids.includes("created-during-refresh"),
    "the concurrent commit must survive the refresh",
  );
});

function claudeLanes(loggedIn: boolean): AgentSessionLaneOptions {
  return [
    {
      agentKind: "claude",
      lanes: [
        { kind: "claudeSubscription", loggedIn, credentialProfiles: [] },
      ],
    },
  ];
}

function laneStore(laneOptions: () => Promise<AgentSessionLaneOptions>) {
  return new AgentSessionStore({
    probe: async () => ({ found: true }),
    list: async () => [],
    laneOptions,
    create: async () => session("created"),
    send: async () => undefined,
    kill: async () => session("killed"),
    rename: async () => session("renamed"),
    subscribe: async () => () => undefined,
    savedTotals: async () => [],
  });
}

test("pushed lane options make a re-login visible to terminal launches", () => {
  // Regression guard: ConnectGate refreshed its own lane copy after a
  // re-login. The store copy kept loggedIn false, so launches failed.
  const store = laneStore(async () => claudeLanes(false));
  store.applyLaneOptions(claudeLanes(false));
  assert.equal(
    terminalSplitAgentChoice(store.getSnapshot().laneOptions, "claude").ready,
    false,
  );

  let notified = 0;
  store.subscribe(() => {
    notified += 1;
  });
  store.applyLaneOptions(claudeLanes(true));
  assert.equal(notified, 1);
  assert.equal(
    terminalSplitAgentChoice(store.getSnapshot().laneOptions, "claude").ready,
    true,
  );
});

test("refreshLaneOptions commits fresh lanes into the store", async () => {
  const store = laneStore(async () => claudeLanes(true));
  store.applyLaneOptions(claudeLanes(false));
  await store.refreshLaneOptions();
  assert.equal(
    terminalSplitAgentChoice(store.getSnapshot().laneOptions, "claude").ready,
    true,
  );
});

test("a launch re-reads stale logged-out lanes before it refuses", async () => {
  const store = laneStore(async () => claudeLanes(true));
  store.applyLaneOptions(claudeLanes(false));
  const lane = await resolveTerminalSplitAgentLane(
    store.getSnapshot().laneOptions,
    "claude",
    () => store.refreshLaneOptions(),
  );
  assert.deepEqual(lane, { kind: "claudeSubscription" });
  assert.equal(
    terminalSplitAgentChoice(store.getSnapshot().laneOptions, "claude").ready,
    true,
  );
});

test("a launch skips the refresh when the cached lane is ready", async () => {
  let refreshes = 0;
  const lane = await resolveTerminalSplitAgentLane(
    claudeLanes(true),
    "claude",
    async () => {
      refreshes += 1;
      return claudeLanes(true);
    },
  );
  assert.deepEqual(lane, { kind: "claudeSubscription" });
  assert.equal(refreshes, 0);
});

test("a launch still refuses when the fresh lanes stay logged out", async () => {
  await assert.rejects(
    resolveTerminalSplitAgentLane(claudeLanes(false), "claude", async () =>
      claudeLanes(false),
    ),
    /signed-in subscription or saved credential profile/,
  );
  await assert.rejects(
    resolveTerminalSplitAgentLane(claudeLanes(false), "claude", async () => {
      throw new Error("ipc down");
    }),
    /signed-in subscription or saved credential profile/,
  );
});

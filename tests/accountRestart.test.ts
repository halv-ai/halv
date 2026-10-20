import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  accountRestartSummary,
  createAccountRestartStore,
  endedSessionIds,
  mergeStaleSessions,
  nextRestartablePane,
  restartPaneOnActiveAccount,
  waitForTerminalExit,
  type AccountRestartEntry,
  type PaneRestartSteps,
} from "../src/accountRestart.ts";
// @ts-expect-error Node's test runner needs the source extension.
import type { WalletStaleSession } from "../src/accountWallet.ts";

function stale(
  sessionId: string,
  provider: "claude" | "codex" = "codex",
  nextAccountId: string | null = "lab",
): WalletStaleSession {
  return { sessionId, provider, previousAccountId: "ops", nextAccountId };
}

function entry(
  sessionId: string,
  phase: AccountRestartEntry["phase"],
  provider: "claude" | "codex" = "codex",
): AccountRestartEntry {
  return { ...stale(sessionId, provider), phase, error: null };
}

test("a pane keeps its answer while the target account stays the same", () => {
  const current = {
    queued: entry("queued", "queued"),
    asked: entry("asked", "ask"),
    restarting: entry("restarting", "restarting"),
    closed: entry("closed", "ask"),
  };
  const merged = mergeStaleSessions(current, [
    stale("queued"),
    stale("asked", "codex", "other"),
    stale("new"),
  ]);
  assert.equal(merged.queued.phase, "queued");
  // Another switch asks again.
  assert.equal(merged.asked.phase, "ask");
  assert.equal(merged.asked.nextAccountId, "other");
  assert.equal(merged.new.phase, "ask");
  // A restart in flight stays until it ends. A closed pane goes.
  assert.equal(merged.restarting.phase, "restarting");
  assert.equal("closed" in merged, false);
});

test("the summary splits the panes that ask from the panes that wait", () => {
  const entries = {
    a: entry("a", "ask"),
    b: entry("b", "failed"),
    c: entry("c", "queued"),
    d: entry("d", "restarting"),
    e: entry("e", "ask", "claude"),
  };
  assert.deepEqual(accountRestartSummary(entries, "codex"), {
    askIds: ["a", "b"],
    pendingCount: 2,
  });
  assert.deepEqual(accountRestartSummary(entries, "claude"), {
    askIds: ["e"],
    pendingCount: 0,
  });
});

test("only a queued pane of this panel restarts, and never during a turn", () => {
  const entries = {
    working: entry("working", "queued"),
    resting: entry("resting", "queued"),
    elsewhere: entry("elsewhere", "queued"),
    asking: entry("asking", "ask"),
  };
  const owned = new Set(["working", "resting", "asking"]);
  const busy = (id: string) => id === "working";
  assert.equal(nextRestartablePane(entries, owned, busy), "resting");
  assert.equal(
    nextRestartablePane(entries, new Set(["working", "asking"]), busy),
    null,
  );
});

test("panes whose agent ended leave the prompt", () => {
  assert.deepEqual(
    endedSessionIds(
      ["live", "exited", "gone"],
      [
        { session: { id: "live", status: "running" } },
        { session: { id: "exited", status: "exited" } },
      ],
    ),
    ["exited", "gone"],
  );
});

test("the store asks, queues, claims, and keeps panes", async () => {
  const kept: string[][] = [];
  let reported = [stale("a"), stale("b"), stale("c", "claude")];
  const store = createAccountRestartStore({
    list: async () => reported,
    keep: async (ids) => {
      kept.push(ids);
    },
  });
  await store.refresh();
  assert.deepEqual(Object.keys(store.getSnapshot().entries).sort(), ["a", "b", "c"]);

  store.queue(["a", "b"]);
  assert.equal(store.getSnapshot().entries.a.phase, "queued");
  assert.equal(store.begin("a"), true);
  // A second panel cannot claim the same pane.
  assert.equal(store.begin("a"), false);
  store.requeue("a");
  assert.equal(store.getSnapshot().entries.a.phase, "queued");
  assert.equal(store.begin("a"), true);
  store.finish("a");
  assert.equal("a" in store.getSnapshot().entries, false);

  assert.equal(store.begin("b"), true);
  store.fail("b", "Halv could not restart this session.");
  assert.equal(store.getSnapshot().entries.b.phase, "failed");
  assert.equal(store.getSnapshot().entries.b.error, "Halv could not restart this session.");

  store.keep(["c"]);
  assert.equal("c" in store.getSnapshot().entries, false);
  assert.deepEqual(kept, [["c"]]);

  // A switch result is newer than a refresh in flight.
  let release: (value: WalletStaleSession[]) => void = () => undefined;
  reported = [];
  const slow = createAccountRestartStore({
    list: () => new Promise((resolve) => (release = resolve)),
    keep: async () => undefined,
  });
  const refresh = slow.refresh();
  slow.apply([stale("x")]);
  release([]);
  await refresh;
  assert.deepEqual(Object.keys(slow.getSnapshot().entries), ["x"]);
});

type Row = { id: string };
type Saved = { paneId: string };

function steps(overrides: Partial<PaneRestartSteps<Row, Saved>> = {}) {
  const calls: string[] = [];
  let bound = "old";
  const base: PaneRestartSteps<Row, Saved> = {
    find: () => ({
      paneId: "pane-1",
      row: { id: "old" },
      saved: { paneId: "pane-1" },
      cliSessionId: "cli-1",
    }),
    resumeBusy: () => false,
    stop: async () => {
      calls.push("stop");
    },
    waitForExit: async () => {
      calls.push("wait");
    },
    resume: async () => {
      calls.push("resume");
      bound = "new";
    },
    boundSessionId: () => bound,
    keepForRetry: () => {
      calls.push("keep");
    },
    forgetOld: async () => {
      calls.push("forget");
    },
    messages: { noCliSession: "no cli session", resumeFailed: "resume failed" },
  };
  return { steps: { ...base, ...overrides }, calls };
}

test("a restart stops the old process, then resumes the CLI session in the pane", async () => {
  const { steps: done, calls } = steps();
  assert.equal(await restartPaneOnActiveAccount("old", done), "done");
  assert.deepEqual(calls, ["stop", "wait", "resume", "forget"]);
});

test("a restart waits while another resume runs, and touches nothing", async () => {
  const { steps: busy, calls } = steps({ resumeBusy: () => true });
  assert.equal(await restartPaneOnActiveAccount("old", busy), "later");
  assert.deepEqual(calls, []);
  const { steps: gone } = steps({ find: () => null });
  assert.equal(await restartPaneOnActiveAccount("old", gone), "gone");
});

test("a pane without a CLI session id never stops", async () => {
  const { steps: missing, calls } = steps({
    find: () => ({
      paneId: "pane-1",
      row: { id: "old" },
      saved: { paneId: "pane-1" },
      cliSessionId: null,
    }),
  });
  await assert.rejects(restartPaneOnActiveAccount("old", missing), /no cli session/);
  assert.deepEqual(calls, []);
});

test("a failed resume keeps the saved pane for a manual resume", async () => {
  const { steps: failing, calls } = steps({
    resume: async () => undefined,
  });
  await assert.rejects(restartPaneOnActiveAccount("old", failing), /resume failed/);
  assert.deepEqual(calls, ["stop", "wait", "keep"]);
});

test("the restart waits until the old terminal stops", async () => {
  let polls = 0;
  await waitForTerminalExit(
    "old",
    async () => {
      polls += 1;
      return polls < 3 ? [{ id: "old", status: "running" }] : [{ id: "old", status: "exited" }];
    },
    { sleep: async () => undefined },
  );
  assert.equal(polls, 3);
  await assert.rejects(
    waitForTerminalExit("old", async () => [{ id: "old", status: "running" }], {
      timeoutMs: 300,
      pollMs: 150,
      sleep: async () => undefined,
    }),
    /did not stop in time/,
  );
});

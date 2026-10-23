import assert from "node:assert/strict";
import test from "node:test";
import type { AgentPaneView } from "../src/agentPaneStatus.ts";
import {
  COMPLETION_NOTIFY_DELAY_MS,
  NEEDS_INPUT_NOTIFY_DELAY_MS,
  createAgentStatusNotifier,
  paneNotificationFor,
  paneNotificationStillDue,
} from "../src/agentStatusNotifications.ts";

function view(overrides: Partial<AgentPaneView> = {}): AgentPaneView {
  return {
    status: "working",
    reason: null,
    unread: false,
    waitSeq: 0,
    turnSeq: 1,
    prompt: null,
    source: "presence",
    updatedAt: 0,
    ...overrides,
  };
}

function harness() {
  const timers: { callback: () => void; delayMs: number; cancelled: boolean }[] = [];
  const panes = new Map<string, AgentPaneView | null>();
  const viewed = new Set<string>();
  const delivered: string[] = [];
  const notifier = createAgentStatusNotifier({
    current: (id) => panes.get(id) ?? null,
    paneViewed: (id) => viewed.has(id),
    deliver: (id, kind, current) => delivered.push(`${id}:${kind}:${current.status}`),
    schedule: (callback, delayMs) => {
      const timer = { callback, delayMs, cancelled: false };
      timers.push(timer);
      return () => {
        timer.cancelled = true;
      };
    },
  });
  const change = (id: string, next: AgentPaneView | null) => {
    const previous = panes.get(id) ?? null;
    panes.set(id, next);
    notifier.update(id, next, previous);
  };
  const flush = () => {
    for (const timer of timers.splice(0)) {
      if (!timer.cancelled) timer.callback();
    }
  };
  return { change, flush, timers, viewed, delivered, notifier };
}

test("a wait earns one banner after the delay", () => {
  const { change, flush, timers, delivered } = harness();
  change("pane", view());
  change("pane", view({ status: "needsApproval", reason: "permission", waitSeq: 1 }));
  assert.equal(timers.at(-1)?.delayMs, NEEDS_INPUT_NOTIFY_DELAY_MS);
  flush();
  assert.deepEqual(delivered, ["pane:permission:needsApproval"]);

  // The same wait again, for example after a webview reload, stays quiet.
  change("pane", view({ status: "working" }));
  change("pane", view({ status: "needsApproval", reason: "permission", waitSeq: 1 }));
  flush();
  assert.equal(delivered.length, 1);

  // A second wait in the same turn is new.
  change("pane", view({ status: "needsInput", reason: "question", waitSeq: 2 }));
  flush();
  assert.deepEqual(delivered.at(-1), "pane:question:needsInput");
});

test("a wait that ends inside the delay sends nothing", () => {
  const { change, flush, delivered } = harness();
  change("pane", view({ status: "needsApproval", waitSeq: 1 }));
  change("pane", view({ status: "working" }));
  flush();
  assert.deepEqual(delivered, []);
});

test("an unread result earns a completion banner, a read one does not", () => {
  const { change, flush, timers, delivered } = harness();
  change("pane", view({ status: "done", unread: true, turnSeq: 3 }));
  assert.equal(timers.at(-1)?.delayMs, COMPLETION_NOTIFY_DELAY_MS);
  flush();
  assert.deepEqual(delivered, ["pane:completion:done"]);

  change("other", view({ status: "done", unread: true, turnSeq: 1 }));
  change("other", view({ status: "done", unread: false, turnSeq: 1 }));
  flush();
  change("third", view({ status: "failed", unread: true, turnSeq: 2 }));
  flush();
  assert.deepEqual(delivered, ["pane:completion:done", "third:error:failed"]);
});

test("the pane the user looks at gets no banner", () => {
  const { change, flush, viewed, delivered } = harness();
  viewed.add("pane");
  change("pane", view({ status: "needsInput", waitSeq: 1 }));
  flush();
  assert.deepEqual(delivered, []);
  // Looking away later does not send the old wait.
  viewed.delete("pane");
  change("pane", view({ status: "needsInput", waitSeq: 1, updatedAt: 5 }));
  flush();
  assert.deepEqual(delivered, []);
});

test("a later change replaces the pending banner of the same pane", () => {
  const { change, flush, delivered } = harness();
  change("pane", view({ status: "needsApproval", waitSeq: 1 }));
  change("pane", view({ status: "done", unread: true, turnSeq: 1 }));
  flush();
  assert.deepEqual(delivered, ["pane:completion:done"]);
});

test("forget and dispose cancel pending banners", () => {
  const { change, flush, notifier, delivered } = harness();
  change("pane", view({ status: "needsApproval", waitSeq: 1 }));
  notifier.forget("pane");
  change("other", view({ status: "needsApproval", waitSeq: 1 }));
  notifier.dispose();
  flush();
  assert.deepEqual(delivered, []);
});

test("only an agent signal earns a banner", () => {
  const heuristic = view({ status: "done", unread: true, turnSeq: 1, source: "heuristic" });
  const agent = view({ status: "done", unread: true, turnSeq: 1, source: "presence" });
  const rollout = view({ status: "done", unread: true, turnSeq: 1, source: "rollout" });
  assert.equal(paneNotificationFor(null, heuristic), null);
  assert.equal(paneNotificationFor(null, agent)?.kind, "completion");
  assert.notEqual(
    paneNotificationFor(null, agent)?.key,
    paneNotificationFor(null, rollout)?.key,
  );
  assert.equal(paneNotificationFor(agent, agent), null);
  assert.equal(paneNotificationFor(null, view({ status: "idle" })), null);
});

test("a hook detail that arrives inside the delay reaches the one banner", () => {
  const timers: (() => void)[] = [];
  const panes = new Map<string, AgentPaneView>();
  const prompts: unknown[] = [];
  const notifier = createAgentStatusNotifier({
    current: (id) => panes.get(id) ?? null,
    paneViewed: () => false,
    deliver: (_id, _kind, current) => prompts.push(current.prompt),
    schedule: (callback) => {
      timers.push(callback);
      return () => {};
    },
  });
  const change = (next: AgentPaneView) => {
    const previous = panes.get("pane") ?? null;
    panes.set("pane", next);
    notifier.update("pane", next, previous);
  };
  change(view());
  // The presence file shows the wait first. The hook then adds the command
  // to the same wait.
  const wait = view({ status: "needsApproval", reason: "permission", waitSeq: 1 });
  change(wait);
  change({ ...wait, prompt: { kind: "approval", toolName: "Bash", command: "pnpm test" } });
  assert.equal(timers.length, 1);
  timers[0]();
  assert.deepEqual(prompts, [{ kind: "approval", toolName: "Bash", command: "pnpm test" }]);
});

test("a Codex wait from a terminal notification earns a banner", () => {
  assert.deepEqual(
    paneNotificationFor(
      view({ source: "rollout" }),
      view({ status: "needsApproval", reason: "permission", waitSeq: 1, source: "terminal" }),
    ),
    { kind: "permission", key: "wait:terminal:1", delayMs: NEEDS_INPUT_NOTIFY_DELAY_MS },
  );
});

test("a failed turn earns an error banner with the completion delay", () => {
  const failed = view({ status: "failed", unread: true, turnSeq: 4, source: "terminal" });
  const banner = paneNotificationFor(null, failed);
  assert.equal(banner?.kind, "error");
  assert.equal(banner?.delayMs, COMPLETION_NOTIFY_DELAY_MS);
  assert.equal(paneNotificationFor(failed, failed), null);
  assert.ok(banner);
  assert.equal(paneNotificationStillDue(banner, failed), true);
  assert.equal(paneNotificationStillDue(banner, { ...failed, unread: false }), false);
  assert.equal(paneNotificationStillDue(banner, { ...failed, status: "done" }), false);
  assert.equal(paneNotificationStillDue(banner, null), false);
});

test("a finished turn and a failed turn of one sequence use different keys", () => {
  const done = paneNotificationFor(null, view({ status: "done", unread: true, turnSeq: 4, source: "terminal" }));
  const failed = paneNotificationFor(null, view({ status: "failed", unread: true, turnSeq: 4, source: "terminal" }));
  assert.equal(done?.kind, "completion");
  assert.notEqual(done?.key, failed?.key);
});

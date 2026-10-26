import assert from "node:assert/strict";
import test from "node:test";

import {
  addTaskRouteDismissals,
  batchAgentCount,
  groupTaskBoardRoutes,
  isTerminalTaskRoute,
  receiptTokens,
  workflowRouteApplied,
  readTaskRouteDismissals,
  routeChain,
  routingSavingsLabel,
  savedLabel,
  sortRoutesNewestFirst,
  sortTaskBoardRoutes,
  splitHalvCloudFallbackRoutes,
  taskRouteModelLabel,
  taskRouteReasonLabel,
  summarizeRoutes,
  taskRouteDismissalsStorageKey,
  tierWeight,
  visibleTaskRoutes,
  writeTaskRouteDismissals,
} from "../src/taskBoard.ts";
import type { TaskRoute, TaskRouteReceipt } from "../src/types.ts";

function route(overrides: Partial<TaskRoute> & Pick<TaskRoute, "id">): TaskRoute {
  return {
    sessionId: "session-1",
    conversationId: "conversation-1",
    title: `Task ${overrides.id}`,
    briefChars: 200,
    complexity: "standard",
    complexityConfidence: 0.7,
    needsReasoning: 0.3,
    risky: 0.1,
    tier: "cheap",
    lane: "claude",
    model: "sonnet",
    effort: "high",
    reasons: ["difficulty"],
    workerLane: "auto",
    status: "routed",
    escalatedFromId: null,
    provider: "jev",
    providerModel: "typesafe/jev-1.13-20260917",
    latencyMs: 150,
    createdAt: "2026-09-21T10:00:00.000Z",
    updatedAt: "2026-09-21T10:00:00.000Z",
    ...overrides,
  };
}

function fakeT(key: string, vars?: Record<string, string | number>): string {
  if (!vars) return key;
  return key.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in vars ? String(vars[name]) : match,
  );
}

test("sorts routes newest first", () => {
  const routes = [
    route({ id: "a", createdAt: "2026-09-21T10:00:00.000Z" }),
    route({ id: "c", createdAt: "2026-09-21T12:00:00.000Z" }),
    route({ id: "b", createdAt: "2026-09-21T11:00:00.000Z" }),
  ];
  assert.deepEqual(
    sortRoutesNewestFirst(routes).map((entry) => entry.id),
    ["c", "b", "a"],
  );
});

test("keeps the input order for equal timestamps and does not mutate the input", () => {
  const routes = [route({ id: "a" }), route({ id: "b" })];
  assert.deepEqual(
    sortRoutesNewestFirst(routes).map((entry) => entry.id),
    ["a", "b"],
  );
  assert.deepEqual(
    routes.map((entry) => entry.id),
    ["a", "b"],
  );
});

test("rebuilds an escalation chain across two escalations", () => {
  const routes = [
    route({ id: "root", tier: "cheapest", status: "failed" }),
    route({ id: "second", escalatedFromId: "root", status: "failed" }),
    route({ id: "third", escalatedFromId: "second", tier: "strong" }),
  ];
  assert.deepEqual(
    routeChain(routes, "third").map((entry) => entry.id),
    ["root", "second", "third"],
  );
  assert.deepEqual(
    routeChain(routes, "second").map((entry) => entry.id),
    ["root", "second"],
  );
  assert.deepEqual(routeChain(routes, "missing"), []);
});

test("stops a chain that points at an unknown or cyclic parent", () => {
  const routes = [
    route({ id: "one", escalatedFromId: "gone" }),
    route({ id: "loop-a", escalatedFromId: "loop-b" }),
    route({ id: "loop-b", escalatedFromId: "loop-a" }),
  ];
  assert.deepEqual(
    routeChain(routes, "one").map((entry) => entry.id),
    ["one"],
  );
  assert.deepEqual(
    routeChain(routes, "loop-a").map((entry) => entry.id),
    ["loop-b", "loop-a"],
  );
});

test("weights the tier ladder from cheapest to max", () => {
  assert.deepEqual(
    (["cheapest", "cheap", "medium", "strong", "max"] as const).map(tierWeight),
    [1, 2, 3, 4, 5],
  );
});

test("summarizes the fixture route shape", () => {
  const routes = [
    route({ id: "1", tier: "cheapest", status: "passed" }),
    route({ id: "2", tier: "cheap", status: "failed" }),
    route({ id: "3", tier: "medium", status: "routed", escalatedFromId: "2" }),
    route({ id: "4", tier: "strong", status: "escalated" }),
    route({ id: "5", tier: "max", status: "passed" }),
  ];
  assert.deepEqual(summarizeRoutes(routes), {
    total: 5,
    active: 1,
    passed: 2,
    failed: 1,
    escalated: 1,
    terminal: 4,
    belowCap: 4,
  });
  assert.deepEqual(summarizeRoutes([]), {
    total: 0,
    active: 0,
    passed: 0,
    failed: 0,
    escalated: 0,
    terminal: 0,
    belowCap: 0,
  });
});

test("a routed task becomes completed after the backend refresh", () => {
  const routed = route({ id: "jev-task" });
  const refreshed = route({
    id: "jev-task",
    status: "passed",
    updatedAt: "2026-09-21T10:02:00.000Z",
  });

  assert.deepEqual(summarizeRoutes([routed]), {
    total: 1,
    active: 1,
    passed: 0,
    failed: 0,
    escalated: 0,
    terminal: 0,
    belowCap: 1,
  });
  assert.deepEqual(summarizeRoutes([refreshed]), {
    total: 1,
    active: 0,
    passed: 1,
    failed: 0,
    escalated: 0,
    terminal: 1,
    belowCap: 1,
  });
  assert.equal(visibleTaskRoutes([refreshed], [])[0]?.status, "passed");
});

test("dismissing terminal routes preserves active routes and successors", () => {
  const failed = route({ id: "failed", status: "failed" });
  const successor = route({
    id: "successor",
    escalatedFromId: failed.id,
    createdAt: "2026-09-21T10:01:00.000Z",
  });
  const active = route({ id: "active" });
  const passed = route({ id: "passed", status: "passed" });
  const routes = [failed, successor, active, passed];
  const dismissed = addTaskRouteDismissals(
    [],
    routes,
    new Set([failed.id, successor.id, passed.id]),
  );

  assert.deepEqual(
    visibleTaskRoutes(routes, dismissed).map((item) => item.id),
    [successor.id, active.id],
  );
  assert.equal(isTerminalTaskRoute(successor), false);
  assert.equal(isTerminalTaskRoute(failed), true);
});

test("completed dismissals persist across reloads without changing route data", () => {
  const storage = memoryStorage();
  const completed = route({ id: "complete", status: "passed" });
  const dismissed = addTaskRouteDismissals(
    [],
    [completed],
    new Set([completed.id]),
  );

  writeTaskRouteDismissals("conversation-1", dismissed, storage);
  const reloaded = readTaskRouteDismissals("conversation-1", storage);

  assert.deepEqual(reloaded, [
    { id: completed.id, updatedAt: completed.updatedAt },
  ]);
  assert.deepEqual(visibleTaskRoutes([completed], reloaded), []);
  assert.equal(completed.status, "passed");
});

test("new routes and new revisions appear after earlier routes were dismissed", () => {
  const completed = route({ id: "complete", status: "passed" });
  const dismissed = addTaskRouteDismissals(
    [],
    [completed],
    new Set([completed.id]),
  );
  const newRoute = route({
    id: "new-route",
    createdAt: "2026-09-21T10:03:00.000Z",
    updatedAt: "2026-09-21T10:03:00.000Z",
  });
  const revisedRoute = route({
    id: completed.id,
    status: "routed",
    updatedAt: "2026-09-21T10:04:00.000Z",
  });

  assert.deepEqual(
    visibleTaskRoutes([completed, newRoute, revisedRoute], dismissed).map(
      (item) => item.id,
    ),
    [newRoute.id, revisedRoute.id],
  );
});

test("dismissals stay isolated to their conversation", () => {
  const storage = memoryStorage();
  const completed = route({ id: "complete", status: "passed" });
  const dismissed = addTaskRouteDismissals(
    [],
    [completed],
    new Set([completed.id]),
  );

  writeTaskRouteDismissals("conversation-1", dismissed, storage);

  assert.deepEqual(readTaskRouteDismissals("conversation-1", storage), dismissed);
  assert.deepEqual(readTaskRouteDismissals("conversation-2", storage), []);
  assert.notEqual(
    taskRouteDismissalsStorageKey("conversation-1"),
    taskRouteDismissalsStorageKey("conversation-2"),
  );
});

test("labels savings only when a route ran below the coordinator tier", () => {
  assert.equal(savedLabel([], fakeT), "no savings yet");
  assert.equal(
    savedLabel([route({ id: "1", tier: "max" })], fakeT),
    "no savings yet",
  );
  assert.equal(
    savedLabel(
      [route({ id: "1", tier: "max" }), route({ id: "2", tier: "cheap" })],
      fakeT,
    ),
    "1 tasks below the coordinator tier",
  );
});

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
}


test("active routes precede newer terminal routes with stable group ordering", () => {
  const routes = [
    route({ id: "done", status: "passed", createdAt: "2026-09-22T10:00:00Z" }),
    route({ id: "active-a" }),
    route({ id: "failed", status: "failed" }),
    route({ id: "active-b" }),
    route({ id: "escalated", status: "escalated" }),
  ];
  assert.deepEqual(sortTaskBoardRoutes(routes).map(({ id }) => id),
    ["active-a", "active-b", "done", "failed", "escalated"]);
  const dismissed = addTaskRouteDismissals([], routes, new Set(routes.map(({ id }) => id)));
  assert.deepEqual(visibleTaskRoutes(routes, dismissed).map(({ id }) => id), ["active-a", "active-b"]);
  assert.equal(routes[0].id, "done");
});

test("worker labels identify recommendations without claiming execution", () => {
  assert.equal(taskRouteModelLabel(route({ id: "claude", model: "opus", effort: "xhigh" }), fakeT),
    "Suggested route: claude · opus · xhigh");
  assert.equal(taskRouteModelLabel(route({ id: "codex", lane: "codex", model: "gpt-6-sol", effort: null }), fakeT),
    "Suggested route: codex · gpt-6-sol");
  assert.equal(taskRouteModelLabel(route({ id: "max", tier: "max", lane: "codex", model: null, effort: null }), fakeT),
    "Coordinator · codex");
});

function workflowRoute(
  id: string,
  key: string,
  overrides: Partial<TaskRoute> = {},
): TaskRoute {
  return route({
    id,
    workflow: { batchId: "batch-1", name: "fix-findings", key, role: "read", instances: 1 },
    defaultModel: "claude-opus-5-5",
    ...overrides,
  });
}

function receipt(models: string[], overrides: Partial<TaskRouteReceipt> = {}): TaskRouteReceipt {
  return {
    routeId: "r",
    sessionId: "session-1",
    outcome: "passed",
    agents: 1,
    failedAgents: 0,
    inputTokens: 10,
    outputTokens: 20,
    cacheReadTokens: 300,
    cacheCreationTokens: 40,
    durationMs: 1_000,
    usageSource: "measured",
    reportedAt: "2026-09-21T10:00:00.000Z",
    models,
    ...overrides,
  };
}

test("groups the routes of a workflow batch at the place of its first route", () => {
  const routes = [
    route({ id: "legacy-1" }),
    workflowRoute("scout", "scout"),
    route({ id: "legacy-2" }),
    workflowRoute("fix", "fix"),
    workflowRoute("other", "x", {
      workflow: { batchId: "batch-2", name: null, key: "x", role: "verify", instances: 1 },
    }),
  ];
  const entries = groupTaskBoardRoutes(routes);
  assert.deepEqual(
    entries.map((entry) => (entry.kind === "route" ? entry.route.id : entry.batchId)),
    ["legacy-1", "batch-1", "legacy-2", "batch-2"],
  );
  const batch = entries[1];
  assert.equal(batch?.kind, "batch");
  if (batch?.kind === "batch") {
    assert.deepEqual(batch.routes.map((entry) => entry.id), ["scout", "fix"]);
    assert.equal(batch.name, "fix-findings");
  }
});

test("a batch counts the instances of its stages, not its retries", () => {
  const routes = [
    workflowRoute("scout", "scout", {
      workflow: { batchId: "batch-1", name: null, key: "scout", role: "read", instances: 4 },
    }),
    workflowRoute("fix", "fix"),
    workflowRoute("fix-retry", "fix", { escalatedFromId: "fix", tier: "medium" }),
  ];
  assert.equal(batchAgentCount(routes), 5);
  assert.equal(summarizeRoutes(routes).belowCap, 6);
  assert.equal(savedLabel(routes, fakeT), "6 tasks below the coordinator tier");
});

test("a workflow route shows whether its agents ran on the routed model", () => {
  assert.equal(workflowRouteApplied(workflowRoute("a", "a")), null);
  assert.equal(
    workflowRouteApplied(workflowRoute("a", "a", { receipt: receipt(["claude-sonnet-5-5"]) })),
    true,
  );
  assert.equal(
    workflowRouteApplied(
      workflowRoute("a", "a", { receipt: receipt(["claude-sonnet-5-5", "claude-opus-5-5"]) }),
    ),
    false,
  );
  assert.equal(
    workflowRouteApplied(
      workflowRoute("a", "a", {
        tier: "max",
        model: null,
        receipt: receipt(["claude-opus-5-5"]),
      }),
    ),
    true,
  );
  assert.equal(
    workflowRouteApplied(route({ id: "legacy", receipt: receipt(["claude-opus-5-5"]) })),
    null,
  );
});

test("an inherited workflow route names the default model", () => {
  assert.equal(
    taskRouteModelLabel(workflowRoute("a", "a", { tier: "max", model: null }), fakeT),
    "Inherits claude-opus-5-5",
  );
  assert.equal(
    taskRouteModelLabel(
      workflowRoute("a", "a", { tier: "max", model: null, defaultModel: null }),
      fakeT,
    ),
    "Inherits the coordinator",
  );
});

test("receipt tokens add every class and stay null without usage", () => {
  assert.equal(receiptTokens(receipt([])), 370);
  assert.equal(
    receiptTokens(
      receipt([], {
        inputTokens: null,
        outputTokens: null,
        cacheReadTokens: null,
        cacheCreationTokens: null,
        usageSource: "none",
      }),
    ),
    null,
  );
});

test("the routing header never shows a loss", () => {
  const t = (key: string, vars?: Record<string, string | number>) =>
    Object.entries(vars ?? {}).reduce(
      (text, [name, value]) => text.replace(`{${name}}`, String(value)),
      key,
    );
  const money = (value: number) => `$${value.toFixed(2)}`;
  const estimated = (netUsd: number) => ({
    state: "estimated" as const,
    netUsd,
    lowUsd: netUsd,
    highUsd: 1,
    chains: 2,
  });
  assert.equal(
    routingSavingsLabel(estimated(0.42), t, money),
    "\u2248\u00a0$0.42 saved by routing",
  );
  assert.equal(
    routingSavingsLabel(estimated(-0.35), t, money),
    "\u2248\u00a0$0.00 saved by routing",
  );
  assert.equal(
    routingSavingsLabel({ state: "not_estimated" }, t, money),
    "Routing savings not estimated yet",
  );
});

test("routes without Halv Cloud leave the board list and name the reason", () => {
  const routes = [
    route({ id: "real" }),
    route({ id: "cloud", tier: "max", reasons: ["halv_cloud_required"] }),
    route({ id: "session", tier: "max", reasons: ["sign_in_required"] }),
  ];
  const { routed, fallbackCount } = splitHalvCloudFallbackRoutes(routes);
  assert.deepEqual(routed.map((entry) => entry.id), ["real"]);
  assert.equal(fallbackCount, 2);
  const t = (key: string) => key;
  assert.equal(taskRouteReasonLabel("halv_cloud_required", t), "Needs Halv Cloud");
  assert.equal(taskRouteReasonLabel("sign_in_required", t), "Sign in needed");
  assert.equal(taskRouteReasonLabel("brief_sharing_off", t), "Task briefs declined");
  assert.equal(taskRouteReasonLabel("difficulty", t), "difficulty");
});

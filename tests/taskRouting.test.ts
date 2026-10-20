import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_CATALOG,
  DEFAULT_TIER_AGENTS,
  catalogFromAvailableModels,
  chooseRoute,
  clampToCap,
  collapseCoordinatorModel,
  agentOpts,
  chooseWorkflowRoute,
  escalate,
  floorFromDecision,
  workflowEscalation,
  workflowFallbackRoute,
  type WorkflowRole,
  type WorkflowRoutingInput,
  type Lane,
  type LaneCatalog,
  type LaneState,
  type RoutingInput,
} from "../src/taskRouting.ts";
import type { RouteDecision, RouteTier } from "../src/types.ts";

function decision(
  choice: RouteTier,
  confidence: number,
  overrides: Partial<Pick<RouteDecision, "needsReasoning" | "risky">> = {},
): RouteDecision {
  return {
    schema: "task-tier-v1",
    provider: "jev",
    model: "typesafe/jev-1.13-20260917",
    complexity: {
      choice,
      confidence,
      probabilities: { trivial: 0.1, standard: 0.1, hard: 0.1 },
    },
    needsReasoning: overrides.needsReasoning ?? 0.2,
    risky: overrides.risky ?? 0.05,
    usage: { inputTokens: 392, outputTokens: 65 },
    latencyMs: 143,
  };
}

function lane(overrides: Partial<LaneState> = {}): LaneState {
  return {
    cap: "strong",
    available: true,
    remainingPercent: null,
    reservePercent: 10,
    ...overrides,
  };
}

function input(
  route: RouteDecision,
  overrides: Partial<RoutingInput> = {},
): RoutingInput {
  return {
    decision: route,
    coordinatorLane: "claude",
    lanes: { claude: lane(), codex: lane() },
    ...overrides,
  };
}

test("trivial with high confidence routes to the cheapest tier on the coordinator lane", () => {
  const result = chooseRoute(input(decision("trivial", 0.97)));
  assert.equal(result.tier, "cheapest");
  assert.equal(result.lane, "claude");
  assert.equal(result.model, "haiku");
  assert.equal(result.effort, null);
  assert.deepEqual(result.reasons, ["difficulty"]);
});

test("low confidence raises the floor one tier", () => {
  const floor = floorFromDecision(decision("standard", 0.6));
  assert.equal(floor.tier, "medium");
  assert.deepEqual(floor.reasons, ["difficulty", "confidence"]);

  const result = chooseRoute(input(decision("standard", 0.6)));
  assert.equal(result.tier, "medium");
  assert.equal(result.model, "opus");
  assert.equal(result.effort, "medium");
  assert.deepEqual(result.reasons, ["difficulty", "confidence"]);
});

test("needs_reasoning raises the floor to medium", () => {
  const floor = floorFromDecision(
    decision("trivial", 0.95, { needsReasoning: 0.8 }),
  );
  assert.equal(floor.tier, "medium");
  assert.deepEqual(floor.reasons, ["difficulty", "reasoning"]);
});

test("a risky task routes to strong and records the risk reason", () => {
  const result = chooseRoute(input(decision("hard", 0.95, { risky: 0.97 })));
  assert.equal(result.tier, "strong");
  assert.equal(result.model, "opus");
  assert.equal(result.effort, "xhigh");
  assert.ok(result.reasons.includes("risk"));
});

test("clampToCap collapses tiers above the cap", () => {
  assert.equal(clampToCap("strong", "medium"), "medium");
  assert.equal(clampToCap("max", "cheap"), "cheap");
  assert.equal(clampToCap("cheapest", "medium"), "cheapest");
});

test("an Opus coordinator collapses a hard task into the cap", () => {
  const result = chooseRoute(
    input(decision("hard", 0.95), {
      lanes: {
        claude: lane({ cap: "medium" }),
        codex: lane({ cap: "medium", available: false }),
      },
    }),
  );
  assert.equal(result.tier, "medium");
  assert.equal(result.lane, "claude");
  assert.equal(result.model, "opus");
  assert.equal(result.effort, "medium");
  assert.ok(result.reasons.includes("cap"));
});

test("the reserve of the coordinator lane sends the task to the other lane", () => {
  const result = chooseRoute(
    input(decision("standard", 0.9), {
      lanes: {
        claude: lane({ remainingPercent: 3, reservePercent: 10 }),
        codex: lane({ remainingPercent: 60 }),
      },
    }),
  );
  assert.equal(result.lane, "codex");
  assert.equal(result.tier, "cheap");
  assert.equal(result.model, "gpt-6-luna");
  assert.equal(result.effort, "high");
  assert.ok(result.reasons.includes("quota"));
  assert.ok(result.reasons.includes("reserve"));
});

test("an unknown quota counts as 50 percent and keeps the coordinator lane on a tie", () => {
  const result = chooseRoute(
    input(decision("standard", 0.9), {
      lanes: {
        claude: lane({ remainingPercent: null }),
        codex: lane({ remainingPercent: 50 }),
      },
    }),
  );
  assert.equal(result.lane, "claude");
  assert.deepEqual(result.reasons, ["difficulty"]);
});

test("no available lane falls back to the coordinator at max", () => {
  const result = chooseRoute(
    input(decision("standard", 0.9), {
      lanes: {
        claude: lane({ available: false }),
        codex: lane({ available: false }),
      },
    }),
  );
  assert.equal(result.tier, "max");
  assert.equal(result.lane, "claude");
  assert.equal(result.model, null);
  assert.ok(result.reasons.includes("reserve"));
});

test("a catalog override replaces the model names", () => {
  const catalog: Record<Lane, LaneCatalog> = {
    ...DEFAULT_CATALOG,
    claude: {
      cheapest: { model: "haiku-next", effort: "low" },
      cheap: { model: "sonnet-next", effort: "high" },
      medium: { model: "opus-next", effort: "medium" },
      strong: { model: "opus-next", effort: "xhigh" },
    },
  };
  const result = chooseRoute(input(decision("trivial", 0.97), { catalog }));
  assert.equal(result.model, "haiku-next");
  assert.equal(result.effort, "low");
});

test("escalation raises one tier on the same lane below the cap", () => {
  const routingInput = input(decision("trivial", 0.97), {
    lanes: { claude: lane({ cap: "medium" }), codex: lane({ cap: "strong" }) },
  });
  const first = chooseRoute(routingInput);
  assert.equal(first.tier, "cheapest");

  const second = escalate(first, routingInput);
  assert.equal(second.tier, "cheap");
  assert.equal(second.lane, "claude");
  assert.equal(second.model, "sonnet");
  assert.equal(second.effort, "high");
  assert.deepEqual(second.reasons, ["difficulty", "escalated"]);
});

test("escalation at the cap crosses to the other lane", () => {
  const routingInput = input(decision("hard", 0.95), {
    lanes: { claude: lane({ cap: "medium" }), codex: lane({ cap: "strong" }) },
  });
  const first = chooseRoute(routingInput);
  assert.equal(first.tier, "medium");
  assert.equal(first.lane, "claude");

  const second = escalate(first, routingInput);
  assert.equal(second.tier, "medium");
  assert.equal(second.lane, "codex");
  assert.equal(second.model, "gpt-6.1-sol");
  assert.equal(second.effort, "medium");
  assert.deepEqual(second.reasons, [
    ...first.reasons,
    "escalated",
    "cap",
  ]);
});

test("escalation with no lane left returns max on the coordinator lane", () => {
  const routingInput = input(decision("hard", 0.95), {
    coordinatorLane: "codex",
    lanes: {
      claude: lane({ cap: "strong", remainingPercent: 0 }),
      codex: lane({ cap: "medium", remainingPercent: 80 }),
    },
  });
  const first = chooseRoute(routingInput);
  assert.equal(first.lane, "codex");
  assert.equal(first.tier, "medium");

  const second = escalate(first, routingInput);
  assert.equal(second.tier, "max");
  assert.equal(second.lane, "codex");
  assert.equal(second.model, null);
  assert.deepEqual(second.reasons, [...first.reasons, "reserve"]);
});

test("fixed codex lane routes a standard task to luna even when claude has more quota", () => {
  const result = chooseRoute(
    input(decision("standard", 0.9), {
      workerLane: "codex",
      lanes: {
        claude: lane({ remainingPercent: 90 }),
        codex: lane({ remainingPercent: 20 }),
      },
    }),
  );
  assert.equal(result.lane, "codex");
  assert.equal(result.tier, "cheap");
  assert.equal(result.model, "gpt-6-luna");
  assert.equal(result.effort, "high");
  assert.deepEqual(result.reasons, ["difficulty", "lane"]);
});

test("fixed codex lane with no codex quota falls back to max", () => {
  const result = chooseRoute(
    input(decision("standard", 0.9), {
      workerLane: "codex",
      lanes: {
        claude: lane({ remainingPercent: 90 }),
        codex: lane({ remainingPercent: 0 }),
      },
    }),
  );
  assert.equal(result.tier, "max");
  assert.equal(result.lane, "claude");
  assert.equal(result.model, null);
  assert.deepEqual(result.reasons, ["difficulty", "reserve"]);
});

test("fixed lane escalation never crosses lanes", () => {
  const routingInput = input(decision("standard", 0.9), {
    workerLane: "codex",
    lanes: { claude: lane(), codex: lane({ cap: "medium" }) },
  });
  const first = chooseRoute(routingInput);
  assert.equal(first.lane, "codex");
  assert.equal(first.tier, "cheap");

  const second = escalate(first, routingInput);
  assert.equal(second.lane, "codex");
  assert.equal(second.tier, "medium");
  assert.equal(second.model, "gpt-6.1-sol");
  assert.equal(second.effort, "medium");

  const third = escalate(second, routingInput);
  assert.equal(third.tier, "max");
  assert.equal(third.lane, "claude");
  assert.equal(third.model, null);
  assert.deepEqual(third.reasons, [...second.reasons, "reserve"]);
});

test("auto keeps current behavior", () => {
  const result = chooseRoute(
    input(decision("standard", 0.9), {
      lanes: {
        claude: lane({ remainingPercent: 3, reservePercent: 10 }),
        codex: lane({ remainingPercent: 60 }),
      },
    }),
  );
  assert.equal(result.lane, "codex");
  assert.equal(result.tier, "cheap");
  assert.equal(result.model, "gpt-6-luna");
  assert.equal(result.effort, "high");
  assert.ok(result.reasons.includes("quota"));
  assert.ok(!result.reasons.includes("lane"));
});

test("tier agent assignments select the saved agent and catalog entry", () => {
  const tierAgents = {
    ...DEFAULT_TIER_AGENTS,
    cheap: {
      lane: "codex" as const,
      model: "gpt-6.1-sol",
      effort: "xhigh",
    },
    strong: { lane: "claude" as const, model: "opus", effort: "xhigh" },
  };
  const catalog = {
    ...DEFAULT_CATALOG,
    codex: {
      ...DEFAULT_CATALOG.codex,
      cheap: { model: "gpt-6.2-luna", effort: "medium" },
    },
  };
  const cheap = chooseRoute(
    input(decision("standard", 0.9), {
      tierAgents,
      catalog,
      workerLane: "claude",
    }),
  );
  assert.equal(cheap.tier, "cheap");
  assert.equal(cheap.lane, "codex");
  assert.equal(cheap.model, "gpt-6.1-sol");
  assert.equal(cheap.effort, "xhigh");
  assert.ok(cheap.reasons.includes("lane"));

  const strong = chooseRoute(input(decision("hard", 0.95), { tierAgents }));
  assert.equal(strong.tier, "strong");
  assert.equal(strong.lane, "claude");
  assert.equal(strong.model, "opus");
  assert.equal(strong.effort, "xhigh");
});

test("escalation uses the agent assigned to the next tier", () => {
  const tierAgents = {
    cheapest: { lane: "claude" as const, model: "haiku", effort: null },
    cheap: { lane: "codex" as const, model: "gpt-6-luna", effort: "high" },
    medium: { lane: "claude" as const, model: "opus", effort: "medium" },
    strong: { lane: "codex" as const, model: "gpt-6.1-sol", effort: "xhigh" },
  };
  const routingInput = input(decision("trivial", 0.97), { tierAgents });
  const first = chooseRoute(routingInput);
  assert.equal(first.lane, "claude");
  assert.equal(first.tier, "cheapest");

  const second = escalate(first, routingInput);
  assert.equal(second.lane, "codex");
  assert.equal(second.tier, "cheap");
  assert.equal(second.model, "gpt-6-luna");
  assert.deepEqual(second.reasons, ["difficulty", "lane", "escalated", "lane"]);
});

test("the default catalog pairs every worker tier with a model and an effort", () => {
  assert.deepEqual(DEFAULT_CATALOG, {
    claude: {
      cheapest: { model: "haiku", effort: null },
      cheap: { model: "sonnet", effort: "high" },
      medium: { model: "opus", effort: "medium" },
      strong: { model: "opus", effort: "xhigh" },
    },
    codex: {
      cheapest: { model: "gpt-6-luna", effort: "medium" },
      cheap: { model: "gpt-6-luna", effort: "high" },
      medium: { model: "gpt-6.1-sol", effort: "medium" },
      strong: { model: "gpt-6.1-sol", effort: "xhigh" },
    },
  });

  const workerModels = Object.values(DEFAULT_CATALOG).flatMap((laneCatalog) =>
    Object.values(laneCatalog).map((entry) => entry.model),
  );
  assert.equal(workerModels.length, 8);
  assert.ok(!workerModels.includes("fable"));
  assert.ok(!workerModels.includes("gpt-6-astra"));
});

test("the CLI catalog selects the newest advertised Codex family", () => {
  const catalog = catalogFromAvailableModels([
    { provider: "codex_cli", id: "gpt-6-sol" },
    { provider: "codex_cli", id: "gpt-6.1-sol" },
    {
      provider: "codex_cli",
      id: "gpt-6.2-sol",
      reasoningEfforts: ["medium", "high"],
      defaultEffort: "medium",
    },
    { provider: "codex_cli", id: "gpt-5.6-luna" },
    { provider: "codex_cli", id: "gpt-6-luna" },
    { provider: "other", id: "gpt-99-sol" },
    { provider: "codex_cli", id: "gpt-7-sol-preview" },
  ]);

  assert.equal(catalog.codex.cheapest.model, "gpt-6-luna");
  assert.equal(catalog.codex.cheap.model, "gpt-6-luna");
  assert.equal(catalog.codex.medium.model, "gpt-6.2-sol");
  assert.equal(catalog.codex.strong.model, "gpt-6.2-sol");
  assert.equal(catalog.codex.medium.effort, "medium");
  assert.equal(catalog.codex.strong.effort, "high");
});

test("family discovery preserves Claude aliases and safe Codex fallbacks", () => {
  const fallback = catalogFromAvailableModels([
    { provider: "claude_cli", id: "claude-sonnet-5-5" },
    { provider: "codex_cli", id: "gpt-invalid-sol" },
  ]);
  assert.deepEqual(fallback, DEFAULT_CATALOG);

  const older = catalogFromAvailableModels([
    { provider: "codex_cli", id: "gpt-6-sol" },
  ]);
  assert.equal(older.codex.medium.model, "gpt-6-sol");
  assert.equal(older.claude.cheap.model, "sonnet");
});

test("a worker on the coordinator model collapses into the coordinator", () => {
  const routed = chooseRoute(input(decision("standard", 0.6)));
  assert.equal(routed.model, "opus");
  const result = collapseCoordinatorModel(routed, "claude", "claude-opus-5-5");
  assert.equal(result.tier, "max");
  assert.equal(result.model, null);
  assert.equal(result.effort, null);
  assert.equal(result.reasons.at(-1), "cap");

  const sol = chooseRoute(input(decision("hard", 0.95), { workerLane: "codex" }));
  assert.equal(sol.model, "gpt-6.1-sol");
  assert.equal(sol.effort, "xhigh");
  for (const coordinator of ["gpt-6-sol", "gpt-6.1-sol"]) {
    const collapsed = collapseCoordinatorModel(sol, "codex", coordinator);
    assert.equal(collapsed.tier, "max");
    assert.equal(collapsed.model, null);
    assert.equal(collapsed.effort, null);
    assert.equal(collapsed.reasons.at(-1), "cap");
  }
});

test("a cheaper worker or a stronger coordinator keeps the route", () => {
  const cheap = chooseRoute(input(decision("trivial", 0.97)));
  assert.deepEqual(collapseCoordinatorModel(cheap, "claude", "opus"), cheap);
  const medium = chooseRoute(input(decision("standard", 0.6)));
  assert.deepEqual(collapseCoordinatorModel(medium, "claude", "fable"), medium);
  assert.deepEqual(collapseCoordinatorModel(medium, "claude", null), medium);
  assert.deepEqual(collapseCoordinatorModel(medium, "codex", "gpt-6-sol"), medium);
});

function workflowInput(
  route: RouteDecision,
  role: WorkflowRole,
  overrides: Partial<WorkflowRoutingInput> = {},
): WorkflowRoutingInput {
  return { decision: route, role, coordinatorCap: "strong", ...overrides };
}

test("workflow routes keep Opus tiers and never collapse into the coordinator", () => {
  const implement = chooseWorkflowRoute(
    workflowInput(decision("hard", 0.95), "implement"),
  );
  assert.equal(implement.tier, "strong");
  assert.deepEqual(agentOpts(implement), { model: "opus", effort: "xhigh" });
});

test("a read agent stays on Sonnet; reasoning raises its effort and risk does not raise it", () => {
  const reasoning = chooseWorkflowRoute(
    workflowInput(decision("hard", 0.98, { needsReasoning: 0.9 }), "read"),
  );
  assert.deepEqual(agentOpts(reasoning), { model: "sonnet", effort: "xhigh" });
  assert.deepEqual(reasoning.reasons, ["difficulty", "reasoning", "role"]);

  const risky = chooseWorkflowRoute(
    workflowInput(decision("hard", 0.98, { risky: 0.9 }), "read"),
  );
  assert.deepEqual(agentOpts(risky), { model: "sonnet", effort: "high" });
  assert.ok(!risky.reasons.includes("risk"));
});

test("reasoning raises the effort of implement agents, not the tier", () => {
  const route = chooseWorkflowRoute(
    workflowInput(decision("standard", 0.93, { needsReasoning: 0.9 }), "implement"),
  );
  assert.equal(route.tier, "cheap");
  assert.deepEqual(agentOpts(route), { model: "sonnet", effort: "xhigh" });
});

test("verify agents stay at Opus medium or above and the coordinator cap wins", () => {
  const verify = chooseWorkflowRoute(
    workflowInput(decision("trivial", 0.97, { needsReasoning: 0.1 }), "verify"),
  );
  assert.deepEqual(agentOpts(verify), { model: "opus", effort: "medium" });

  const sonnet = chooseWorkflowRoute(
    workflowInput(decision("hard", 0.95, { needsReasoning: 0.9 }), "verify", {
      coordinatorCap: "cheap",
    }),
  );
  assert.equal(sonnet.model, "sonnet");
  assert.ok(sonnet.reasons.includes("cap"));
});

test("a Haiku route gets an explicit effort and Codex assignments are dropped", () => {
  const haiku = chooseWorkflowRoute(
    workflowInput(decision("trivial", 0.97, { needsReasoning: 0.1 }), "read", {
      defaultEffort: "medium",
    }),
  );
  assert.deepEqual(agentOpts(haiku), { model: "haiku", effort: "medium" });

  const codex = chooseWorkflowRoute(
    workflowInput(decision("standard", 0.9, { needsReasoning: 0.1 }), "implement", {
      tierAgents: {
        ...DEFAULT_TIER_AGENTS,
        cheap: { lane: "codex", model: "gpt-6-luna", effort: "high" },
      },
    }),
  );
  assert.equal(codex.lane, "claude");
  assert.equal(codex.model, "sonnet");
  assert.equal(codex.reasons.at(-1), "lane");
});

test("workflow escalation walks the band, then inherits, then stops", () => {
  const input = workflowInput(decision("standard", 0.9, { needsReasoning: 0.1 }), "implement");
  const chain = [chooseWorkflowRoute(input)];
  for (;;) {
    const next = workflowEscalation(chain[chain.length - 1]!, input);
    if (!next) break;
    chain.push(next);
  }
  assert.deepEqual(
    chain.map(agentOpts),
    [
      { model: "sonnet", effort: "high" },
      { model: "opus", effort: "medium" },
      { model: "opus", effort: "xhigh" },
      {},
    ],
  );
  assert.equal(workflowEscalation(workflowFallbackRoute(), input), null);
  assert.deepEqual(workflowFallbackRoute().reasons, ["fallback"]);
});

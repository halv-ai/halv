import type { RouteDecision } from "./types";

export type Lane = "claude" | "codex";
export type LadderTier = "cheapest" | "cheap" | "medium" | "strong" | "max";
export const LADDER: readonly LadderTier[] = [
  "cheapest",
  "cheap",
  "medium",
  "strong",
  "max",
];

/** One rung of the ladder: the model and the reasoning effort it runs at. */
export type CatalogEntry = { model: string; effort: string | null };

export type LaneCatalog = Record<Exclude<LadderTier, "max">, CatalogEntry>;
export const DEFAULT_CATALOG: Record<Lane, LaneCatalog> = {
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
};

export type AvailableWorkerModel = {
  id: string;
  provider: string;
  reasoningEfforts?: readonly string[];
  defaultEffort?: string;
};

const CODEX_CATALOG_PROVIDER = "codex_cli";

function codexFamilyVersion(model: string, family: string): number[] | null {
  const normalized = model.trim().toLowerCase();
  const suffix = `-${family}`;
  if (!normalized.startsWith("gpt-") || !normalized.endsWith(suffix)) return null;
  const version = normalized.slice("gpt-".length, -suffix.length);
  if (!/^\d+(?:\.\d+)*$/.test(version)) return null;
  return version.split(".").map(Number);
}

function compareVersions(left: number[], right: number[]): number {
  const length = Math.max(left.length, right.length);
  for (let index = 0; index < length; index += 1) {
    const difference = (left[index] ?? 0) - (right[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}

function newestCodexFamilyModel(
  models: readonly AvailableWorkerModel[],
  family: string,
): AvailableWorkerModel | null {
  let newest: { model: AvailableWorkerModel; version: number[] } | null = null;
  for (const model of models) {
    if (model.provider !== CODEX_CATALOG_PROVIDER) continue;
    const version = codexFamilyVersion(model.id, family);
    if (version === null) continue;
    if (newest === null || compareVersions(version, newest.version) > 0) {
      newest = { model, version };
    }
  }
  return newest?.model ?? null;
}

const EFFORT_ORDER = ["low", "medium", "high", "xhigh", "max", "ultra"];

function supportedEffort(
  model: AvailableWorkerModel,
  requested: string | null,
): string | null {
  if (requested === null) return null;
  const supported = model.reasoningEfforts ?? [];
  if (supported.length === 0 || supported.includes(requested)) return requested;
  const requestedRank = EFFORT_ORDER.indexOf(requested);
  const lower = supported
    .filter((effort) => {
      const rank = EFFORT_ORDER.indexOf(effort);
      return rank >= 0 && rank <= requestedRank;
    })
    .sort((left, right) => EFFORT_ORDER.indexOf(right) - EFFORT_ORDER.indexOf(left))[0];
  if (lower) return lower;
  if (model.defaultEffort && supported.includes(model.defaultEffort)) {
    return model.defaultEffort;
  }
  return supported[0] ?? requested;
}

/** Resolves versioned Codex families advertised by the local CLI catalog. */
export function catalogFromAvailableModels(
  models: readonly AvailableWorkerModel[],
): Record<Lane, LaneCatalog> {
  const luna = newestCodexFamilyModel(models, "luna");
  const sol = newestCodexFamilyModel(models, "sol");
  const codexEntry = (entry: CatalogEntry, model: AvailableWorkerModel | null) => ({
    model: model?.id ?? entry.model,
    effort: model ? supportedEffort(model, entry.effort) : entry.effort,
  });
  return {
    claude: {
      cheapest: { ...DEFAULT_CATALOG.claude.cheapest },
      cheap: { ...DEFAULT_CATALOG.claude.cheap },
      medium: { ...DEFAULT_CATALOG.claude.medium },
      strong: { ...DEFAULT_CATALOG.claude.strong },
    },
    codex: {
      cheapest: codexEntry(DEFAULT_CATALOG.codex.cheapest, luna),
      cheap: codexEntry(DEFAULT_CATALOG.codex.cheap, luna),
      medium: codexEntry(DEFAULT_CATALOG.codex.medium, sol),
      strong: codexEntry(DEFAULT_CATALOG.codex.strong, sol),
    },
  };
}

export type LaneState = {
  cap: LadderTier;
  available: boolean;
  remainingPercent: number | null;
  reservePercent: number;
};

/** The lane every worker must use. `auto` lets the policy pick the lane. */
export type WorkerLane = Lane | "auto";

export type TierAgentAssignment = {
  lane: WorkerLane;
  model: string | null;
  effort: string | null;
};

export type TierAgentAssignments = Record<
  Exclude<LadderTier, "max">,
  TierAgentAssignment
>;

export const DEFAULT_TIER_AGENTS: TierAgentAssignments = {
  cheapest: { lane: "auto", model: null, effort: null },
  cheap: { lane: "auto", model: null, effort: null },
  medium: { lane: "auto", model: null, effort: null },
  strong: { lane: "auto", model: null, effort: null },
};

export type RoutingInput = {
  decision: RouteDecision;
  coordinatorLane: Lane;
  lanes: Record<Lane, LaneState>;
  catalog?: Record<Lane, LaneCatalog>;
  workerLane?: WorkerLane;
  tierAgents?: TierAgentAssignments;
};

export type RoutingReason =
  | "difficulty"
  | "reasoning"
  | "risk"
  | "confidence"
  | "cap"
  | "quota"
  | "reserve"
  | "escalated"
  | "lane"
  | "role"
  | "fallback";

export type RoutingResult = {
  tier: LadderTier;
  lane: Lane;
  model: string | null;
  /** The reasoning effort of the chosen tier. `null` when the tier has none. */
  effort: string | null;
  /** Every rule that set or moved the tier, in the order the policy applied it. */
  reasons: RoutingReason[];
};

const LANES: readonly Lane[] = ["claude", "codex"];
const UNKNOWN_QUOTA_SCORE = 50;

const FLOOR_BY_COMPLEXITY: Record<RouteDecision["complexity"]["choice"], LadderTier> = {
  trivial: "cheapest",
  standard: "cheap",
  hard: "strong",
};

function tierIndex(tier: LadderTier): number {
  return LADDER.indexOf(tier);
}

function raiseTo(tier: LadderTier, floor: LadderTier): LadderTier {
  return tierIndex(tier) < tierIndex(floor) ? floor : tier;
}

/** The decision model never selects `max`, so a raise stops at `strong`. */
function raiseOneTier(tier: LadderTier): LadderTier {
  const next = LADDER[Math.min(tierIndex(tier) + 1, tierIndex("strong"))];
  return next ?? tier;
}

export function clampToCap(tier: LadderTier, cap: LadderTier): LadderTier {
  return tierIndex(tier) > tierIndex(cap) ? cap : tier;
}

export function modelForTier(
  lane: Lane,
  tier: LadderTier,
  catalog: Record<Lane, LaneCatalog> = DEFAULT_CATALOG,
): string | null {
  return tier === "max" ? null : catalog[lane][tier].model;
}

export function effortForTier(
  lane: Lane,
  tier: LadderTier,
  catalog: Record<Lane, LaneCatalog> = DEFAULT_CATALOG,
): string | null {
  return tier === "max" ? null : catalog[lane][tier].effort;
}

export function floorFromDecision(decision: RouteDecision): {
  tier: LadderTier;
  reasons: RoutingReason[];
} {
  const reasons: RoutingReason[] = ["difficulty"];
  let tier = FLOOR_BY_COMPLEXITY[decision.complexity.choice] ?? "cheap";

  if (decision.needsReasoning >= 0.5) {
    tier = raiseTo(tier, "medium");
    reasons.push("reasoning");
  }
  if (decision.risky >= 0.5) {
    tier = raiseTo(tier, "strong");
    reasons.push("risk");
  }
  if (decision.complexity.confidence < 0.7) {
    tier = raiseOneTier(tier);
    reasons.push("confidence");
  }
  return { tier, reasons };
}

function hasQuota(
  lane: Lane,
  state: LaneState,
  coordinatorLane: Lane,
): boolean {
  if (state.remainingPercent === null) return true;
  return lane === coordinatorLane
    ? state.remainingPercent > state.reservePercent
    : state.remainingPercent > 0;
}

function quotaScore(state: LaneState): number {
  return state.remainingPercent ?? UNKNOWN_QUOTA_SCORE;
}

/** Returns the lane the user or the coordinator pinned, if there is one. */
function fixedLane(input: RoutingInput, tier: LadderTier): Lane | null {
  if (tier === "max") return null;
  const tierAgent = input.tierAgents?.[tier]?.lane ?? "auto";
  if (tierAgent !== "auto") return tierAgent;
  const workerLane = input.workerLane ?? "auto";
  return workerLane === "auto" ? null : workerLane;
}

function assignedEntry(
  input: RoutingInput,
  lane: Lane,
  tier: Exclude<LadderTier, "max">,
  catalog: Record<Lane, LaneCatalog>,
): CatalogEntry {
  const assignment = input.tierAgents?.[tier];
  if (assignment?.lane === lane && assignment.model) {
    return { model: assignment.model, effort: assignment.effort };
  }
  return catalog[lane][tier];
}

export function chooseRoute(input: RoutingInput): RoutingResult {
  const catalog = input.catalog ?? DEFAULT_CATALOG;
  const floor = floorFromDecision(input.decision);
  const coordinatorLane = input.coordinatorLane;

  const pinned = fixedLane(input, floor.tier);
  if (pinned !== null) {
    const state = input.lanes[pinned];
    if (!state.available || !hasQuota(pinned, state, coordinatorLane)) {
      return {
        tier: "max",
        lane: coordinatorLane,
        model: null,
        effort: null,
        reasons: [...floor.reasons, "reserve"],
      };
    }
    const tier = clampToCap(floor.tier, state.cap);
    const reasons = [...floor.reasons];
    if (tier !== floor.tier) reasons.push("cap");
    reasons.push("lane");
    const entry =
      tier === "max" ? null : assignedEntry(input, pinned, tier, catalog);
    return {
      tier,
      lane: pinned,
      model: entry?.model ?? null,
      effort: entry?.effort ?? null,
      reasons,
    };
  }

  let chosen: { lane: Lane; tier: LadderTier; score: number } | null = null;
  for (const lane of LANES) {
    const state = input.lanes[lane];
    if (!state.available) continue;
    if (!hasQuota(lane, state, coordinatorLane)) continue;
    const score = quotaScore(state);
    const better =
      chosen === null ||
      score > chosen.score ||
      (score === chosen.score && lane === coordinatorLane);
    if (better) {
      chosen = { lane, tier: clampToCap(floor.tier, state.cap), score };
    }
  }

  const coordinatorState = input.lanes[coordinatorLane];
  const skippedByReserve =
    coordinatorState.available &&
    coordinatorState.remainingPercent !== null &&
    coordinatorState.remainingPercent <= coordinatorState.reservePercent;

  if (chosen === null) {
    return {
      tier: "max",
      lane: coordinatorLane,
      model: null,
      effort: null,
      reasons: [...floor.reasons, "reserve"],
    };
  }

  const reasons = [...floor.reasons];
  if (chosen.tier !== floor.tier) reasons.push("cap");
  if (chosen.lane !== coordinatorLane) reasons.push("quota");
  if (skippedByReserve) reasons.push("reserve");

  return {
    tier: chosen.tier,
    lane: chosen.lane,
    model: modelForTier(chosen.lane, chosen.tier, catalog),
    effort: effortForTier(chosen.lane, chosen.tier, catalog),
    reasons,
  };
}

function otherLane(lane: Lane): Lane {
  return lane === "claude" ? "codex" : "claude";
}

export function escalate(
  current: RoutingResult,
  input: RoutingInput,
): RoutingResult {
  const catalog = input.catalog ?? DEFAULT_CATALOG;
  const coordinatorLane = input.coordinatorLane;
  const currentState = input.lanes[current.lane];

  const explicitLane = input.workerLane ?? "auto";
  const pinned =
    input.tierAgents || explicitLane === "auto" ? null : explicitLane;
  if (pinned !== null) {
    const state = input.lanes[pinned];
    if (current.tier !== "max" && tierIndex(current.tier) < tierIndex(state.cap)) {
      const tier = raiseOneTier(current.tier);
      return {
        tier,
        lane: pinned,
        model: modelForTier(pinned, tier, catalog),
        effort: effortForTier(pinned, tier, catalog),
        reasons: [...current.reasons, "escalated"],
      };
    }
    return {
      tier: "max",
      lane: coordinatorLane,
      model: null,
      effort: null,
      reasons: [...current.reasons, "reserve"],
    };
  }

  if (input.tierAgents) {
    if (current.tier === "strong" || current.tier === "max") {
      return {
        tier: "max",
        lane: coordinatorLane,
        model: null,
        effort: null,
        reasons: [...current.reasons, "reserve"],
      };
    }
    const tier = raiseOneTier(current.tier);
    const assigned = fixedLane(input, tier);
    if (assigned !== null) {
      const state = input.lanes[assigned];
      if (
        state.available &&
        hasQuota(assigned, state, coordinatorLane) &&
        tierIndex(tier) <= tierIndex(state.cap)
      ) {
        const entry =
          tier === "max" ? null : assignedEntry(input, assigned, tier, catalog);
        return {
          tier,
          lane: assigned,
          model: entry?.model ?? null,
          effort: entry?.effort ?? null,
          reasons: [...current.reasons, "escalated", "lane"],
        };
      }
      return {
        tier: "max",
        lane: coordinatorLane,
        model: null,
        effort: null,
        reasons: [...current.reasons, "reserve"],
      };
    }
  }

  if (
    current.tier !== "max" &&
    tierIndex(current.tier) < tierIndex(currentState.cap)
  ) {
    const tier = raiseOneTier(current.tier);
    return {
      tier,
      lane: current.lane,
      model: modelForTier(current.lane, tier, catalog),
      effort: effortForTier(current.lane, tier, catalog),
      reasons: [...current.reasons, "escalated"],
    };
  }

  const crossLane = otherLane(current.lane);
  const crossState = input.lanes[crossLane];
  if (
    current.tier !== "max" &&
    crossState.available &&
    hasQuota(crossLane, crossState, coordinatorLane) &&
    clampToCap(current.tier, crossState.cap) === current.tier
  ) {
    return {
      tier: current.tier,
      lane: crossLane,
      model: modelForTier(crossLane, current.tier, catalog),
      effort: effortForTier(crossLane, current.tier, catalog),
      reasons: [...current.reasons, "escalated", "cap"],
    };
  }

  return {
    tier: "max",
    lane: coordinatorLane,
    model: null,
    effort: null,
    reasons: [...current.reasons, "reserve"],
  };
}

const MODEL_FAMILIES = ["haiku", "sonnet", "opus", "fable", "luna", "terra", "sol", "astra"] as const;

function modelFamily(model: string): string | null {
  const normalized = model.trim().toLowerCase();
  return MODEL_FAMILIES.find((family) => normalized.includes(family)) ?? null;
}

/**
 * Sends a worker route on the coordinator's own model back to the coordinator.
 * Such a worker costs the same per token and repeats the coordinator's
 * repository discovery.
 */
export function collapseCoordinatorModel(
  result: RoutingResult,
  coordinatorLane: Lane,
  coordinatorModel: string | null,
): RoutingResult {
  if (result.model === null || coordinatorModel === null) return result;
  const family = modelFamily(result.model);
  const sameFamily = family !== null && modelFamily(coordinatorModel) === family;
  if (result.tier === "max" || result.lane !== coordinatorLane || !sameFamily) {
    return result;
  }
  return {
    tier: "max",
    lane: coordinatorLane,
    model: null,
    effort: null,
    reasons: [...result.reasons, "cap"],
  };
}

// Workflow mode (ADR 0014). A Claude Code Workflow script starts each agent
// with a fresh context, so this policy never collapses a route into the
// coordinator. It routes on the Claude lane only, and the role of the agent
// sets the tier band.

/** What a workflow agent does. The role sets the tier band of the agent. */
export type WorkflowRole = "read" | "implement" | "verify";

/** The lowest and the highest tier of each role band. */
export const WORKFLOW_ROLE_BANDS: Record<
  WorkflowRole,
  { floor: LadderTier; cap: LadderTier }
> = {
  read: { floor: "cheapest", cap: "cheap" },
  implement: { floor: "cheap", cap: "strong" },
  verify: { floor: "medium", cap: "strong" },
};

/** The `agent()` options of one workflow route. Empty options inherit. */
export type AgentOpts = { model?: string; effort?: string };

export type WorkflowRoutingInput = {
  decision: RouteDecision;
  role: WorkflowRole;
  /** The cap of the coordinator model. */
  coordinatorCap: LadderTier;
  catalog?: Record<Lane, LaneCatalog>;
  workerLane?: WorkerLane;
  tierAgents?: TierAgentAssignments;
  /** The effort of a tier that has no effort in the catalog. */
  defaultEffort?: string | null;
};

const WORKFLOW_FALLBACK_EFFORT = "low";
const CLAUDE_EFFORTS = ["low", "medium", "high", "xhigh", "max"];
const WORKFLOW_EFFORT_CEILING = "xhigh";

function claudeEffortRank(effort: string): number {
  return Math.max(CLAUDE_EFFORTS.indexOf(effort), 0);
}

/** Raises an effort one step. A raise stops at `xhigh` and never lowers `max`. */
function raiseWorkflowEffort(effort: string): string {
  const rank = claudeEffortRank(effort);
  if (rank >= claudeEffortRank(WORKFLOW_EFFORT_CEILING)) return effort;
  return CLAUDE_EFFORTS[rank + 1] ?? effort;
}

function maxTier(left: LadderTier, right: LadderTier): LadderTier {
  return tierIndex(left) >= tierIndex(right) ? left : right;
}

/**
 * JEV marks almost every brief as reasoning work, so `needsReasoning` raises
 * the effort. Only a read agent also moves up, to its band cap. A read agent
 * changes no files, so `risky` does not raise it.
 */
function workflowFloor(decision: RouteDecision, role: WorkflowRole): {
  tier: LadderTier;
  reasons: RoutingReason[];
  raiseEffort: boolean;
} {
  const reasons: RoutingReason[] = ["difficulty"];
  let tier = FLOOR_BY_COMPLEXITY[decision.complexity.choice] ?? "cheap";
  let raiseEffort = false;
  if (decision.needsReasoning >= 0.5) {
    if (role === "read") tier = raiseTo(tier, "medium");
    raiseEffort = true;
    reasons.push("reasoning");
  }
  if (decision.risky >= 0.5 && role !== "read") {
    tier = raiseTo(tier, "strong");
    reasons.push("risk");
  }
  if (decision.complexity.confidence < 0.7) {
    tier = raiseOneTier(tier);
    reasons.push("confidence");
  }
  return { tier, reasons, raiseEffort };
}

function workflowTop(input: WorkflowRoutingInput): LadderTier {
  return clampToCap(
    WORKFLOW_ROLE_BANDS[input.role].cap,
    clampToCap(input.coordinatorCap, "strong"),
  );
}

function workflowDropsCodex(input: WorkflowRoutingInput, tier: LadderTier): boolean {
  if (input.workerLane === "codex") return true;
  return tier !== "max" && input.tierAgents?.[tier]?.lane === "codex";
}

function workflowRouteAt(
  input: WorkflowRoutingInput,
  tier: Exclude<LadderTier, "max">,
  raiseEffort: boolean,
  reasons: RoutingReason[],
): RoutingResult {
  const catalog = input.catalog ?? DEFAULT_CATALOG;
  const assignment = input.tierAgents?.[tier];
  const entry =
    assignment?.lane === "claude" && assignment.model
      ? { model: assignment.model, effort: assignment.effort }
      : catalog.claude[tier];
  const effort = entry.effort ?? input.defaultEffort ?? WORKFLOW_FALLBACK_EFFORT;
  return {
    tier,
    lane: "claude",
    model: entry.model,
    effort: raiseEffort ? raiseWorkflowEffort(effort) : effort,
    reasons,
  };
}

/** Routes one workflow agent inside the band of its role. */
export function chooseWorkflowRoute(input: WorkflowRoutingInput): RoutingResult {
  const floor = workflowFloor(input.decision, input.role);
  const band = WORKFLOW_ROLE_BANDS[input.role];
  const reasons = [...floor.reasons];
  const banded = clampToCap(maxTier(floor.tier, band.floor), band.cap);
  if (banded !== floor.tier) reasons.push("role");
  const tier = clampToCap(banded, clampToCap(input.coordinatorCap, "strong"));
  if (tier !== banded) reasons.push("cap");
  if (workflowDropsCodex(input, tier)) reasons.push("lane");
  return workflowRouteAt(
    input,
    tier as Exclude<LadderTier, "max">,
    floor.raiseEffort,
    reasons,
  );
}

/**
 * The route of one retry after a failed check, or `null` at the top: one tier
 * up inside the band, then the next effort at the band cap, then the
 * coordinator model.
 */
export function workflowEscalation(
  current: RoutingResult,
  input: WorkflowRoutingInput,
): RoutingResult | null {
  if (current.tier === "max") return null;
  const reasons: RoutingReason[] = [...current.reasons, "escalated"];
  if (tierIndex(current.tier) < tierIndex(workflowTop(input))) {
    const { raiseEffort } = workflowFloor(input.decision, input.role);
    return workflowRouteAt(
      input,
      raiseOneTier(current.tier) as Exclude<LadderTier, "max">,
      raiseEffort,
      reasons,
    );
  }
  const effort = current.effort ?? WORKFLOW_FALLBACK_EFFORT;
  if (claudeEffortRank(effort) < claudeEffortRank(WORKFLOW_EFFORT_CEILING)) {
    return { ...current, effort: raiseWorkflowEffort(effort), reasons };
  }
  return { tier: "max", lane: "claude", model: null, effort: null, reasons };
}

/** The route of a workflow agent when JEV cannot answer: inherit. */
export function workflowFallbackRoute(): RoutingResult {
  return { tier: "max", lane: "claude", model: null, effort: null, reasons: ["fallback"] };
}

/** The `agent()` options of a workflow route. A `max` route inherits. */
export function agentOpts(result: RoutingResult): AgentOpts {
  if (result.tier === "max") return {};
  const opts: AgentOpts = {};
  if (result.model) opts.model = result.model;
  if (result.effort) opts.effort = result.effort;
  return opts;
}

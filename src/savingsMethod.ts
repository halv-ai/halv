import type { RoutingTotals } from "./types";

// The dollar metric of ADR 0015: shared helpers of the savings surfaces.

/** A window event that opens Settings at "How Halv estimates savings". */
export const OPEN_SAVINGS_METHOD_EVENT = "halv:open-savings-method";

export function openSavingsMethod(): void {
  window.dispatchEvent(new CustomEvent(OPEN_SAVINGS_METHOD_EVENT));
}

// Lanes with a flat subscription. Their dollars are the API value of the
// work that Halv avoided, not money back. Sessions name lanes in camelCase;
// the database uses snake_case.
const PLAN_LANES = new Set([
  "claudeSubscription",
  "codexChatGpt",
  "kimiSubscription",
  "zaiGlm",
  "claude_subscription",
  "codex_chatgpt",
  "kimi_subscription",
  "zai_glm",
]);

export function laneIsPlan(lane: string | null | undefined): boolean {
  return lane != null && PLAN_LANES.has(lane);
}

/**
 * The routing (JEV) part of the dollars saved (ADR 0016). Halv settles a
 * routed task chain when it ends. Until a chain with a price settles,
 * routing is "not estimated yet".
 *
 * `netUsd` is the counted value: the low end of the range. The headline
 * adds it, so a total with routing shows "≈".
 */
export type RoutingSavings =
  | { state: "not_estimated" }
  | {
      state: "estimated";
      netUsd: number;
      lowUsd: number;
      highUsd: number;
      chains: number;
    };

export const ROUTING_NOT_ESTIMATED: RoutingSavings = { state: "not_estimated" };

/** True when a total includes a routing estimate, so the UI shows "≈". */
export function includesRoutingEstimate(routing: RoutingSavings): boolean {
  return routing.state === "estimated";
}

/** The routing state of a period's routing totals. */
export function routingSavingsFromTotals(
  totals: RoutingTotals | null | undefined,
): RoutingSavings {
  if (!totals || totals.chains === 0) return ROUTING_NOT_ESTIMATED;
  // Savings never go below zero. Storage floors each chain, and this guard
  // keeps an old report from showing a loss.
  const lowUsd = Math.max(0, totals.lowUsd);
  return {
    state: "estimated",
    netUsd: lowUsd,
    lowUsd,
    highUsd: Math.max(lowUsd, totals.highUsd),
    chains: totals.chains,
  };
}

/** The counted routing dollars: 0 when routing is not estimated. */
export function countedRoutingUsd(routing: RoutingSavings): number {
  return routing.state === "estimated" ? routing.netUsd : 0;
}

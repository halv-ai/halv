import type { TaskRoute, TaskRouteReceipt } from "./types";
import type { RoutingSavings } from "./savingsMethod.ts";

export const TASK_ROUTE_DISMISSALS_STORAGE_PREFIX =
  "halv.task-route-dismissals.v1:";

export type TaskRouteDismissal = Pick<TaskRoute, "id" | "updatedAt">;

type TaskRouteDismissalStore = {
  version: 1;
  routes: readonly TaskRouteDismissal[];
};

type TaskRouteDismissalStorage = Pick<Storage, "getItem" | "setItem">;

export type TaskRouteSummary = {
  total: number;
  active: number;
  passed: number;
  failed: number;
  escalated: number;
  terminal: number;
  belowCap: number;
};

const TIER_WEIGHTS: Record<TaskRoute["tier"], number> = {
  cheapest: 1,
  cheap: 2,
  medium: 3,
  strong: 4,
  max: 5,
};

/** Rank of a ladder tier, cheapest 1 up to max 5. */
export function tierWeight(tier: TaskRoute["tier"]): number {
  return TIER_WEIGHTS[tier] ?? 0;
}

/** Newest route first. Equal timestamps keep the input order. */
export function sortRoutesNewestFirst(routes: TaskRoute[]): TaskRoute[] {
  return routes
    .map((route, index) => ({ route, index }))
    .sort((left, right) => {
      const leftTime = Date.parse(left.route.createdAt);
      const rightTime = Date.parse(right.route.createdAt);
      if (leftTime !== rightTime) return rightTime - leftTime;
      return left.index - right.index;
    })
    .map((entry) => entry.route);
}

/** Active routes come first. Each group keeps newest-first order. */
export function sortTaskBoardRoutes(routes: TaskRoute[]): TaskRoute[] {
  return sortRoutesNewestFirst(routes).sort(
    (left, right) => Number(isTerminalTaskRoute(left)) - Number(isTerminalTaskRoute(right)),
  );
}

/** Route metadata describes an assignment, not a verified worker execution. */
export function taskRouteModelLabel(
  route: TaskRoute,
  t: (key: string, vars?: Record<string, string | number>) => string,
): string {
  if (route.tier === "max" && route.workflow) {
    return route.defaultModel
      ? t("Inherits {model}", { model: route.defaultModel })
      : t("Inherits the coordinator");
  }
  if (route.tier === "max") return t("Coordinator · {lane}", { lane: route.lane });
  return t("Suggested route: {model}", {
    model: [route.lane, route.model, route.effort].filter(Boolean).join(" · "),
  });
}

/** The escalation chain that ends at `id`, oldest route first. */
export function routeChain(routes: TaskRoute[], id: string): TaskRoute[] {
  const byId = new Map(routes.map((route) => [route.id, route]));
  const chain: TaskRoute[] = [];
  const seen = new Set<string>();
  let current = byId.get(id) ?? null;
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    chain.push(current);
    current = current.escalatedFromId
      ? (byId.get(current.escalatedFromId) ?? null)
      : null;
  }
  return chain.reverse();
}

/** Counts per status, plus the routes below the coordinator tier. */
export function summarizeRoutes(routes: TaskRoute[]): TaskRouteSummary {
  let active = 0;
  let passed = 0;
  let failed = 0;
  let escalated = 0;
  let belowCap = 0;
  for (const route of routes) {
    if (route.status === "routed") active += 1;
    if (route.status === "passed") passed += 1;
    if (route.status === "failed") failed += 1;
    if (route.status === "escalated") escalated += 1;
    // A workflow route stands for all agents of its stage.
    if (tierWeight(route.tier) < tierWeight("max")) {
      belowCap += route.workflow?.instances ?? 1;
    }
  }
  return {
    total: routes.length,
    active,
    passed,
    failed,
    escalated,
    terminal: passed + failed + escalated,
    belowCap,
  };
}

/** Passed, failed, and escalated routes no longer have active workers. */
export function isTerminalTaskRoute(route: TaskRoute): boolean {
  return route.status !== "routed";
}

export function taskRouteDismissalsStorageKey(conversationId: string): string {
  return `${TASK_ROUTE_DISMISSALS_STORAGE_PREFIX}${conversationId}`;
}

/** Reads UI-only route dismissals for one conversation. */
export function readTaskRouteDismissals(
  conversationId: string,
  storage: TaskRouteDismissalStorage | null = browserStorage(),
): TaskRouteDismissal[] {
  if (!storage) return [];
  try {
    const parsed = JSON.parse(
      storage.getItem(taskRouteDismissalsStorageKey(conversationId)) ?? "null",
    ) as Partial<TaskRouteDismissalStore> | null;
    if (!parsed || parsed.version !== 1 || !Array.isArray(parsed.routes)) return [];
    const seen = new Set<string>();
    const routes: TaskRouteDismissal[] = [];
    for (const route of parsed.routes) {
      if (
        !route ||
        typeof route.id !== "string" ||
        !route.id ||
        typeof route.updatedAt !== "string" ||
        !route.updatedAt
      ) {
        continue;
      }
      const identity = `${route.id}\u0000${route.updatedAt}`;
      if (seen.has(identity)) continue;
      seen.add(identity);
      routes.push({ id: route.id, updatedAt: route.updatedAt });
    }
    return routes;
  } catch {
    return [];
  }
}

/** Saves UI-only dismissals without changing stored routes or savings. */
export function writeTaskRouteDismissals(
  conversationId: string,
  routes: readonly TaskRouteDismissal[],
  storage: TaskRouteDismissalStorage | null = browserStorage(),
): void {
  if (!storage) return;
  try {
    storage.setItem(
      taskRouteDismissalsStorageKey(conversationId),
      JSON.stringify({ version: 1, routes } satisfies TaskRouteDismissalStore),
    );
  } catch {
    // Restricted webviews can reject storage. The in-memory dismissal still wins.
  }
}

/** Adds terminal route identities to the existing dismissal list. */
export function addTaskRouteDismissals(
  dismissed: readonly TaskRouteDismissal[],
  routes: readonly TaskRoute[],
  routeIds: ReadonlySet<string>,
): TaskRouteDismissal[] {
  const next = new Map(
    dismissed.map((route) => [`${route.id}\u0000${route.updatedAt}`, route]),
  );
  for (const route of routes) {
    if (!routeIds.has(route.id) || !isTerminalTaskRoute(route)) continue;
    next.set(`${route.id}\u0000${route.updatedAt}`, {
      id: route.id,
      updatedAt: route.updatedAt,
    });
  }
  return [...next.values()];
}

/** Hides only the exact route revisions the user dismissed. */
export function visibleTaskRoutes(
  routes: readonly TaskRoute[],
  dismissed: readonly TaskRouteDismissal[],
): TaskRoute[] {
  const hidden = new Set(
    dismissed.map((route) => `${route.id}\u0000${route.updatedAt}`),
  );
  return routes.filter(
    (route) => !hidden.has(`${route.id}\u0000${route.updatedAt}`),
  );
}

/** Header savings text: how many routes ran below the coordinator tier. */
export function savedLabel(
  routes: TaskRoute[],
  t: (key: string, vars?: Record<string, string | number>) => string,
): string {
  const { belowCap } = summarizeRoutes(routes);
  if (belowCap === 0) return t("no savings yet");
  return t("{count} tasks below the coordinator tier", { count: belowCap });
}

/**
 * Header savings text in dollars (ADR 0016): the low estimate of the settled
 * chains, or "not estimated yet" before the first chain with a price.
 */
export function routingSavingsLabel(
  routing: RoutingSavings,
  t: (key: string, vars?: Record<string, string | number>) => string,
  formatAmount: (value: number) => string,
): string {
  if (routing.state === "estimated") {
    // Savings never go below zero (ADR 0016).
    const label = t("\u2248 {amount} saved by routing", {
      amount: formatAmount(Math.max(0, routing.netUsd)),
    });
    // Keep "≈" on the line of its amount.
    return label.replace("\u2248 ", "\u2248\u00a0");
  }
  return t("Routing savings not estimated yet");
}

function browserStorage(): TaskRouteDismissalStorage | null {
  return typeof window === "undefined" ? null : window.localStorage;
}

/** One entry of the task board: a route, or a Workflow batch and its routes. */
export type TaskBoardEntry =
  | { kind: "route"; route: TaskRoute }
  | {
      kind: "batch";
      batchId: string;
      name: string | null;
      routes: TaskRoute[];
    };

/**
 * Groups the routes of each `route_tasks` batch (ADR 0014). A batch takes
 * the place of its first route, and its routes keep their order.
 */
export function groupTaskBoardRoutes(routes: readonly TaskRoute[]): TaskBoardEntry[] {
  const entries: TaskBoardEntry[] = [];
  const batches = new Map<string, Extract<TaskBoardEntry, { kind: "batch" }>>();
  for (const route of routes) {
    const workflow = route.workflow;
    if (!workflow) {
      entries.push({ kind: "route", route });
      continue;
    }
    let batch = batches.get(workflow.batchId);
    if (!batch) {
      batch = {
        kind: "batch",
        batchId: workflow.batchId,
        name: workflow.name,
        routes: [],
      };
      batches.set(workflow.batchId, batch);
      entries.push(batch);
    }
    batch.routes.push(route);
  }
  return entries;
}

/** The agents of a batch. A retry route counts no new stage. */
export function batchAgentCount(routes: readonly TaskRoute[]): number {
  return routes
    .filter((route) => route.escalatedFromId === null)
    .reduce((sum, route) => sum + (route.workflow?.instances ?? 1), 0);
}

const CLAUDE_FAMILIES = ["haiku", "sonnet", "opus", "fable"] as const;

function claudeFamily(model: string | null | undefined): string | null {
  const normalized = model?.toLowerCase() ?? "";
  return CLAUDE_FAMILIES.find((family) => normalized.includes(family)) ?? null;
}

/**
 * Whether the measured agents ran on the routed model family. `null` when
 * Halv has no measured model for the route.
 */
export function workflowRouteApplied(route: TaskRoute): boolean | null {
  const models = route.receipt?.models ?? [];
  if (!route.workflow || models.length === 0) return null;
  const expected = claudeFamily(
    route.tier === "max" ? route.defaultModel : route.model,
  );
  if (expected === null) return null;
  return models.every((model) => claudeFamily(model) === expected);
}

/** All tokens of a receipt, or `null` when the receipt has no usage. */
export function receiptTokens(receipt: TaskRouteReceipt): number | null {
  const parts = [
    receipt.inputTokens,
    receipt.outputTokens,
    receipt.cacheReadTokens,
    receipt.cacheCreationTokens,
  ];
  if (parts.every((part) => part === null)) return null;
  return parts.reduce<number>((sum, part) => sum + (part ?? 0), 0);
}

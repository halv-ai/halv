import { useEffect, useState, useSyncExternalStore } from "react";
import { accountStatusStore } from "../gate";
import { cloudAccess } from "../halvCloud.ts";
import { formatMoney, useT, type Translate } from "../i18n/runtime";
import {
  routingSavingsFromTotals,
  type RoutingSavings,
} from "../savingsMethod.ts";
import { ipc } from "../ipc.ts";
import { formatCompactTokens } from "../format.ts";
import {
  addTaskRouteDismissals,
  batchAgentCount,
  groupTaskBoardRoutes,
  isTerminalTaskRoute,
  receiptTokens,
  readRoutingCloudCardDismissed,
  readTaskRouteDismissals,
  routingSavingsLabel,
  savedLabel,
  sortTaskBoardRoutes,
  splitHalvCloudFallbackRoutes,
  taskRouteModelLabel,
  taskRouteReasonLabel,
  summarizeRoutes,
  visibleTaskRoutes,
  workflowRouteApplied,
  writeRoutingCloudCardDismissed,
  writeTaskRouteDismissals,
  type TaskRouteDismissal,
} from "../taskBoard.ts";
import { HalvCloudCard } from "./HalvCloudCard";
import type {
  AccountAuthStatus,
  RouteCost,
  RoutingSavingsReport,
  TaskRoute,
  TaskRouteStatus,
} from "../types";

const REFRESH_INTERVAL_MS = 2_000;

const STATUS_DOT_CLASS: Record<TaskRouteStatus, string> = {
  routed: "bg-accent",
  // The theme has no success token, so a passed route uses the strong accent.
  passed: "bg-accent-strong",
  failed: "bg-danger",
  escalated: "bg-warning",
};

type TaskBoardProps = {
  conversationId: string | null;
  refreshSignal?: number;
  /**
   * The routing dollars of the JEV estimator (ADR 0016). Without it, the
   * board reads the settled chains of the conversation.
   */
  routingSavings?: RoutingSavings;
  /** Opens the Halv sign-in flow from the Halv Cloud card. */
  onSignIn?: () => void;
  onAccountStatusChange?: (status: AccountAuthStatus) => void;
};

type RouteSnapshot = {
  conversationId: string | null;
  routes: TaskRoute[];
  report: RoutingSavingsReport | null;
};

type DismissalSnapshot = {
  conversationId: string | null;
  routes: TaskRouteDismissal[];
};

/** Lists every route Halv made for the current conversation. */
export function TaskBoard({
  conversationId,
  refreshSignal,
  routingSavings: routingSavingsOverride,
  onSignIn,
  onAccountStatusChange,
}: TaskBoardProps): JSX.Element | null {
  const t = useT();
  const account = useSyncExternalStore(
    accountStatusStore.subscribe,
    accountStatusStore.getSnapshot,
    accountStatusStore.getSnapshot,
  );
  const [cloudCardDismissed, setCloudCardDismissed] = useState(
    readRoutingCloudCardDismissed,
  );
  const [routeSnapshot, setRouteSnapshot] = useState<RouteSnapshot>({
    conversationId: null,
    routes: [],
    report: null,
  });
  const [dismissalSnapshot, setDismissalSnapshot] =
    useState<DismissalSnapshot>({ conversationId: null, routes: [] });
  const [open, setOpen] = useState(false);

  useEffect(() => {
    setOpen(false);
    setDismissalSnapshot({
      conversationId,
      routes: conversationId ? readTaskRouteDismissals(conversationId) : [],
    });
  }, [conversationId]);

  useEffect(() => {
    if (!conversationId) {
      setRouteSnapshot({ conversationId: null, routes: [], report: null });
      return;
    }
    let disposed = false;
    let latestRequest = 0;
    setRouteSnapshot({ conversationId, routes: [], report: null });
    const load = async () => {
      const request = ++latestRequest;
      try {
        const [next, report] = await Promise.all([
          ipc.storage.listTaskRoutes({ conversationId }),
          // Routing dollars are optional: an older backend has no command.
          ipc.storage.routingSavings({ conversationId }).catch(() => null),
        ]);
        if (!disposed && request === latestRequest) {
          setRouteSnapshot({ conversationId, routes: next, report });
        }
      } catch (error) {
        if (!disposed && request === latestRequest) {
          console.error("Task route list failed", error);
        }
      }
    };
    void load();
    const interval = window.setInterval(() => void load(), REFRESH_INTERVAL_MS);
    return () => {
      disposed = true;
      window.clearInterval(interval);
    };
  }, [conversationId, refreshSignal]);

  const routes =
    routeSnapshot.conversationId === conversationId ? routeSnapshot.routes : [];
  const report =
    routeSnapshot.conversationId === conversationId ? routeSnapshot.report : null;
  const routingSavings =
    routingSavingsOverride ?? routingSavingsFromTotals(report?.totals);
  const costById = new Map(
    (report?.routeCosts ?? []).map((cost) => [cost.routeId, cost]),
  );
  const dismissals =
    dismissalSnapshot.conversationId === conversationId
      ? dismissalSnapshot.routes
      : [];
  // A route without Halv Cloud access is no real route: the coordinator did
  // the task itself (ADR 0021). The board lists only real routes. A fallback
  // route is the point of use, so the board shows the Halv Cloud card once.
  const { routed: visibleRoutes, fallbackCount } = splitHalvCloudFallbackRoutes(
    visibleTaskRoutes(routes, dismissals),
  );
  const access = cloudAccess(account);
  const cloudCard =
    conversationId &&
    fallbackCount > 0 &&
    !access.granted &&
    !cloudCardDismissed &&
    onSignIn &&
    onAccountStatusChange ? (
      <aside aria-label={t("Halv Cloud")} className="task-board task-board-open p-3">
        <HalvCloudCard
          access={access}
          account={account}
          feature="routing"
          onDismiss={() => {
            writeRoutingCloudCardDismissed();
            setCloudCardDismissed(true);
          }}
          onSignIn={onSignIn}
          onStatusChange={onAccountStatusChange}
          placement="inline"
        />
      </aside>
    ) : null;

  if (!conversationId || visibleRoutes.length === 0) return cloudCard;

  const ordered = sortTaskBoardRoutes(visibleRoutes);
  const summary = summarizeRoutes(visibleRoutes);
  const collapsedSummary = [
    summary.active > 0 ? `${summary.active} ${t("Active")}` : null,
    summary.passed > 0 ? `${summary.passed} ${t("Completed")}` : null,
    summary.failed > 0 ? `${summary.failed} ${t("Failed")}` : null,
    summary.escalated > 0
      ? `${summary.escalated} ${t("Escalated")}`
      : null,
  ]
    .filter((label): label is string => label !== null)
    .join(" · ");
  const collapsedAriaLabel = `${t("Open task routing")} · ${collapsedSummary}`;
  const titleById = new Map(routes.map((route) => [route.id, route.title]));

  const dismissRoutes = (routeIds: ReadonlySet<string>) => {
    setDismissalSnapshot((current) => {
      if (!conversationId) return current;
      const existing =
        current.conversationId === conversationId
          ? current.routes
          : readTaskRouteDismissals(conversationId);
      const next = addTaskRouteDismissals(existing, routes, routeIds);
      writeTaskRouteDismissals(conversationId, next);
      return { conversationId, routes: next };
    });
  };

  if (!open) {
    return (
      <aside aria-label={t("Task routing")} className="task-board task-board-collapsed">
        <button
          aria-expanded="false"
          aria-label={collapsedAriaLabel}
          className="task-board-edge-button"
          onClick={() => setOpen(true)}
          title={collapsedAriaLabel}
          type="button"
        >
          <span className="task-board-edge-title">{t("Task routing")}</span>
          <strong>{summary.active}</strong>
          <span aria-hidden className="task-board-edge-summary">
            <span>{collapsedSummary}</span>
          </span>
        </button>
      </aside>
    );
  }

  return (
    <aside aria-label={t("Task routing")} className="task-board task-board-open">
      <header className="task-board-header">
        <h4>
          {t("Task routing · {count} tasks · {saved}", {
            count: ordered.length,
            saved: routingSavingsLabel(routingSavings, t, formatMoney),
          })}
        </h4>
        <div className="task-board-actions">
          {summary.terminal > 0 ? (
            <button
              className="task-board-close-all"
              onClick={() => dismissRoutes(new Set(visibleRoutes.filter(isTerminalTaskRoute).map((route) => route.id)))}
              title={t("Dismiss finished tasks")}
              type="button"
            >
              {t("Close all")}
            </button>
          ) : null}
          <button
            aria-expanded="true"
            aria-label={t("Collapse task routing")}
            className="task-board-close-button"
            onClick={() => setOpen(false)}
            title={t("Collapse task routing")}
            type="button"
          >
            ›
          </button>
        </div>
      </header>
      <p className="task-board-tier-summary">{savedLabel(routes, t)}</p>
      {routingSavings.state === "estimated" ? (
        <p className="task-board-tier-summary">
          {t("Range {low} to {high}", {
            low: formatMoney(routingSavings.lowUsd),
            high: formatMoney(routingSavings.highUsd),
          })}
        </p>
      ) : null}
      <ul className="task-board-list">
        {groupTaskBoardRoutes(ordered).map((entry) => {
          const row = (route: TaskRoute) => (
            <TaskBoardRow
              cost={costById.get(route.id) ?? null}
              key={route.id}
              escalatedFromTitle={
                route.escalatedFromId
                  ? (titleById.get(route.escalatedFromId) ?? null)
                  : null
              }
              onDismiss={() => dismissRoutes(new Set([route.id]))}
              route={route}
              t={t}
            />
          );
          if (entry.kind === "route") return row(entry.route);
          const count = batchAgentCount(entry.routes);
          return (
            <li className="task-board-batch" key={entry.batchId}>
              <span className="task-board-batch-title">
                {entry.name
                  ? t("Workflow · {name} · {count} agents", {
                      name: entry.name,
                      count,
                    })
                  : t("Workflow · {count} agents", { count })}
              </span>
              <ul className="task-board-batch-list">{entry.routes.map(row)}</ul>
            </li>
          );
        })}
      </ul>
    </aside>
  );
}

function TaskBoardRow({
  cost,
  escalatedFromTitle,
  onDismiss,
  route,
  t,
}: {
  cost: RouteCost | null;
  escalatedFromTitle: string | null;
  onDismiss: () => void;
  route: TaskRoute;
  t: Translate;
}) {
  const terminal = isTerminalTaskRoute(route);
  const workflow = route.workflow ?? null;
  const tokens = route.receipt ? receiptTokens(route.receipt) : null;
  const applied = workflowRouteApplied(route);
  return (
    <li className="task-board-row">
      <span
        aria-hidden
        className={`task-board-status h-2 w-2 shrink-0 rounded-full ${STATUS_DOT_CLASS[route.status]}`}
      />
      <span className="task-board-title text-content-primary">{route.title}</span>
      <span className="task-board-status-label" data-status={route.status}>
        {statusLabel(route.status, t)}
      </span>
      {terminal ? (
        <button
          aria-label={
            route.status === "passed"
              ? t("Dismiss completed task {title}", { title: route.title })
              : t("Dismiss finished task {title}", { title: route.title })
          }
          className="task-board-row-dismiss"
          onClick={onDismiss}
          title={
            route.status === "passed"
              ? t("Dismiss completed task {title}", { title: route.title })
              : t("Dismiss finished task {title}", { title: route.title })
          }
          type="button"
        >
          ×
        </button>
      ) : null}
      <span className="rounded border border-surface-border/30 px-1.5 py-0.5 text-xs uppercase tracking-wide text-content-secondary">
        {route.tier}
      </span>
      <span className="text-xs text-content-secondary">
        {taskRouteModelLabel(route, t)}
      </span>
      {workflow ? (
        <span className="task-board-workflow-key">
          {workflow.instances > 1
            ? `${workflow.key} · ${workflow.role} ×${workflow.instances}`
            : `${workflow.key} · ${workflow.role}`}
        </span>
      ) : null}
      {route.receipt && tokens !== null ? (
        <span className="text-xs text-content-secondary">
          {route.receipt.usageSource === "measured"
            ? t("{tokens} tokens measured", {
                tokens: formatCompactTokens(tokens),
              })
            : t("{tokens} tokens reported", {
                tokens: formatCompactTokens(tokens),
              })}
        </span>
      ) : null}
      {cost ? (
        <span className="text-xs text-content-secondary">
          {t("Cost {amount} on {model}", {
            amount: formatMoney(cost.actualUsd),
            model: cost.models.join(", "),
          })}
        </span>
      ) : null}
      {applied === false ? (
        <span
          className="task-board-not-applied"
          title={t("The agent ran on {models}, not on the routed model.", {
            models: route.receipt?.models.join(", ") ?? "",
          })}
        >
          {t("Not applied")}
        </span>
      ) : null}
      {route.reasons.map((reason) => (
        <span
          className="rounded bg-surface-base px-1.5 py-0.5 text-xs text-content-secondary"
          key={reason}
        >
          {taskRouteReasonLabel(reason, t)}
        </span>
      ))}
      {escalatedFromTitle ? (
        <span className="text-xs text-content-secondary">
          {t("escalated from {title} →", { title: escalatedFromTitle })}
        </span>
      ) : null}
    </li>
  );
}

function statusLabel(status: TaskRouteStatus, t: Translate): string {
  if (status === "passed") return t("Completed");
  if (status === "failed") return t("Failed");
  if (status === "escalated") return t("Escalated");
  return t("Running");
}

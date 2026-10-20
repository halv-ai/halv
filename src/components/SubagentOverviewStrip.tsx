import { useMemo } from "react";
import { useT, type Translate } from "../i18n/runtime";
import { subagentActivities } from "../subagents";
import type { SubagentThread, ToolCallUi } from "../types";

type SubagentOverviewStripProps = {
  threads?: SubagentThread[];
  toolCalls: ToolCallUi[];
  onExpand: () => void;
  onStopAll: () => void;
};

export function SubagentOverviewStrip({
  threads = [],
  toolCalls,
  onExpand,
  onStopAll,
}: SubagentOverviewStripProps) {
  const t = useT();
  const activities = useMemo(
    () => subagentActivities(toolCalls, threads),
    [threads, toolCalls],
  );
  const activeCount = activities.filter((activity) => activity.active).length;
  const failedCount = activities.filter((activity) => activity.failed).length;

  if (activeCount === 0) {
    return null;
  }

  const summary = subagentSummaryLabel(activeCount, failedCount, t);

  return (
    <div
      className="relative mx-auto mb-2 flex w-[calc(100%-2.5rem)] max-w-5xl flex-none cursor-pointer items-center justify-between gap-3 rounded-full border border-surface-border/25 bg-surface-raised/55 px-3 py-1.5 text-xs text-content-secondary shadow-sm transition-colors hover:bg-surface-raised"
    >
      <button
        aria-label={subagentSummaryAriaLabel(activeCount, failedCount, t)}
        className="absolute inset-0 z-0 rounded-full focus:outline-none focus:ring-2 focus:ring-accent/30"
        onClick={onExpand}
        type="button"
      />
      <span className="pointer-events-none relative z-10 flex min-w-0 items-center gap-2">
        <span
          aria-hidden
          className="h-2 w-2 shrink-0 animate-pulse rounded-full bg-warning"
        />
        <span className="truncate font-medium">{summary}</span>
      </span>
      <span className="relative z-10 flex flex-none items-center gap-1.5">
        <button
          className="rounded-md px-2 py-1 text-[11px] text-content-muted transition-colors hover:bg-surface-panel hover:text-content-secondary focus:outline-none focus:ring-2 focus:ring-accent/30"
          onClick={(event) => {
            event.stopPropagation();
            onExpand();
          }}
          type="button"
        >
          {t("View")}
        </button>
        <button
          className="rounded-md border border-danger/35 px-2 py-1 text-[11px] text-danger transition-colors hover:bg-danger-surface/60"
          onClick={(event) => {
            event.stopPropagation();
            onStopAll();
          }}
          type="button"
        >
          {t("Stop all")}
        </button>
      </span>
    </div>
  );
}

function subagentSummaryLabel(
  activeCount: number,
  failedCount: number,
  t: Translate,
) {
  if (failedCount > 0) {
    return activeCount === 1
      ? t("{activeCount} agent running, {failedCount} failed", {
          activeCount,
          failedCount,
        })
      : t("{activeCount} agents running, {failedCount} failed", {
          activeCount,
          failedCount,
        });
  }
  return activeCount === 1
    ? t("{activeCount} agent running", { activeCount })
    : t("{activeCount} agents running", { activeCount });
}

function subagentSummaryAriaLabel(
  activeCount: number,
  failedCount: number,
  t: Translate,
) {
  if (failedCount > 0) {
    return activeCount === 1
      ? t(
          "View subagent activity: {activeCount} agent running, {failedCount} failed",
          { activeCount, failedCount },
        )
      : t(
          "View subagent activity: {activeCount} agents running, {failedCount} failed",
          { activeCount, failedCount },
        );
  }
  return activeCount === 1
    ? t("View subagent activity: {activeCount} agent running", {
        activeCount,
      })
    : t("View subagent activity: {activeCount} agents running", {
        activeCount,
      });
}

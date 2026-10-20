// The resource chip at the right end of the status bar: the memory of Halv
// plus its terminals, and the count of open terminals. The popover lists
// each process tree. Orca's ResourceUsageStatusSegment (stablyai/orca, MIT)
// has the same shape.
import { useEffect, useId, useRef, useState } from "react";

import { useT } from "../i18n/runtime";
import { ipc } from "../ipc.ts";
import {
  RESOURCE_USAGE_OPEN_POLL_MS,
  RESOURCE_USAGE_POLL_MS,
  formatMemoryBytes,
  resourceUsageRows,
  runningResourceSessions,
  type ResourceUsageSession,
} from "../resourceUsage";
import type { ResourceUsageSnapshot } from "../types";

type ResourceUsageSegmentProps = {
  terminalSessions: readonly ResourceUsageSession[];
  compact: boolean;
};

export function ResourceUsageSegment({
  terminalSessions,
  compact,
}: ResourceUsageSegmentProps) {
  const t = useT();
  const popoverId = useId();
  const [open, setOpen] = useState(false);
  const [snapshot, setSnapshot] = useState<ResourceUsageSnapshot | null>(null);
  const [failed, setFailed] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    const poll = async () => {
      if (document.visibilityState === "hidden") {
        schedule();
        return;
      }
      try {
        const next = await ipc.diagnostics.resourceUsage();
        if (disposed) return;
        setSnapshot(next);
        setFailed(false);
      } catch {
        if (disposed) return;
        setFailed(true);
      }
      schedule();
    };
    const schedule = () => {
      if (disposed) return;
      timer = window.setTimeout(
        () => void poll(),
        open ? RESOURCE_USAGE_OPEN_POLL_MS : RESOURCE_USAGE_POLL_MS,
      );
    };
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      window.clearTimeout(timer);
      void poll();
    };
    document.addEventListener("visibilitychange", onVisible);
    void poll();
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (!target) return;
      if (popoverRef.current?.contains(target)) return;
      if (buttonRef.current?.contains(target)) return;
      setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setOpen(false);
      buttonRef.current?.focus();
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const running = runningResourceSessions(terminalSessions);
  const count = running.length;
  const totalLabel = snapshot ? formatMemoryBytes(snapshot.totalBytes) : "—";
  const countLabel =
    count === 1 ? t("1 terminal") : t("{count} terminals", { count });
  const rows = resourceUsageRows(snapshot, running, { app: t("Halv app") });

  return (
    <>
      <button
        aria-controls={popoverId}
        aria-expanded={open}
        aria-label={t("Memory {memory}, {terminals}", {
          memory: totalLabel,
          terminals: countLabel,
        })}
        className={`inline-flex h-5 shrink-0 items-center gap-1.5 rounded px-1 font-mono text-[11px] tabular-nums transition-colors hover:bg-surface-raised focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 ${
          failed ? "text-warning" : "text-content-secondary"
        }`}
        data-resource-usage-segment={failed ? "failed" : snapshot ? "ready" : "loading"}
        onClick={() => setOpen((current) => !current)}
        ref={buttonRef}
        title={
          failed
            ? t("Could not read the memory of Halv.")
            : t("Memory of Halv and its terminals. Click for details.")
        }
        type="button"
      >
        <MemoryIcon />
        <span>{totalLabel}</span>
        {compact ? null : (
          <>
            <span aria-hidden className="text-content-muted">
              {"·"}
            </span>
            <span>{countLabel}</span>
          </>
        )}
      </button>
      {open ? (
        <div
          aria-label={t("Resource usage")}
          className="fixed bottom-8 right-2 z-[60] flex max-h-[calc(100vh-5rem)] w-[300px] max-w-[calc(100vw-1rem)] flex-col overflow-hidden whitespace-normal rounded-lg border border-surface-border/30 bg-surface-panel text-content-primary shadow-2xl shadow-black/25 outline-none"
          data-resource-usage-popover
          id={popoverId}
          ref={popoverRef}
          role="dialog"
          tabIndex={-1}
        >
          <header className="flex items-center gap-2 border-b border-surface-border/25 px-3 py-2">
            <span className="min-w-0 flex-1 truncate text-xs font-semibold">
              {t("Resource usage")}
            </span>
            <span className="shrink-0 font-mono text-[10px] tabular-nums text-content-muted">
              {totalLabel}
            </span>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto px-3 py-2">
            {failed ? (
              <p className="text-[10px] leading-relaxed text-warning">
                {t("Could not read the memory of Halv.")}
              </p>
            ) : null}
            <ul className="space-y-1">
              {rows.map((row) => (
                <li
                  className="flex items-center gap-2 text-[11px]"
                  data-resource-usage-row={row.kind}
                  key={row.id}
                >
                  <span
                    aria-hidden
                    className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${
                      row.kind === "app" ? "bg-accent" : "bg-content-muted/70"
                    }`}
                  />
                  <span className="min-w-0 flex-1 truncate text-content-primary">
                    {row.label}
                  </span>
                  <span className="shrink-0 font-mono text-[10px] tabular-nums text-content-secondary">
                    {row.bytes === null ? "—" : formatMemoryBytes(row.bytes)}
                  </span>
                </li>
              ))}
            </ul>
            {count === 0 ? (
              <p className="mt-1.5 text-[10px] leading-relaxed text-content-muted">
                {t("No terminals open")}
              </p>
            ) : null}
          </div>
          <footer className="border-t border-surface-border/25 px-3 py-1.5 text-[10px] text-content-muted">
            {t("Resident memory of each process tree. Terminals include the agents they run.")}
          </footer>
        </div>
      ) : null}
    </>
  );
}

function MemoryIcon() {
  return (
    <svg
      aria-hidden
      className="h-3 w-3 shrink-0"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth={1.8}
      viewBox="0 0 24 24"
    >
      <rect height="12" rx="2" width="18" x="3" y="6" />
      <path d="M7 10v4M11 10v4M15 10v4M19 10v4" />
    </svg>
  );
}

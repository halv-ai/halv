import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { strongestGlyph } from "../agentPaneStatus.ts";
import {
  attentionFinished,
  dockRows,
  queueDockRowLimit,
  queuePopoverPosition,
  QUEUE_POPOVER_WIDTH_PX,
} from "../attentionQueue.ts";
import { useT } from "../i18n/runtime";
import { shortAge, shortAgeLabel } from "../shortAge.ts";
import type { AttentionQueueRow } from "../useAttentionQueue.ts";
import { AgentStateGlyph } from "./AgentStateGlyph";

// The attention queue UI. The dock sits at the bottom of the expanded
// sidebar. A chip in the status bar takes its place while the sidebar is
// collapsed. Both show nothing for an empty queue.

const CLOCK_TICK_MS = 30_000;

type QueueActions = {
  onOpen: (row: AttentionQueueRow) => void;
  onMarkRead: (row: AttentionQueueRow) => void;
  onMarkAllRead: () => void;
};

type QueueProps = QueueActions & {
  rows: readonly AttentionQueueRow[];
};

function useClock(intervalMs: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(interval);
  }, [intervalMs]);
  return now;
}

function useWindowHeight() {
  const [height, setHeight] = useState(() => window.innerHeight);
  useEffect(() => {
    const onResize = () => setHeight(window.innerHeight);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return height;
}

function rowSummary(row: AttentionQueueRow) {
  return row.detail ?? row.heading;
}

function QueueRow({
  row,
  head,
  now,
  variant,
  onOpen,
  onMarkRead,
}: {
  row: AttentionQueueRow;
  head: boolean;
  now: number;
  variant: "dock" | "popover";
  onOpen: (row: AttentionQueueRow) => void;
  onMarkRead: (row: AttentionQueueRow) => void;
}) {
  const t = useT();
  const finished = attentionFinished(row.kind);
  const age = shortAgeLabel(shortAge(row.enteredAt, now), t);
  return (
    <li
      className="group relative"
      data-attention-head={head ? "" : undefined}
      data-attention-kind={row.kind}
      data-attention-row={row.key}
    >
      {head ? (
        <span
          aria-hidden="true"
          className="pointer-events-none absolute inset-y-1 left-0 w-0.5 rounded-full bg-accent"
          data-attention-accent=""
        />
      ) : null}
      <button
        aria-label={`${row.title} · ${row.heading}`}
        className="flex w-full min-w-0 items-center gap-2 rounded-md py-1.5 pl-2.5 pr-2 text-left transition-colors hover:bg-surface-raised focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
        onClick={() => onOpen(row)}
        title={`${row.title} · ${rowSummary(row)}`}
        type="button"
      >
        <AgentStateGlyph glyph={row.glyph} label={row.heading} size="sm" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[13px] leading-5 text-content-primary">
            {row.title}
          </span>
          {variant === "popover" ? (
            <span className="block truncate text-[11px] leading-4 text-content-muted">
              {rowSummary(row)}
            </span>
          ) : null}
        </span>
        <span
          className={`shrink-0 font-mono text-[10px] tabular-nums text-content-muted transition-opacity ${
            // The mark button takes the place of the age on a finished row.
            finished ? "group-focus-within:opacity-0 group-hover:opacity-0" : ""
          }`}
          data-attention-age=""
        >
          {age}
        </span>
      </button>
      {finished ? (
        <button
          aria-label={t("Mark as read")}
          className="absolute right-1.5 top-1/2 flex h-5 w-5 -translate-y-1/2 items-center justify-center rounded text-[11px] text-content-muted opacity-0 transition-opacity hover:bg-surface-sunken hover:text-content-primary focus:outline-none focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-accent/40 group-hover:opacity-100"
          data-attention-mark-read=""
          onClick={() => onMarkRead(row)}
          title={t("Mark as read")}
          type="button"
        >
          <span aria-hidden="true">{"✓"}</span>
        </button>
      ) : null}
    </li>
  );
}

function QueuePopover({
  anchor,
  id,
  now,
  onClose,
  popoverRef,
  rows,
  onOpen,
  onMarkRead,
  onMarkAllRead,
}: QueueActions & {
  anchor: ReturnType<typeof queuePopoverPosition>;
  id: string;
  now: number;
  onClose: () => void;
  popoverRef: RefObject<HTMLDivElement>;
  rows: readonly AttentionQueueRow[];
}) {
  const t = useT();
  const finishedCount = rows.filter((row) => attentionFinished(row.kind)).length;
  return createPortal(
    <div
      aria-label={t("{count} in queue", { count: rows.length })}
      className="fixed z-[60] flex w-[360px] max-w-[calc(100vw-1rem)] flex-col overflow-hidden whitespace-normal rounded-lg border border-surface-border/30 bg-surface-panel text-content-primary shadow-2xl shadow-black/25 outline-none"
      data-attention-queue-popover=""
      id={id}
      ref={popoverRef}
      role="dialog"
      style={{ left: anchor.left, bottom: anchor.bottom, maxHeight: anchor.maxHeight }}
      tabIndex={-1}
    >
      <header className="flex items-baseline gap-2 border-b border-surface-border/25 px-3 py-2">
        <span className="min-w-0 flex-1 truncate text-xs font-semibold">
          {t("{count} in queue", { count: rows.length })}
        </span>
        <span className="shrink-0 text-[10px] text-content-muted">
          {t("Oldest first")}
        </span>
      </header>
      <ul className="min-h-0 flex-1 overflow-y-auto p-1.5" data-attention-list="popover">
        {rows.map((row, index) => (
          <QueueRow
            head={index === 0}
            key={row.key}
            now={now}
            onMarkRead={onMarkRead}
            onOpen={(opened) => {
              onClose();
              onOpen(opened);
            }}
            row={row}
            variant="popover"
          />
        ))}
      </ul>
      {finishedCount >= 2 ? (
        <footer className="border-t border-surface-border/25 px-3 py-1.5">
          <button
            className="rounded px-1 py-0.5 text-[11px] text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
            data-attention-mark-all=""
            onClick={() => {
              onMarkAllRead();
              onClose();
            }}
            type="button"
          >
            {t("Mark all as read")}
          </button>
        </footer>
      ) : null}
    </div>,
    document.body,
  );
}

// The popover opens above its trigger. Outside clicks and Escape close it.
// A resize keeps it over the trigger.
function useQueuePopover(trigger: RefObject<HTMLElement>) {
  const id = useId();
  const popoverRef = useRef<HTMLDivElement>(null);
  const [anchor, setAnchor] = useState<ReturnType<typeof queuePopoverPosition> | null>(
    null,
  );
  const open = anchor !== null;

  const measure = useCallback(() => {
    const element = trigger.current;
    if (!element) return null;
    const rect = element.getBoundingClientRect();
    return queuePopoverPosition(
      { left: rect.left, top: rect.top },
      { width: window.innerWidth, height: window.innerHeight },
      QUEUE_POPOVER_WIDTH_PX,
    );
  }, [trigger]);

  const close = useCallback(
    (restoreFocus = false) => {
      setAnchor(null);
      if (restoreFocus) trigger.current?.focus();
    },
    [trigger],
  );

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (!target) return;
      if (popoverRef.current?.contains(target)) return;
      if (trigger.current?.contains(target)) return;
      setAnchor(null);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      close(true);
    };
    const onResize = () => setAnchor(measure());
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("resize", onResize);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("resize", onResize);
    };
  }, [close, measure, open, trigger]);

  useEffect(() => {
    if (open) popoverRef.current?.focus();
  }, [open]);

  return {
    id,
    popoverRef,
    anchor,
    open,
    close,
    toggle: () => setAnchor((current) => (current ? null : measure())),
  };
}

/** The dock at the bottom of the expanded sidebar. */
export function AttentionQueueDock({
  rows,
  onOpen,
  onMarkRead,
  onMarkAllRead,
}: QueueProps) {
  const t = useT();
  const now = useClock(CLOCK_TICK_MS);
  const height = useWindowHeight();
  const moreRef = useRef<HTMLButtonElement>(null);
  const popover = useQueuePopover(moreRef);
  const { rows: shown, more } = dockRows(rows, queueDockRowLimit(height));
  // The queue can empty while its popover is open. Nothing stays on screen.
  if (rows.length === 0) return null;
  return (
    <section
      aria-label={t("{count} in queue", { count: rows.length })}
      className="flex-none border-t border-surface-border/25 px-3 pb-3 pt-2"
      data-attention-queue="dock"
    >
      <h2
        className="px-1 pb-1 font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted"
        data-attention-title=""
      >
        {t("{count} in queue", { count: rows.length })}
      </h2>
      <ul className="space-y-0.5" data-attention-list="dock">
        {shown.map((row, index) => (
          <QueueRow
            head={index === 0}
            key={row.key}
            now={now}
            onMarkRead={onMarkRead}
            onOpen={onOpen}
            row={row}
            variant="dock"
          />
        ))}
      </ul>
      {more > 0 ? (
        <button
          aria-controls={popover.open ? popover.id : undefined}
          aria-expanded={popover.open}
          aria-haspopup="dialog"
          className="mt-0.5 w-full rounded-md px-2.5 py-1 text-left text-[11px] text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
          data-attention-more=""
          onClick={popover.toggle}
          ref={moreRef}
          type="button"
        >
          {t("+{count} more", { count: more })}
        </button>
      ) : null}
      {popover.anchor ? (
        <QueuePopover
          anchor={popover.anchor}
          id={popover.id}
          now={now}
          onClose={() => popover.close()}
          onMarkAllRead={onMarkAllRead}
          onMarkRead={onMarkRead}
          onOpen={onOpen}
          popoverRef={popover.popoverRef}
          rows={rows}
        />
      ) : null}
    </section>
  );
}

/** The chip at the left of the status bar, for a collapsed sidebar. */
export function AttentionQueueChip({
  rows,
  onOpen,
  onMarkRead,
  onMarkAllRead,
}: QueueProps) {
  const t = useT();
  const now = useClock(CLOCK_TICK_MS);
  const chipRef = useRef<HTMLButtonElement>(null);
  const popover = useQueuePopover(chipRef);
  if (rows.length === 0) return null;
  const glyph = strongestGlyph(rows.map((row) => row.glyph));
  const label = t("{count} in queue", { count: rows.length });
  return (
    <>
      <button
        aria-controls={popover.open ? popover.id : undefined}
        aria-expanded={popover.open}
        aria-haspopup="dialog"
        className={`inline-flex h-5 shrink-0 items-center gap-1.5 rounded px-1 text-content-primary transition-colors hover:bg-surface-raised focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 ${
          popover.open ? "bg-surface-raised" : ""
        }`}
        data-attention-queue="chip"
        onClick={popover.toggle}
        ref={chipRef}
        title={label}
        type="button"
      >
        <AgentStateGlyph glyph={glyph} label={label} size="sm" />
        {/* A narrow bar shows the count only, so the savings label still fits. */}
        <span aria-hidden className="font-medium tabular-nums sm:hidden">
          {rows.length}
        </span>
        <span className="hidden font-medium tabular-nums sm:inline">{label}</span>
      </button>
      {popover.anchor ? (
        <QueuePopover
          anchor={popover.anchor}
          id={popover.id}
          now={now}
          onClose={() => popover.close()}
          onMarkAllRead={onMarkAllRead}
          onMarkRead={onMarkRead}
          onOpen={onOpen}
          popoverRef={popover.popoverRef}
          rows={rows}
        />
      ) : null}
    </>
  );
}

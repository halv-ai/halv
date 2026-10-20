import type React from "react";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  buildActivityGrid,
  type ActivityCell,
  type ActivityDay,
} from "../tokenActivity";

type TokenActivityGridProps = {
  title: string;
  days: ActivityDay[];
  formatValue: (value: number) => string;
  /** The translated total line, for example "{amount} in the last year". */
  formatTotal?: (formattedTotal: string) => string;
  /** Controls next to the title, for example a unit switch. */
  controls?: React.ReactNode;
  now?: Date;
};

const WEEK_COUNT = 53;
const GRID_GAP_PX = 3;
const DAY_LABELS = ["", "Mon", "", "Wed", "", "Fri", ""] as const;
const LEGEND_INTENSITIES = [0.2, 0.4, 0.6, 0.8, 1] as const;
const GRID_TEMPLATE_COLUMNS = `repeat(${WEEK_COUNT}, minmax(0, 1fr))`;

type TooltipState = {
  anchorLeft: number;
  label: string;
  left: number;
  top: number;
};

export function TokenActivityGrid({
  title,
  days,
  formatValue,
  formatTotal = (formattedTotal) => `${formattedTotal} in the last year`,
  controls = null,
  now,
}: TokenActivityGridProps) {
  const [defaultNow] = useState(() => new Date());
  const [cellSize, setCellSize] = useState<number | null>(null);
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);
  const cellsGridRef = useRef<HTMLDivElement>(null);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const referenceNow = now ?? defaultNow;
  const grid = useMemo(
    () => buildActivityGrid(days, referenceNow),
    [days, referenceNow],
  );
  const datedCells = grid.weeks
    .flat()
    .filter((cell) => cell.date !== null);
  const firstDate = datedCells.at(0)?.date ?? "no recorded date";
  const lastDate = datedCells.at(-1)?.date ?? "no recorded date";

  useLayoutEffect(() => {
    const cellsGrid = cellsGridRef.current;
    if (!cellsGrid) {
      return;
    }

    const updateCellSize = (containerWidth: number) => {
      const gapsWidth = (WEEK_COUNT - 1) * GRID_GAP_PX;
      setCellSize(Math.max(0, containerWidth - gapsWidth) / WEEK_COUNT);
    };

    updateCellSize(cellsGrid.getBoundingClientRect().width);

    const resizeObserver = new ResizeObserver(([entry]) => {
      updateCellSize(entry.contentRect.width);
    });
    resizeObserver.observe(cellsGrid);

    return () => resizeObserver.disconnect();
  }, []);

  useLayoutEffect(() => {
    if (!tooltip || !tooltipRef.current || !wrapperRef.current) {
      return;
    }

    const tooltipWidth = tooltipRef.current.offsetWidth;
    const wrapperWidth = wrapperRef.current.clientWidth;
    const centeredLeft = tooltip.anchorLeft - tooltipWidth / 2;
    const left = Math.min(
      Math.max(centeredLeft, 0),
      Math.max(0, wrapperWidth - tooltipWidth),
    );

    if (Math.abs(left - tooltip.left) > 0.5) {
      setTooltip((current) => (current ? { ...current, left } : current));
    }
  }, [tooltip]);

  function showTooltip(label: string, cellElement: HTMLDivElement) {
    const wrapper = wrapperRef.current;
    if (!wrapper) {
      return;
    }

    const cellBounds = cellElement.getBoundingClientRect();
    const wrapperBounds = wrapper.getBoundingClientRect();
    const anchorLeft =
      cellBounds.left - wrapperBounds.left + cellBounds.width / 2;

    setTooltip({
      anchorLeft,
      label,
      left: anchorLeft,
      top: cellBounds.top - wrapperBounds.top,
    });
  }

  return (
    <section className="min-w-0 rounded-md border border-surface-border/25 bg-surface-raised/25 p-3">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h5 className="font-mono text-[10px] uppercase tracking-[0.16em] text-content-muted">
          {title}
        </h5>
        {controls}
        <span className="font-mono text-[10px] text-content-muted">
          {formatTotal(formatValue(grid.total))}
        </span>
      </div>

      <div className="relative" ref={wrapperRef}>
        <div className="grid min-w-0 grid-cols-[1.5rem_minmax(0,1fr)] gap-x-2">
          <div aria-hidden="true" />
          <div
            aria-hidden="true"
            className="grid h-3 overflow-visible font-mono text-[9px] leading-none text-content-muted"
            style={{
              columnGap: GRID_GAP_PX,
              gridTemplateColumns: GRID_TEMPLATE_COLUMNS,
            }}
          >
            {cellSize === null
              ? null
              : grid.monthLabels.map((month) => (
                  <span
                    className="whitespace-nowrap overflow-visible"
                    key={`${month.weekIndex}-${month.label}`}
                    style={{
                      gridColumnStart: month.weekIndex + 1,
                      gridRowStart: 1,
                    }}
                  >
                    {month.label}
                  </span>
                ))}
          </div>

          <div
            aria-hidden="true"
            className="grid font-mono text-[9px] leading-none text-content-muted"
            style={{
              gap: GRID_GAP_PX,
              gridTemplateRows:
                cellSize === null
                  ? undefined
                  : `repeat(${DAY_LABELS.length}, ${cellSize}px)`,
            }}
          >
            {cellSize === null
              ? null
              : DAY_LABELS.map((label, dayIndex) => (
                  <span
                    className="flex items-center"
                    key={`${dayIndex}-${label}`}
                  >
                    {label}
                  </span>
                ))}
          </div>

          <div
            aria-colcount={WEEK_COUNT}
            aria-label={`${title}: ${formatValue(grid.total)} total from ${firstDate} to ${lastDate}.`}
            aria-rowcount={DAY_LABELS.length}
            className="grid min-w-0 grid-rows-7"
            ref={cellsGridRef}
            role="grid"
            style={{
              gap: GRID_GAP_PX,
              gridTemplateColumns: GRID_TEMPLATE_COLUMNS,
              gridTemplateRows:
                cellSize === null
                  ? undefined
                  : `repeat(${DAY_LABELS.length}, ${cellSize}px)`,
            }}
          >
            {DAY_LABELS.map((_, dayIndex) =>
              grid.weeks.map((week, weekIndex) => {
                const cell = week[dayIndex];
                if (!cell?.date) {
                  return (
                    <div
                      aria-hidden="true"
                      className="invisible aspect-square w-full rounded-[2px] bg-surface-border/25"
                      key={`pad-${weekIndex}-${dayIndex}`}
                    />
                  );
                }

                const label = activityLabel(cell, formatValue);

                return (
                  <div
                    aria-colindex={weekIndex + 1}
                    aria-label={label}
                    aria-rowindex={dayIndex + 1}
                    className={`aspect-square w-full rounded-[2px] focus:outline-none focus-visible:ring-1 focus-visible:ring-accent ${
                      cell.value > 0 ? "" : "bg-surface-border/25"
                    }`}
                    key={cell.date}
                    onBlur={() => setTooltip(null)}
                    onFocus={(event) => showTooltip(label, event.currentTarget)}
                    onMouseEnter={(event) =>
                      showTooltip(label, event.currentTarget)
                    }
                    onMouseLeave={() => setTooltip(null)}
                    role="gridcell"
                    style={
                      cell.value > 0
                        ? { backgroundColor: accentColor(cell.intensity) }
                        : undefined
                    }
                    tabIndex={0}
                  />
                );
              }),
            )}
          </div>
        </div>

        {tooltip ? (
          <div
            className="pointer-events-none absolute z-10 -mt-1.5 -translate-y-full whitespace-nowrap rounded border border-surface-border/40 bg-surface-raised px-2 py-1 font-mono text-[10px] text-content-primary shadow"
            ref={tooltipRef}
            role="tooltip"
            style={{ left: tooltip.left, top: tooltip.top }}
          >
            {tooltip.label}
          </div>
        ) : null}
      </div>

      <div className="mt-2 flex items-center justify-end gap-1 font-mono text-[9px] text-content-muted">
        <span>Less</span>
        {LEGEND_INTENSITIES.map((intensity) => (
          <span
            aria-hidden="true"
            className="h-2.5 w-2.5 rounded-[2px]"
            key={intensity}
            style={{ backgroundColor: accentColor(intensity) }}
          />
        ))}
        <span>More</span>
      </div>
    </section>
  );
}

function activityLabel(
  cell: ActivityCell,
  formatValue: (value: number) => string,
) {
  const exactValue =
    cell.value >= 1000 ? ` (${cell.value.toLocaleString()})` : "";
  return `${formatValue(cell.value)} \u00b7 ${cell.date}${exactValue}`;
}

function accentColor(intensity: number) {
  return `rgb(var(--accent) / ${0.18 + 0.82 * intensity})`;
}

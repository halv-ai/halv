import assert from "node:assert/strict";
import test from "node:test";

import {
  buildActivityGrid,
  filterMonthLabels,
  type ActivityCell,
} from "../src/tokenActivity.ts";

const NOW = new Date(2026, 7, 21, 18, 30);

function datedCells(weeks: ActivityCell[][]) {
  return weeks.flat().filter((cell) => cell.date !== null);
}

test("builds 53 Sunday-first weeks with seven grid rows", () => {
  const grid = buildActivityGrid([], NOW);

  assert.equal(grid.weeks.length, 53);
  assert.ok(grid.weeks.every((week) => week.length === 7));
  assert.equal(new Date(`${grid.weeks[0]?.[0]?.date}T12:00:00`).getDay(), 0);
  assert.equal(grid.weeks.at(-1)?.[5]?.date, "2026-08-21");
});

test("places a month label on the first week containing that month's day 1", () => {
  const grid = buildActivityGrid([], NOW);
  const august = grid.monthLabels.find((label) => label.label === "Aug");

  assert.ok(august);
  assert.ok(
    grid.weeks[august.weekIndex]?.some(
      (cell) => cell.date === "2026-08-01",
    ),
  );
  assert.ok(
    grid.weeks
      .slice(0, august.weekIndex)
      .every((week) => week.every((cell) => cell.date !== "2026-08-01")),
  );
});

test("skips colliding month labels and keeps the first label", () => {
  assert.deepEqual(
    filterMonthLabels([
      { weekIndex: 0, label: "Jan" },
      { weekIndex: 2, label: "Feb" },
      { weekIndex: 3, label: "Mar" },
      { weekIndex: 5, label: "Apr" },
      { weekIndex: 6, label: "May" },
    ]),
    [
      { weekIndex: 0, label: "Jan" },
      { weekIndex: 3, label: "Mar" },
      { weekIndex: 6, label: "May" },
    ],
  );
});

test("scales intensity and level against the maximum daily value", () => {
  const grid = buildActivityGrid(
    [
      { date: "2026-08-19", value: 100 },
      { date: "2026-08-20", value: 1 },
      { date: "2026-08-21", value: 0 },
    ],
    NOW,
  );
  const maximum = datedCells(grid.weeks).find(
    (cell) => cell.date === "2026-08-19",
  );
  const onePercent = datedCells(grid.weeks).find(
    (cell) => cell.date === "2026-08-20",
  );
  const zero = datedCells(grid.weeks).find(
    (cell) => cell.date === "2026-08-21",
  );

  assert.equal(maximum?.intensity, 1);
  assert.equal(maximum?.level, 5);
  assert.equal(onePercent?.intensity, 0.01);
  assert.equal(onePercent?.level, 1);
  assert.equal(zero?.intensity, 0);
  assert.equal(zero?.level, 0);
  assert.equal(grid.max, 100);
  assert.equal(grid.total, 101);
});

test("future pad cells have null dates, zero values, and no intensity", () => {
  const grid = buildActivityGrid(
    [{ date: "2026-08-21", value: 100 }],
    NOW,
  );
  const currentWeek = grid.weeks.at(-1) ?? [];

  assert.deepEqual(currentWeek.slice(6), [
    { date: null, value: 0, intensity: 0, level: 0 },
  ]);
});

test("quantizes positive values into levels 1 through 5", () => {
  const grid = buildActivityGrid(
    [
      { date: "2026-08-16", value: 1 },
      { date: "2026-08-17", value: 2 },
      { date: "2026-08-18", value: 4 },
      { date: "2026-08-19", value: 6 },
      { date: "2026-08-20", value: 8 },
      { date: "2026-08-21", value: 10 },
    ],
    NOW,
  );

  assert.deepEqual(
    grid.weeks.at(-1)?.slice(0, 6).map((cell) => cell.level),
    [1, 1, 2, 3, 4, 5],
  );
});

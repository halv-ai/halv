import assert from "node:assert/strict";
import test from "node:test";

import { buildSavingsSparkline } from "../src/savingsSparkline.ts";

test("always returns seven daily bars and keeps the latest seven values", () => {
  const oneDay = buildSavingsSparkline([40]);
  const manyDays = buildSavingsSparkline(
    Array.from({ length: 10 }, (_, index) => index + 1),
  );

  assert.deepEqual(
    oneDay.bars.map((bar) => bar.value),
    [0, 0, 0, 0, 0, 0, 40],
  );
  assert.deepEqual(
    manyDays.bars.map((bar) => bar.value),
    [4, 5, 6, 7, 8, 9, 10],
  );
});

test("distinguishes zero savings from small positive savings", () => {
  const series = buildSavingsSparkline([0, 1, 10, -2, Number.NaN]);

  assert.equal(series.hasSavings, true);
  assert.deepEqual(
    series.bars.map((bar) => bar.value),
    [0, 0, 0, 1, 10, 0, 0],
  );
  assert.equal(series.bars[0]?.height, 2);
  assert.equal(series.bars[3]?.height, 5);
  assert.equal(series.bars[4]?.height, 36);
});

test("reports an all-zero series without inventing a visual trend", () => {
  const series = buildSavingsSparkline([0, 0, 0]);

  assert.equal(series.hasSavings, false);
  assert.deepEqual(
    series.bars.map((bar) => bar.height),
    [2, 2, 2, 2, 2, 2, 2],
  );
});

test("keeps low-variance values on an honest zero-based scale", () => {
  const series = buildSavingsSparkline([99, 100]);

  assert.equal(series.bars[5]?.height, 35.64);
  assert.equal(series.bars[6]?.height, 36);
});

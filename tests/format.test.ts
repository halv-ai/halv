import assert from "node:assert/strict";
import test from "node:test";

import { formatPlanResetDuration } from "../src/format.ts";

test("formats plan reset durations below one hour", () => {
  assert.equal(formatPlanResetDuration(5), "05m");
});

test("formats plan reset durations below one day", () => {
  assert.equal(formatPlanResetDuration(90), "1h 30m");
  assert.equal(formatPlanResetDuration(5 * 60 + 2), "5h 02m");
});

test("formats plan reset durations above one day", () => {
  assert.equal(formatPlanResetDuration(2 * 24 * 60 + 5 * 60 + 2), "2d 05h 02m");
});

test("formats the exact one-day boundary with day units", () => {
  assert.equal(formatPlanResetDuration(24 * 60), "1d 00h 00m");
});

import { formatUsdCompact } from "../src/format.ts";

test("formats dollars saved in the compact form of ADR 0015", () => {
  const cases: [number, string][] = [
    [0, "$0"],
    [0.004, "<$0.01"],
    [0.42, "$0.42"],
    [4.27, "$4.27"],
    [9.996, "$10"],
    [12.34, "$12.3"],
    [12, "$12"],
    [99.96, "$100"],
    [123.4, "$123"],
    [999.6, "$1K"],
    [1_234, "$1.2K"],
    [12_000, "$12K"],
    [1_234_567, "$1.2M"],
    [-0.42, "-$0.42"],
    [Number.NaN, "$0"],
  ];
  for (const [value, expected] of cases) {
    assert.equal(formatUsdCompact(value), expected, String(value));
  }
  assert.equal(formatUsdCompact(null), "$0");
});

test("formats dollars saved in the app locale", () => {
  assert.equal(formatUsdCompact(0.42, "pt-BR"), "$ 0,42");
  assert.equal(formatUsdCompact(12.34, "pt-BR"), "$ 12,3");
  assert.equal(formatUsdCompact(0.42, "de-DE"), "0,42 $");
  assert.equal(formatUsdCompact(0.004, "de-DE"), "<0,01 $");
  assert.equal(formatUsdCompact(1_234_567, "de-DE"), "1,2 Mio. $");
  assert.equal(formatUsdCompact(0.42, "ja-JP"), "$0.42");
  assert.equal(formatUsdCompact(12_345, "ja-JP"), "$1.2万");
});

test("keeps the status bar amount within seven characters in English", () => {
  for (const value of [0.004, 4.27, 99.9, 999, 12_345, 999_999, 12_345_678]) {
    assert.ok(formatUsdCompact(value).length <= 7, formatUsdCompact(value));
  }
});

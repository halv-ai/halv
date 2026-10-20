const BAR_COUNT = 7;
const MAX_HEIGHT = 36;
const MIN_SAVINGS_HEIGHT = 5;
const ZERO_HEIGHT = 2;

export type SavingsSparklineBar = {
  value: number;
  height: number;
};

export type SavingsSparklineSeries = {
  bars: SavingsSparklineBar[];
  hasSavings: boolean;
};

function normalizeSavedTokens(value: number) {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}

export function buildSavingsSparkline(
  values: number[],
  barCount: number = BAR_COUNT,
  padWithZeros: boolean = true,
): SavingsSparklineSeries {
  const recent = values.slice(-barCount).map(normalizeSavedTokens);
  const dailyValues = padWithZeros
    ? [
        ...Array.from({ length: barCount - recent.length }, () => 0),
        ...recent,
      ]
    : recent;
  const max = Math.max(0, ...dailyValues);

  return {
    bars: dailyValues.map((value) => ({
      value,
      height:
        value === 0 || max === 0
          ? ZERO_HEIGHT
          : Math.max(MIN_SAVINGS_HEIGHT, (value / max) * MAX_HEIGHT),
    })),
    hasSavings: max > 0,
  };
}

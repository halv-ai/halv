const WEEK_COUNT = 53;
const DAYS_PER_WEEK = 7;

const MONTH_LABELS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

export type ActivityDay = {
  date: string;
  value: number;
};

export type ActivityLevel = 0 | 1 | 2 | 3 | 4 | 5;

export type ActivityCell = {
  date: string | null;
  value: number;
  intensity: number;
  level: ActivityLevel;
};

export type ActivityMonthLabel = {
  weekIndex: number;
  label: string;
};

export type ActivityGrid = {
  weeks: ActivityCell[][];
  monthLabels: ActivityMonthLabel[];
  max: number;
  total: number;
};

export function buildActivityGrid(
  days: readonly ActivityDay[],
  now: Date,
): ActivityGrid {
  const today = atLocalNoon(now);
  const firstSunday = atLocalNoon(today);
  firstSunday.setDate(
    firstSunday.getDate() - firstSunday.getDay() - (WEEK_COUNT - 1) * 7,
  );

  const todayKey = localDateKey(today);
  const valuesByDate = new Map<string, number>();

  for (const day of days) {
    if (day.date < localDateKey(firstSunday) || day.date > todayKey) {
      continue;
    }

    valuesByDate.set(
      day.date,
      (valuesByDate.get(day.date) ?? 0) + normalizeValue(day.value),
    );
  }

  const dailyWeeks = Array.from({ length: WEEK_COUNT }, (_, weekIndex) =>
    Array.from({ length: DAYS_PER_WEEK }, (_, dayIndex) => {
      const date = atLocalNoon(firstSunday);
      date.setDate(
        date.getDate() + weekIndex * DAYS_PER_WEEK + dayIndex,
      );
      const dateKey = localDateKey(date);

      return {
        date: dateKey <= todayKey ? dateKey : null,
        value: dateKey <= todayKey ? (valuesByDate.get(dateKey) ?? 0) : 0,
      };
    }),
  );

  const total = dailyWeeks.reduce(
    (gridTotal, week) =>
      gridTotal + week.reduce((weekTotal, cell) => weekTotal + cell.value, 0),
    0,
  );
  const max = dailyWeeks.reduce(
    (gridMax, week) =>
      Math.max(gridMax, ...week.map((cell) => cell.value)),
    0,
  );

  const weeks = dailyWeeks.map((week) =>
    week.map(({ date, value }): ActivityCell => {
      const intensity = activityIntensity(value, max);

      return {
        date,
        value,
        intensity,
        level: activityLevel(intensity),
      };
    }),
  );

  return {
    weeks,
    monthLabels: filterMonthLabels(buildMonthLabels(weeks)),
    max,
    total,
  };
}

export function filterMonthLabels(
  labels: readonly ActivityMonthLabel[],
  minimumColumnDistance = 3,
): ActivityMonthLabel[] {
  const filtered: ActivityMonthLabel[] = [];

  for (const label of labels) {
    const previous = filtered.at(-1);
    if (
      previous === undefined ||
      label.weekIndex - previous.weekIndex >= minimumColumnDistance
    ) {
      filtered.push(label);
    }
  }

  return filtered;
}

function buildMonthLabels(weeks: ActivityCell[][]): ActivityMonthLabel[] {
  const labels: ActivityMonthLabel[] = [];

  weeks.forEach((week, weekIndex) => {
    const firstOfMonth = week.find(
      (cell) => cell.date !== null && Number(cell.date.slice(8, 10)) === 1,
    );

    if (firstOfMonth?.date) {
      const monthIndex = Number(firstOfMonth.date.slice(5, 7)) - 1;
      labels.push({
        weekIndex,
        label: MONTH_LABELS[monthIndex] ?? "",
      });
    }
  });

  return labels;
}

function activityIntensity(value: number, max: number) {
  if (value <= 0 || max <= 0) {
    return 0;
  }

  return value / max;
}

function activityLevel(intensity: number): ActivityLevel {
  if (intensity <= 0) {
    return 0;
  }

  return Math.min(5, Math.ceil(intensity * 5)) as ActivityLevel;
}

function normalizeValue(value: number) {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}

function atLocalNoon(date: Date) {
  const normalized = new Date(date);
  normalized.setHours(12, 0, 0, 0);
  return normalized;
}

function localDateKey(date: Date) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

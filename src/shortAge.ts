import type { Translate } from "./i18n/runtime";

// The short age of a sidebar row, as Orca shows it: "now", "18m", "3h",
// "5d". A time in the future (clock skew) and a bad time read as "now".

export type ShortAge =
  | { unit: "now" }
  | { unit: "m" | "h" | "d"; amount: number };

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

export function shortAge(at: number | string, now: number): ShortAge {
  const time = typeof at === "number" ? at : Date.parse(at);
  const elapsed = now - time;
  if (!Number.isFinite(elapsed) || elapsed < MINUTE_MS) return { unit: "now" };
  if (elapsed < HOUR_MS) return { unit: "m", amount: Math.floor(elapsed / MINUTE_MS) };
  if (elapsed < DAY_MS) return { unit: "h", amount: Math.floor(elapsed / HOUR_MS) };
  return { unit: "d", amount: Math.floor(elapsed / DAY_MS) };
}

export function shortAgeLabel(age: ShortAge, t: Translate) {
  switch (age.unit) {
    case "now":
      return t("now");
    case "m":
      return t("{amount}m", { amount: age.amount });
    case "h":
      return t("{amount}h", { amount: age.amount });
    case "d":
      return t("{amount}d", { amount: age.amount });
  }
}

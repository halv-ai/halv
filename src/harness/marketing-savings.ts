import {
  MARKETING_ALL_TIME_TOKENS,
  MARKETING_ALL_TIME_USD,
  MARKETING_SESSION_TOKENS,
  MARKETING_WEEK_TOKENS,
} from "./marketing-realism.ts";

export const MARKETING_SAVINGS_STATE_NAMES = [
  "marketing-savings",
  "marketing-savings-dark",
  "marketing-savings-light",
] as const;

export const MARKETING_SAVINGS_WEEK_TOKENS = MARKETING_WEEK_TOKENS;
export const MARKETING_SAVINGS_ALL_TIME_TOKENS = MARKETING_ALL_TIME_TOKENS;
export const MARKETING_SAVINGS_ALL_TIME_USD = MARKETING_ALL_TIME_USD;
export const MARKETING_SAVINGS_SESSION_TOKENS = MARKETING_SESSION_TOKENS;

export function prepareMarketingSavingsHarness(stateName: string) {
  if (!MARKETING_SAVINGS_STATE_NAMES.some((name) => name === stateName)) return;
}

export function marketingSavingsDashboardIsReady(root: ParentNode = document) {
  const advancedDiagnostics = [...root.querySelectorAll("details")].find(
    (details) =>
      details.querySelector("summary")?.textContent?.trim() ===
      "Advanced diagnostics",
  );
  const dashboard = advancedDiagnostics?.parentElement;
  const metricGrid = dashboard?.firstElementChild;
  if (
    !advancedDiagnostics ||
    !dashboard ||
    !(metricGrid instanceof HTMLElement) ||
    metricGrid.children.length !== 3
  ) {
    return false;
  }

  const metricCards = [...metricGrid.children].filter(
    (element): element is HTMLElement => element instanceof HTMLElement,
  );
  const text = dashboard.innerText.replace(/\s+/g, " ").toUpperCase();
  return (
    metricCards.length === 3 &&
    text.includes("ALL-TIME SAVED $49.2 21.9M TOKENS KEPT OUT OF CONTEXT") &&
    text.includes("THIS APP RUN $0.11 44.9K TOKENS KEPT OUT OF CONTEXT") &&
    text.includes("TOKEN USAGE") &&
    text.includes("TOKENS SAVED") &&
    !text.includes("SAVED PER AGENT")
  );
}

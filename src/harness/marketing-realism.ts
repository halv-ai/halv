import type { HarnessFixtureConfig } from "./states.ts";

export const MARKETING_REALISM_STATE_NAMES = [
  "marketing-app-dark",
  "marketing-app-light",
  "marketing-pro-split-dark",
  "marketing-pro-split-light",
  "marketing-subagents-dark",
  "marketing-subagents-light",
  "marketing-crux-dark",
  "marketing-crux-light",
  "marketing-savings",
  "marketing-savings-dark",
  "marketing-savings-light",
  "marketing-workspace-dark",
  "marketing-workspace-light",
] as const;

export const MARKETING_WEEK_TOKENS = 21_676_683;
export const MARKETING_ALL_TIME_TOKENS = 21_900_000;
export const MARKETING_ALL_TIME_USD = 101.16;
export const MARKETING_SESSION_TOKENS = 44_876;
// Dollars with the methods of ADR 0015: RTK and crux tokens once at the
// input price, Headroom tokens at the session's prompt rate.
export const MARKETING_ALL_TIME_DOLLARS = 49.2;
export const MARKETING_WEEK_DOLLARS = 48.6;
export const MARKETING_TODAY_DOLLARS = 0.66;
export const MARKETING_SESSION_DOLLARS = 0.11;
export const MARKETING_ENGINE_SAVINGS = {
  crux: 7_400_000,
  rtk: 5_200_000,
  leanContext: 3_900_000,
  toolSearch: 2_800_000,
  compression: 1_600_000,
} as const;

export const MARKETING_RECENT_SAVINGS = [
  88_421,
  121_905,
  64_280,
  173_448,
  96_772,
  248_130,
  139_604,
  2_164_882,
  317_490,
  186_225,
  742_118,
  202_641,
  126_904,
  388_720,
  155_302,
  271_084,
  113_976,
  624_413,
  194_208,
  349_765,
  168_114,
  481_992,
  231_506,
  293_847,
] as const;

export const MARKETING_RECENT_DOLLARS = MARKETING_RECENT_SAVINGS.map(
  (tokens) => Math.round(tokens * 0.000_002_24 * 10_000) / 10_000,
);

const marketingUsageReset = (minutesFromNow: number) =>
  new Date(Date.now() + minutesFromNow * 60_000).toISOString();

export const MARKETING_CLAUDE_USAGE_WINDOWS = [
  {
    label: "5h",
    usedPercent: 18,
    resetsAt: marketingUsageReset(158),
  },
  {
    label: "7d",
    usedPercent: 55,
    resetsAt: marketingUsageReset(2_728),
  },
] as const;

export const MARKETING_CODEX_USAGE_WINDOWS = [
  {
    label: "7d",
    usedPercent: 97,
    resetsAt: marketingUsageReset(152),
  },
] as const;

export function isMarketingRealismStateName(stateName: string) {
  return MARKETING_REALISM_STATE_NAMES.some((name) => name === stateName);
}

export function isMarketingRealismFixture(fixture: HarnessFixtureConfig) {
  return Boolean(
    fixture.marketingProSplit ||
      fixture.marketingTerminalScene ||
      fixture.marketingSubagents ||
      fixture.marketingSavings ||
      fixture.marketingWorkspace,
  );
}

export function prepareMarketingRealismHarness(stateName: string) {
  if (!isMarketingRealismStateName(stateName)) return;

  window.localStorage.setItem("halv.savingsPanel", "expanded");

  const stageRealChrome = () => {
    hideSidebarTerminalSection();
    hideUpdateChrome();
  };
  stageRealChrome();
  const observer = new MutationObserver(stageRealChrome);
  observer.observe(document.documentElement, { childList: true, subtree: true });
}

export function marketingLiveSavingsIsReady(root: ParentNode = document) {
  const toggle = root.querySelector<HTMLElement>(
    'button[aria-controls="live-savings-details"][aria-expanded="true"]',
  );
  const details = root.querySelector<HTMLElement>("#live-savings-details");
  const sparkline = details?.querySelector<HTMLElement>('[role="img"]');
  if (!toggle || !details || !sparkline) return false;

  const mountedBars = sparkline.querySelectorAll("span");
  const planRows = details.querySelectorAll<HTMLElement>(
    'div[class~="grid-cols-[1.6rem_minmax(0,1fr)_auto]"]',
  );
  const text = normalizedText(`${toggle.innerText} ${details.innerText}`);
  const uppercaseText = text.toUpperCase();
  return (
    mountedBars.length === MARKETING_RECENT_SAVINGS.length &&
    planRows.length === 3 &&
    [
      "LIVE SAVINGS last 7 days",
      "21.7M TOKENS SAVED",
      "RECENT CALLS",
      "All time 21.9M · $101.16",
      "PLAN WINDOWS",
      "CLAUDE",
      "5h 82% · 2h 38m",
      "7d 45% · 1d 21h 28m",
      "CODEX",
      "7d 3% · 2h 32m",
    ].every((value) => uppercaseText.includes(value.toUpperCase())) &&
    !uppercaseText.includes("SAVED PER AGENT") &&
    !uppercaseText.includes("GLM") &&
    !uppercaseText.includes("KIMI") &&
    !uppercaseText.includes("GROK")
  );
}

export function marketingForbiddenChromeIsAbsent(root: ParentNode = document) {
  const visibleText = [...root.querySelectorAll<HTMLElement>("body *")]
    .filter(
      (element) =>
        !element.hidden &&
        element.closest("[hidden]") === null &&
        element.getAttribute("aria-hidden") !== "true" &&
        element.children.length === 0,
    )
    .map((element) => element.textContent ?? "")
    .join("\n");
  return (
    ![
      "BYPASSING PERMISSIONS ON",
      "Update installed · Restart to update",
      "Restart to update",
    ].some((value) => visibleText.includes(value)) &&
    !/^Halv .+ is available\.$/m.test(visibleText)
  );
}

export function marketingExpandedSidebarIsReady(
  root: ParentNode = document,
  requireTerminalBadges = true,
) {
  const navigation = root.querySelector<HTMLElement>(
    '[aria-label="Conversation navigation"]',
  );
  const newConversation = [...root.querySelectorAll<HTMLElement>("button")].find(
    (element) => normalizedText(element.textContent ?? "") === "+ New chat",
  );
  const search = root.querySelector<HTMLElement>(
    '[aria-label="Search conversations"]',
  );
  const conversations = root.querySelector<HTMLElement>(
    '[aria-label="Conversations"]',
  );
  const terminalBadges = root.querySelectorAll<HTMLElement>(
    'button[aria-label^="Focus terminal for "]',
  );
  const terminalHeading = [...root.querySelectorAll<HTMLElement>("span")].find(
    (element) =>
      element.children.length === 0 && element.textContent?.trim() === "Terminals",
  );
  const terminalSection = terminalHeading?.closest<HTMLElement>("section");
  return (
    navigation !== null &&
    newConversation !== null &&
    search !== null &&
    conversations !== null &&
    (!requireTerminalBadges || terminalBadges.length > 0) &&
    (terminalSection == null || terminalSection.hidden)
  );
}

function hideSidebarTerminalSection() {
  const heading = [...document.querySelectorAll<HTMLElement>("span")].find(
    (element) =>
      element.children.length === 0 && element.textContent?.trim() === "Terminals",
  );
  const section = heading?.closest("section");
  if (section instanceof HTMLElement) section.hidden = true;
}

function hideUpdateChrome() {
  const updateText = [...
    document.querySelectorAll<HTMLElement>("span, p, footer"),
  ].filter(
    (element) =>
      element.children.length === 0 &&
      (/^Halv .+ is available\.$/.test(element.textContent?.trim() ?? "") ||
        /Update installed · Restart to update/.test(element.textContent ?? "")),
  );
  for (const element of updateText) {
    const container = element.closest<HTMLElement>('[role="status"], footer');
    (container ?? element.parentElement ?? element).hidden = true;
  }
}

function normalizedText(value: string) {
  return value.replace(/\s+/g, " ").trim();
}

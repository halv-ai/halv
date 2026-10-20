import type { MarketingStateName } from "./states.ts";

type MarketingExport = {
  state: MarketingStateName;
  viewport: { width: number; height: number };
  deviceScaleFactor: 2;
  colorScheme: "dark" | "light";
};

export const MARKETING_GHOSTTY_WINDOW_TITLE = "Claude Code — halv — Ghostty";
export const MARKETING_GHOSTTY_PLAN_MODE_STATUS =
  "⏸ plan mode on (shift+tab to cycle)";

export const MARKETING_EXPORTS = [
  marketingExport("marketing-app-dark", 1600, 1000, "dark"),
  marketingExport("marketing-app-light", 1600, 1000, "light"),
  marketingExport("marketing-pro-split-dark", 1600, 900, "dark"),
  marketingExport("marketing-pro-split-light", 1600, 900, "light"),
  marketingExport("marketing-subagents-dark", 1600, 1000, "dark"),
  marketingExport("marketing-subagents-light", 1600, 1000, "light"),
  marketingExport("marketing-crux-dark", 1600, 1000, "dark"),
  marketingExport("marketing-crux-light", 1600, 1000, "light"),
  marketingExport("marketing-savings-dark", 1600, 1000, "dark"),
  marketingExport("marketing-savings-light", 1600, 1000, "light"),
  marketingExport("marketing-ghostty-dark", 1200, 900, "dark"),
  marketingExport("marketing-ghostty-light", 1200, 900, "light"),
  // The hero of halv.ai. With the window frame of the site, the PNG is
  // 3248 x 2118 px, the size of the earlier hero.
  marketingExport("marketing-workspace-dark", 1512, 947, "dark"),
  marketingExport("marketing-workspace-light", 1512, 947, "light"),
] as const satisfies readonly MarketingExport[];

export function marketingGhosttyIsReady(root: ParentNode = document) {
  const windowFrame = root.querySelector<HTMLElement>(
    '[data-marketing-ghostty-window="true"]',
  );
  if (!windowFrame) return false;

  const titleBar = windowFrame.querySelector<HTMLElement>(
    '[data-marketing-ghostty-title-bar="true"]',
  );
  const title = titleBar?.querySelector<HTMLElement>(
    "[data-marketing-ghostty-title]",
  );
  const trafficLights = [
    ...windowFrame.querySelectorAll<HTMLElement>(
      "[data-marketing-ghostty-traffic-light]",
    ),
  ];
  const trafficLightActions = new Set(
    trafficLights.map(
      (light) => light.dataset.marketingGhosttyTrafficLight ?? "",
    ),
  );
  const terminal = windowFrame.querySelector<HTMLElement>(
    '[data-terminal-scene-complete="true"]',
  );
  const composer = terminal?.querySelector<HTMLElement>(
    "[data-marketing-ghostty-composer-status]",
  );

  return (
    titleBar !== null &&
    title?.textContent?.trim() === MARKETING_GHOSTTY_WINDOW_TITLE &&
    trafficLights.length === 3 &&
    ["close", "minimize", "zoom"].every((action) =>
      trafficLightActions.has(action),
    ) &&
    terminal?.querySelector(".xterm-screen") !== null &&
    composer?.dataset.marketingGhosttyComposerStatus ===
      MARKETING_GHOSTTY_PLAN_MODE_STATUS
  );
}

function marketingExport(
  state: MarketingStateName,
  width: number,
  height: number,
  colorScheme: MarketingExport["colorScheme"],
): MarketingExport {
  return {
    state,
    viewport: { width, height },
    deviceScaleFactor: 2,
    colorScheme,
  };
}

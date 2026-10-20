import { expect, type Page } from "@playwright/test";
import { openVisualState } from "./capture";

// Shared helpers of the workspace strip specs (ADR 0010).

export const TABS_QUERY = "flags=projectTree,workspaceTabs&sidebar=projects&ghostty=off";

export async function openWorkspaceTabs(
  page: Page,
  theme: "dark" | "light" = "dark",
  extraQuery = "",
) {
  await openVisualState(
    page,
    theme === "dark" ? "workspace-tabs-dark" : "workspace-tabs-light",
    extraQuery ? `${TABS_QUERY}&${extraQuery}` : TABS_QUERY,
  );
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
}

/**
 * Terminal adoption (ADR 0020) moves the unowned fixture terminals (the
 * shell in the project root and the loose Codex agent) into shell tabs at
 * start. The strip specs start with both marked as handled, so their
 * strips keep only the tabs that they open. The adoption specs leave this
 * out. The mark is written once, so a reload keeps later marks.
 */
export async function skipFixtureAdoption(page: Page) {
  await page.addInitScript(() => {
    const key = "halv.tab-adoption.v1";
    if (window.localStorage.getItem(key)) return;
    window.localStorage.setItem(
      key,
      JSON.stringify({ version: 1, terminals: ["tree-shell", "tree-codex-loose"], conversations: [] }),
    );
  });
}

/** The strip with shell tabs too (stages S5b and S7). The fixture
 *  terminals stay out of the strips (`skipFixtureAdoption`). */
export async function openWorkspaceTabsWithShells(
  page: Page,
  theme: "dark" | "light" = "dark",
  extraQuery = "",
) {
  await skipFixtureAdoption(page);
  const query = TABS_QUERY.replace(/^flags=[^&]+/, "flags=projectTree,workspaceTabs,shellTabs");
  await openVisualState(
    page,
    theme === "dark" ? "workspace-tabs-dark" : "workspace-tabs-light",
    extraQuery ? `${query}&${extraQuery}` : query,
  );
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
}

export function strip(page: Page) {
  return page.locator("[data-workspace-tab-strip]");
}

export function stripTab(page: Page, title: string | RegExp) {
  return strip(page).getByRole("tab", { name: title });
}

export function activeTab(page: Page) {
  return strip(page).locator('.workspace-tab[data-active="true"]');
}

export function tabBody(page: Page, conversationId: string) {
  return page.locator(`[data-workspace-tab-body="conversation:${conversationId}"]`);
}

/** The tree lists open tabs only. Other conversations open from a hit of
 *  the sidebar search (ADR 0020: the Recent view is gone). The search
 *  empties again unless the caller keeps the focus where the click put it. */
export async function openFromSearch(
  page: Page,
  query: string,
  title: string,
  options: { clear?: boolean } = {},
) {
  const search = page.getByRole("searchbox", { name: "Search conversations" });
  await search.fill(query);
  await page
    .getByRole("complementary", { name: "Conversation navigation" })
    .getByRole("button", { name: title, exact: true })
    .first()
    .click();
  if (options.clear !== false) {
    await search.fill("");
    // The tree comes back once the search is empty.
    await expect(page.getByRole("tree", { name: "Projects", exact: true })).toBeVisible();
  }
}

export function treeRow(page: Page, key: string) {
  return page.locator(`[role="treeitem"][data-tree-key="${key}"] > .project-tree-row`);
}

export async function tabTitles(page: Page) {
  return strip(page).locator(".workspace-tab .workspace-tab-title").allInnerTexts();
}

/** The data-workspace-tab-key of every strip tab, in order. */
export async function tabKeys(page: Page) {
  return strip(page)
    .locator(".workspace-tab")
    .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-workspace-tab-key")));
}

export type LayoutFrame = {
  headerHeight: number;
  stripTop: number | null;
  stackTop: number;
  stackHeight: number;
  activeSurfaces: number;
  bottomPanelsOpen: number;
  emptyTerminalChoices: number;
  activeBodyKeys: string[];
};

/**
 * Samples the layout on every animation frame until `stopLayoutProbe`.
 * A clean flow keeps the header and the body box still, shows one
 * surface at a time, never opens the bottom panel by itself, and never
 * flashes the empty "Open agent terminal" choice.
 */
export async function startLayoutProbe(page: Page) {
  await page.evaluate(() => {
    const probe = window as typeof window & {
      __layoutFrames?: unknown[];
      __layoutProbeOn?: boolean;
    };
    probe.__layoutFrames = [];
    probe.__layoutProbeOn = true;
    const sample = () => {
      const header = document.querySelector("[data-app-header]");
      const stack = document.querySelector(".conversation-mode-stack");
      const stripNode = document.querySelector("[data-workspace-tab-strip]");
      const active = [
        ...document.querySelectorAll<HTMLElement>('.conversation-mode-surface[data-active="true"]'),
      ];
      probe.__layoutFrames!.push({
        headerHeight: header?.getBoundingClientRect().height ?? -1,
        stripTop: stripNode ? stripNode.getBoundingClientRect().top : null,
        stackTop: stack?.getBoundingClientRect().top ?? -1,
        stackHeight: stack?.getBoundingClientRect().height ?? -1,
        activeSurfaces: active.length,
        bottomPanelsOpen: document.querySelectorAll('.terminal-diff-surface[data-open="true"]').length,
        emptyTerminalChoices: active.filter((node) => node.querySelector(".terminal-empty-state"))
          .length,
        activeBodyKeys: active.map((node) => node.dataset.workspaceTabBody ?? "chat"),
      });
      if (probe.__layoutProbeOn) requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });
}

export async function stopLayoutProbe(page: Page): Promise<LayoutFrame[]> {
  return page.evaluate(() => {
    const probe = window as typeof window & {
      __layoutFrames?: LayoutFrame[];
      __layoutProbeOn?: boolean;
    };
    probe.__layoutProbeOn = false;
    return probe.__layoutFrames ?? [];
  });
}

/** The assertions of a clean flow over every sampled frame. */
export function expectStableLayout(frames: LayoutFrame[], options: { allowBottomPanel?: boolean } = {}) {
  expect(frames.length, "the probe sampled frames").toBeGreaterThan(1);
  const first = frames[0];
  for (const [index, frame] of frames.entries()) {
    const at = `frame ${index}: ${JSON.stringify(frame)}`;
    expect(Math.abs(frame.headerHeight - first.headerHeight), `header height, ${at}`).toBeLessThan(0.5);
    expect(Math.abs(frame.stackTop - first.stackTop), `body top, ${at}`).toBeLessThan(0.5);
    if (frame.stripTop !== null && first.stripTop !== null) {
      expect(Math.abs(frame.stripTop - first.stripTop), `strip top, ${at}`).toBeLessThan(0.5);
    }
    expect(frame.activeSurfaces, `one visible body, ${at}`).toBeLessThanOrEqual(1);
    expect(frame.emptyTerminalChoices, `no empty terminal flash, ${at}`).toBe(0);
    if (!options.allowBottomPanel) {
      expect(frame.bottomPanelsOpen, `no half-open bottom terminal, ${at}`).toBe(0);
    }
  }
}

/** The focused element sits in this tab body (a terminal pane). */
export async function focusedBodyKey(page: Page) {
  return page.evaluate(
    () =>
      document.activeElement?.closest("[data-workspace-tab-body]")?.getAttribute("data-workspace-tab-body") ??
      (document.activeElement?.getAttribute("aria-label") === "Message" ? "composer" : null),
  );
}

export async function terminalWrites(page: Page, sessionId?: string) {
  return page.evaluate(
    (id) =>
      ((window as typeof window & { __harnessTerminalWrites?: Array<{ sessionId: string; data: string }> })
        .__harnessTerminalWrites ?? [])
        .filter((write) => !id || write.sessionId === id)
        .map((write) => write.data)
        .join(""),
    sessionId ?? null,
  );
}

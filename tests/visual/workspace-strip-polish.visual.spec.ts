import { expect, test, type Page } from "@playwright/test";
import {
  activeTab,
  openWorkspaceTabsWithShells,
  strip,
  stripTab,
  tabKeys,
  tabTitles,
} from "./workspaceTabsProbe";

// Stage S7 of ADR 0010 (parity polish of the strip, copied from Orca): drag
// reorder with an insertion bar, overflow chevrons with a fade mask, the
// tab context menu, reopen of a closed tab, the Ctrl+Tab switcher, the
// workspace chords, the hover card and the compact Pro header.

/** Opens `count` shell tabs with Cmd+T. */
async function openShells(page: Page, count: number) {
  for (let index = 0; index < count; index += 1) {
    const before = (await tabKeys(page)).length;
    await page.keyboard.press("Meta+t");
    await expect.poll(async () => (await tabKeys(page)).length).toBe(before + 1);
  }
}

async function centerOf(page: Page, title: string | RegExp) {
  const box = (await stripTab(page, title).boundingBox())!;
  return { x: box.x + box.width / 2, y: box.y + box.height / 2, box };
}

async function startDrag(page: Page, title: string | RegExp) {
  const from = await centerOf(page, title);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + 20, from.y, { steps: 4 });
  return from;
}

test.describe("drag reorder", () => {
  test("a drag shows the insertion bar, the drop moves the tab, and the order survives a reload", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 2);
    expect(await tabTitles(page)).toEqual(["Audit agent refactor", "Terminal 1", "Terminal 2"]);
    const active = await activeTab(page).getAttribute("data-workspace-tab-key");

    await startDrag(page, "Terminal 2");
    await expect(strip(page)).toHaveAttribute("data-tab-dragging", "true");
    await expect(page.locator(".workspace-tab-drag-ghost")).toHaveCount(1);
    const target = await centerOf(page, "Audit agent refactor");
    await page.mouse.move(target.box.x + 8, target.y, { steps: 6 });
    // The bar shows on the left edge of the first tab. The tabs stay put.
    await expect(stripTab(page, "Audit agent refactor")).toHaveAttribute("data-drop-indicator", "left");
    expect(await tabTitles(page)).toEqual(["Audit agent refactor", "Terminal 1", "Terminal 2"]);
    await page.mouse.up();

    await expect(strip(page)).not.toHaveAttribute("data-tab-dragging", "true");
    await expect(page.locator(".workspace-tab-drag-ghost")).toHaveCount(0);
    expect(await tabTitles(page)).toEqual(["Terminal 2", "Audit agent refactor", "Terminal 1"]);
    // A drag never switches tabs.
    expect(await activeTab(page).getAttribute("data-workspace-tab-key")).toBe(active);

    await page.reload();
    await page.locator('main[data-workspace-tabs="ready"]').waitFor();
    await expect.poll(() => tabTitles(page)).toEqual(["Terminal 2", "Audit agent refactor", "Terminal 1"]);
  });

  test("a short press activates; Escape cancels a drag; a drop far from the strip changes nothing", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 1);
    // A press that moves less than 12 px is a click.
    const audit = await centerOf(page, "Audit agent refactor");
    await page.mouse.move(audit.x, audit.y);
    await page.mouse.down();
    await page.mouse.move(audit.x + 6, audit.y + 2, { steps: 3 });
    await page.mouse.up();
    await expect(activeTab(page)).toHaveText("Audit agent refactor");

    const order = await tabTitles(page);
    await startDrag(page, "Terminal 1");
    await page.mouse.move(audit.box.x + 4, audit.y, { steps: 4 });
    await expect(stripTab(page, "Audit agent refactor")).toHaveAttribute("data-drop-indicator", "left");
    await page.keyboard.press("Escape");
    await expect(page.locator(".workspace-tab-drag-ghost")).toHaveCount(0);
    await page.mouse.up();
    expect(await tabTitles(page)).toEqual(order);

    await startDrag(page, "Terminal 1");
    await page.mouse.move(audit.box.x + 4, audit.y + 300, { steps: 6 });
    await expect(strip(page).locator("[data-drop-indicator]")).toHaveCount(0);
    await page.mouse.up();
    expect(await tabTitles(page)).toEqual(order);
    await expect(activeTab(page)).toHaveText("Audit agent refactor");
  });

  for (const theme of ["dark", "light"] as const) {
    test(`the insertion bar in the ${theme} theme`, async ({ page }) => {
      await openWorkspaceTabsWithShells(page, theme);
      await openShells(page, 2);
      await startDrag(page, "Terminal 2");
      const target = await centerOf(page, "Terminal 1");
      await page.mouse.move(target.box.x + 10, target.y + 6, { steps: 6 });
      await expect(stripTab(page, "Terminal 1")).toHaveAttribute("data-drop-indicator", "left");
      const header = (await page.locator("[data-app-header]").boundingBox())!;
      await expect(page).toHaveScreenshot(`workspace-strip-drag-${theme}.png`, {
        clip: { x: header.x, y: header.y, width: 900, height: header.height + 12 },
      });
      await page.mouse.up();
    });
  }
});

test.describe("overflow", () => {
  // Tabs shrink to 72 px before they overflow (Orca), so a 1000 px window
  // with eight tabs overflows.
  test.use({ viewport: { width: 1000, height: 760 } });

  test("chevrons and the fade show when the tabs overflow; the active tab stays in view", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await expect(strip(page).locator(".workspace-tab-chevron")).toHaveCount(0);
    await openShells(page, 7);
    const scroll = strip(page).getByRole("tablist");
    const left = strip(page).getByRole("button", { name: "Scroll tabs left" });
    const right = strip(page).getByRole("button", { name: "Scroll tabs right" });
    await expect(left).toBeVisible();
    await expect(right).toBeVisible();
    // The new tab is active and in view: the strip shows its end.
    await expect(activeTab(page).locator(".workspace-tab-title")).toHaveText("Terminal 7");
    await expect(left).toBeEnabled();
    await expect(right).toBeDisabled();
    await expect(scroll).toHaveClass(/workspace-tab-scroll--fade-start/);
    await expect(scroll).not.toHaveClass(/workspace-tab-scroll--fade-end/);
    const inView = async () => {
      const tab = (await activeTab(page).boundingBox())!;
      const box = (await scroll.boundingBox())!;
      return tab.x >= box.x - 1 && tab.x + tab.width <= box.x + box.width + 1;
    };
    expect(await inView()).toBe(true);

    await left.click();
    await expect(right).toBeEnabled();
    await expect(scroll).toHaveClass(/workspace-tab-scroll--fade-end/);
    // Cmd+1 shows the first tab, and the strip scrolls to it.
    await page.keyboard.press("Meta+1");
    await expect(activeTab(page)).toHaveText("Audit agent refactor");
    await expect.poll(inView).toBe(true);
    await expect(left).toBeDisabled();
    await page.keyboard.press("Meta+9");
    await expect(activeTab(page).locator(".workspace-tab-title")).toHaveText("Terminal 7");
    await expect.poll(inView).toBe(true);
  });

  for (const theme of ["dark", "light"] as const) {
    test(`the overflowing strip in the ${theme} theme`, async ({ page }) => {
      await openWorkspaceTabsWithShells(page, theme);
      await openShells(page, 7);
      await strip(page).getByRole("button", { name: "Scroll tabs left" }).click();
      await page.mouse.move(640, 600);
      await page.waitForTimeout(200);
      await expect(strip(page)).toHaveScreenshot(`workspace-strip-overflow-${theme}.png`);
    });
  }
});

const PARKED_SCOPE = "halv.terminal-layout.v1:conversation:audit-parked";
const PRIMARY_ROOT = "/Users/auditor/Projects/halv";

/** A saved panel for the parked conversation: one Claude pane that can
 *  resume (the same fixture as workspace-tabs.visual.spec.ts). */
async function plantParkedPanel(page: Page) {
  await page.addInitScript(
    ({ scope, root }) => {
      const layout = {
        version: 1,
        activeTabId: "parked-tab",
        activeLeafId: "pane-parked",
        tabs: [{ tabId: "parked-tab", root: { type: "leaf", leafId: "pane-parked" } }],
      };
      const now = new Date().toISOString();
      window.localStorage.setItem(scope, JSON.stringify(layout));
      window.localStorage.setItem(
        `ux-audit.terminal-restore:${scope}`,
        JSON.stringify({
          layout,
          updatedAt: now,
          panes: [
            {
              paneId: "pane-parked",
              sessionId: "parked-session",
              projectId: 1,
              conversationId: "audit-parked",
              projectName: "Halv Desktop",
              cwd: root,
              executionProfile: "full_access",
              agentKind: "claude",
              laneKind: "claudeSubscription",
              credentialProfileId: null,
              customTitle: null,
              cliSessionId: "cli-parked",
              cols: 100,
              rows: 30,
              scrollback: "$ parked\r\n",
              updatedAt: now,
            },
          ],
        }),
      );
    },
    { scope: PARKED_SCOPE, root: PRIMARY_ROOT },
  );
}

async function createRequests(page: Page) {
  return page.evaluate(
    () =>
      (window as typeof window & { __harnessTerminalCreateRequests?: Array<string | null> })
        .__harnessTerminalCreateRequests ?? [],
  );
}

function activeBodyPanes(page: Page) {
  return page.locator(
    '.conversation-mode-surface[data-active="true"] .terminal-pane[data-terminal-session-id]',
  );
}

function tabMenu(page: Page) {
  return page.getByRole("menu", { name: /^Actions for / });
}

async function openTabMenu(page: Page, title: string | RegExp) {
  await stripTab(page, title).click({ button: "right" });
  await expect(tabMenu(page)).toBeVisible();
}

test.describe("tab menu", () => {
  test("the menu lists Orca's actions in Orca's order", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 2);
    await openTabMenu(page, "Terminal 1");
    await expect(tabMenu(page).getByRole("menuitem")).toHaveText([
      /^Split terminal right/,
      /^Split terminal down/,
      "Copy path",
      /^Close tab/,
      "Close other tabs",
      "Close tabs to the right",
      "Close tabs to the left",
      /^Rename/,
    ]);
    // The first item has the focus; Escape gives it back to the tab.
    await expect(tabMenu(page).getByRole("menuitem").first()).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(tabMenu(page)).toHaveCount(0);
    await expect(stripTab(page, "Terminal 1")).toBeFocused();
    // The last tab has nothing to its right; a chat tab cannot split.
    await openTabMenu(page, "Terminal 2");
    await expect(tabMenu(page).getByRole("menuitem", { name: "Close tabs to the right" })).toBeDisabled();
    await page.mouse.click(640, 600);
    await expect(tabMenu(page)).toHaveCount(0);
  });

  for (const theme of ["dark", "light"] as const) {
    test(`the tab menu in the ${theme} theme`, async ({ page }) => {
      await openWorkspaceTabsWithShells(page, theme);
      await openShells(page, 2);
      const tab = (await stripTab(page, "Terminal 1").boundingBox())!;
      await page.mouse.click(tab.x + 40, tab.y + tab.height / 2, { button: "right" });
      await expect(tabMenu(page)).toBeVisible();
      const header = (await page.locator("[data-app-header]").boundingBox())!;
      await expect(page).toHaveScreenshot(`workspace-tab-menu-${theme}.png`, {
        clip: { x: header.x, y: header.y, width: 900, height: 420 },
      });
    });
  }

  test("Close other tabs asks once for the whole set, only because a command runs", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 2);
    const busy = (await activeBodyPanes(page).first().getAttribute("data-terminal-session-id"))!;
    await page.evaluate((sessionId) => {
      (window as typeof window & { __harnessShellBusy?: Record<string, string> }).__harnessShellBusy = {
        [sessionId]: "busy",
      };
    }, busy);
    await openTabMenu(page, "Terminal 1");
    await tabMenu(page).getByRole("menuitem", { name: "Close other tabs" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toHaveCount(1);
    await expect(dialog).toContainText("Close 2 tabs?");
    await expect(dialog).toContainText("A command is running.");
    await dialog.getByRole("button", { name: "Close tabs", exact: true }).click();
    await expect.poll(() => tabTitles(page)).toEqual(["Terminal 1"]);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(activeTab(page).locator(".workspace-tab-title")).toHaveText("Terminal 1");
  });

  test("the side closes need no prompt when nothing is busy", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 3);
    await openTabMenu(page, "Terminal 2");
    await tabMenu(page).getByRole("menuitem", { name: "Close tabs to the right" }).click();
    await expect.poll(() => tabTitles(page)).toEqual(["Audit agent refactor", "Terminal 1", "Terminal 2"]);
    await openTabMenu(page, "Terminal 2");
    await tabMenu(page).getByRole("menuitem", { name: "Close tabs to the left" }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect.poll(() => tabTitles(page)).toEqual(["Terminal 2"]);
  });

  test("Rename from the menu renames the conversation; Cmd+R renames the visible shell tab", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 1);
    await openTabMenu(page, "Audit agent refactor");
    await tabMenu(page).getByRole("menuitem", { name: /^Rename/ }).click();
    const input = strip(page).locator('[data-tab-rename-input="true"]');
    await expect(input).toBeFocused();
    await input.fill("Refactor the agent store");
    await page.keyboard.press("Enter");
    await expect(stripTab(page, "Refactor the agent store")).toHaveCount(1);
    // The conversation got the title, so the sidebar shows it too.
    await expect(page.locator('[role="treeitem"][data-tree-key="conversation:audit-agent-refactor"]')).toContainText(
      "Refactor the agent store",
    );

    await stripTab(page, "Terminal 1").click();
    await activeBodyPanes(page).first().locator(".xterm-helper-textarea").focus();
    await page.keyboard.press("Meta+r");
    await expect(input).toBeFocused();
    await input.fill("dev server");
    await page.keyboard.press("Enter");
    await expect(activeTab(page).locator(".workspace-tab-title")).toHaveText("dev server");
    // The chord never reached the shell.
    const writes = await page.evaluate(
      () =>
        ((window as typeof window & { __harnessTerminalWrites?: Array<{ data: string }> })
          .__harnessTerminalWrites ?? []).map((write) => write.data).join(""),
    );
    expect(writes).not.toContain("r");
  });

  test("Split terminal right splits the pane of the tab; Copy path copies the workspace root", async ({ page }) => {
    await page.addInitScript(() => {
      const copied: string[] = [];
      (window as typeof window & { __copied?: string[] }).__copied = copied;
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText: async (text: string) => void copied.push(text) },
      });
    });
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 1);
    await expect(activeBodyPanes(page)).toHaveCount(1);
    await openTabMenu(page, "Terminal 1");
    await tabMenu(page).getByRole("menuitem", { name: /^Split terminal right/ }).click();
    await expect(activeBodyPanes(page)).toHaveCount(2);

    await openTabMenu(page, "Audit agent refactor");
    await tabMenu(page).getByRole("menuitem", { name: "Copy path" }).click();
    await expect
      .poll(() => page.evaluate(() => (window as typeof window & { __copied?: string[] }).__copied))
      .toEqual([PRIMARY_ROOT]);
  });
});

test.describe("reopen closed tab", () => {
  test("Shift+Cmd+T reopens a shell tab at its place with its title", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 2);
    await strip(page).getByRole("button", { name: "New tab" }).click();
    await expect(page.getByRole("menuitem", { name: /Reopen closed tab/ })).toBeDisabled();
    await page.keyboard.press("Escape");

    await stripTab(page, "Terminal 1").dblclick();
    const input = strip(page).locator('[data-tab-rename-input="true"]');
    await input.fill("dev server");
    await page.keyboard.press("Enter");
    const closedKey = await stripTab(page, "dev server").getAttribute("data-workspace-tab-key");
    await stripTab(page, "dev server").locator(".workspace-tab-close").click();
    await expect.poll(() => tabTitles(page)).toEqual(["Audit agent refactor", "Terminal 2"]);

    await strip(page).getByRole("button", { name: "New tab" }).click();
    await expect(page.getByRole("menuitem", { name: /Reopen closed tab/ })).toBeEnabled();
    await page.keyboard.press("Escape");

    await page.keyboard.press("Meta+Shift+T");
    await expect.poll(() => tabTitles(page)).toEqual(["Audit agent refactor", "dev server", "Terminal 2"]);
    await expect(activeTab(page).locator(".workspace-tab-title")).toHaveText("dev server");
    // A new shell tab with a new shell.
    expect(await activeTab(page).getAttribute("data-workspace-tab-key")).not.toBe(closedKey);
    await expect(activeBodyPanes(page)).toHaveCount(1);
    // The stack is empty now.
    await page.keyboard.press("Meta+Shift+T");
    await expect.poll(() => tabTitles(page)).toEqual(["Audit agent refactor", "dev server", "Terminal 2"]);
  });

  test("a reopened conversation tab resumes its parked agent once", async ({ page }) => {
    await plantParkedPanel(page);
    await openWorkspaceTabsWithShells(page);
    await stripTab(page, "Parked login spike").click();
    await expect.poll(async () => (await createRequests(page)).filter((id) => id === "cli-parked").length).toBe(1);
    const order = await tabTitles(page);
    await activeTab(page).locator(".workspace-tab-close").click();
    await expect(stripTab(page, "Parked login spike")).toHaveCount(0);

    await page.keyboard.press("Meta+Shift+T");
    await expect.poll(() => tabTitles(page)).toEqual(order);
    await expect(activeTab(page)).toHaveText("Parked login spike");
    await expect.poll(async () => (await createRequests(page)).filter((id) => id === "cli-parked").length).toBe(2);
  });
});

test.describe("Ctrl+Tab and the workspace chords", () => {
  function switcher(page: Page) {
    return page.getByRole("listbox", { name: "Switch tabs" });
  }

  test("Ctrl+Tab lists recent tabs while Ctrl is down; the release shows the selection", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 2);
    await activeBodyPanes(page).first().locator(".xterm-helper-textarea").focus();
    await page.evaluate(() => {
      (window as typeof window & { __harnessTerminalWrites?: unknown[] }).__harnessTerminalWrites = [];
    });
    await page.keyboard.down("Control");
    await page.keyboard.press("Tab");
    await expect(switcher(page)).toBeVisible();
    await expect(switcher(page).getByRole("option")).toHaveText([/Terminal 2$/, /Terminal 1$/, "Audit agent refactor"]);
    await expect(switcher(page).getByRole("option", { selected: true })).toHaveText(/Terminal 1$/);
    await page.keyboard.press("Tab");
    await expect(switcher(page).getByRole("option", { selected: true })).toHaveText("Audit agent refactor");
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Shift+Tab");
    await expect(switcher(page).getByRole("option", { selected: true })).toHaveText(/Terminal 2$/);
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.up("Control");
    await expect(switcher(page)).toHaveCount(0);
    await expect(activeTab(page)).toHaveText("Audit agent refactor");
    // A quick press and release toggles back to the previous tab.
    await page.keyboard.press("Control+Tab");
    await expect(activeTab(page).locator(".workspace-tab-title")).toHaveText("Terminal 2");
    // Escape cancels.
    await page.keyboard.down("Control");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Escape");
    await expect(switcher(page)).toHaveCount(0);
    await page.keyboard.up("Control");
    await expect(activeTab(page).locator(".workspace-tab-title")).toHaveText("Terminal 2");
    // No key reached the shell.
    const writes = await page.evaluate(
      () =>
        ((window as typeof window & { __harnessTerminalWrites?: Array<{ data: string }> })
          .__harnessTerminalWrites ?? []).map((write) => write.data).join(""),
    );
    expect(writes).toBe("");
  });

  for (const theme of ["dark", "light"] as const) {
    test(`the switcher in the ${theme} theme`, async ({ page }) => {
      await openWorkspaceTabsWithShells(page, theme);
      await openShells(page, 2);
      await page.keyboard.down("Control");
      await page.keyboard.press("Tab");
      await expect(switcher(page)).toBeVisible();
      await expect(page).toHaveScreenshot(`workspace-tab-switcher-${theme}.png`, {
        clip: { x: 340, y: 60, width: 600, height: 240 },
      });
      await page.keyboard.up("Control");
    });
  }

  test("Option+Cmd+Down and Up move through the workspaces of the tree, with a wrap", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    const tablist = strip(page).getByRole("tablist");
    const current = page.locator('[role="treeitem"][aria-current="page"][data-tree-key^="workspace:"]');
    await expect(tablist).toHaveAttribute("aria-label", "Tabs in main");
    await page.keyboard.press("Meta+Alt+ArrowDown");
    await expect(tablist).toHaveAttribute("aria-label", "Tabs in halv/login-flow");
    await page.keyboard.press("Meta+Alt+ArrowDown");
    await expect(tablist).toHaveAttribute("aria-label", "Tabs in halv/docs-refresh");
    await page.keyboard.press("Meta+Alt+ArrowUp");
    await page.keyboard.press("Meta+Alt+ArrowUp");
    await expect(current).toHaveAttribute("data-tree-key", "workspace:primary-1");
    // Up from the first workspace wraps to the last one.
    await page.keyboard.press("Meta+Alt+ArrowUp");
    await expect(current).toHaveAttribute("data-tree-key", "workspace:primary-2");
    await page.keyboard.press("Meta+Alt+ArrowDown");
    await expect(current).toHaveAttribute("data-tree-key", "workspace:primary-1");
  });
});

test.describe("hover card", () => {
  function card(page: Page) {
    return page.locator("[data-workspace-tab-hover-card]");
  }

  test("a resting pointer opens the card after 500 ms; the next tab shows at once; a press hides it", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await openShells(page, 1);
    const audit = await centerOf(page, "Audit agent refactor");
    await page.mouse.move(audit.x, audit.y);
    await page.waitForTimeout(200);
    await expect(card(page)).toHaveCount(0);
    await expect(card(page)).toHaveCount(1);
    await expect(card(page).locator("[data-tab-hover-card-title]")).toHaveText("Audit agent refactor");
    await expect(card(page).locator("[data-tab-hover-card-program]")).toHaveText("Claude Code");
    await expect(card(page).locator("[data-tab-hover-card-state]")).toHaveText("Done");
    await expect(card(page).locator("[data-tab-hover-card-panes]")).toHaveText("1 pane");
    // Below the tab, aligned to its left edge.
    const box = (await card(page).boundingBox())!;
    expect(Math.abs(box.x - audit.box.x)).toBeLessThan(1);
    expect(Math.abs(box.y - (audit.box.y + audit.box.height))).toBeLessThan(1);

    // The strip is warm: the next tab shows its card at once.
    const shell = await centerOf(page, "Terminal 1");
    await page.mouse.move(shell.x, shell.y, { steps: 3 });
    await expect(card(page).locator("[data-tab-hover-card-title]")).toHaveText("Terminal 1");
    await expect(card(page).locator("[data-tab-hover-card-program]")).toHaveText(/zsh$/);

    // A press hides the card until the pointer leaves the tab.
    await page.mouse.down();
    await page.mouse.up();
    await expect(card(page)).toHaveCount(0);
    await page.waitForTimeout(700);
    await expect(card(page)).toHaveCount(0);
    await page.mouse.move(640, 600);
    await expect(card(page)).toHaveCount(0);
  });

  for (const theme of ["dark", "light"] as const) {
    test(`the hover card in the ${theme} theme`, async ({ page }) => {
      await openWorkspaceTabsWithShells(page, theme);
      const audit = await centerOf(page, "Audit agent refactor");
      await page.mouse.move(audit.x, audit.y);
      await expect(card(page)).toHaveCount(1);
      const header = (await page.locator("[data-app-header]").boundingBox())!;
      await expect(page).toHaveScreenshot(`workspace-tab-hover-${theme}.png`, {
        clip: { x: header.x, y: header.y, width: 640, height: 220 },
      });
    });
  }
});

test.describe("compact Pro header", () => {
  test("a tab body has one header: the pane title bar, with no panel header above it", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    const body = page.locator('.conversation-mode-surface[data-active="true"]');
    await expect(body.locator(".terminal-pane-title")).toHaveCount(1);
    await expect(body.locator(".terminal-header")).toHaveCount(0);
    await expect(body.locator(".terminal-identity")).toHaveCount(0);
    // The pane fills the body, flush under the strip.
    const header = (await page.locator("[data-app-header]").boundingBox())!;
    const pane = (await body.locator(".terminal-pane").first().boundingBox())!;
    expect(Math.abs(pane.y - (header.y + header.height))).toBeLessThan(2);
    // A shell tab body has no panel header either.
    await openShells(page, 1);
    await expect(body.locator(".terminal-header")).toHaveCount(0);
  });

  test("the savings meter of the visible agent tab moves to the app header", async ({ page }) => {
    await openWorkspaceTabsWithShells(page);
    await strip(page).getByRole("button", { name: "New tab" }).click();
    await page.getByRole("menuitem", { name: "Claude Code", exact: true }).click();
    const meter = page.locator(".workspace-header-meter").getByRole("region", { name: "Session savings meter" });
    await expect(meter).toHaveCount(1);
    const height = (await page.locator("[data-app-header]").boundingBox())!.height;
    // "Details" opens as a popover: the header keeps its height.
    await page.locator(".workspace-header-meter summary").click();
    await expect(page.locator(".workspace-header-meter .agent-session-savings-explanation")).toBeVisible();
    expect((await page.locator("[data-app-header]").boundingBox())!.height).toBe(height);
    // A shell tab has no meter.
    await openShells(page, 1);
    await expect(meter).toHaveCount(0);
  });

  test("two saved panes that cannot resume by themselves show one Resume all banner", async ({ page }) => {
    await page.addInitScript(
      ({ scope, root }) => {
        const layout = {
          version: 1,
          activeTabId: "parked-tab",
          activeLeafId: "pane-a",
          tabs: [
            {
              tabId: "parked-tab",
              root: {
                type: "split",
                direction: "vertical",
                ratio: 0.5,
                first: { type: "leaf", leafId: "pane-a" },
                second: { type: "leaf", leafId: "pane-b" },
              },
            },
          ],
        };
        const now = new Date().toISOString();
        const pane = (paneId: string) => ({
          paneId,
          sessionId: `${paneId}-session`,
          projectId: 1,
          conversationId: "audit-parked",
          projectName: "Halv Desktop",
          cwd: root,
          executionProfile: "full_access",
          agentKind: "claude",
          laneKind: "claudeSubscription",
          credentialProfileId: null,
          customTitle: null,
          // No CLI session id: the panes wait for a manual resume.
          cliSessionId: null,
          cols: 100,
          rows: 30,
          scrollback: "$ parked\r\n",
          updatedAt: now,
        });
        window.localStorage.setItem(scope, JSON.stringify(layout));
        window.localStorage.setItem(
          `ux-audit.terminal-restore:${scope}`,
          JSON.stringify({ layout, updatedAt: now, panes: [pane("pane-a"), pane("pane-b")] }),
        );
      },
      { scope: PARKED_SCOPE, root: PRIMARY_ROOT },
    );
    await openWorkspaceTabsWithShells(page);
    await stripTab(page, "Parked login spike").click();
    const banner = page.locator('.conversation-mode-surface[data-active="true"] .terminal-resume-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText("2 saved panes can resume.");
    await expect(banner.getByRole("button", { name: "Resume all" })).toBeEnabled();
  });

  for (const theme of ["dark", "light"] as const) {
    test(`the agent tab body in the ${theme} theme`, async ({ page }) => {
      await openWorkspaceTabsWithShells(page, theme);
      await page.mouse.move(640, 700);
      await expect(page).toHaveScreenshot(`workspace-tab-body-${theme}.png`, {
        clip: { x: 0, y: 0, width: 1280, height: 260 },
      });
    });
  }
});

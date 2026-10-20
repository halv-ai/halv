import { expect, test, type Page } from "@playwright/test";
import {
  activeTab,
  openWorkspaceTabs,
  openWorkspaceTabsWithShells,
  strip,
  tabKeys,
  treeRow,
} from "./workspaceTabsProbe";

// The launcher of an empty workspace in Pro mode (ADR 0020): one click on
// a project opens its primary workspace, and an empty strip asks for an
// agent, a terminal or a text conversation. Each choice opens one tab.

function launcher(page: Page) {
  return page.locator("[data-workspace-launcher]");
}

function launcherItem(page: Page, key: string) {
  return launcher(page).locator(`[data-launcher-item="${key}"]`);
}

function treeItem(page: Page, key: string) {
  return page.locator(`[role="treeitem"][data-tree-key="${key}"]`);
}

for (const theme of ["dark", "light"] as const) {
  test(`a project click opens the launcher of its primary workspace in the ${theme} theme`, async ({ page }) => {
    await openWorkspaceTabsWithShells(page, theme);
    await treeRow(page, "project:2").click();
    await expect(treeItem(page, "workspace:primary-2")).toHaveAttribute("aria-current", "page");
    await expect(treeItem(page, "project:2")).toHaveAttribute("aria-expanded", "true");
    await expect(launcher(page)).toBeVisible();
    await expect(strip(page).locator(".workspace-tab")).toHaveCount(0);
    // No composer and no draft tab: the launcher is the whole body.
    await expect(page.getByRole("textbox", { name: "Message" })).toHaveCount(0);

    const keys = await launcher(page)
      .locator("[data-launcher-item]")
      .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-launcher-item")));
    expect(keys.slice(-2)).toEqual(["terminal", "text"]);
    expect(keys[0]).toMatch(/^agent:/);
    await expect(launcherItem(page, "terminal")).toContainText("Terminal");
    await expect(launcherItem(page, "terminal").locator("kbd")).toHaveText("⌘T");
    await expect(launcherItem(page, keys[0]!).locator("kbd")).toHaveText("⌥⌘T");
    await expect(launcherItem(page, "text")).toContainText("Text conversation");
    // The project click moves the focus to the first choice.
    await expect(launcherItem(page, keys[0]!)).toBeFocused();
    await expect(launcher(page)).toHaveScreenshot(`workspace-launcher-${theme}.png`);
  });
}

test("the keyboard moves through the launcher, and Enter on Terminal opens a shell tab", async ({ page }) => {
  await openWorkspaceTabsWithShells(page);
  await treeRow(page, "project:2").click();
  await expect(launcher(page)).toBeVisible();
  await page.keyboard.press("End");
  await expect(launcherItem(page, "text")).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(launcherItem(page, "terminal")).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(launcher(page)).toHaveCount(0);
  expect(await tabKeys(page)).toEqual([expect.stringMatching(/^shell:/)]);
  await expect(activeTab(page)).toHaveAttribute("data-active", "true");
});

test("an agent choice opens an agent tab in the workspace", async ({ page }) => {
  await openWorkspaceTabsWithShells(page);
  await treeRow(page, "project:2").click();
  const first = launcher(page).locator("[data-launcher-item^='agent:']").first();
  await first.click();
  await expect(launcher(page)).toHaveCount(0);
  expect(await tabKeys(page)).toEqual([expect.stringMatching(/^conversation:/)]);
  await expect(
    page.locator('.conversation-mode-surface[data-active="true"] .terminal-pane[data-terminal-session-id]'),
  ).toHaveCount(1);
});

test("Text conversation starts a chat conversation, also in Pro mode", async ({ page }) => {
  await openWorkspaceTabsWithShells(page);
  await treeRow(page, "project:2").click();
  await launcherItem(page, "text").click();
  const draftTab = strip(page).locator('.workspace-tab[data-draft="true"]');
  await expect(draftTab).toHaveText("Text conversation");
  const composer = page.getByRole("textbox", { name: "Message" });
  await expect(composer).toBeFocused();
  await composer.pressSequentially("Explain the token scale", { delay: 5 });
  await expect(draftTab).toHaveCount(0);
  await expect(activeTab(page)).toHaveText("Explain the token scale");
  await expect(activeTab(page).locator('[data-agent-kind="chat"]')).toBeVisible();
  // The close of an empty draft goes back to the launcher.
  await treeRow(page, "workspace:audit-wt-login").click();
  await page.getByRole("button", { name: "New tab" }).click();
  await page.getByRole("menuitem", { name: "Text conversation" }).click();
  await expect(strip(page).locator('.workspace-tab[data-draft="true"]')).toHaveText("Text conversation");
});

test("closing an empty text draft goes back to the launcher", async ({ page }) => {
  await openWorkspaceTabsWithShells(page);
  await treeRow(page, "project:2").click();
  await launcherItem(page, "text").click();
  const draftTab = strip(page).locator('.workspace-tab[data-draft="true"]');
  await expect(draftTab).toBeVisible();
  await draftTab.locator(".workspace-tab-close").click();
  await expect(launcher(page)).toBeVisible();
  await expect(draftTab).toHaveCount(0);
});

test("a command prefill on the launcher opens the composer with the command", async ({ page }) => {
  await openWorkspaceTabsWithShells(page);
  await treeRow(page, "project:2").click();
  await expect(launcher(page)).toBeVisible();
  await page.keyboard.press("Meta+k");
  await page.getByRole("combobox", { name: "Search commands" }).fill("model");
  await page.locator("#halv-command-option-agent\\.model").click();
  await expect(launcher(page)).toHaveCount(0);
  const composer = page.getByRole("textbox", { name: "Message" });
  await expect(composer).toBeVisible();
  await expect(composer).toHaveText("/model");
  // The draft holds text, so its tab reads like a typed draft.
  await expect(activeTab(page)).toHaveText("/model");
});

test("with Pro mode off an empty workspace keeps the composer", async ({ page }) => {
  await openWorkspaceTabs(page);
  await page.getByRole("switch", { name: "New conversations open in Pro mode" }).click();
  await expect(
    page.getByRole("switch", { name: "New conversations open in Pro mode" }),
  ).toHaveAttribute("aria-checked", "false");
  await treeRow(page, "project:2").click();
  await expect(launcher(page)).toHaveCount(0);
  await expect(strip(page).locator('.workspace-tab[data-draft="true"]')).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.getByRole("textbox", { name: "Message" })).toBeVisible();
});

test("the chevron collapses a project without opening it", async ({ page }) => {
  await openWorkspaceTabsWithShells(page);
  await expect(treeItem(page, "workspace:primary-1")).toHaveAttribute("aria-current", "page");
  await treeItem(page, "project:2").locator(":scope > .project-tree-row .project-tree-project-chevron").click();
  await expect(treeItem(page, "project:2")).toHaveAttribute("aria-expanded", "false");
  await expect(treeItem(page, "workspace:primary-1")).toHaveAttribute("aria-current", "page");
  await expect(launcher(page)).toHaveCount(0);
  // A click on the collapsed row opens it and shows its workspace.
  await treeRow(page, "project:2").click();
  await expect(treeItem(page, "project:2")).toHaveAttribute("aria-expanded", "true");
  await expect(treeItem(page, "workspace:primary-2")).toHaveAttribute("aria-current", "page");
});

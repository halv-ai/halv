import { expect, test } from "@playwright/test";
import {
  activeTab,
  expectStableLayout,
  focusedBodyKey,
  openFromSearch,
  openWorkspaceTabs,
  startLayoutProbe,
  stopLayoutProbe,
  strip,
  stripTab,
  tabBody,
  tabTitles,
  treeRow,
} from "./workspaceTabsProbe";

// The flows that must feel clean with the workspace strip on (ADR 0010,
// stages S4a and S4b): start a conversation, send its first message,
// switch conversations, open one from the tree and from the search, and open
// a chat conversation inside its project. Every flow samples the layout
// on each frame: no half-open terminal, no layout jump, the right tab
// active, and the focus where the user expects it.

for (const theme of ["dark", "light"] as const) {
  test(`a new task becomes a tab and its first message starts the agent in place (${theme})`, async ({ page }) => {
    await openWorkspaceTabs(page, theme, "slow=2500");
    await expect(activeTab(page)).toHaveText("Audit agent refactor");
    await startLayoutProbe(page);

    await page.getByRole("button", { name: "New chat" }).click();
    const draftTab = strip(page).locator('.workspace-tab[data-draft="true"]');
    await expect(draftTab).toHaveAttribute("aria-selected", "true");
    await expect(draftTab).toHaveText("New task");
    const composer = page.getByRole("textbox", { name: "Message" });
    await expect(composer).toBeFocused();
    await expect(page).toHaveScreenshot(`workspace-flow-new-task-${theme}.png`);

    // The first character creates the conversation. Its tab replaces the
    // draft tab in place and follows the text.
    await composer.pressSequentially("Fix the login redirect", { delay: 5 });
    await expect(draftTab).toHaveCount(0);
    await expect(activeTab(page)).toHaveText("Fix the login redirect");
    expect(await tabTitles(page)).toEqual(["Audit agent refactor", "Fix the login redirect"]);
    await expect(composer).toBeFocused();
    await expect(composer).toHaveValue("Fix the login redirect");

    // The send shows the terminal frame at once. It says what starts.
    await page.getByRole("button", { name: "Send" }).first().click();
    const starting = page.locator('[data-terminal-starting="true"]');
    await expect(starting).toBeVisible();
    await expect(starting).toContainText(/Starting (Codex|Claude)…/);
    await expect(page).toHaveScreenshot(`workspace-flow-first-send-starting-${theme}.png`);

    const pane = page.locator('.conversation-mode-surface[data-active="true"] .terminal-pane[data-terminal-session-id]');
    await expect(pane).toHaveCount(1);
    await expect(starting).toHaveCount(0);
    await expect(activeTab(page)).toHaveText("Fix the login redirect");
    // The message rode along with the spawn.
    expect(
      await page.evaluate(() =>
        ((window as typeof window & { __harnessInitialPrompts?: Array<{ prompt: string }> })
          .__harnessInitialPrompts ?? []).map((entry) => entry.prompt),
      ),
    ).toContain("Fix the login redirect");
    await expect(page.getByRole("textbox", { name: "Message" })).toHaveCount(0);
    expectStableLayout(await stopLayoutProbe(page));
    await expect(page).toHaveScreenshot(`workspace-flow-first-send-running-${theme}.png`, {
      mask: [page.locator(".xterm-cursor-layer")],
    });
  });

  test(`switching tabs never shows the previous conversation or moves the layout (${theme})`, async ({ page }) => {
    await openWorkspaceTabs(page, theme);
    // A chat conversation and an agent conversation share the strip.
    await openFromSearch(page, "release verification", "Release checklist");
    await expect(activeTab(page)).toHaveText("Release checklist");
    await page.getByRole("button", { name: "New tab" }).click();
    await page.getByRole("menuitem", { name: "Text conversation" }).click();
    const composer = page.getByRole("textbox", { name: "Message" });
    await composer.pressSequentially("Draft the release notes", { delay: 5 });
    await expect(activeTab(page)).toHaveText("Draft the release notes");

    await startLayoutProbe(page);
    for (let round = 0; round < 3; round += 1) {
      await stripTab(page, "Release checklist").click();
      await expect(page.getByText("Prepare the release verification checklist.")).toBeVisible();
      await expect(composer).toHaveValue("");
      await stripTab(page, "Draft the release notes").click();
      await expect(page.getByText("Prepare the release verification checklist.")).toHaveCount(0);
      await expect(composer).toHaveValue("Draft the release notes");
    }
    expectStableLayout(await stopLayoutProbe(page));
    await expect(composer).toBeFocused();
    await stripTab(page, "Release checklist").click();
    await expect(page.getByText("Prepare the release verification checklist.")).toBeVisible();
    await expect(page).toHaveScreenshot(`workspace-flow-switch-chat-${theme}.png`);
  });

  test(`a tree row opens its tab in its workspace and focuses its pane (${theme})`, async ({ page }) => {
    await openWorkspaceTabs(page, theme);
    await startLayoutProbe(page);
    await treeRow(page, "conversation:audit-docs-sweep").click();
    await expect(strip(page).getByRole("tablist")).toHaveAttribute("aria-label", /halv\/docs-refresh/);
    await expect(activeTab(page)).toHaveText("Docs refresh sweep");
    await expect(tabBody(page, "audit-docs-sweep")).toHaveAttribute("data-active", "true");
    await expect.poll(() => focusedBodyKey(page)).toBe("conversation:audit-docs-sweep");
    // The tree marks the visible tab and its workspace.
    await expect(page.locator('[role="treeitem"][data-tree-key="conversation:audit-docs-sweep"]')).toHaveAttribute(
      "aria-current",
      "page",
    );

    // Back to the primary workspace: its strip still has its own tab.
    await treeRow(page, "workspace:primary-1").click();
    await expect(activeTab(page)).toHaveText("Audit agent refactor");
    await expect.poll(() => focusedBodyKey(page)).toBe("conversation:audit-agent-refactor");
    expectStableLayout(await stopLayoutProbe(page));
    await treeRow(page, "conversation:audit-docs-sweep").click();
    await expect(activeTab(page)).toHaveText("Docs refresh sweep");
    await expect(page).toHaveScreenshot(`workspace-flow-tree-open-${theme}.png`, {
      mask: [page.locator(".xterm-cursor-layer")],
    });
  });

  test(`a search hit opens its tab, and a conversation without a project opens loose (${theme})`, async ({ page }) => {
    await openWorkspaceTabs(page, theme);
    const search = page.getByRole("searchbox", { name: "Search conversations" });
    await search.fill("managed proxy");
    await startLayoutProbe(page);
    await page.getByRole("button", { name: "Proxy restart investigation", exact: true }).first().click();
    await expect(activeTab(page)).toHaveText(/Proxy restart/);
    await expect(strip(page).getByRole("tablist")).toHaveAttribute("aria-label", /halv\/login-flow/);
    await expect.poll(() => focusedBodyKey(page)).toBe("conversation:audit-proxy");
    expectStableLayout(await stopLayoutProbe(page));
    await expect(page).toHaveScreenshot(`workspace-flow-search-open-${theme}.png`, {
      mask: [page.locator(".xterm-cursor-layer")],
    });

    // A chat without a project has no strip: the header shows its title.
    await search.fill("");
    await page
      .getByRole("tree", { name: "Chats", exact: true })
      .locator('[role="treeitem"]', { hasText: "Streaming markdown polish" })
      .locator(":scope > .project-tree-row")
      .click();
    await expect(strip(page)).toHaveCount(0);
    await expect(page.locator("[data-app-header]").getByText("Streaming markdown polish")).toBeVisible();
    await expect(page.getByText("Use a root HTML entry in development:")).toBeVisible();
  });

  test(`a chat conversation in a project opens as a chat tab without a terminal (${theme})`, async ({ page }) => {
    await openWorkspaceTabs(page, theme);
    await startLayoutProbe(page);
    // The click leaves the focus in the composer, so the search stays.
    await openFromSearch(page, "release verification", "Release checklist", { clear: false });
    await expect(strip(page).getByRole("tablist")).toHaveAttribute("aria-label", /Tabs in main/);
    await expect(activeTab(page)).toHaveText("Release checklist");
    await expect(activeTab(page).locator('[data-agent-kind="chat"]')).toBeVisible();
    // The search hit stays in the sidebar; the message shows in the body.
    await expect(
      page
        .locator('.conversation-chat-mode-surface[data-active="true"]')
        .getByText("Prepare the release verification checklist.", { exact: true }),
    ).toBeVisible();
    const composer = page.getByRole("textbox", { name: "Message" });
    await expect(composer).toBeVisible();
    await expect(composer).toBeFocused();
    expectStableLayout(await stopLayoutProbe(page));
    await expect(page.locator('.terminal-diff-surface[data-open="true"]')).toHaveCount(0);
    await expect(page).toHaveScreenshot(`workspace-flow-chat-in-project-${theme}.png`);
  });
}

test("a chat conversation's first message keeps its tab and never shows a loading gap", async ({ page }) => {
  await openWorkspaceTabs(page);
  // Pro off: a new task is a chat conversation.
  await page.getByRole("switch", { name: "New conversations open in Pro mode" }).click();
  await expect(page.getByRole("switch", { name: "New conversations open in Pro mode" })).toHaveAttribute(
    "aria-checked",
    "false",
  );
  await page.getByRole("button", { name: "New chat" }).click();
  const composer = page.getByRole("textbox", { name: "Message" });
  await expect(composer).toBeFocused();
  await composer.pressSequentially("Explain the login redirect", { delay: 5 });
  await expect(activeTab(page)).toHaveText("Explain the login redirect");
  await expect(activeTab(page).locator('[data-agent-kind="chat"]')).toBeVisible();
  await startLayoutProbe(page);
  await page.getByRole("button", { name: "Send" }).first().click();
  await expect(page.getByText("Explain the login redirect").last()).toBeVisible();
  await expect(page.getByText("Browser audit response")).toBeVisible();
  const frames = await stopLayoutProbe(page);
  expectStableLayout(frames);
  // The chat surface stays: no terminal body and no loading state.
  expect(frames.every((frame) => frame.activeBodyKeys.every((key) => key === "chat"))).toBe(true);
  await expect(page.locator(".workspace-chat-loading")).toHaveCount(0);
  await expect(activeTab(page)).toHaveText("Explain the login redirect");
  expect(await tabTitles(page)).toEqual(["Audit agent refactor", "Explain the login redirect"]);
});

import { expect, test, type Page } from "@playwright/test";
import { openVisualState } from "./capture";

// The Projects tree (ADR 0010, stage S1) behind the projectTree flag.

const TREE_QUERY = "flags=projectTree&sidebar=projects";

function tree(page: Page) {
  return page.getByRole("tree", { name: "Projects", exact: true });
}

// Rows by their tree key, for example `workspace:primary-1`.
function item(page: Page, key: string) {
  return page.locator(`[role="treeitem"][data-tree-key="${key}"]`);
}

function row(page: Page, key: string) {
  return item(page, key).locator(":scope > .project-tree-row");
}

function sidebar(page: Page) {
  return page.getByRole("complementary", { name: "Conversation navigation" });
}

async function waitForHarness(page: Page, state: string) {
  await page.waitForFunction(
    (expected) => document.documentElement.dataset.visualReady === expected,
    state,
  );
}

for (const theme of ["dark", "light"] as const) {
  test(`the tree lists projects, workspaces and agent rows in the ${theme} theme`, async ({ page }) => {
    await openVisualState(page, `project-tree-${theme}`, TREE_QUERY);

    await expect(sidebar(page).getByRole("heading", { name: "Projects", exact: true })).toBeVisible();
    const project = item(page, "project:1");
    await expect(project).toHaveAccessibleName("Halv Desktop");
    await expect(project).toHaveAttribute("aria-level", "1");
    await expect(project).toHaveAttribute("aria-expanded", "true");
    await expect(item(page, "project:2")).toBeVisible();

    const primary = item(page, "workspace:primary-1");
    await expect(primary).toHaveAttribute("aria-level", "2");
    await expect(primary).toHaveAccessibleName("Done main primary");
    await expect(primary.locator(".project-tree-primary-badge")).toHaveAttribute(
      "title",
      "Primary workspace (project root)",
    );
    const login = item(page, "workspace:audit-wt-login");
    const docs = item(page, "workspace:audit-wt-docs");
    await expect(login).toBeVisible();
    await expect(docs).toBeVisible();

    // Each workspace lane shows its strongest row; a question outranks work.
    await expect(login.locator(':scope > .project-tree-row [data-agent-glyph="attention"]')).toBeVisible();
    await expect(docs.locator(':scope > .project-tree-row [data-agent-glyph="working"]')).toBeVisible();

    const question = item(page, "conversation:audit-proxy");
    await expect(question).toHaveAttribute("aria-level", "3");
    await expect(question.locator('[data-agent-glyph="attention"]')).toBeVisible();
    await expect(question).toContainText("Keep the old login route as a redirect?");
    await expect(question.locator('[data-agent-kind="claude"]')).toBeVisible();
    await expect(question).toContainText("58m");

    const working = item(page, "conversation:audit-docs-sweep");
    await expect(working.locator('[data-agent-glyph="working"]')).toBeVisible();
    await expect(working.locator('[data-agent-kind="codex"]')).toBeVisible();
    await expect(working).toContainText("3m");

    const finished = item(page, "conversation:audit-agent-refactor");
    await expect(finished.locator('[data-agent-glyph="done"]')).toBeVisible();
    await expect(finished).toContainText("18m");

    // An agent terminal without a conversation sits in the workspace of its
    // cwd. A plain shell is not a workspace row: without shell tabs it
    // waits in the Chats area (ADR 0020).
    await expect(docs.locator('[data-tree-key="terminal:tree-codex-loose"]')).toContainText("Codex 2");
    await expect(tree(page).getByRole("treeitem", { name: /zsh/ })).toHaveCount(0);
    await expect(sidebar(page).getByText("Other terminals", { exact: true })).toHaveCount(0);

    const chats = page.getByRole("tree", { name: "Chats", exact: true });
    await expect(chats.getByRole("treeitem", { name: /zsh/ })).toBeVisible();
    await expect(chats.getByRole("treeitem", { name: /Streaming markdown polish/ })).toBeVisible();
    await expect(chats.locator('[data-agent-kind="chat"]').first()).toBeVisible();

    await expect(sidebar(page)).toHaveScreenshot(`project-tree-${theme}.png`);
  });
}

test("without the flag the sidebar keeps the classic list", async ({ page }) => {
  await openVisualState(page, "project-tree-dark");
  await expect(tree(page)).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Projects", exact: true })).toHaveCount(0);
  await expect(sidebar(page).getByRole("heading", { name: "Conversations" })).toBeVisible();
  await expect(sidebar(page).getByText("Terminals", { exact: true })).toBeVisible();
});

test("a viewer with conversations sees only the Projects tree, also after a reload", async ({ page }) => {
  // ADR 0020 removed the Recent view. Search finds every conversation.
  await openVisualState(page, "project-tree-dark", "flags=projectTree");
  await expect(tree(page)).toBeVisible();
  await expect(page.getByRole("button", { name: "Recent", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Projects", exact: true })).toHaveCount(0);
  await expect(sidebar(page).getByRole("heading", { name: "Projects", exact: true })).toBeVisible();
  await page.reload();
  await waitForHarness(page, "project-tree-dark");
  await expect(tree(page)).toBeVisible();
  await expect(page.getByRole("button", { name: "Recent", exact: true })).toHaveCount(0);
});

for (const theme of ["dark", "light"] as const) {
  test(`a new viewer starts on an empty Projects view in the ${theme} theme`, async ({ page }) => {
    await openVisualState(page, `project-tree-new-${theme}`, "flags=projectTree");
    await expect(page.getByRole("button", { name: "Recent", exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Projects", exact: true })).toHaveCount(0);
    await expect(sidebar(page).getByRole("heading", { name: "Projects", exact: true })).toBeVisible();
    await expect(sidebar(page).getByText("No projects yet", { exact: true })).toBeVisible();
    await expect(sidebar(page).getByRole("button", { name: "Add folder...", exact: true })).toBeVisible();
    await expect(
      sidebar(page).getByRole("button", { name: "Clone from GitHub...", exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("tree", { name: "Chats", exact: true })).toHaveCount(0);
    // The view holds after a reload.
    await page.reload();
    await waitForHarness(page, `project-tree-new-${theme}`);
    await expect(page.getByRole("button", { name: "Recent", exact: true })).toHaveCount(0);
    await expect(sidebar(page)).toHaveScreenshot(`project-tree-new-${theme}.png`);
  });
}

test("a stored Recent view of an older build shows the Projects tree", async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "halv.sidebar.project-tree.v1",
      JSON.stringify({ version: 1, view: "recent", recentList: "legacy", collapsed: [], expanded: [] }),
    );
  });
  await openVisualState(page, "project-tree-dark", "flags=projectTree");
  await expect(tree(page)).toBeVisible();
  await expect(page.getByRole("button", { name: "Recent", exact: true })).toHaveCount(0);
  await expect(sidebar(page).getByRole("heading", { name: "Projects", exact: true })).toBeVisible();
});

test("a collapsed project stays collapsed after a reload", async ({ page }) => {
  await openVisualState(page, "project-tree-dark", TREE_QUERY);
  const design = item(page, "project:2");
  // A row click opens the project (ADR 0020). The chevron collapses it.
  await design.locator(":scope > .project-tree-row .project-tree-project-chevron").click();
  await expect(design).toHaveAttribute("aria-expanded", "false");
  await expect(item(page, "conversation:audit-release")).toHaveCount(0);
  await page.reload();
  await waitForHarness(page, "project-tree-dark");
  await expect(item(page, "project:2")).toHaveAttribute("aria-expanded", "false");
  await expect(item(page, "project:1")).toHaveAttribute("aria-expanded", "true");
});

test("a project click selects its primary workspace and expands it", async ({ page }) => {
  await openVisualState(page, "project-tree-dark", TREE_QUERY);
  const design = item(page, "project:2");
  await design.locator(":scope > .project-tree-row .project-tree-project-chevron").click();
  await expect(design).toHaveAttribute("aria-expanded", "false");
  await row(page, "project:2").click();
  await expect(design).toHaveAttribute("aria-expanded", "true");
  await expect(item(page, "workspace:primary-2")).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("button", { name: "Workspace: main" })).toBeVisible();
  // A second click keeps the project open.
  await row(page, "project:2").click();
  await expect(design).toHaveAttribute("aria-expanded", "true");
});

test("a workspace click selects it, also across projects", async ({ page }) => {
  await openVisualState(page, "project-tree-dark", TREE_QUERY);
  const login = item(page, "workspace:audit-wt-login");
  await login.locator(":scope > .project-tree-row").click();
  await expect(login).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("button", { name: "Workspace: halv/login-flow" })).toBeVisible();

  const designPrimary = item(page, "workspace:primary-2");
  await row(page, "workspace:primary-2").click();
  await expect(designPrimary).toHaveAttribute("aria-current", "page");
  await expect(login).not.toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("button", { name: "Workspace: main" })).toBeVisible();

  // Back to a worktree of the first project: the pending choice wins over
  // the primary workspace that the project switch loads first.
  const docs = item(page, "workspace:audit-wt-docs");
  await docs.locator(":scope > .project-tree-row").click();
  await expect(docs).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("button", { name: "Workspace: halv/docs-refresh" })).toBeVisible();
});

test("an agent row opens its conversation and marks its workspace", async ({ page }) => {
  await openVisualState(page, "project-tree-dark", TREE_QUERY);
  const docsRow = item(page, "conversation:audit-docs-sweep");
  await row(page, "conversation:audit-docs-sweep").click();
  await expect(docsRow).toHaveAttribute("aria-current", "page");
  await expect(item(page, "workspace:audit-wt-docs")).toHaveAttribute("aria-current", "page");
  await expect(page.locator("header").getByText("Docs refresh sweep")).toBeVisible();
});

test("the keyboard moves, collapses, expands and activates rows", async ({ page }) => {
  await openVisualState(page, "project-tree-dark", TREE_QUERY);
  const project = item(page, "project:1");
  // One tab stop: the current workspace row holds it.
  await expect(item(page, "workspace:primary-1")).toHaveAttribute("tabindex", "0");
  await expect(project).toHaveAttribute("tabindex", "-1");
  await expect(tree(page).locator('[role="treeitem"][tabindex="0"]')).toHaveCount(1);
  await project.focus();
  await page.keyboard.press("ArrowDown");
  await expect(item(page, "workspace:primary-1")).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  await expect(item(page, "workspace:primary-1")).toHaveAttribute("aria-expanded", "false");
  await page.keyboard.press("ArrowLeft");
  await expect(project).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  await expect(project).toHaveAttribute("aria-expanded", "false");
  await page.keyboard.press("ArrowRight");
  await expect(project).toHaveAttribute("aria-expanded", "true");
  await page.keyboard.press("ArrowRight");
  await expect(item(page, "workspace:primary-1")).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await expect(item(page, "workspace:primary-1")).toHaveAttribute("aria-expanded", "true");

  await item(page, "workspace:audit-wt-docs").focus();
  await page.keyboard.press("ArrowDown");
  const docsRow = item(page, "conversation:audit-docs-sweep");
  await expect(docsRow).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(docsRow).toHaveAttribute("aria-current", "page");
  await page.keyboard.press("End");
  await expect(tree(page).getByRole("treeitem").last()).toBeFocused();
  await page.keyboard.press("Home");
  await expect(project).toBeFocused();
});

test("the tree fits a 375 px window with no horizontal scroll", async ({ page }) => {
  // The harness waits for a row text, so the sidebar opens at desktop
  // width first. At 375 px it becomes the overlay.
  await openVisualState(page, "project-tree-light", TREE_QUERY);
  await page.setViewportSize({ width: 375, height: 760 });
  const open = page.getByRole("button", { name: "Open conversation history" });
  if (await open.isVisible()) await open.click();
  await expect(tree(page)).toBeVisible();
  await expect(sidebar(page)).toHaveCSS("position", "absolute");
  const overflow = await page.evaluate(() => {
    const nav = document.querySelector<HTMLElement>('nav[aria-label="Conversations"]')!;
    return {
      page: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      nav: nav.scrollWidth - nav.clientWidth,
    };
  });
  expect(overflow).toEqual({ page: 0, nav: 0 });
});

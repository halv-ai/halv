import { expect, test, type Page } from "@playwright/test";
import { openVisualState } from "./capture";
import { activeTab, strip } from "./workspaceTabsProbe";

// The parity polish of the Projects tree (ADR 0010, stage S7): terminal
// adoption and "Open in tab" (ADR 0020), the row menus, worktree actions,
// project groups, per-pane rows and the History of a workspace.

const ALL_FLAGS = "projectTree,workspaceTabs,shellTabs";

type ListedTerminal = { id: string; status: string; shellTabId?: string | null };

async function openTree(
  page: Page,
  options: { theme?: "dark" | "light"; flags?: string; query?: string } = {},
) {
  const query = `flags=${options.flags ?? ALL_FLAGS}&sidebar=projects&ghostty=off`;
  await openVisualState(
    page,
    options.theme === "light" ? "workspace-tabs-light" : "workspace-tabs-dark",
    options.query ? `${query}&${options.query}` : query,
  );
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
}

function sidebar(page: Page) {
  return page.getByRole("complementary", { name: "Conversation navigation" });
}

function item(page: Page, key: string) {
  return page.locator(`[role="treeitem"][data-tree-key="${key}"]`);
}

async function invoke(page: Page, command: string, args: Record<string, unknown> = {}) {
  return page.evaluate(
    ({ command, args }) =>
      (
        window as typeof window & {
          __TAURI_INTERNALS__: { invoke: (name: string, args: unknown) => Promise<unknown> };
        }
      ).__TAURI_INTERNALS__.invoke(command, args),
    { command, args },
  );
}

async function listed(page: Page) {
  return (await invoke(page, "terminal_list")) as ListedTerminal[];
}

test("an unowned terminal in a project folder becomes a tab of its workspace, without focus", async ({ page }) => {
  await openTree(page);
  const openRequests = () =>
    page.evaluate(
      () =>
        (window as typeof window & { __harnessTerminalOpenRequests?: unknown[] })
          .__harnessTerminalOpenRequests?.length ?? 0,
    );
  // The shell in the project root joins the primary strip, and the agent
  // terminal without a conversation joins the strip of its worktree
  // (ADR 0020). Each keeps its session: no new shell starts.
  await expect
    .poll(async () => (await listed(page)).find((row) => row.id === "tree-shell")?.shellTabId ?? null)
    .not.toBeNull();
  await expect
    .poll(async () => (await listed(page)).find((row) => row.id === "tree-codex-loose")?.shellTabId ?? null)
    .not.toBeNull();
  expect(await openRequests()).toBe(0);
  await expect(strip(page).locator(".workspace-tab")).toContainText(["Audit agent refactor", "zsh"]);
  // The view stays: the same workspace and the same active tab.
  await expect(item(page, "workspace:primary-1")).toHaveAttribute("aria-current", "page");
  await expect(activeTab(page)).toContainText("Audit agent refactor");
  // Nothing is left in the Chats area.
  const chats = page.getByRole("tree", { name: "Chats", exact: true });
  await expect(chats.locator('[data-tree-key^="terminal:"]')).toHaveCount(0);

  await item(page, "workspace:audit-wt-docs").locator(":scope > .project-tree-row").click();
  await expect(strip(page).locator(".workspace-tab")).toContainText(["Codex 2"]);

  // A reload adopts nothing twice.
  const tabsBefore = await page.evaluate(() =>
    document.querySelectorAll("[data-workspace-tab-key]").length,
  );
  await page.reload();
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
  await page.waitForTimeout(300);
  expect(
    await page.evaluate(() => document.querySelectorAll("[data-workspace-tab-key]").length),
  ).toBe(tabsBefore);
});

test("a terminal in a git repository outside every project adds the project; a home shell stays in Chats", async ({ page }) => {
  await openTree(page, { query: "adoption=1" });
  // The repository root becomes a project, and its shell a tab there.
  const notes = page.locator('[role="treeitem"][data-tree-key^="project:"]', { hasText: "orca-notes" });
  await expect(notes).toHaveCount(1);
  await expect
    .poll(async () => (await listed(page)).find((row) => row.id === "adoption-shell-repo")?.shellTabId ?? null)
    .not.toBeNull();
  // The home folder has no repository: the shell stays in the Chats area.
  const chats = page.getByRole("tree", { name: "Chats", exact: true });
  await expect(chats.locator('[data-tree-key="terminal:adoption-shell-home"]')).toBeVisible();
  expect((await listed(page)).find((row) => row.id === "adoption-shell-home")?.shellTabId ?? null).toBeNull();
  // The view stays on the primary workspace of the first project.
  await expect(item(page, "workspace:primary-1")).toHaveAttribute("aria-current", "page");
  await expect(activeTab(page)).toContainText("Audit agent refactor");

  await notes.locator(":scope > .project-tree-row").click();
  await expect(strip(page).locator(".workspace-tab")).toContainText(["notes shell"]);
});

test("Open in tab moves a Chats terminal into the strip on screen", async ({ page }) => {
  await openTree(page, { query: "adoption=1" });
  await rowMenu(page, "terminal:adoption-shell-home");
  await treeMenu(page).getByRole("menuitem", { name: "Open in tab", exact: true }).click();
  await expect(activeTab(page)).toHaveAttribute("data-workspace-tab-key", /^shell:[0-9a-f-]{36}$/);
  const key = await activeTab(page).getAttribute("data-workspace-tab-key");
  await expect(activeTab(page)).toContainText("home shell");
  await expect(item(page, "workspace:primary-1")).toHaveAttribute("aria-current", "page");
  const body = page.locator(`[data-workspace-tab-body="${key}"]`);
  await expect(body.locator('.terminal-pane[data-terminal-session-id="adoption-shell-home"]')).toHaveCount(1);
  expect((await listed(page)).find((row) => row.id === "adoption-shell-home")?.shellTabId).toBe(
    key!.slice("shell:".length),
  );
  await expect(item(page, "terminal:adoption-shell-home")).toHaveCount(0);
});

test("without shell tabs a shell waits in the Chats area and has no Open in tab", async ({ page }) => {
  await openTree(page, { flags: "projectTree,workspaceTabs" });
  await expect(item(page, "terminal:tree-shell")).toBeVisible();
  await rowMenu(page, "terminal:tree-shell");
  await expect(treeMenu(page).getByRole("menuitem", { name: "Open in tab", exact: true })).toHaveCount(0);
  await expect(treeMenu(page).getByRole("menuitem", { name: "Rename", exact: true })).toBeVisible();
});

for (const theme of ["dark", "light"] as const) {
  test(`a Chats terminal menu offers Open in tab in the ${theme} theme`, async ({ page }) => {
    await openTree(page, { theme, query: "adoption=1" });
    await expect(item(page, "terminal:adoption-shell-home")).toBeVisible();
    await rowMenu(page, "terminal:adoption-shell-home");
    await expect(treeMenu(page).getByRole("menuitem", { name: "Open in tab", exact: true })).toBeVisible();
    await expect(sidebar(page)).toHaveScreenshot(`tree-open-in-tab-${theme}.png`);
  });
}

function treeMenu(page: Page) {
  return page.getByRole("menu").first();
}

async function rowMenu(page: Page, key: string) {
  await item(page, key).locator(":scope > .project-tree-row").click({ button: "right" });
  await expect(treeMenu(page)).toBeVisible();
  return treeMenu(page);
}

async function menuIds(page: Page) {
  return treeMenu(page)
    .locator(':scope > [role="menuitem"], :scope > [role="separator"]')
    .evaluateAll((nodes) =>
      nodes.map((node) =>
        node.getAttribute("role") === "separator"
          ? "|"
          : `${node.getAttribute("aria-disabled") === "true" ? "!" : ""}${node.textContent?.trim()}`,
      ),
    );
}

for (const theme of ["dark", "light"] as const) {
  test(`an agent row has Orca's row menu in the ${theme} theme`, async ({ page }) => {
    await openTree(page, { theme });
    await rowMenu(page, "conversation:audit-docs-sweep");
    expect(await menuIds(page)).toEqual([
      "Open",
      "Rename",
      "|",
      "Copy title",
      "Copy path",
      "|",
      "Mark as unread",
      "|",
      "Close tab",
      "Archive",
    ]);
    await expect(page).toHaveScreenshot(`tree-agent-menu-${theme}.png`, {
      clip: { x: 0, y: 0, width: 640, height: 800 },
    });
  });
}

test("the agent row menu renames, copies, marks unread and read, and archives", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openTree(page);
  const docs = item(page, "conversation:audit-docs-sweep");

  await rowMenu(page, "conversation:audit-docs-sweep");
  await treeMenu(page).getByRole("menuitem", { name: "Copy path", exact: true }).click();
  await expect(treeMenu(page)).toHaveCount(0);
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    "/Users/auditor/Library/Application Support/Halv/worktrees/c3d4/halv",
  );

  await rowMenu(page, "conversation:audit-docs-sweep");
  await treeMenu(page).getByRole("menuitem", { name: "Rename", exact: true }).click();
  const field = docs.getByRole("textbox", { name: "Rename conversation" });
  await expect(field).toBeFocused();
  await field.fill("Docs sweep, second pass");
  await field.press("Enter");
  await expect(docs).toContainText("Docs sweep, second pass");

  await rowMenu(page, "conversation:audit-docs-sweep");
  await treeMenu(page).getByRole("menuitem", { name: "Mark as unread", exact: true }).click();
  await expect(docs.locator(":scope > .project-tree-row .font-semibold")).toBeVisible();
  await rowMenu(page, "conversation:audit-docs-sweep");
  await treeMenu(page).getByRole("menuitem", { name: "Mark as read", exact: true }).click();
  await expect(docs.locator(":scope > .project-tree-row .font-semibold")).toHaveCount(0);

  // Archive closes the tab first. The working agent asks once.
  await rowMenu(page, "conversation:audit-docs-sweep");
  await treeMenu(page).getByRole("menuitem", { name: "Archive", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Close tab", exact: true }).click();
  await expect(docs).toHaveCount(0);
  await expect(sidebar(page).getByRole("button", { name: /Archived \(1\)/ })).toBeVisible();
});

test("Close tab from the row menu closes the tab and stops its agent", async ({ page }) => {
  await openTree(page);
  await rowMenu(page, "conversation:audit-docs-sweep");
  await treeMenu(page).getByRole("menuitem", { name: "Close tab", exact: true }).click();
  // The working agent asks once, as the strip does.
  await page.getByRole("dialog").getByRole("button", { name: "Close tab", exact: true }).click();
  await expect(item(page, "conversation:audit-docs-sweep")).toHaveCount(0);
  expect((await listed(page)).find((row) => row.id === "tree-codex-docs")?.status).not.toBe("running");
});

test("the keyboard opens a row menu and Escape gives the focus back", async ({ page }) => {
  await openTree(page);
  const login = item(page, "workspace:audit-wt-login");
  await login.focus();
  await page.keyboard.press("Shift+F10");
  await expect(treeMenu(page)).toBeVisible();
  await expect(treeMenu(page).getByRole("menuitem").first()).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(treeMenu(page).getByRole("menuitem", { name: "New terminal here" })).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  await expect(treeMenu(page).getByRole("menuitem", { name: "Remove worktree…" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(treeMenu(page)).toHaveCount(0);
  await expect(login).toBeFocused();
});

test("the workspace menu opens the folder, the editor and a terminal there", async ({ page }) => {
  await openTree(page);
  await rowMenu(page, "workspace:audit-wt-login");
  expect(await menuIds(page)).toEqual([
    "New agent here",
    "New terminal here",
    "|",
    expect.stringMatching(/^Open in (Finder|File Explorer|file manager)$/),
    "Open in editor",
    "Copy path",
    "Copy branch name",
    "|",
    "Pin",
    "|",
    "Remove worktree…",
  ]);
  await treeMenu(page).getByRole("menuitem", { name: /^Open in (Finder|File Explorer|file manager)$/ }).click();
  await rowMenu(page, "workspace:audit-wt-login");
  await treeMenu(page).getByRole("menuitem", { name: "Open in editor" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () => (window as typeof window & { __harnessWorkspaceOpenIn?: unknown[] }).__harnessWorkspaceOpenIn,
      ),
    )
    .toEqual([
      { workspaceId: "audit-wt-login", target: "fileManager" },
      { workspaceId: "audit-wt-login", target: "editor" },
    ]);

  await rowMenu(page, "workspace:audit-wt-login");
  await treeMenu(page).getByRole("menuitem", { name: "New terminal here" }).click();
  await expect(item(page, "workspace:audit-wt-login")).toHaveAttribute("aria-current", "page");
  await expect(activeTab(page)).toHaveAttribute("data-workspace-tab-key", /^shell:/);
  const opened = await page.evaluate(
    () =>
      (window as typeof window & { __harnessTerminalOpenRequests?: Array<Record<string, unknown>> })
        .__harnessTerminalOpenRequests ?? [],
  );
  await expect
    .poll(async () =>
      (
        await page.evaluate(
          () =>
            (window as typeof window & { __harnessTerminalOpenRequests?: Array<Record<string, unknown>> })
              .__harnessTerminalOpenRequests ?? [],
        )
      ).at(-1)?.workspaceId,
    )
    .toBe("audit-wt-login");
  expect(opened.length).toBeGreaterThanOrEqual(0);

  await rowMenu(page, "workspace:audit-wt-login");
  await treeMenu(page).getByRole("menuitem", { name: "Pin", exact: true }).click();
  await rowMenu(page, "workspace:audit-wt-login");
  await expect(treeMenu(page).getByRole("menuitem", { name: "Unpin", exact: true })).toBeVisible();
});

for (const theme of ["dark", "light"] as const) {
  test(`Remove worktree checks the worktree and blocks a used one in the ${theme} theme`, async ({ page }) => {
    await openTree(page, { theme });
    await rowMenu(page, "workspace:audit-wt-login");
    await treeMenu(page).getByRole("menuitem", { name: "Remove worktree…" }).click();
    const dialog = page.getByRole("dialog", { name: "Remove worktree" });
    await expect(dialog).toContainText("Remove halv/login-flow from git and delete its worktree folder.");
    await expect(dialog).toContainText("1 conversation uses this worktree.");
    await expect(dialog.getByRole("button", { name: "Remove worktree" })).toBeDisabled();
    await expect(dialog).toHaveScreenshot(`tree-remove-worktree-blocked-${theme}.png`);
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(item(page, "workspace:audit-wt-login")).toBeVisible();
  });
}

test("a dirty worktree cannot be removed", async ({ page }) => {
  await openTree(page, { query: "spike=dirty" });
  await rowMenu(page, "workspace:audit-wt-spike");
  await treeMenu(page).getByRole("menuitem", { name: "Remove worktree…" }).click();
  const dialog = page.getByRole("dialog", { name: "Remove worktree" });
  await expect(dialog).toContainText("uncommitted or untracked changes");
  await expect(dialog.getByRole("button", { name: "Remove worktree" })).toBeDisabled();
});

test("a removal dims the row with Removing… and the worktree leaves the tree", async ({ page }) => {
  await openTree(page, { query: "spike=clean" });
  const spike = item(page, "workspace:audit-wt-spike");
  await expect(spike).toContainText("halv/old-spike");
  await rowMenu(page, "workspace:audit-wt-spike");
  await treeMenu(page).getByRole("menuitem", { name: "Remove worktree…" }).click();
  const dialog = page.getByRole("dialog", { name: "Remove worktree" });
  await expect(dialog).toContainText("No uncommitted or untracked changes");
  await page.evaluate(() => {
    (window as typeof window & { __harnessHoldWorktreeOps?: boolean }).__harnessHoldWorktreeOps = true;
  });
  await dialog.getByRole("button", { name: "Remove worktree" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(spike).toHaveClass(/project-tree-removing/);
  await expect(spike).toContainText("Removing…");
  await expect(sidebar(page).locator(".project-tree-root")).toHaveScreenshot("tree-removing-dark.png");
  await page.evaluate(() =>
    (window as typeof window & { __harnessReleaseWorktreeOps?: () => void }).__harnessReleaseWorktreeOps?.(),
  );
  await expect(spike).toHaveCount(0);
});

test("a failed removal stays under the workspace until dismissed", async ({ page }) => {
  await openTree(page, { query: "spike=clean" });
  await page.evaluate(() => {
    (window as typeof window & { __harnessWorkspaceRemoveError?: string }).__harnessWorkspaceRemoveError =
      "git worktree remove failed: fatal: validation failed";
  });
  await rowMenu(page, "workspace:audit-wt-spike");
  await treeMenu(page).getByRole("menuitem", { name: "Remove worktree…" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Remove worktree" }).click();
  const failed = item(page, "remove-failed:audit-wt-spike");
  await expect(failed).toContainText("Remove failed");
  await expect(failed).toContainText("validation failed");
  await expect(item(page, "workspace:audit-wt-spike")).not.toHaveClass(/project-tree-removing/);
  await failed.getByRole("button", { name: "Dismiss" }).click();
  await expect(failed).toHaveCount(0);
});

test("the primary workspace offers the project removal, which waits for the worktrees", async ({ page }) => {
  await openTree(page);
  await rowMenu(page, "workspace:primary-1");
  await expect(treeMenu(page).getByRole("menuitem", { name: "Remove worktree…" })).toHaveAttribute(
    "aria-disabled",
    "true",
  );
  await treeMenu(page).getByRole("menuitem", { name: "Remove project from Halv…" }).click();
  const dialog = page.getByRole("dialog", { name: "Remove project" });
  await expect(dialog).toContainText("This only removes Halv Desktop from Halv. It is still on your disk.");
  await expect(dialog).toContainText("This project has 2 worktrees. Remove them first.");
  await expect(dialog.getByRole("button", { name: "Remove", exact: true })).toBeDisabled();
});

for (const theme of ["dark", "light"] as const) {
  test(`the project menu collapses all and removes a project in the ${theme} theme`, async ({ page }) => {
    await openTree(page, { theme });
    const project = item(page, "project:1");
    await project.locator(":scope > .project-tree-row").hover();
    await project.getByTitle("Project actions").click();
    expect(await menuIds(page)).toEqual([
      "New worktree…",
      "|",
      expect.stringMatching(/^Open in (Finder|File Explorer|file manager)$/),
      "Copy path",
      "Collapse all",
      "|",
      "New group from project…",
      "|",
      "Remove project from Halv…",
    ]);
    await treeMenu(page).getByRole("menuitem", { name: "Collapse all" }).click();
    for (const key of ["workspace:primary-1", "workspace:audit-wt-login", "workspace:audit-wt-docs"]) {
      await expect(item(page, key)).toHaveAttribute("aria-expanded", "false");
    }
    await project.locator(":scope > .project-tree-row").hover();
    await project.getByTitle("Project actions").click();
    await expect(treeMenu(page).getByRole("menuitem", { name: "Expand all" })).toBeVisible();
    await page.keyboard.press("Escape");

    await rowMenu(page, "project:2");
    await treeMenu(page).getByRole("menuitem", { name: "Remove project from Halv…" }).click();
    const dialog = page.getByRole("dialog", { name: "Remove project" });
    await expect(dialog).toContainText("This only removes Design System from Halv.");
    await expect(dialog).toHaveScreenshot(`tree-remove-project-${theme}.png`);
    await dialog.getByRole("button", { name: "Remove", exact: true }).click();
    await expect(item(page, "project:2")).toHaveCount(0);
  });
}

async function openNewWorktree(page: Page) {
  const project = item(page, "project:1");
  await project.locator(":scope > .project-tree-row").hover();
  await project.getByTitle("New worktree in Halv Desktop").click();
  const dialog = page.getByRole("dialog", { name: "Create worktree" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByLabel("Create from")).toBeEnabled();
  return dialog;
}

for (const theme of ["dark", "light"] as const) {
  test(`New worktree shows a pending row, then the new worktree in the ${theme} theme`, async ({ page }) => {
    await openTree(page, { theme });
    const dialog = await openNewWorktree(page);
    await dialog.getByLabel("Branch name").fill("feature/tree-spike");
    await dialog.getByLabel("Create from").selectOption("main");
    await expect(dialog).toHaveScreenshot(`tree-new-worktree-${theme}.png`);
    await page.evaluate(() => {
      (window as typeof window & { __harnessHoldWorktreeOps?: boolean }).__harnessHoldWorktreeOps = true;
    });
    await dialog.getByRole("button", { name: "Create", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    const pending = item(page, "project:1").locator(".project-tree-pending");
    await expect(pending).toContainText("feature/tree-spike");
    await expect(pending).toContainText("Creating worktree…");
    await expect(sidebar(page).locator(".project-tree-root")).toHaveScreenshot(
      `tree-creating-worktree-${theme}.png`,
    );
    await page.evaluate(() =>
      (window as typeof window & { __harnessReleaseWorktreeOps?: () => void }).__harnessReleaseWorktreeOps?.(),
    );
    await expect(pending).toHaveCount(0);
    const created = page.locator('[role="treeitem"][data-tree-key^="workspace:audit-wt-"]', {
      hasText: "feature/tree-spike",
    });
    await expect(created).toHaveAttribute("aria-current", "page");
    const request = (await invoke(page, "workspace_list", { projectId: 1 })) as Array<{
      branch: string | null;
      baseRef: string;
    }>;
    expect(request.find((row) => row.branch === "feature/tree-spike")?.baseRef).toBe("main");
  });
}

test("a failed creation keeps its row with the error until dismissed", async ({ page }) => {
  await openTree(page);
  await page.evaluate(() => {
    (window as typeof window & { __harnessWorktreeCreateError?: string }).__harnessWorktreeCreateError =
      "fatal: a branch named 'feature/taken' already exists";
  });
  const dialog = await openNewWorktree(page);
  await dialog.getByLabel("Branch name").fill("feature/taken");
  await dialog.getByLabel("Branch name").press("Enter");
  const failed = item(page, "project:1").locator(".project-tree-failed");
  await expect(failed).toContainText("feature/taken");
  await expect(failed).toContainText("already exists");
  await failed.getByRole("button", { name: "Dismiss" }).click();
  await expect(failed).toHaveCount(0);
});

test("the project menu opens New worktree too", async ({ page }) => {
  await openTree(page);
  await rowMenu(page, "project:1");
  await treeMenu(page).getByRole("menuitem", { name: "New worktree…" }).click();
  await expect(page.getByRole("dialog", { name: "Create worktree" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Create worktree" })).toHaveCount(0);
});

async function newGroupFromProject(page: Page, projectKey: string, name: string) {
  await rowMenu(page, projectKey);
  await treeMenu(page).getByRole("menuitem", { name: "New group from project…" }).click();
  const dialog = page.getByRole("dialog", { name: "New project group" });
  await expect(dialog.getByLabel("Group name")).toBeFocused();
  await dialog.getByLabel("Group name").fill(name);
  await dialog.getByRole("button", { name: "Create", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  return page.locator('[role="treeitem"][data-tree-key^="group:"]', { hasText: name });
}

async function dragRow(page: Page, fromKey: string, toKey: string, edge: "top" | "bottom" | "middle") {
  const from = await item(page, fromKey).locator(":scope > .project-tree-row").boundingBox();
  const to = await item(page, toKey).locator(":scope > .project-tree-row").boundingBox();
  if (!from || !to) throw new Error("row not found");
  await page.mouse.move(from.x + 40, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + 40, from.y + from.height / 2 + 12, { steps: 3 });
  const y = edge === "top" ? to.y + 3 : edge === "bottom" ? to.y + to.height - 3 : to.y + to.height / 2;
  await page.mouse.move(to.x + 40, y, { steps: 6 });
}

for (const theme of ["dark", "light"] as const) {
  test(`a project group holds projects, and the group menu renames it in the ${theme} theme`, async ({ page }) => {
    await openTree(page, { theme });
    const group = await newGroupFromProject(page, "project:2", "Clients");
    await expect(group).toHaveAttribute("aria-level", "1");
    await expect(group.locator('[data-tree-key="project:2"]')).toHaveAttribute("aria-level", "2");
    await expect(group.locator('[data-tree-key="workspace:primary-2"]')).toHaveAttribute("aria-level", "3");
    // Groups come first, as in Orca.
    const keys = await page
      .locator('.project-tree-root [role="treeitem"][aria-level="1"]')
      .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-tree-key")));
    expect(keys[0]).toMatch(/^group:/);
    expect(keys.slice(1)).toEqual(["project:1"]);

    // "Move to group" is a submenu of the groups.
    await rowMenu(page, "project:1");
    await treeMenu(page).getByRole("menuitem", { name: "Move to group" }).click();
    await page.getByRole("menu", { name: "Move to group" }).getByRole("menuitem", { name: "Clients" }).click();
    await expect(group.locator('[data-tree-key="project:1"]')).toHaveAttribute("aria-level", "2");
    await expect(sidebar(page).locator(".project-tree-root")).toHaveScreenshot(`tree-group-${theme}.png`);

    await group.locator(":scope > .project-tree-row").hover();
    await group.getByTitle("Group actions").click();
    await treeMenu(page).getByRole("menuitem", { name: "Rename group…" }).click();
    const rename = page.getByRole("dialog", { name: "Rename project group" });
    await rename.getByLabel("Group name").fill("Client work");
    await rename.getByRole("button", { name: "Rename", exact: true }).click();
    await expect(group).toHaveCount(0);
    await expect(page.locator('[role="treeitem"][data-tree-key^="group:"]')).toContainText("Client work");
  });
}

test("a group collapses, stays collapsed after a reload, and a delete ungroups", async ({ page }) => {
  await openTree(page);
  const group = await newGroupFromProject(page, "project:2", "Design");
  await group.locator(":scope > .project-tree-row").click();
  await expect(group).toHaveAttribute("aria-expanded", "false");
  await expect(item(page, "project:2")).toHaveCount(0);
  await page.reload();
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
  const again = page.locator('[role="treeitem"][data-tree-key^="group:"]', { hasText: "Design" });
  await expect(again).toHaveAttribute("aria-expanded", "false");

  await rowMenu(page, (await again.getAttribute("data-tree-key"))!);
  await treeMenu(page).getByRole("menuitem", { name: "Delete group…" }).click();
  const dialog = page.getByRole("dialog", { name: "Delete project group" });
  await expect(dialog).toContainText("Design System");
  await dialog.getByRole("button", { name: "Delete group" }).click();
  await expect(again).toHaveCount(0);
  await expect(item(page, "project:2")).toHaveAttribute("aria-level", "1");
});

test("dragging a project onto a group moves it in, and back out", async ({ page }) => {
  await openTree(page);
  const group = await newGroupFromProject(page, "project:2", "Clients");
  const groupKey = (await group.getAttribute("data-tree-key"))!;
  await dragRow(page, "project:1", groupKey, "middle");
  await expect(group).toHaveClass(/project-tree-drop-inside/);
  await page.mouse.up();
  await expect(group.locator('[data-tree-key="project:1"]')).toHaveAttribute("aria-level", "2");
  // The drop is not a click: the project stays open.
  await expect(item(page, "project:1")).toHaveAttribute("aria-expanded", "true");
  const order = () =>
    group.locator('[role="treeitem"][aria-level="2"]').evaluateAll((nodes) =>
      nodes.map((node) => node.getAttribute("data-tree-key")),
    );
  expect(await order()).toEqual(["project:2", "project:1"]);
  // Before project 2 in the same group: the order changes.
  await dragRow(page, "project:1", "project:2", "top");
  await expect(page.locator(".project-tree-drop-line")).toBeVisible();
  await page.mouse.up();
  await expect.poll(order).toEqual(["project:1", "project:2"]);
  // "Remove from group" takes a project out.
  await rowMenu(page, "project:2");
  await treeMenu(page).getByRole("menuitem", { name: "Remove from group" }).click();
  await expect(item(page, "project:2")).toHaveAttribute("aria-level", "1");
});

for (const theme of ["dark", "light"] as const) {
  test(`a conversation with two agent panes shows one child row per pane in the ${theme} theme`, async ({ page }) => {
    await openTree(page, { theme, query: "panes=1" });
    const docs = item(page, "conversation:audit-docs-sweep");
    await expect(docs).toHaveAttribute("aria-expanded", "true");
    const first = item(page, "pane:tree-codex-docs");
    const second = item(page, "pane:tree-claude-docs-review");
    await expect(first).toHaveAttribute("aria-level", "4");
    await expect(first.locator('[data-agent-glyph="working"]')).toBeVisible();
    await expect(first.locator('[data-agent-kind="codex"]')).toBeVisible();
    await expect(second).toContainText("Claude 3");
    await expect(second.locator('[data-agent-glyph="attention"]')).toBeVisible();
    await expect(second.locator('[data-agent-kind="claude"]')).toBeVisible();
    // The conversation row shows its strongest pane.
    await expect(docs.locator(':scope > .project-tree-row [data-agent-glyph="attention"]')).toBeVisible();
    await expect(sidebar(page).locator(".project-tree-root")).toHaveScreenshot(`tree-pane-rows-${theme}.png`);

    // Left folds the pane rows, Right opens them again.
    await docs.focus();
    await page.keyboard.press("ArrowLeft");
    await expect(docs).toHaveAttribute("aria-expanded", "false");
    await expect(second).toHaveCount(0);
    await page.keyboard.press("ArrowRight");
    await expect(second).toBeVisible();
    // A pane row opens the conversation and focuses that pane.
    await second.locator(":scope > .project-tree-row").click();
    await expect(docs).toHaveAttribute("aria-current", "page");
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            document.activeElement?.closest("[data-terminal-session-id]")?.getAttribute(
              "data-terminal-session-id",
            ) ?? null,
        ),
      )
      .toBe("tree-claude-docs-review");
  });
}

for (const theme of ["dark", "light"] as const) {
  test(`the History of a workspace lists its closed conversations and opens one as a tab in the ${theme} theme`, async ({ page }) => {
    await openTree(page, { theme });
    const primary = item(page, "workspace:primary-1");
    // The History stays out of the tree until the user opens it.
    await expect(item(page, "history:primary-1")).toHaveCount(0);
    await rowMenu(page, "workspace:primary-1");
    const show = treeMenu(page).getByRole("menuitem", { name: /^Show history \(\d+\)$/ });
    await expect(show).toBeVisible();
    await show.click();
    const history = item(page, "history:primary-1");
    await expect(history).toHaveAttribute("aria-level", "3");
    await expect(history).toHaveAttribute("aria-expanded", "true");
    const parked = history.locator('[data-tree-key="conversation:audit-parked"]');
    await expect(parked).toHaveAttribute("aria-level", "4");
    await expect(parked).toContainText("Parked login spike");
    // Newest first.
    const times = await history
      .locator('[role="treeitem"][data-tree-key^="conversation:"]')
      .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-tree-key")));
    expect(times.length).toBeGreaterThan(0);
    await expect(sidebar(page).locator(".project-tree-root")).toHaveScreenshot(`tree-history-${theme}.png`);

    // A click opens the conversation as a tab. Its row leaves the History.
    await parked.locator(":scope > .project-tree-row").click();
    await expect(strip(page).getByRole("tab", { name: /Parked login spike/ })).toBeVisible();
    // Two tabs now fold into the agents pill of the workspace.
    await expect(primary.locator('[data-tree-key="agents:primary-1"]')).toBeVisible();
    // It was the only closed conversation, so the History goes.
    await expect(item(page, "history:primary-1")).toHaveCount(0);
  });
}

test("the clock button of a workspace row opens its History", async ({ page }) => {
  await openTree(page);
  const row = item(page, "workspace:primary-1").locator(":scope > .project-tree-row");
  await row.hover();
  await item(page, "workspace:primary-1").getByTitle(/^Show history \(\d+\)$/).click();
  await expect(item(page, "history:primary-1")).toBeVisible();
  // The choice survives a reload.
  await page.reload();
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
  await expect(item(page, "history:primary-1")).toBeVisible();
  // The History header closes it again.
  await item(page, "history:primary-1").locator(":scope > .project-tree-row").click();
  await expect(item(page, "history:primary-1")).toHaveCount(0);
});

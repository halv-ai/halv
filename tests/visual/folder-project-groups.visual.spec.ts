import { expect, test, type Page } from "@playwright/test";
import { openVisualState } from "./capture";

// Folder projects with nested git repositories (ADR 0019). The harness scan
// finds two repositories below /Users/auditor/Clients and below
// /Users/auditor/Projects. Any other folder is a git repository.

function sidebar(page: Page) {
  return page.getByRole("complementary", { name: "Conversation navigation" });
}

function item(page: Page, key: string) {
  return page.locator(`[role="treeitem"][data-tree-key="${key}"]`);
}

function groups(page: Page) {
  return page.locator('.project-tree-root [role="treeitem"][data-tree-key^="group:"]');
}

// The "Add folder..." button of the empty tree opens the path form.
async function addFolder(page: Page, path: string) {
  await sidebar(page).getByRole("button", { name: "Add folder...", exact: true }).click();
  const input = page.getByRole("textbox", { name: "Folder path", exact: true });
  await input.fill(path);
  await page.getByRole("button", { name: "Add folder", exact: true }).click();
  await expect(input).toHaveCount(0);
}

async function groupMembers(page: Page) {
  return groups(page)
    .first()
    .locator('[role="treeitem"][aria-level="2"]')
    .evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-tree-key")));
}

for (const theme of ["dark", "light"] as const) {
  test(`adding a folder with repositories groups them under the folder in the ${theme} theme`, async ({ page }) => {
    await openVisualState(page, `project-tree-new-${theme}`, "flags=projectTree");
    await addFolder(page, "/Users/auditor/Clients");

    // One group, named after the folder: the folder first, then each
    // repository as a project with its own workspaces.
    await expect(groups(page)).toHaveCount(1);
    await expect(groups(page).first()).toHaveAttribute("aria-level", "1");
    await expect(groups(page).first().locator(":scope > .project-tree-row")).toContainText("Clients");
    await expect.poll(() => groupMembers(page)).toEqual(["project:3", "project:4", "project:5"]);
    const folder = item(page, "project:3");
    await expect(folder).toHaveAccessibleName("Clients folder");
    await expect(folder.locator(":scope > .project-tree-row .project-tree-primary-badge")).toHaveAttribute(
      "title",
      "The folder that holds the repositories of this group",
    );
    await expect(folder.locator(":scope > .project-tree-row .project-tree-folder-avatar")).toBeVisible();
    // A plain folder has no worktrees, so it has no "+" button.
    await expect(folder.getByTitle("New worktree in Clients")).toHaveCount(0);
    await expect(item(page, "project:4")).toHaveAccessibleName(/^api/);
    await expect(item(page, "project:5")).toHaveAccessibleName(/^site/);
    await expect(item(page, "workspace:primary-4")).toHaveAttribute("aria-level", "3");
    await expect(item(page, "project:4").getByTitle("New worktree in api")).toHaveCount(1);
    await expect(sidebar(page).locator(".project-tree-root")).toHaveScreenshot(
      `tree-folder-group-${theme}.png`,
    );
  });
}

test("adding a git repository keeps one project without a group", async ({ page }) => {
  await openVisualState(page, "project-tree-new-dark", "flags=projectTree");
  await addFolder(page, "/Users/auditor/Projects/solo");
  await expect(item(page, "project:3")).toHaveAttribute("aria-level", "1");
  await expect(groups(page)).toHaveCount(0);
});

test("the one-time scan groups an existing folder project and its repositories", async ({ page }) => {
  await openVisualState(page, "project-tree-dark", "flags=projectTree&sidebar=projects&nested=1");
  // The two fixture projects are repositories below the folder project.
  await expect(groups(page)).toHaveCount(1);
  await expect(groups(page).first().locator(":scope > .project-tree-row")).toContainText("Projects");
  await expect.poll(() => groupMembers(page)).toEqual(["project:3", "project:1", "project:2"]);
  await expect(item(page, "project:3")).toHaveAccessibleName("Projects folder");

  // The scan runs once. A reload adds no second group.
  await page.reload();
  await page.waitForFunction(() => document.documentElement.dataset.visualReady === "project-tree-dark");
  await expect.poll(() => groupMembers(page)).toEqual(["project:3", "project:1", "project:2"]);
  await expect(groups(page)).toHaveCount(1);
  const stored = await page.evaluate(() =>
    JSON.parse(window.localStorage.getItem("halv.sidebar.project-tree.v1") ?? "null"),
  );
  expect(stored.groups).toHaveLength(1);
  expect(stored.groups[0]).toMatchObject({
    name: "Projects",
    createdFrom: "folder-scan",
    folderProjectId: 3,
    parentPath: "/Users/auditor/Projects",
  });
  expect(stored.scannedFolders).toEqual(
    expect.arrayContaining([
      "/Users/auditor/Projects",
      "/Users/auditor/Projects/halv",
      "/Users/auditor/Projects/design-system",
    ]),
  );
});

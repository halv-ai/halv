import { expect, test, type Page } from "@playwright/test";
import { openVisualState } from "./capture";

// The card sits 16 px from the right edge and 36 px from the bottom,
// which is 12 px above the 24 px status bar. It moves 12 px above the chat
// composer when it would cover it.
const VIEWPORT = { width: 1280, height: 800 };

function updateCard(page: Page) {
  return page.getByRole("region", { name: "Halv update", exact: true });
}

function updateSegment(page: Page) {
  return page
    .getByRole("region", { name: "Status bar", exact: true })
    .locator("[data-app-update-segment]");
}

for (const theme of ["dark", "light"] as const) {
  test(`the ready card sits above the status bar and clear of the composer in ${theme} mode`, async ({ page }) => {
    await openVisualState(page, "chat-active", `update=ready&theme=${theme}`);
    const card = updateCard(page);
    await expect(card).toBeVisible();
    await expect(card).toContainText("Halv 0.7.2 is ready");
    await expect(card).toContainText("Your agent sessions keep running while Halv restarts.");
    await expect(card.getByRole("button", { name: "Restart now", exact: true })).toBeVisible();
    await expect(card.getByRole("button", { name: "What's new", exact: true })).toBeVisible();

    const box = (await card.boundingBox())!;
    expect(box.width).toBe(360);
    expect(box.x + box.width).toBe(VIEWPORT.width - 16);

    const bar = (await page.getByRole("region", { name: "Status bar", exact: true }).boundingBox())!;
    expect(box.y + box.height).toBeLessThanOrEqual(bar.y - 12);
    const composer = (await page.locator("[data-update-card-avoid]").boundingBox())!;
    expect(Math.abs(composer.y - 12 - (box.y + box.height))).toBeLessThanOrEqual(1);
    expect(box.y).toBeGreaterThan(0);
    const send = (await page.getByRole("button", { name: "Send message", exact: true }).boundingBox())!;
    const overlaps =
      send.x < box.x + box.width &&
      send.x + send.width > box.x &&
      send.y < box.y + box.height &&
      send.y + send.height > box.y;
    expect(overlaps).toBe(false);
    await expect(updateSegment(page)).toHaveText("Update ready");
    await expect(card).toHaveScreenshot(`app-update-card-ready-${theme}.png`);
  });

  test(`the download and the blockers step render in ${theme} mode`, async ({ page }) => {
    await openVisualState(page, "chat-active", `update=downloading&theme=${theme}`);
    const card = updateCard(page);
    await expect(card).toContainText("Downloading Halv 0.7.2");
    await expect(card).toContainText("41% · 72.5 of 176.9 MB");
    await expect(card.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "41");
    await expect(updateSegment(page)).toHaveText("↓ 41%");
    await expect(card).toHaveScreenshot(`app-update-card-downloading-${theme}.png`);

    await openVisualState(page, "chat-active", `update=blockers&theme=${theme}`);
    await expect(card).toContainText("Work is still running");
    await expect(card).toContainText("1 chat reply");
    await expect(card).toContainText("2 agent turns");
    await expect(card.getByRole("button", { name: "Restart when done", exact: true })).toBeVisible();
    await expect(card).toHaveScreenshot(`app-update-card-blockers-${theme}.png`);
  });
}

test("Escape hides the card into the status bar, and the segment brings it back", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=ready");
  const card = updateCard(page);
  await card.getByRole("button", { name: "Later", exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(card).toBeHidden();

  const segment = updateSegment(page);
  await expect(segment).toHaveText("Update ready");
  await segment.click();
  await expect(card).toBeVisible();
  await segment.click();
  await expect(card).toBeHidden();
});

test("Later hides the card for this version and keeps the status bar segment", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=ready");
  const card = updateCard(page);
  await card.getByRole("button", { name: "Later", exact: true }).click();
  await expect(card).toBeHidden();
  await expect(updateSegment(page)).toHaveText("Update ready");
});

test("a restart with work in progress asks first and can wait for the work", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=busy");
  const card = updateCard(page);
  await card.getByRole("button", { name: "Restart now", exact: true }).click();
  await expect(card).toContainText("Work is still running");
  await expect(card).toContainText("Restarting now stops this work:");

  await card.getByRole("button", { name: "Restart when done", exact: true }).click();
  await expect(card).toContainText("Halv restarts when the work is done");
  await card.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(card).toContainText("Halv 0.7.2 is ready");

  await card.getByRole("button", { name: "Restart now", exact: true }).click();
  await card.getByRole("button", { name: "Restart now", exact: true }).click();
  await expect(card).toContainText("Installing Halv 0.7.2…");
  await expect(updateSegment(page)).toHaveText("Installing update…");
});

test("Windows says that terminals and agents restart", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=windows");
  await expect(updateCard(page)).toContainText(
    "Halv closes to install the update. Open terminals and agents will restart.",
  );
});

test("the card also shows over the Connect gate", async ({ page }) => {
  await openVisualState(page, "connect-gate", "update=ready");
  const card = updateCard(page);
  await expect(card).toBeVisible();
  await expect(card).toContainText("Halv 0.7.2 is ready");
  const box = (await card.boundingBox())!;
  expect(box.x + box.width).toBe(VIEWPORT.width - 16);
  expect(box.y + box.height).toBe(VIEWPORT.height - 36);
});

test("after an update the card says how many agent sessions reconnected", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=updated");
  const card = updateCard(page);
  await expect(card).toContainText("Updated to Halv 0.7.2");
  await expect(card).toContainText("3 agent sessions reconnected.");
  await card.getByRole("button", { name: "Dismiss", exact: true }).click();
  await expect(card).toBeHidden();
});

test("a user check reports an up-to-date Halv and hides after 3 seconds", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=uptodate");
  const card = updateCard(page);
  await expect(card).toContainText("Halv is up to date.");
  await expect(card).toContainText("You have version 0.7.1.");
  await expect(card).toBeHidden({ timeout: 5_000 });
  await expect(updateSegment(page)).toHaveCount(0);
});

test("a failed download offers a retry", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=error");
  const card = updateCard(page);
  await expect(card).toContainText("The update did not download");
  await expect(card).toContainText(
    "Could not reach the update server. Check your connection and try again.",
  );
  await expect(updateSegment(page)).toHaveText("Update failed");
  await card.getByRole("button", { name: "Dismiss", exact: true }).click();
  await expect(card).toBeHidden();
  await expect(updateSegment(page)).toHaveText("Update failed");
});

test("the savings drawer covers the card", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=ready");
  const card = updateCard(page);
  const box = (await card.boundingBox())!;
  await page
    .getByRole("region", { name: "Status bar", exact: true })
    .getByRole("button", { name: /saved/ })
    .click();
  await expect(page.locator("#savings-drawer")).toBeVisible();
  const topElementIsCard = await page.evaluate(
    ({ x, y }) => Boolean(document.elementFromPoint(x, y)?.closest("[data-app-update-card]")),
    { x: box.x + box.width / 2, y: box.y + box.height / 2 },
  );
  expect(topElementIsCard).toBe(false);
});

test("Settings shows the update state, the download switch, and the supervisor", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=available");
  await page.getByLabel("Workspace and application menu", { exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await dialog.getByRole("button", { name: /^System/ }).click();

  await expect(dialog).toContainText("Version 0.7.2 is available.");
  const autoDownload = dialog.getByRole("checkbox", {
    name: "Download updates automatically",
    exact: true,
  });
  await expect(autoDownload).not.toBeChecked();
  await autoDownload.check();
  await expect(autoDownload).toBeChecked();
  await expect(dialog.getByRole("heading", { name: "Terminal supervisor", exact: true })).toBeVisible();
  await expect(dialog).toContainText(
    "From Halv 0.4.15. Agent sessions keep running when Halv restarts.",
  );

  await dialog.getByRole("button", { name: "Download", exact: true }).click();
  await expect(dialog).toContainText("Downloading Halv 0.7.2");
});

test("the card gives way to the composer command list in a narrow window", async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 680 });
  await openVisualState(page, "chat-active", "update=ready");
  const card = updateCard(page);
  await expect(card).toBeVisible();
  const region = page.locator(".app-update-card-region");
  await expect(region).toHaveAttribute("data-yielded", "false");
  const cardBox = (await card.boundingBox())!;

  const message = page.getByRole("textbox", { name: "Message", exact: true });
  await message.fill("/");
  const menu = page.locator("[data-update-card-yield]").first();
  await expect(menu).toBeVisible();
  await expect(region).toHaveAttribute("data-yielded", "true");
  await expect(card).toBeHidden();
  // A click where the card was lands on the menu.
  const menuBox = (await menu.boundingBox())!;
  const left = Math.max(cardBox.x, menuBox.x);
  const right = Math.min(cardBox.x + cardBox.width, menuBox.x + menuBox.width);
  const top = Math.max(cardBox.y, menuBox.y);
  const bottom = Math.min(cardBox.y + cardBox.height, menuBox.y + menuBox.height);
  expect(right - left).toBeGreaterThan(0);
  expect(bottom - top).toBeGreaterThan(0);
  const topElementIsMenu = await page.evaluate(
    ({ x, y }) => Boolean(document.elementFromPoint(x, y)?.closest("[data-update-card-yield]")),
    { x: (left + right) / 2, y: (top + bottom) / 2 },
  );
  expect(topElementIsMenu).toBe(true);

  await message.fill("");
  await expect(region).toHaveAttribute("data-yielded", "false");
  await expect(card).toBeVisible();
});

test("the Connect gate scrolls its controls clear of the card", async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 680 });
  await openVisualState(page, "connect-gate", "update=ready");
  const card = updateCard(page);
  await expect(card).toBeVisible();
  await page.getByText(/Prefer an API key/).click();
  const saves = page.getByRole("button", { name: "Save", exact: true });
  await expect(saves.first()).toBeVisible();
  await page.locator("main").first().evaluate((main) => main.scrollTo(0, main.scrollHeight));
  const cardBox = (await card.boundingBox())!;
  for (const save of await saves.all()) {
    const box = (await save.boundingBox())!;
    expect(box.y + box.height).toBeLessThanOrEqual(cardBox.y);
  }
});

test("keyboard focus stays in the card when the restart asks first", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=busy");
  const card = updateCard(page);
  await card.getByRole("button", { name: "Restart now", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(card).toContainText("Work is still running");
  await expect(
    card.getByRole("button", { name: "Restart when done", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(card).toBeHidden();
  await expect(updateSegment(page)).toBeFocused();
});

test("the status bar segment names the visible label and the card state", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=downloading");
  const segment = updateSegment(page);
  await expect(segment).toHaveAccessibleName("↓ 41%");
  await expect(segment).toHaveAttribute("aria-expanded", "true");
  await segment.click();
  await expect(segment).toHaveAttribute("aria-expanded", "false");
});

test("a copy that cannot update itself asks the user to move Halv", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=read-only");
  const card = updateCard(page);
  await expect(card).toContainText("Halv 0.7.2 is available");
  await expect(card).toContainText(
    "Halv cannot update itself in this folder. Move Halv to your Applications folder, then open it again.",
  );
  await expect(card.getByRole("button", { name: "Download", exact: true })).toHaveCount(0);
});

test("without the supervisor the ready card says that terminals close", async ({ page }) => {
  await openVisualState(page, "chat-active", "update=no-supervisor");
  await expect(updateCard(page)).toContainText(
    "Open terminals and agents close when Halv restarts.",
  );
});

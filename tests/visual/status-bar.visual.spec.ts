import { expect, test } from "@playwright/test";
import { openVisualState } from "./capture";

test("the status line sits under the whole window and the drawer leaves it visible", async ({ page }, testInfo) => {
  await openVisualState(page, "chat-active");
  const bar = page.getByRole("region", { name: "Status bar", exact: true });
  const box = await bar.boundingBox();
  expect(box).not.toBeNull();
  expect(box!.height).toBe(24);
  expect(box!.x).toBe(0);
  expect(box!.width).toBe(1280);
  expect(box!.y + box!.height).toBe(800);

  const navigation = await page
    .getByRole("complementary", { name: "Conversation navigation" })
    .boundingBox();
  expect(navigation!.y + navigation!.height).toBeLessThanOrEqual(box!.y);

  await bar.getByRole("button", { name: /saved/ }).click();
  const drawer = page.locator("#savings-drawer");
  await expect(drawer).toBeVisible();
  const drawerBox = await drawer.boundingBox();
  expect(drawerBox!.y + drawerBox!.height).toBeLessThanOrEqual(box!.y);
  await expect(bar.getByRole("button", { name: /saved/ })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  await page.screenshot({ path: testInfo.outputPath("status-bar-drawer.png") });
});

test("the Orca popover switches accounts, shows the pending state, and closes", async ({ page }, testInfo) => {
  await openVisualState(page, "status-bar-orca");
  const pill = page.locator('[data-status-provider="claude"]');
  await expect(pill).toHaveAttribute("aria-expanded", "true");
  await expect(pill).toContainText("13% used 3h 54m");
  await expect(pill).toContainText("41% used 6d 7h");

  const popover = page.getByRole("dialog", { name: "Claude usage and accounts" });
  await expect(popover).toContainText("Synced with Orca");
  // Claude panes follow a switch through Orca's shared default folder, so
  // the popover asks for no restart.
  await expect(popover).not.toContainText("Restart");
  const active = popover.locator('button[aria-current="true"]');
  await expect(active).toContainText("dev@acme.example");
  await expect(active).toContainText("Active", { useInnerText: true });

  const research = popover.getByRole("button", { name: /research@lab\.example/ });
  await expect(research).toContainText("86% 41m");
  await expect(research).toContainText("92% wk");
  await research.click();
  await expect(research).toContainText("Switching…");
  await expect(popover.locator('button[aria-current="true"]')).toContainText(
    "research@lab.example",
  );
  await expect(popover.getByRole("button", { name: /dev@acme\.example/ })).toContainText(
    "13% 3h 54m",
  );
  await popover.screenshot({ path: testInfo.outputPath("status-bar-switched.png") });

  await page.keyboard.press("Escape");
  await expect(popover).toBeHidden();
  await expect(pill).toBeFocused();
  await expect(pill).toHaveAttribute("aria-expanded", "false");

  await pill.click();
  await expect(popover).toBeVisible();
  await page.mouse.click(640, 300);
  await expect(popover).toBeHidden();
});

test("popover links open Settings connections and start the add flow", async ({ page }) => {
  await openVisualState(page, "status-bar-orca");
  const popover = page.getByRole("dialog", { name: "Claude usage and accounts" });
  await popover.getByRole("button", { name: "Manage accounts…", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "Settings", exact: true });
  await expect(settings.getByRole("button", { name: /^AI connections/ })).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(settings.getByText("Synced with Orca").first()).toBeVisible();
  await settings.getByRole("button", { name: "Close", exact: true }).click();

  await page.locator('[data-status-provider="codex"]').click();
  await page
    .getByRole("dialog", { name: "Codex usage and accounts" })
    .getByRole("button", { name: "Add account…", exact: true })
    .click();
  await expect(
    settings.getByRole("region", { name: "Add Codex account", exact: true }),
  ).toBeVisible();
});

test("narrow windows step the pills down to compact and letter badges", async ({ page }, testInfo) => {
  await openVisualState(page, "status-bar-orca");
  await page.keyboard.press("Escape");
  const pill = page.locator('[data-status-provider="claude"]');

  await page.setViewportSize({ width: 800, height: 700 });
  await expect(pill).toHaveText(/41% used 6d 7h/);
  await expect(pill).not.toContainText("13% used");
  await expect(pill.locator("[data-usage-bar]")).toHaveCount(0);

  await page.setViewportSize({ width: 450, height: 700 });
  await expect(pill).toHaveText(/C\s*41%/);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("status-bar-minimal.png") });
});

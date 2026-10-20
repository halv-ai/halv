import { expect, test, type Page } from "@playwright/test";

async function pin(page: Page, title: string) {
  await page.getByRole("button", { name: `Actions for ${title}`, exact: true }).click();
  await page.getByRole("menuitem", { name: "Pin conversation", exact: true }).click();
}

async function openPins(page: Page, theme: "dark" | "light") {
  await page.goto(`/ux-audit.html?state=marketing-app-${theme}&ghostty=off`);
  await expect(page.locator(".terminal-pane:visible")).toHaveCount(3);
  await page.getByRole("button", { name: "Actions for Authentication middleware refactor", exact: true }).click();
  await page.getByRole("menuitem", { name: "Rename conversation", exact: true }).click();
  await page.getByRole("textbox", { name: "Rename conversation" }).fill("Halv");
  await page.getByRole("textbox", { name: "Rename conversation" }).press("Enter");
  for (const title of ["Halv", "Audit agent refactor", "Release checklist"]) await pin(page, title);
  await page.getByRole("button", { name: "Collapse sidebar", exact: true }).click();
  const rail = page.getByRole("navigation", { name: "Pinned", exact: true });
  await expect(rail.locator("button")).toHaveCount(3);
  return rail;
}

for (const theme of ["dark", "light"] as const) {
  test(`collapsed pins show letters, colors and active selection in ${theme} mode`, async ({ page }, testInfo) => {
    const rail = await openPins(page, theme);
    await expect(rail.getByRole("button", { name: "Halv", exact: true }).locator("span").first()).toHaveText("H");
    const colors = await rail.locator("button").evaluateAll((buttons) => buttons.map((button) => getComputedStyle(button).backgroundColor));
    expect(new Set(colors).size).toBeGreaterThan(1);
    await rail.getByRole("button", { name: "Release checklist", exact: true }).click();
    await expect(rail.getByRole("button", { name: "Release checklist", exact: true })).toHaveAttribute("aria-current", "page");
    await page.screenshot({ path: testInfo.outputPath(`pinned-sidebar-${theme}.png`) });
    await page.getByRole("button", { name: "Expand sidebar", exact: true }).click();
    await page.getByRole("button", { name: "Actions for Release checklist", exact: true }).click();
    await page.getByRole("menuitem", { name: "Unpin conversation", exact: true }).click();
    await page.getByRole("button", { name: "Collapse sidebar", exact: true }).click();
    await expect(rail.locator("button")).toHaveCount(2);
    await expect(rail.getByRole("button", { name: "Release checklist", exact: true })).toHaveCount(0);
  });
}

test("pin keyboard cycling wraps and never writes its keys into the terminal", async ({ page }) => {
  const rail = await openPins(page, "dark");
  await rail.getByRole("button", { name: "Halv", exact: true }).click();
  const terminal = page.locator(".terminal-pane:visible .xterm-helper-textarea").first();
  await terminal.focus();
  const before = await page.evaluate(() => (window as any).__harnessTerminalWrites?.length ?? 0);
  const modifier = await page.evaluate(() => /mac/i.test(navigator.platform) ? "Meta" : "Control");
  const names = await rail.locator("button").evaluateAll((buttons) => buttons.map((button) => button.getAttribute("aria-label")!));
  let current = names.indexOf("Halv");
  for (let step = 0; step < names.length; step++) {
    await page.keyboard.press(`${modifier}+Alt+ArrowRight`);
    current = (current + 1) % names.length;
    await expect(rail.getByRole("button", { name: names[current], exact: true })).toHaveAttribute("aria-current", "page");
  }
  await terminal.focus();
  const after = await page.evaluate(() => (window as any).__harnessTerminalWrites?.length ?? 0);
  expect(after).toBe(before);
  await page.keyboard.press("Control+c");
  await expect.poll(() => page.evaluate(() => (window as any).__harnessTerminalWrites?.at(-1)?.data)).toBe("\u0003");
  await page.keyboard.press(`${modifier}+Alt+ArrowLeft`);
  current = (current - 1 + names.length) % names.length;
  await expect(rail.getByRole("button", { name: names[current], exact: true })).toHaveAttribute("aria-current", "page");
});

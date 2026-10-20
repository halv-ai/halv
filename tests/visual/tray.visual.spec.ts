import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 376, height: 600 } });

for (const theme of ["dark", "light"]) {
  test(`tray shows the approved ${theme} panel and matching icon`, async ({ page }) => {
    await page.goto(`/tray-preview.html?theme=${theme}&locale=pt-BR`);
    await expect(page.getByRole("progressbar")).toHaveCount(4);
    await expect(page.locator(".brand img")).toHaveAttribute("data-theme", theme);
    const icon = await page.locator(".brand img").evaluate(async (image: HTMLImageElement) => (await fetch(image.src)).text());
    expect(icon).toContain(theme === "light" ? 'fill="#F5F3ED"' : 'fill="#0C0C0C"');
    await expect(page.locator(".panel")).toHaveScreenshot(`tray-${theme}.png`);
    expect(await page.locator(".panel").evaluate(panel => panel.scrollWidth <= panel.clientWidth)).toBe(true);
  });
}

test("tray displays exhausted and fractional capacity without rounding up", async ({ page }) => {
  await page.goto("/tray-preview.html?state=low");
  const codex = page.getByRole("region", { name: "Codex" });
  await expect(codex.getByText("0% left", { exact: true })).toBeVisible();
  await expect(codex.getByText("<1% left", { exact: true })).toBeVisible();
  await expect(codex.getByRole("progressbar").first()).toHaveAttribute("aria-valuenow", "0");
});

for (const state of ["loading", "unavailable", "stale"]) {
  test(`tray does not invent capacity when ${state}`, async ({ page }) => {
    await page.goto(`/tray-preview.html?state=${state}`);
    await expect(page.getByRole("progressbar")).toHaveCount(0);
    await expect(page.locator(".fill")).toHaveCount(0);
    await expect(page.locator(".panel")).toBeVisible();
  });
}

test("tray actions and keyboard focus preserve their targets", async ({ page }) => {
  await page.goto("/tray-preview.html");
  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: /Saved at API prices/ })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("body")).toHaveAttribute("data-action", "tray.usage");
  for (const [label, action] of [["New chat", "tray.new-chat"], ["Show Halv", "tray.show"], ["Quit Halv", "tray.quit"], ["Quit and Stop Agents", "tray.quit-stop-agents"]]) {
    await page.getByRole("button", { name: label, exact: true }).click();
    await expect(page.locator("body")).toHaveAttribute("data-action", action);
  }
});

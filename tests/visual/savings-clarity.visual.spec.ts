import { expect, test } from "@playwright/test";

test("terminal savings details explain the period and remain readable in a compact window", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto("/ux-audit.html?state=marketing-app-dark&ghostty=off");
  const meter = page.locator('.terminal-header').getByRole("region", { name: "Session savings meter" }).first();
  await expect(meter).toBeVisible();
  await meter.getByText("Details", { exact: true }).click();
  await expect(meter.getByText(/means Halv/)).toBeVisible();
  await expect(meter.getByText(/Savings use API prices for the model that ran/)).toBeVisible();
  await expect(meter.getByText(/You pay a flat plan/)).toBeVisible();
  await expect(meter.getByText(/^Context reduction: .+ \(.+ tokens\)$/)).toBeVisible();
  await expect(meter.getByText("Routing savings not estimated yet")).toBeVisible();
  await expect(meter.locator("strong")).toContainText(/^\$[\d.,K<]+ saved (in this conversation|this session)$/);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("savings-details.png") });
});

test("a later measured sample does not relabel earlier estimates as measured", async ({ page }) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark&ghostty=off");
  const pane = page.locator('.terminal-pane[data-terminal-scene-complete="true"]').first();
  await expect(pane).toBeVisible();
  const sessionId = (await pane.getAttribute("data-terminal-session-id"))!;
  await page.evaluate(async (sessionId) => {
    const { emitHarnessEvent } = await import("/src/harness/mock-ipc.ts");
    for (const savingsSource of ["estimated", "measured"]) {
      await emitHarnessEvent("agent-session-event", { sessionId, event: {
        type: "savingsSample", savingsTokens: 100, savingsSource, turnBoundary: true,
      } });
    }
  }, sessionId);
  const meter = page.locator('.terminal-header').getByRole("region", { name: "Session savings meter" }).first();
  await expect(meter.getByText("estimated", { exact: true })).toBeVisible();
  // The chip carries "estimated"; only a routing estimate adds "≈".
  await expect(meter.locator("strong")).toHaveText(/saved/);
  await expect(meter.locator("strong")).not.toHaveText(/^\u2248/);
});

test("the meter link opens How Halv estimates savings in Settings", async ({ page }, testInfo) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark&ghostty=off");
  const meter = page.locator('.terminal-header').getByRole("region", { name: "Session savings meter" }).first();
  await meter.getByText("Details", { exact: true }).click();
  await meter.getByRole("button", { name: "How Halv estimates savings" }).click();
  const section = page.locator("#savings-method");
  await expect(section).toBeVisible();
  await expect(section).toContainText("Halv counts the dollars that the same work would cost at API list prices.");
  await expect(section).toContainText(/Prices as of/);
  await section.screenshot({ path: testInfo.outputPath("savings-method.png") });
});

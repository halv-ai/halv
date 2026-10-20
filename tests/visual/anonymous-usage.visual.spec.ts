import { expect, test, type Page } from "@playwright/test";

const label = "Share anonymous usage events";

async function savedPreference(page: Page) {
  return page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    return (await ipc.preferences.get()).analyticsEnabled;
  });
}

test("welcome discloses anonymous analytics and saves the choice", async ({
  page,
}, testInfo) => {
  await page.goto("/ux-audit.html?state=welcome");
  const checkbox = page.getByRole("checkbox", { name: label });
  await expect(checkbox).toBeEnabled();
  await expect(checkbox).toBeChecked();
  await expect(
    page.getByText(/No prompts, code, emails, or file paths/),
  ).toBeVisible();
  await expect(page.getByText(/No telemetry/)).toHaveCount(0);
  await checkbox.uncheck();
  await expect.poll(() => savedPreference(page)).toBe(false);
  await expect(checkbox).not.toBeChecked();
  await page.screenshot({ path: testInfo.outputPath("welcome-analytics.png") });
});

test("onboarding and Settings share the persisted preference in both directions", async ({
  page,
}, testInfo) => {
  await page.goto("/ux-audit.html?state=first-run-codex");
  const onboardingCheckbox = page
    .getByRole("checkbox", { name: label })
    .first();
  await expect(onboardingCheckbox).toBeEnabled();
  await onboardingCheckbox.uncheck();
  await expect.poll(() => savedPreference(page)).toBe(false);
  await page
    .getByRole("button", { name: "Privacy settings", exact: true })
    .click();
  const settings = page.getByRole("dialog", { name: "Settings", exact: true });
  const settingsCheckbox = settings.getByRole("checkbox", { name: label });
  await expect(settingsCheckbox).not.toBeChecked();
  await settingsCheckbox.check();
  await expect.poll(() => savedPreference(page)).toBe(true);
  await expect(onboardingCheckbox).toBeChecked();
  await settings.getByRole("button", { name: "Close", exact: true }).click();
  await expect(onboardingCheckbox).toBeChecked();
  await page.screenshot({
    path: testInfo.outputPath("onboarding-analytics.png"),
  });
  await page
    .getByRole("button", { name: "Privacy settings", exact: true })
    .click();
  await expect(settingsCheckbox).toBeChecked();
});

test("a failed save keeps the actual preference and can be retried", async ({
  page,
}) => {
  await page.goto("/ux-audit.html?state=welcome");
  const checkbox = page.getByRole("checkbox", { name: label });
  await expect(checkbox).toBeEnabled();
  await page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    const original = ipc.preferences.setAnalyticsEnabled;
    ipc.preferences.setAnalyticsEnabled = async () => {
      throw new Error("disk full");
    };
    (window as any).__restoreAnalyticsPreference = () => {
      ipc.preferences.setAnalyticsEnabled = original;
    };
  });
  await checkbox.click();
  await expect(page.getByRole("alert")).toContainText(
    "Could not save the anonymous usage preference: disk full",
  );
  await expect(checkbox).toBeChecked();
  expect(await savedPreference(page)).toBe(true);
  await page.evaluate(() => (window as any).__restoreAnalyticsPreference());
  await checkbox.uncheck();
  await expect.poll(() => savedPreference(page)).toBe(false);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

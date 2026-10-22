// The consent list version 2 in the whole app (ADR 0022). These checks read
// the saved answer and the native calls, not pixels, so they need no baseline.
import { expect, test, type Page } from "@playwright/test";
import { openVisualState } from "./capture";

type AnalyticsCall = { command: string; args: Record<string, unknown> };
type HarnessWindow = Window & { __harnessAnalyticsCalls?: AnalyticsCall[] };

const NOTICE = "Halv now also sends a daily usage summary";

function analyticsCalls(page: Page) {
  return page.evaluate(
    () => (window as HarnessWindow).__harnessAnalyticsCalls ?? [],
  );
}

async function savedConsent(page: Page) {
  return page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    const preferences = await ipc.preferences.get();
    return {
      consent: preferences.analyticsConsent,
      version: preferences.analyticsConsentVersion,
      asked: preferences.analyticsConsentAsked,
    };
  });
}

async function openSettings(page: Page, category: RegExp) {
  await page
    .getByLabel("Workspace and application menu", { exact: true })
    .click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await dialog.getByRole("button", { name: category }).click();
  return dialog;
}

// Two frames after the saved answer loaded, React has drawn the banner.
async function settle(page: Page) {
  await expect.poll(() => savedConsent(page)).toMatchObject({ consent: "on" });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
}

test("the notice asks a version 1 yes, and a yes saves version 2", async ({
  page,
}) => {
  await openVisualState(page, "chat-active", "consent=notice");
  const notice = page.getByRole("complementary", { name: "Usage events" });
  await expect(notice.getByText(NOTICE)).toBeVisible();
  expect(await savedConsent(page)).toEqual({
    consent: "on",
    version: 1,
    asked: 1,
  });
  await notice.getByRole("button", { name: "Share the summary too" }).click();
  await expect(page.getByText(NOTICE)).toHaveCount(0);
  expect(await savedConsent(page)).toEqual({
    consent: "on",
    version: 2,
    asked: 2,
  });
});

test("the notice closes on a no, and the answer keeps version 1", async ({
  page,
}) => {
  await openVisualState(page, "chat-active", "consent=notice");
  await expect(page.getByText(NOTICE)).toBeVisible();
  await page.getByRole("button", { name: "Keep the current list" }).click();
  await expect(page.getByText(NOTICE)).toHaveCount(0);
  expect(await savedConsent(page)).toEqual({
    consent: "on",
    version: 1,
    asked: 2,
  });
});

test("the notice stays away from an answer of version 2 and from the welcome screen", async ({
  page,
}) => {
  await openVisualState(page, "chat-active", "consent=on");
  await settle(page);
  await expect(page.getByText(NOTICE)).toHaveCount(0);
  await openVisualState(page, "welcome", "consent=notice");
  await settle(page);
  await expect(page.getByText(NOTICE)).toHaveCount(0);
});

test("Settings moves a version 1 answer to version 2 after the notice closed", async ({
  page,
}) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark&consent=notice");
  // The user keeps version 1 in the notice, so the notice does not come back.
  await page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    await ipc.preferences.setAnalyticsConsent({
      consent: "on",
      consentVersion: 1,
    });
  });
  const dialog = await openSettings(page, /^Agent & privacy/);
  await expect(
    dialog.getByRole("heading", { name: "Diagnostics & privacy" }),
  ).toBeVisible();
  await dialog.getByText("What is sent", { exact: true }).click();
  const version = dialog.getByText("List version you agreed to:");
  const note = dialog.getByText(/^Version 1 sends only the 13 events/);
  await expect(version).toContainText("1");
  await expect(note).toBeVisible();
  await dialog.getByRole("button", { name: "Share the summary too" }).click();
  await expect(note).toHaveCount(0);
  await expect(version).toContainText("2");
  expect(await savedConsent(page)).toEqual({
    consent: "on",
    version: 2,
    asked: 2,
  });
});

test("Settings shows no version 1 button for an answer of version 2", async ({
  page,
}) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark&consent=on");
  const dialog = await openSettings(page, /^Agent & privacy/);
  await dialog.getByText("What is sent", { exact: true }).click();
  await expect(dialog.getByText("List version you agreed to:")).toContainText(
    "2",
  );
  await expect(
    dialog.getByRole("button", { name: "Share the summary too" }),
  ).toHaveCount(0);
});

test("the webview sends the UI language at start and on each change", async ({
  page,
}) => {
  await page.goto("/ux-audit.html?state=marketing-app-dark");
  const locales = async () =>
    (await analyticsCalls(page))
      .filter((call) => call.command === "analytics_set_locale")
      .map((call) => call.args.locale);
  await expect.poll(locales).toEqual(["en"]);
  const dialog = await openSettings(page, /^Preferences/);
  await dialog.locator("#settings-language").selectOption("pt-BR");
  await expect.poll(locales).toEqual(["en", "pt-BR"]);
});

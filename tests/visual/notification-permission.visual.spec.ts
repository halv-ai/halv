import { expect, test, type Page } from "@playwright/test";

type PermissionWindow = Window & {
  __harnessNotificationPermission?: string;
  __harnessNotificationSettingsOpened?: number;
  __harnessTerminalNotifications?: Array<{
    sessionId: string;
    title: string;
    body: string;
  }>;
};

async function openNotificationSettings(page: Page, state: string) {
  await page.addInitScript((value) => {
    (window as PermissionWindow).__harnessNotificationPermission = value;
  }, state);
  await page.goto("/ux-audit.html?state=marketing-app-dark");
  await page
    .getByLabel("Workspace and application menu", { exact: true })
    .click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await dialog.getByRole("button", { name: /^Preferences/ }).click();
  return dialog;
}

test("a denied macOS state shows the cause and opens System Settings", async ({
  page,
}, testInfo) => {
  const dialog = await openNotificationSettings(page, "denied");
  const notice = dialog.getByTestId("notification-permission-notice");
  await expect(notice.getByRole("alert")).toContainText(
    "macOS blocks notifications from Halv",
  );
  await expect(
    notice.getByRole("button", { name: "Allow notifications" }),
  ).toHaveCount(0);
  await notice.scrollIntoViewIfNeeded();
  await notice.screenshot({
    animations: "disabled",
    path: testInfo.outputPath("notification-denied.png"),
  });
  await notice
    .getByRole("button", { name: "Open System Settings", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as PermissionWindow).__harnessNotificationSettingsOpened ?? 0,
      ),
    )
    .toBe(1);
});

test("an undecided macOS state asks, then shows the allowed state", async ({
  page,
}, testInfo) => {
  const dialog = await openNotificationSettings(page, "notDetermined");
  const notice = dialog.getByTestId("notification-permission-notice");
  await expect(notice).toContainText("has not asked yet");
  await notice.scrollIntoViewIfNeeded();
  await notice.screenshot({
    animations: "disabled",
    path: testInfo.outputPath("notification-undecided.png"),
  });
  await notice
    .getByRole("button", { name: "Allow notifications", exact: true })
    .click();
  await expect(notice).toContainText("macOS allows notifications from Halv.");
  await expect(
    notice.getByRole("button", { name: "Allow notifications" }),
  ).toHaveCount(0);
});

test("alert style None offers System Settings, not a prompt", async ({
  page,
}) => {
  const dialog = await openNotificationSettings(page, "bannersOff");
  const notice = dialog.getByTestId("notification-permission-notice");
  await expect(notice).toContainText("the alert style is None");
  await expect(
    notice.getByRole("button", { name: "Open System Settings", exact: true }),
  ).toBeVisible();
});

test("the test button waits, then sends one banner that names no pane", async ({
  page,
}, testInfo) => {
  test.setTimeout(30_000);
  const dialog = await openNotificationSettings(page, "granted");
  const notice = dialog.getByTestId("notification-permission-notice");
  await expect(notice).toContainText("macOS allows notifications from Halv.");
  await notice
    .getByRole("button", { name: "Send test notification", exact: true })
    .click();
  // macOS shows no banner while Halv is in front, so the banner waits.
  await expect(notice.getByRole("status").last()).toContainText(
    "Switch to another app now",
  );
  expect(
    await page.evaluate(
      () => (window as PermissionWindow).__harnessTerminalNotifications ?? [],
    ),
  ).toEqual([]);
  await expect(notice.getByRole("status").last()).toContainText(
    "Test notification sent",
    { timeout: 15_000 },
  );
  const sent = await page.evaluate(
    () => (window as PermissionWindow).__harnessTerminalNotifications ?? [],
  );
  expect(sent).toEqual([
    {
      sessionId: "",
      title: "Halv test notification",
      body: "If you see this banner, Halv notifications work.",
    },
  ]);
  await notice.scrollIntoViewIfNeeded();
  await notice.screenshot({
    animations: "disabled",
    path: testInfo.outputPath("notification-granted.png"),
  });
});

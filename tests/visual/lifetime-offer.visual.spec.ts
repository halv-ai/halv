import { expect, test, type Page } from "@playwright/test";

import { LIFETIME_OFFER_DISMISSED_STORAGE_KEY } from "../../src/lifetimeOffer.ts";
import { openVisualState } from "./capture";

declare global {
  interface Window {
    __lifetimeOfferAppearances?: number;
  }
}

async function openSettings(page: Page) {
  await page
    .getByLabel("Workspace and application menu", { exact: true })
    .click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings", exact: true });
  await expect(dialog).toBeVisible();
  return dialog;
}

async function trackLifetimeOfferAppearances(page: Page) {
  await page.evaluate(() => {
    window.__lifetimeOfferAppearances = 0;
    const observer = new MutationObserver(() => {
      if (document.querySelector("[data-lifetime-offer]")) {
        window.__lifetimeOfferAppearances =
          (window.__lifetimeOfferAppearances ?? 0) + 1;
      }
    });
    observer.observe(document.documentElement, {
      childList: true,
      subtree: true,
    });
  });
}

test("subscriber dismissal survives a reload", async ({ page }, testInfo) => {
  await openVisualState(page, "chat-active");
  const dialog = await openSettings(page);
  const offer = dialog.locator("[data-lifetime-offer]");

  await expect(offer).toBeVisible();
  await offer.screenshot({
    path: testInfo.outputPath("subscriber-visible.png"),
  });
  await offer.getByRole("button", { name: "Dismiss", exact: true }).click();
  await expect(offer).toBeHidden();
  await expect
    .poll(() =>
      page.evaluate(
        (key) => window.localStorage.getItem(key),
        LIFETIME_OFFER_DISMISSED_STORAGE_KEY,
      ),
    )
    .toBe("1");

  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => document.documentElement.dataset.visualReady === "chat-active",
  );
  const reloadedDialog = await openSettings(page);
  await expect(reloadedDialog.locator("[data-lifetime-offer]")).toHaveCount(0);
  await reloadedDialog.screenshot({
    animations: "disabled",
    path: testInfo.outputPath("subscriber-dismissed-after-reload.png"),
  });
});

test("lifetime account never mounts the offer", async ({ page }, testInfo) => {
  await openVisualState(page, "chat-active");
  await page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    await ipc.billing.checkoutBrowser({ plan: "lifetime", interval: "month" });
    window.dispatchEvent(new Event("focus"));
  });
  await page.waitForTimeout(100);
  await trackLifetimeOfferAppearances(page);

  const dialog = await openSettings(page);
  await expect(dialog.getByText(/Plan Lifetime \/ synced/)).toBeVisible();
  await expect(dialog.locator("[data-lifetime-offer]")).toHaveCount(0);
  await page.waitForTimeout(500);
  expect(await page.evaluate(() => window.__lifetimeOfferAppearances)).toBe(0);
  await dialog.screenshot({
    animations: "disabled",
    path: testInfo.outputPath("lifetime-account.png"),
  });
});

test("pending offer does not flash while entitlements are unresolved", async ({
  page,
}, testInfo) => {
  await openVisualState(page, "chat-active");
  await trackLifetimeOfferAppearances(page);
  await page.evaluate(async () => {
    const { ipc } = await import("/src/ipc.ts");
    const account = await ipc.auth.status();
    const lifetimeOffer = ipc.billing.lifetimeOffer;
    ipc.billing.lifetimeOffer = () =>
      new Promise((resolve, reject) => {
        window.setTimeout(() => {
          void lifetimeOffer().then(resolve, reject);
        }, 750);
      });
    ipc.auth.refreshEntitlements = async () => ({
      ...account,
      entitlements: null,
      syncedAt: null,
    });
  });

  const dialog = await openSettings(page);
  await dialog
    .getByRole("button", { name: "Refresh status", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Verifying your subscription..." }),
  ).toBeVisible();
  await page.waitForTimeout(1_000);
  await expect(page.locator("[data-lifetime-offer]")).toHaveCount(0);
  expect(await page.evaluate(() => window.__lifetimeOfferAppearances)).toBe(0);
  await page.screenshot({
    animations: "disabled",
    fullPage: true,
    path: testInfo.outputPath("entitlements-loading.png"),
  });
});

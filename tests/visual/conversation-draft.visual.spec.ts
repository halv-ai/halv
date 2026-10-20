import { expect, test } from "@playwright/test";

const HARNESS_URL = "/ux-audit.html?scenario=normal";

test("an unsent new chat remains in the sidebar and restores its draft", async ({
  page,
}) => {
  await page.goto(HARNESS_URL);
  await page
    .locator(".group button", { hasText: "Audit agent refactor" })
    .first()
    .waitFor({ timeout: 30_000 });

  await page.getByTitle("New conversation").click();
  const composer = page.getByPlaceholder(/describe a task|message/i).first();
  await composer.fill("Draft navigation check");

  const draftRow = page
    .locator("button", { hasText: "Draft navigation check" })
    .first();
  await expect(draftRow.locator('[title="Unsent draft"]')).toBeVisible();

  await page
    .locator(".group button", { hasText: "Audit agent refactor" })
    .first()
    .click();
  await expect(draftRow).toBeVisible();

  await draftRow.click();
  await expect(composer).toHaveValue("Draft navigation check");

  await page.getByRole("button", { name: "Send" }).first().click();
  await expect(composer).toHaveValue("");
  await expect(draftRow.locator('[title="Unsent draft"]')).toHaveCount(0);
  await expect(
    page.locator("button", { hasText: "Draft navigation check" }),
  ).toHaveCount(1);
});

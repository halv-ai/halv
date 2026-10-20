import { expect, test, type Page } from "@playwright/test";
import { openVisualState } from "./capture";
import { TABS_QUERY, stripTab } from "./workspaceTabsProbe";

// Orca's agent state vocabulary (ADR 0010, stage S6): a spinner for work,
// a question bubble, a check for a finished turn that stays after a view,
// an amber dashed ring for a pane that reports nothing, a red dot for a
// failure, and state chips for the folded agents of a workspace.

async function openGlyphs(page: Page, theme: "dark" | "light" = "dark") {
  await openVisualState(page, `agent-glyphs-${theme}`, TABS_QUERY);
  await page.locator('main[data-workspace-tabs="ready"]').waitFor();
}

function sidebar(page: Page) {
  return page.getByRole("complementary", { name: "Conversation navigation" });
}

function item(page: Page, key: string) {
  return page.locator(`[role="treeitem"][data-tree-key="${key}"]`);
}

function rowGlyph(page: Page, key: string) {
  return item(page, key).locator(":scope > .project-tree-row [data-agent-glyph]").first();
}

for (const theme of ["dark", "light"] as const) {
  test(`folded agents show Orca's state chips, and rows keep their state, in the ${theme} theme`, async ({ page }) => {
    await openGlyphs(page, theme);
    const pill = item(page, "agents:primary-1");
    await expect(pill).toHaveAttribute("aria-expanded", "false");
    const chips = pill.locator(".project-tree-chip");
    await expect(chips).toHaveCount(3);
    expect(
      await chips.evaluateAll((elements) =>
        elements.map((element) => element.getAttribute("data-chip-glyph")),
      ),
    ).toEqual(["failed", "done", "unverifiable"]);
    // Two finished Claude agents: one icon and +1.
    await expect(pill.locator('.project-tree-chip[data-chip-glyph="done"] .project-tree-chip-icon')).toHaveCount(1);
    await expect(pill.locator('.project-tree-chip[data-chip-glyph="done"] .project-tree-chip-extra')).toHaveText("+1");
    await expect(pill.locator(".project-tree-chips > .project-tree-chip-extra")).toHaveCount(0);
    await expect(pill.locator(":scope > .project-tree-row")).toHaveAttribute(
      "title",
      "4 agents, Failed 1, Done 2, Not reporting status 1",
    );
    await expect(sidebar(page)).toHaveScreenshot(`agent-glyphs-chips-${theme}.png`);

    await pill.locator(":scope > .project-tree-row").click();
    await expect(pill).toHaveAttribute("aria-expanded", "true");
    await expect(rowGlyph(page, "conversation:glyph-review")).toHaveAttribute("data-agent-glyph", "done");
    await expect(rowGlyph(page, "conversation:audit-agent-refactor")).toHaveAttribute("data-agent-glyph", "done");
    await expect(rowGlyph(page, "conversation:glyph-deps")).toHaveAttribute("data-agent-glyph", "unverifiable");
    await expect(rowGlyph(page, "conversation:glyph-deps")).toHaveAttribute("aria-label", "Not reporting status");
    await expect(rowGlyph(page, "conversation:glyph-config")).toHaveAttribute("data-agent-glyph", "failed");
    // The visible tab read its finished turn: the check stays and the row
    // is not bold. The other finished turn is unread and bold.
    await expect(item(page, "conversation:audit-agent-refactor").locator(".font-semibold")).toHaveCount(0);
    await expect(item(page, "conversation:glyph-review").locator(".font-semibold")).toHaveCount(1);
    await expect(sidebar(page)).toHaveScreenshot(`agent-glyphs-rows-${theme}.png`);
  });
}

test("workspace lanes summarize their agents: a spinner, a question, a red dot", async ({ page }) => {
  await openGlyphs(page);
  const docs = rowGlyph(page, "workspace:audit-wt-docs");
  await expect(docs).toHaveAttribute("data-agent-glyph", "working");
  await expect(docs.locator(".agent-working-spinner")).toHaveCount(1);
  await expect(rowGlyph(page, "workspace:audit-wt-login")).toHaveAttribute("data-agent-glyph", "attention");
  await expect(rowGlyph(page, "workspace:primary-1")).toHaveAttribute("data-agent-glyph", "failed");
});

test("a tab shows the glyph of its row; a read check stays and only news is bold", async ({ page }) => {
  await openGlyphs(page);
  const refactor = stripTab(page, "Audit agent refactor");
  await expect(refactor).toHaveAttribute("aria-selected", "true");
  await expect(refactor.locator("[data-agent-glyph]")).toHaveAttribute("data-agent-glyph", "done");
  await expect(refactor).not.toHaveAttribute("data-unread", /.*/);
  const review = stripTab(page, "Review flaky tests");
  await expect(review.locator("[data-agent-glyph]")).toHaveAttribute("data-agent-glyph", "done");
  await expect(review).toHaveAttribute("data-unread", "true");
  await expect(stripTab(page, "Bump dependencies").locator("[data-agent-glyph]")).toHaveAttribute(
    "data-agent-glyph",
    "unverifiable",
  );
  // A view reads the turn. The check stays; the bold goes.
  await review.click();
  await expect(review).toHaveAttribute("aria-selected", "true");
  await expect(review).not.toHaveAttribute("data-unread", /.*/);
  await expect(review.locator("[data-agent-glyph]")).toHaveAttribute("data-agent-glyph", "done");
});

test("the working ring turns without reduced motion and stands still with it", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await openGlyphs(page);
  // The capture helper stops every animation for stable screenshots.
  await page.evaluate(() => {
    for (const style of document.querySelectorAll("style")) {
      if (style.textContent?.includes("animation: none !important")) style.remove();
    }
  });
  const spinner = rowGlyph(page, "workspace:audit-wt-docs").locator(".agent-working-spinner");
  await expect(spinner).toHaveCSS("animation-name", "agent-spinner-rotate");
  await expect(spinner).toHaveCSS("border-top-color", "rgba(0, 0, 0, 0)");
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect(spinner).toHaveCSS("animation-name", "none");
});

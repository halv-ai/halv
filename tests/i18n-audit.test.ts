import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { localeDefinitions } from "../src/i18n/config.ts";
import { invariantSources } from "../src/i18n/messages.ts";

const catalogLocales = localeDefinitions.filter(({ code }) => code !== "en");

const optionGroups = [
  ["active", "disabled", "invalid"],
  ["never", "background", "always"],
  ["system", "light", "dark"],
  ["Healthy", "Needs attention", "Unavailable"],
  ["Settings", "Preferences"],
] as const;

// The Basic plan cap UI and its "Upgrade to Unlimited" button are gone
// (ADR 0015), so this source names the plan now.
const planSource = "Plan Unlimited";
const quantitySources = [
  "Unlimited",
  "Saved. Concurrent child agents are Unlimited.",
  "Enter a positive whole number, or leave it empty for Unlimited.",
  "Optionally cap how many child agents can run at once to help control usage. Leave this empty for Unlimited.",
] as const;

type Catalog = Record<string, string>;
type CatalogEntry = { locale: string; catalog: Catalog };
type ReviewData = { sources: string[]; catalogs: CatalogEntry[] };

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function sourceHasWholeWord(source: string, term: string): boolean {
  const escaped = escapeRegExp(term);
  return new RegExp(
    `(^|[^\\p{L}\\p{N}_])${escaped}(?=$|[^\\p{L}\\p{N}_])`,
    "u",
  ).test(source);
}

async function readReviewData(): Promise<ReviewData> {
  const sourcesUrl = new URL("../src/i18n/sources.json", import.meta.url);
  const parsedSources = JSON.parse(await readFile(sourcesUrl, "utf8")) as unknown;
  assert.ok(Array.isArray(parsedSources));
  assert.ok(parsedSources.every((source) => typeof source === "string"));
  const sources = parsedSources as string[];

  const catalogs = await Promise.all(
    catalogLocales.map(async ({ code }) => {
      const catalogUrl = new URL(
        `../src/i18n/catalogs/${code}.json`,
        import.meta.url,
      );
      const parsed = JSON.parse(await readFile(catalogUrl, "utf8")) as unknown;
      assert.ok(
        parsed !== null && typeof parsed === "object" && !Array.isArray(parsed),
        code,
      );
      return { locale: code, catalog: parsed as Catalog };
    }),
  );

  return { sources, catalogs };
}

const reviewData = readReviewData();

function translationFor(
  locale: string,
  catalog: Catalog,
  source: string,
): string {
  const translation = catalog[source];
  assert.equal(typeof translation, "string", `${locale}: missing ${source}`);
  return translation;
}

test("translations preserve invariant terms used as whole words", async () => {
  const { sources, catalogs } = await reviewData;

  for (const { locale, catalog } of catalogs) {
    for (const source of sources) {
      if (!Object.prototype.hasOwnProperty.call(catalog, source)) continue;
      const translation = catalog[source];
      for (const term of invariantSources) {
        if (!sourceHasWholeWord(source, term)) continue;
        assert.ok(
          translation.includes(term),
          `${locale}: ${term} missing from ${source}`,
        );
      }
    }
  }
});

test("translations keep control option groups distinct", async () => {
  const { sources, catalogs } = await reviewData;
  const sourceSet = new Set(sources);

  for (const group of optionGroups) {
    for (const source of group) {
      assert.ok(sourceSet.has(source), `sources.json missing ${source}`);
    }
  }

  for (const { locale, catalog } of catalogs) {
    for (const group of optionGroups) {
      const translations = group.map((source) =>
        translationFor(locale, catalog, source),
      );
      assert.equal(
        new Set(translations).size,
        group.length,
        `${locale}: option collision in ${group.join(" / ")}`,
      );
    }
  }
});

test("translations distinguish the Unlimited plan from quantities", async () => {
  const { sources, catalogs } = await reviewData;
  const sourceSet = new Set(sources);
  assert.ok(sourceSet.has(planSource), `sources.json missing ${planSource}`);
  for (const source of quantitySources) {
    assert.ok(sourceSet.has(source), `sources.json missing ${source}`);
  }

  for (const { locale, catalog } of catalogs) {
    assert.ok(
      translationFor(locale, catalog, planSource).includes("Unlimited"),
      `${locale}: plan name must contain Unlimited`,
    );
    for (const source of quantitySources) {
      assert.ok(
        !translationFor(locale, catalog, source).includes("Unlimited"),
        `${locale}: quantity must translate Unlimited in ${source}`,
      );
    }
  }
});

test("translations preserve trailing ellipses", async () => {
  const { sources, catalogs } = await reviewData;
  const ellipsisSources = sources.filter((source) => source.endsWith("..."));

  for (const { locale, catalog } of catalogs) {
    for (const source of ellipsisSources) {
      if (!Object.prototype.hasOwnProperty.call(catalog, source)) continue;
      const translation = catalog[source];
      assert.ok(
        translation.endsWith("...") || translation.endsWith("…"),
        `${locale}: trailing ellipsis missing from ${source}`,
      );
    }
  }
});

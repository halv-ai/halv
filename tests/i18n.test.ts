import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  localeDefinitions,
  matchLanguageTag,
} from "../src/i18n/config.ts";
import {
  detectSystemLocale,
  resolveLanguagePreference,
} from "../src/i18n/language.ts";
import { formatMessage, translateText } from "../src/i18n/messages.ts";

const catalogFiles = localeDefinitions
  .filter(({ code }) => code !== "en")
  .map(({ code }) => `${code}.json`);

function placeholders(value: string): string[] {
  return [
    ...new Set(
      [...value.matchAll(/\{([a-zA-Z][\w-]*)\}/g)].map(
        (match) => match[1],
      ),
    ),
  ].sort();
}

test("matchLanguageTag resolves supported locale variants", () => {
  const cases = [
    ["en", "en"],
    ["en-US", "en"],
    ["pt-BR", "pt-BR"],
    ["pt_BR", "pt-BR"],
    ["pt-PT", "pt-BR"],
    ["es", "es"],
    ["es-419", "es"],
    ["fr-CA", "fr"],
    ["de-AT", "de"],
    ["it-IT", "it"],
    ["nl-BE", "nl"],
    ["ja-JP", "ja"],
    ["zh", "zh-CN"],
    ["zh-Hans", "zh-CN"],
    ["zh-SG", "zh-CN"],
    ["zh-TW", "zh-TW"],
    ["ZH-HANT", "zh-TW"],
    ["zh-HK", "zh-TW"],
    ["ko-KR", "ko"],
    ["sv-SE", null],
  ] as const;

  for (const [tag, expected] of cases) {
    assert.equal(matchLanguageTag(tag), expected, tag);
  }
});

test("detectSystemLocale falls back and uses the first supported tag", () => {
  assert.equal(detectSystemLocale([]), "en");
  assert.equal(detectSystemLocale(["sv-SE", "fr-CA"]), "fr");
});

test("resolveLanguagePreference preserves explicit choices", () => {
  assert.equal(resolveLanguagePreference("de", ["pt-BR"]), "de");
  assert.equal(resolveLanguagePreference("system", ["pt-BR"]), "pt-BR");
});

test("translateText falls back for English, invariants, and missing keys", () => {
  assert.equal(translateText("en", "Language"), "Language");
  assert.equal(translateText("de", "Halv"), "Halv");
  assert.equal(translateText("fr", "Missing translation"), "Missing translation");
});

test("formatMessage substitutes known placeholders only", () => {
  assert.equal(
    formatMessage("Hello, {name}. Keep {unknown}.", { name: "Ada" }),
    "Hello, Ada. Keep {unknown}.",
  );
  assert.equal(formatMessage("Saved {count} tokens.", { count: 42 }), "Saved 42 tokens.");
});

test("translation catalogs have valid structure", async () => {
  const sourcesUrl = new URL("../src/i18n/sources.json", import.meta.url);
  const sources = JSON.parse(await readFile(sourcesUrl, "utf8")) as unknown;
  assert.ok(Array.isArray(sources));
  assert.ok(sources.every((source) => typeof source === "string"));
  const sourceSet = new Set<string>(sources);

  for (const file of catalogFiles) {
    const catalogUrl = new URL(`../src/i18n/catalogs/${file}`, import.meta.url);
    const parsed = JSON.parse(await readFile(catalogUrl, "utf8")) as unknown;
    assert.ok(parsed !== null && typeof parsed === "object" && !Array.isArray(parsed), file);

    for (const [source, translation] of Object.entries(parsed)) {
      assert.equal(typeof translation, "string", `${file}: ${source}`);
      assert.ok(translation.trim().length > 0, `${file}: ${source}`);
      assert.ok(sourceSet.has(source), `${file}: stale key ${source}`);
      assert.deepEqual(
        placeholders(translation),
        placeholders(source),
        `${file}: ${source}`,
      );
    }
  }
});

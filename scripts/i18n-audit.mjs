#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { localeDefinitions } from "../src/i18n/config.ts";
import { invariantSources } from "../src/i18n/messages.ts";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const ROOT_DIR = resolve(SCRIPT_DIR, "..");
const SOURCES_PATH = resolve(ROOT_DIR, "src/i18n/sources.json");
const CATALOG_DIR = resolve(ROOT_DIR, "src/i18n/catalogs");

const LOCALES = localeDefinitions
  .filter(({ code }) => code !== "en")
  .map(({ code }) => code);

const LOWERCASE_ENUMS = [
  "active",
  "always",
  "background",
  "dark",
  "disabled",
  "friendly",
  "invalid",
  "light",
  "never",
  "pragmatic",
  "system",
  "unknown",
];

const MAX_WARNING_DETAILS = 40;

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function sourceHasWholeWord(source, term) {
  const escaped = escapeRegExp(term);
  return new RegExp(
    `(^|[^\\p{L}\\p{N}_])${escaped}(?=$|[^\\p{L}\\p{N}_])`,
    "u",
  ).test(source);
}

function placeholders(value) {
  return [
    ...new Set(
      [...value.matchAll(/\{([a-zA-Z][\w-]*)\}/g)].map(
        (match) => match[1],
      ),
    ),
  ].sort();
}

function saveFamilySources(sources) {
  return sources.filter((source) => {
    const withoutPlaceholders = source.replace(/\{[^}]+\}/g, "");
    return /\b(?:save|saved|saving)\b/i.test(withoutPlaceholders);
  });
}

function addSaveFamilyWarnings(catalog, sources, warnings) {
  const saveLabel = catalog.Save;
  if (typeof saveLabel !== "string" || !saveLabel.trim()) return;

  const firstWord = saveLabel.normalize("NFKC").match(/[\p{L}\p{N}]+/u)?.[0];
  if (!firstWord) return;

  const stem = [...firstWord.toLowerCase()].slice(0, 4).join("");
  for (const source of saveFamilySources(sources)) {
    const translation = catalog[source];
    if (typeof translation !== "string") continue;
    if (translation.normalize("NFKC").toLowerCase().includes(stem)) continue;
    warnings.push(
      `save-family term differs from ${JSON.stringify(saveLabel)}: ` +
        `${JSON.stringify(source)} -> ${JSON.stringify(translation)}`,
    );
  }
}

function auditLocale(locale, sources, requireComplete) {
  const problems = [];
  const warnings = [];
  const catalogPath = resolve(CATALOG_DIR, `${locale}.json`);
  let raw;

  try {
    raw = readFileSync(catalogPath, "utf8");
  } catch (error) {
    problems.push(`could not read catalog: ${error.message}`);
    return { locale, keyCount: 0, translatedCount: 0, problems, warnings };
  }

  if (!raw.endsWith("\n")) {
    problems.push("file does not end with a newline");
  }
  if (raw.startsWith("\uFEFF")) {
    problems.push("file has a BOM");
  }

  let catalog;
  try {
    catalog = JSON.parse(raw.startsWith("\uFEFF") ? raw.slice(1) : raw);
  } catch (error) {
    problems.push(`invalid JSON: ${error.message}`);
    return { locale, keyCount: 0, translatedCount: 0, problems, warnings };
  }

  if (catalog === null || typeof catalog !== "object" || Array.isArray(catalog)) {
    problems.push("catalog is not a JSON object");
    return { locale, keyCount: 0, translatedCount: 0, problems, warnings };
  }

  const sourceSet = new Set(sources);
  const keys = Object.keys(catalog);
  const missing = sources.filter(
    (source) => !Object.prototype.hasOwnProperty.call(catalog, source),
  );
  const stale = keys.filter((key) => !sourceSet.has(key));
  const translatedCount = sources.length - missing.length;

  if (missing.length) {
    const missingMessage =
      `missing ${missing.length} keys: ${JSON.stringify(missing.slice(0, 5))}`;
    (requireComplete ? problems : warnings).push(missingMessage);
  }
  if (stale.length) {
    problems.push(
      `stale ${stale.length} keys: ${JSON.stringify(stale.slice(0, 5))}`,
    );
  }

  const ordered =
    keys.length === sources.length &&
    keys.every((key, index) => key === sources[index]);
  if (!ordered) {
    warnings.push("key order differs from sources.json");
  }

  for (const [source, value] of Object.entries(catalog)) {
    if (typeof value !== "string") {
      problems.push(`non-string value: ${JSON.stringify(source)}`);
      continue;
    }
    if (!value.trim()) {
      problems.push(`empty value: ${JSON.stringify(source)}`);
      continue;
    }

    const sourcePlaceholders = placeholders(source);
    const valuePlaceholders = placeholders(value);
    if (
      JSON.stringify(sourcePlaceholders) !== JSON.stringify(valuePlaceholders)
    ) {
      problems.push(
        `placeholder mismatch: ${JSON.stringify(source)} -> ` +
          `${JSON.stringify(value)} (${sourcePlaceholders} vs ${valuePlaceholders})`,
      );
    }

    for (const term of invariantSources) {
      if (sourceHasWholeWord(source, term) && !value.includes(term)) {
        problems.push(
          `invariant ${JSON.stringify(term)} lost: ` +
            `${JSON.stringify(source)} -> ${JSON.stringify(value)}`,
        );
      }
    }

    if (
      source.endsWith("...") &&
      !value.endsWith("...") &&
      !value.endsWith("…")
    ) {
      warnings.push(
        `lost trailing ellipsis: ${JSON.stringify(source)} -> ${JSON.stringify(value)}`,
      );
    }
    if (
      /[.!?]$/.test(source) &&
      !source.endsWith("...") &&
      !/[.!?。！？]$/.test(value)
    ) {
      warnings.push(
        `lost terminal punctuation: ${JSON.stringify(source)} -> ${JSON.stringify(value)}`,
      );
    }
    if (value === source && !invariantSources.has(source)) {
      warnings.push(
        `untranslated (identical to source): ${JSON.stringify(source)}`,
      );
    }
    if (source.length > 0 && value.length > source.length * 1.8) {
      warnings.push(
        `translation exceeds 1.8x source length (${source.length}->${value.length}): ` +
          `${JSON.stringify(source)} -> ${JSON.stringify(value)}`,
      );
    }
  }

  for (const label of LOWERCASE_ENUMS) {
    const value = catalog[label];
    if (value && /^[A-ZÀ-Þ]/.test(value) && locale !== "de") {
      warnings.push(`enum label capitalized: ${label} -> ${value}`);
    }
  }

  addSaveFamilyWarnings(catalog, sources, warnings);
  return {
    locale,
    keyCount: keys.length,
    translatedCount,
    problems,
    warnings,
  };
}

function printLocaleReport(report, sourceCount) {
  const coveragePercent = sourceCount
    ? Math.round((report.translatedCount / sourceCount) * 100)
    : 100;
  console.log(`=== ${report.locale} ===`);
  console.log(`keys: ${report.keyCount} / ${sourceCount}`);
  console.log(
    `coverage: ${report.translatedCount} / ${sourceCount} translated (${coveragePercent}%)`,
  );
  console.log(`FAIL: ${report.problems.length}`);
  for (const problem of report.problems) console.log(`  ! ${problem}`);
  console.log(`WARN: ${report.warnings.length}`);
  for (const warning of report.warnings.slice(0, MAX_WARNING_DETAILS)) {
    console.log(`  ~ ${warning}`);
  }
  if (report.warnings.length > MAX_WARNING_DETAILS) {
    console.log(
      `  ~ ...and ${report.warnings.length - MAX_WARNING_DETAILS} more`,
    );
  }
}

function main() {
  const args = process.argv.slice(2);
  const requireComplete = args.includes("--require-complete");
  const requestedLocales = args.filter((arg) => arg !== "--require-complete");
  const invalidLocales = requestedLocales.filter(
    (locale) => !LOCALES.includes(locale),
  );
  if (invalidLocales.length) {
    console.error(`Unknown locale: ${invalidLocales.join(", ")}`);
    console.error(`Available locales: ${LOCALES.join(", ")}`);
    return 2;
  }

  let sources;
  try {
    sources = JSON.parse(readFileSync(SOURCES_PATH, "utf8"));
  } catch (error) {
    console.error(`Could not read sources.json: ${error.message}`);
    return 2;
  }
  if (!Array.isArray(sources) || !sources.every((source) => typeof source === "string")) {
    console.error("sources.json is not an array of strings");
    return 2;
  }

  const locales = requestedLocales.length ? requestedLocales : LOCALES;
  const reports = locales.map((locale) =>
    auditLocale(locale, sources, requireComplete),
  );
  for (const report of reports) printLocaleReport(report, sources.length);

  const failCount = reports.reduce(
    (total, report) => total + report.problems.length,
    0,
  );
  const warningCount = reports.reduce(
    (total, report) => total + report.warnings.length,
    0,
  );
  console.log("=== total ===");
  console.log(`locales: ${reports.length}`);
  console.log(`FAIL: ${failCount}`);
  console.log(`WARN: ${warningCount}`);
  return failCount ? 1 : 0;
}

process.exitCode = main();

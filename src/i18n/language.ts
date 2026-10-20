import {
  defaultLocale,
  localeDefinitions,
  matchLanguageTag,
  type Locale,
} from "./config.ts";

export type LanguagePreference = "system" | Locale;

export const LANGUAGE_STORAGE_KEY = "halv.language";
export const languagePreferences: LanguagePreference[] = [
  "system",
  ...localeDefinitions.map((definition) => definition.code),
];

export function isLanguagePreference(
  value: string,
): value is LanguagePreference {
  return languagePreferences.includes(value as LanguagePreference);
}

export function detectSystemLocale(tags: readonly string[]): Locale {
  for (const tag of tags) {
    const locale = matchLanguageTag(tag);
    if (locale) return locale;
  }
  return defaultLocale;
}

export function resolveLanguagePreference(
  preference: LanguagePreference,
  systemLanguageTags: readonly string[],
): Locale {
  return preference === "system"
    ? detectSystemLocale(systemLanguageTags)
    : preference;
}

export function readSystemLanguageTags(): readonly string[] {
  try {
    if (navigator.languages.length > 0) return [...navigator.languages];
    return navigator.language ? [navigator.language] : [];
  } catch {
    return [];
  }
}

export function readLanguagePreference(): LanguagePreference {
  try {
    const stored = window.localStorage.getItem(LANGUAGE_STORAGE_KEY);
    return stored && isLanguagePreference(stored) ? stored : "system";
  } catch {
    return "system";
  }
}

export function writeLanguagePreference(
  preference: LanguagePreference,
): void {
  try {
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, preference);
  } catch {
    // Language persistence is best effort; the active locale still updates.
  }
}

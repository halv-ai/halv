export const localeDefinitions = [
  {
    code: "en",
    label: "English",
    htmlLang: "en",
    intlLocale: "en-US",
  },
  {
    code: "pt-BR",
    label: "Português (Brasil)",
    htmlLang: "pt-BR",
    intlLocale: "pt-BR",
  },
  {
    code: "es",
    label: "Español",
    htmlLang: "es",
    intlLocale: "es-ES",
  },
  {
    code: "fr",
    label: "Français",
    htmlLang: "fr",
    intlLocale: "fr-FR",
  },
  {
    code: "de",
    label: "Deutsch",
    htmlLang: "de",
    intlLocale: "de-DE",
  },
  {
    code: "it",
    label: "Italiano",
    htmlLang: "it",
    intlLocale: "it-IT",
  },
  {
    code: "nl",
    label: "Nederlands",
    htmlLang: "nl",
    intlLocale: "nl-NL",
  },
  {
    code: "ja",
    label: "日本語",
    htmlLang: "ja",
    intlLocale: "ja-JP",
  },
  {
    code: "zh-CN",
    label: "简体中文",
    htmlLang: "zh-CN",
    intlLocale: "zh-CN",
  },
  {
    code: "zh-TW",
    label: "繁體中文",
    htmlLang: "zh-TW",
    intlLocale: "zh-TW",
  },
  {
    code: "ko",
    label: "한국어",
    htmlLang: "ko",
    intlLocale: "ko-KR",
  },
] as const;

export type Locale = (typeof localeDefinitions)[number]["code"];
export type LocaleDefinition = (typeof localeDefinitions)[number];

export const defaultLocale: Locale = "en";

const definitionByCode = new Map<Locale, LocaleDefinition>(
  localeDefinitions.map((definition) => [definition.code, definition]),
);

export function getLocaleDefinition(locale: Locale): LocaleDefinition {
  return definitionByCode.get(locale) ?? localeDefinitions[0];
}

export function matchLanguageTag(tag: string): Locale | null {
  const normalized = tag.trim().replace(/_/g, "-").toLowerCase();
  if (!normalized) return null;

  const parts = normalized.split("-");
  const language = parts[0];
  if (language === "zh") {
    if (
      parts.includes("hant") ||
      parts.includes("tw") ||
      parts.includes("hk") ||
      parts.includes("mo")
    ) {
      return "zh-TW";
    }
    return "zh-CN";
  }

  const localeByLanguage: Partial<Record<string, Locale>> = {
    en: "en",
    pt: "pt-BR",
    es: "es",
    fr: "fr",
    de: "de",
    it: "it",
    nl: "nl",
    ja: "ja",
    ko: "ko",
  };
  return localeByLanguage[language] ?? null;
}

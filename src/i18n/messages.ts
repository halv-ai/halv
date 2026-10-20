import type { Locale } from "./config.ts";
import de from "./catalogs/de.json" with { type: "json" };
import es from "./catalogs/es.json" with { type: "json" };
import fr from "./catalogs/fr.json" with { type: "json" };
import it from "./catalogs/it.json" with { type: "json" };
import ja from "./catalogs/ja.json" with { type: "json" };
import ko from "./catalogs/ko.json" with { type: "json" };
import nl from "./catalogs/nl.json" with { type: "json" };
import ptBR from "./catalogs/pt-BR.json" with { type: "json" };
import zhCN from "./catalogs/zh-CN.json" with { type: "json" };
import zhTW from "./catalogs/zh-TW.json" with { type: "json" };

export type TranslationCatalog = Record<string, string>;
export type MessageValues = Readonly<Record<string, string | number>>;

export const invariantSources = new Set([
  "Halv",
  "Pro",
  "Claude",
  "Codex",
  "ChatGPT",
  "OpenAI",
  "Anthropic",
  "Kimi",
  "Grok",
  "Ghostty",
  "macOS",
  "Windows",
  "Linux",
  "MCP",
  "CLI",
  "API",
  "JSON",
  "URL",
  "Git",
  "GitHub",
]);

export const catalogs: Partial<Record<Locale, TranslationCatalog>> = {
  "pt-BR": ptBR,
  es,
  fr,
  de,
  it,
  nl,
  ja,
  "zh-CN": zhCN,
  "zh-TW": zhTW,
  ko,
};

export function hasTranslation(locale: Locale, source: string): boolean {
  if (locale === "en") return true;
  const catalog = catalogs[locale];
  return (
    catalog !== undefined &&
    Object.prototype.hasOwnProperty.call(catalog, source)
  );
}

export function translateText(locale: Locale, source: string): string {
  if (locale === "en" || invariantSources.has(source)) return source;
  return catalogs[locale]?.[source] ?? source;
}

export function formatMessage(
  template: string,
  values: MessageValues,
): string {
  return template.replace(/\{([a-zA-Z][\w-]*)\}/g, (placeholder, key) =>
    Object.prototype.hasOwnProperty.call(values, key)
      ? String(values[key])
      : placeholder,
  );
}

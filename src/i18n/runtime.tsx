import React, { useCallback, useContext, useSyncExternalStore } from "react";
import { formatUsdCompact } from "../format.ts";
import { getLocaleDefinition, type Locale } from "./config.ts";
import {
  formatMessage,
  translateText,
  type MessageValues,
} from "./messages.ts";

type LocaleListener = () => void;
export type Translate = (source: string, values?: MessageValues) => string;

let activeLocale: Locale = "en";
const localeListeners = new Set<LocaleListener>();

export function getActiveLocale(): Locale {
  return activeLocale;
}

export function setActiveLocale(locale: Locale): void {
  if (locale === activeLocale) return;
  activeLocale = locale;
  for (const listener of localeListeners) listener();
}

export function subscribeActiveLocale(listener: LocaleListener): () => void {
  localeListeners.add(listener);
  return () => localeListeners.delete(listener);
}

export const LocaleContext = React.createContext<Locale>(activeLocale);

export const LocaleProvider: React.FC<{ children: React.ReactNode }> = ({
  children,
}) => {
  const locale = useSyncExternalStore(
    subscribeActiveLocale,
    getActiveLocale,
    getActiveLocale,
  );
  return (
    <LocaleContext.Provider value={locale}>{children}</LocaleContext.Provider>
  );
};

export function useLocale(): Locale {
  return useContext(LocaleContext);
}

export function useT(): Translate {
  const locale = useLocale();
  return useCallback(
    (source, values) => {
      const translated = translateText(locale, source);
      return values ? formatMessage(translated, values) : translated;
    },
    [locale],
  );
}

export function formatNumber(
  value: number,
  options?: Intl.NumberFormatOptions,
): string {
  const { intlLocale } = getLocaleDefinition(activeLocale);
  return new Intl.NumberFormat(intlLocale, options).format(value);
}

/** Dollars saved in the compact form of ADR 0015, in the app locale. */
export function formatMoney(value: number | null | undefined): string {
  return formatUsdCompact(value, getLocaleDefinition(activeLocale).intlLocale);
}

export function formatDateTime(
  value: Date | number,
  options?: Intl.DateTimeFormatOptions,
): string {
  const { intlLocale } = getLocaleDefinition(activeLocale);
  return new Intl.DateTimeFormat(intlLocale, options).format(value);
}

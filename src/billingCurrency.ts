import type { BillingCurrency } from "./types";

export function formatPrice(
  minorUnits: number,
  currency: BillingCurrency,
  locale: string,
): string {
  const hasFraction = Math.abs(minorUnits) % 100 !== 0;
  return new Intl.NumberFormat(locale, {
    style: "currency",
    currency: currency.toUpperCase(),
    currencyDisplay: "narrowSymbol",
    minimumFractionDigits: hasFraction ? 2 : 0,
    maximumFractionDigits: hasFraction ? 2 : 0,
  }).format(minorUnits / 100);
}

import { useEffect, useState } from "react";
import {
  billingPrices,
  type BillingInterval,
  type BillingPlan,
} from "../billing";
import { formatPrice } from "../billingCurrency";
import { useLocale, useT, type Translate } from "../i18n/runtime";
import type { BillingPrices } from "../types";
import type { BillingCheckoutController } from "../useBillingCheckout";

type SubscriptionPlansProps = {
  checkout: BillingCheckoutController;
  defaultPlan?: BillingPlan;
  availablePlans?: BillingPlan[];
  compact?: boolean;
  secondary?: boolean;
  onClose?: () => void;
};

const FALLBACK_PLAN_DETAILS: Record<
  Exclude<BillingPlan, "lifetime">,
  {
    name: string;
    monthPrice: string;
    yearPrice: string;
  }
> = {
  basic: {
    name: "Basic",
    monthPrice: "$2",
    yearPrice: "$16",
  },
  unlimited: {
    name: "Unlimited",
    monthPrice: "$10",
    yearPrice: "$79",
  },
};

let billingPricesCache: BillingPrices | null = null;
let billingPricesRequest: Promise<BillingPrices> | null = null;

function loadBillingPrices(): Promise<BillingPrices> {
  if (billingPricesCache) return Promise.resolve(billingPricesCache);

  if (billingPricesRequest) return billingPricesRequest;

  billingPricesRequest = billingPrices()
    .then((response) => {
      if (!validBillingPrices(response)) {
        throw new Error("Billing prices response was invalid.");
      }
      billingPricesCache = response;
      return response;
    })
    .finally(() => {
      billingPricesRequest = null;
    });
  return billingPricesRequest;
}

export function useBillingPrices(): BillingPrices | null {
  const [prices, setPrices] = useState<BillingPrices | null>(billingPricesCache);

  useEffect(() => {
    if (billingPricesCache) {
      setPrices(billingPricesCache);
      return;
    }

    let active = true;
    void loadBillingPrices().then(
      (response) => {
        if (active) setPrices(response);
      },
      () => {
        if (active) setPrices(null);
      },
    );
    return () => {
      active = false;
    };
  }, []);

  return prices;
}

function validBillingPrices(prices: BillingPrices): boolean {
  return (
    (prices.currency === "usd" || prices.currency === "brl") &&
    validMinorUnits(prices.plans?.basic?.month) &&
    validMinorUnits(prices.plans?.basic?.year) &&
    validMinorUnits(prices.plans?.unlimited?.month) &&
    validMinorUnits(prices.plans?.unlimited?.year)
  );
}

function validMinorUnits(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

export function SubscriptionPlans({
  checkout,
  defaultPlan = "unlimited",
  availablePlans: _availablePlans,
  compact = false,
  secondary = false,
  onClose,
}: SubscriptionPlansProps) {
  const t = useT();
  const locale = useLocale();
  const availablePlans: BillingPlan[] = ["unlimited"];
  const [selectedPlan, setSelectedPlan] = useState<BillingPlan>("unlimited");
  const billingInterval: BillingInterval = "month";
  const localizedPrices = useBillingPrices();

  useEffect(() => {
    if (!availablePlans.includes(selectedPlan)) {
      setSelectedPlan(availablePlans[0] ?? defaultPlan);
    }
  }, [availablePlans, defaultPlan, selectedPlan]);

  if (checkout.phase === "activating" && checkout.activePlan !== "lifetime") {
    return (
      <CheckoutStatusCard
        detail={t(
          "Stripe confirmed checkout. Halv is waiting for the subscription to reach your account.",
        )}
        title={t("Activating your subscription…")}
      >
        <div className="mt-4 h-1.5 overflow-hidden rounded-full bg-surface-sunken">
          <div className="h-full w-1/2 animate-pulse rounded-full bg-accent" />
        </div>
        <p className="mt-3 text-xs text-content-muted">
          {t("This usually takes only a moment. You can leave this screen open.")}
        </p>
      </CheckoutStatusCard>
    );
  }

  if (checkout.phase === "timedOut" && checkout.activePlan !== "lifetime") {
    return (
      <CheckoutStatusCard
        detail={t(
          "We could not confirm the new subscription yet. If Stripe completed the checkout, retry the account check.",
        )}
        title={t("Activation is taking longer than expected")}
      >
        <div className="mt-4 flex flex-wrap gap-2">
          <button
            className="rounded-md bg-accent px-3 py-2 text-sm font-semibold text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40"
            onClick={checkout.retryActivation}
            type="button"
          >
            {t("Retry activation")}
          </button>
          <button
            className="rounded-md border border-surface-border/35 px-3 py-2 text-sm text-content-secondary transition-colors hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30"
            onClick={() => void checkout.contactSupport()}
            type="button"
          >
            {t("Contact support at halv.ai")}
          </button>
          <button
            className="rounded-md px-3 py-2 text-sm text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary"
            onClick={checkout.reset}
            type="button"
          >
            {t("Choose a plan again")}
          </button>
        </div>
        {checkout.error ? <CheckoutError message={checkout.error} /> : null}
      </CheckoutStatusCard>
    );
  }

  const busy = checkout.phase !== "idle";
  const price = displayPrice(
    selectedPlan,
    billingInterval,
    localizedPrices,
    locale,
  );

  if (secondary) {
    return <section className="flex flex-col self-start rounded-xl border border-surface-border/35 bg-surface-panel p-5 sm:p-6" aria-label={t("Monthly membership")}>
      <p className="text-xs font-medium uppercase tracking-wider text-content-muted">{t("Prefer monthly?")}</p>
      <h2 className="mt-3 text-xl font-semibold text-content-primary">{t("Monthly membership")}</h2>
      <p className="mt-2 text-sm leading-6 text-content-secondary">{t("All features. No savings cap.")}</p>
      <p className="my-6"><span className="text-4xl font-semibold tracking-tight text-content-primary">{price}</span><span className="ml-1 text-sm text-content-muted">{t("/mo")}</span></p>
      <p className="text-xs leading-5 text-content-secondary">{t("7-day free trial · card required · cancel anytime")}</p>
      <div className="mt-auto pt-6">
        <button type="button" disabled={busy} onClick={()=>void checkout.start("unlimited","month")} className="w-full rounded-lg border border-surface-border/60 bg-surface-raised px-4 py-3 text-sm font-semibold text-content-primary transition-colors hover:border-accent/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-50">
          {busy && checkout.activePlan === "unlimited" ? t("Checkout open") : t("Start free trial · {price}/mo", {price})}
        </button>
        <button type="button" disabled={busy} onClick={()=>void checkout.openInBrowser("unlimited","month")} className="mt-3 w-full rounded text-xs text-content-muted underline underline-offset-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-50">{t("Open in browser instead")}</button>
        {checkout.error && checkout.activePlan === "unlimited" ? <CheckoutError message={checkout.error}/> : null}
      </div>
    </section>;
  }

  return (
    <div
      className={
        compact
          ? "rounded-md border border-surface-border/30 bg-surface-base/35 p-3"
          : ""
      }
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-base font-semibold text-content-primary">
            {t("Unlimited")}
          </h2>
          <p className="mt-1 text-xs text-content-muted">
            {t("7-day free trial · card required · cancel anytime")}
          </p>
        </div>

      </div>

      <div
        className={`mt-4 grid gap-3 ${availablePlans.length > 1 ? "sm:grid-cols-2" : "grid-cols-1"}`}
      >
        {availablePlans.map((plan) => {
          const details = FALLBACK_PLAN_DETAILS[plan === "lifetime" ? "unlimited" : plan];
          const selectedNow = selectedPlan === plan;
          const planPrice = displayPrice(
            plan,
            billingInterval,
            localizedPrices,
            locale,
          );
          return (
            <button
              aria-pressed={selectedNow}
              className={`rounded-lg border p-4 text-left transition-colors focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-70 ${
                selectedNow
                  ? "border-accent/65 bg-accent/10"
                  : "border-surface-border/30 bg-surface-raised/40 hover:border-surface-border/60 hover:bg-surface-raised/70"
              }`}
              disabled={busy}
              key={plan}
              onClick={() => setSelectedPlan(plan)}
              type="button"
            >
              <div className="flex items-start justify-between gap-3">
                <div>
                  <span className="text-sm font-semibold text-content-primary">
                    {details.name}
                  </span>
                  <p className="mt-1 text-xs leading-5 text-content-muted">
                    {planAudience(plan, t)}
                  </p>
                </div>
                <span
                  aria-hidden
                  className={`mt-0.5 flex h-4 w-4 items-center justify-center rounded-full border text-[10px] ${
                    selectedNow
                      ? "border-accent bg-accent text-accent-contrast"
                      : "border-surface-border/50"
                  }`}
                >
                  {selectedNow ? "✓" : ""}
                </span>
              </div>
              <p className="mt-4 text-content-primary">
                <span className="text-2xl font-semibold">{planPrice}</span>
                <span className="ml-1 text-xs text-content-muted">
                  {billingInterval === "month" ? t("/mo") : t("/yr")}
                </span>
              </p>
              <p className="mt-2 text-xs font-medium text-content-secondary">
                {planBenefit(plan, t)}
              </p>
            </button>
          );
        })}
      </div>

      {checkout.phase === "checkout" ? (
        <div className="mt-4 rounded-md border border-accent/30 bg-accent/10 px-3 py-2 text-sm text-accent-strong">
          {t(
            "Complete checkout in the secure Halv window. Closing it brings you back here.",
          )}
        </div>
      ) : null}

      <button
        className="mt-4 w-full rounded-md bg-accent px-4 py-2.5 text-sm font-semibold text-accent-contrast transition-colors hover:bg-accent-strong focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:cursor-not-allowed disabled:opacity-60"
        disabled={busy}
        onClick={() =>
          void checkout.start(selectedPlan, billingInterval)
        }
        type="button"
      >
        {checkout.phase === "opening"
          ? t("Opening secure checkout…")
          : checkout.phase === "checkout"
            ? t("Checkout open")
            : billingInterval === "month"
              ? t("Start free trial · {price}/mo", { price })
              : t("Start free trial · {price}/yr", { price })}
      </button>

      <div className="mt-3 text-center">
        <button
          className="text-xs text-content-muted underline decoration-surface-border underline-offset-4 transition-colors hover:text-content-primary disabled:cursor-not-allowed disabled:opacity-50"
          disabled={busy}
          onClick={() =>
            void checkout.openInBrowser(
              selectedPlan,
              billingInterval,
            )
          }
          type="button"
        >
          {checkout.phase === "openingExternal"
            ? t("Opening browser…")
            : t("Open in browser instead")}
        </button>
        <p className="mt-1 text-[11px] leading-4 text-content-muted">
          {t(
            "Use your browser for Apple Pay or if embedded checkout does not load.",
          )}
        </p>
      </div>

      {checkout.error ? <CheckoutError message={checkout.error} /> : null}

      {onClose && checkout.phase === "idle" ? (
        <button
          className="mt-3 w-full rounded-md px-3 py-2 text-xs text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary"
          onClick={onClose}
          type="button"
        >
          {t("Not now")}
        </button>
      ) : null}
    </div>
  );
}

function displayPrice(
  plan: BillingPlan,
  interval: BillingInterval,
  prices: BillingPrices | null,
  locale: string,
): string {
  if (plan === "lifetime") return "$90";
  if (!prices) {
    const fallback = FALLBACK_PLAN_DETAILS[plan];
    return interval === "month" ? fallback.monthPrice : fallback.yearPrice;
  }
  return formatPrice(
    prices.plans[plan][interval],
    prices.currency,
    locale,
  );
}

function planAudience(plan: BillingPlan, t: Translate) {
  switch (plan) {
    case "basic":
      return t("For the $20 ChatGPT plan");
    case "unlimited":
      return t("All features. No savings cap.");
  }
}

function planBenefit(plan: BillingPlan, t: Translate) {
  switch (plan) {
    case "basic":
      return t("Limited monthly savings");
    case "unlimited":
      return t("No savings cap");
  }
}

function CheckoutStatusCard({
  title,
  detail,
  children,
}: {
  title: string;
  detail: string;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-lg border border-accent/30 bg-accent/10 p-4">
      <h2 className="text-base font-semibold text-content-primary">{title}</h2>
      <p className="mt-2 text-sm leading-6 text-content-secondary">{detail}</p>
      {children}
    </div>
  );
}

function CheckoutError({ message }: { message: string }) {
  return (
    <div className="mt-3 rounded-md border border-danger/40 bg-danger-surface/55 px-3 py-2 text-sm text-danger">
      {message}
    </div>
  );
}

import { useEffect, useState } from "react";
import { ipc } from "../ipc";
import { useT, useLocale } from "../i18n/runtime";
import { formatPrice } from "../billingCurrency";
import type { BillingCheckoutController } from "../useBillingCheckout";
import type { AccountAuthStatus, LifetimeOfferStatus } from "../types";
import {
  canDismissLifetimeOffer,
  canLoadLifetimeOffer,
  readLifetimeOfferDismissed,
  remainingOfferSeconds,
  subscribeLifetimeOfferDismissed,
  writeLifetimeOfferDismissed,
} from "../lifetimeOffer";

export function LifetimeOffer({ account, checkout }: { account: AccountAuthStatus | null; checkout: BillingCheckoutController }) {
  const t = useT();
  const locale = useLocale();
  const [snapshot, setSnapshot] = useState<{offer: LifetimeOfferStatus;received: number} | null>(null);
  const [tick, setTick] = useState(0);
  const [dismissed, setDismissed] = useState(() =>
    readLifetimeOfferDismissed(),
  );
  const canLoad = canLoadLifetimeOffer(account, dismissed);
  const canDismiss = canDismissLifetimeOffer(account);

  useEffect(
    () => subscribeLifetimeOfferDismissed(() => setDismissed(true)),
    [],
  );

  useEffect(() => {
    if (!canLoad) return;
    let live = true;
    const refresh = () => void ipc.billing.lifetimeOffer().then(offer => {
      if (live) setSnapshot({offer,received:performance.now()});
    }).catch(() => { if (live) setSnapshot(null); });
    refresh();
    const refreshTimer = window.setInterval(refresh,30_000);
    const clock = window.setInterval(() => setTick(x=>x+1),1_000);
    const visible = () => { if(document.visibilityState === 'visible') refresh(); };
    document.addEventListener('visibilitychange',visible);
    return () => { live=false;window.clearInterval(refreshTimer);window.clearInterval(clock);document.removeEventListener('visibilitychange',visible); };
  },[account?.email,canLoad]);

  function dismiss() {
    setDismissed(true);
    writeLifetimeOfferDismissed();
  }

  void tick;
  if (!snapshot || !canLoad) return null;
  const {offer,received}=snapshot;
  const seconds=remainingOfferSeconds(offer,performance.now()-received);
  const inCheckout = checkout.activePlan === "lifetime" && checkout.phase !== "idle";
  if ((!offer.active || offer.owned || seconds <= 0 || offer.available <= 0) && !inCheckout) return null;
  const days=Math.floor(seconds/86400);
  const time=[Math.floor(seconds%86400/3600),Math.floor(seconds%3600/60),seconds%60].map(v=>String(v).padStart(2,'0')).join(':');
  const busy=checkout.phase !== 'idle';
  const price=formatPrice(offer.amount,offer.currency,locale);
  const selected = checkout.activePlan === "lifetime";
  const waiting = selected && checkout.phase === "activating";
  const timedOut = selected && checkout.phase === "timedOut";
  return <section data-lifetime-offer className="relative flex flex-col overflow-hidden rounded-xl border border-accent/60 bg-gradient-to-br from-accent/10 via-surface-panel to-surface-panel p-5 shadow-[0_8px_32px_-16px_rgb(var(--accent)/0.3)] sm:p-6" aria-label={t("Lifetime membership")}>
    <div aria-hidden="true" className="absolute inset-x-0 top-0 h-1 bg-accent" />
    <div className="flex flex-wrap items-center justify-between gap-2">
      <p className="text-xs font-semibold uppercase tracking-wider text-accent-strong">{t("Lifetime membership")}</p>
      <div className="flex items-center gap-2">
        <span className="rounded-full border border-accent/25 bg-accent/10 px-2.5 py-1 text-[10px] font-medium text-accent-strong">{t("Limited to 100 lifetime members")}</span>
        {canDismiss ? (
          <button
            aria-label={t("Dismiss")}
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full border border-surface-border/35 text-lg leading-none text-content-muted transition-colors hover:bg-surface-raised hover:text-content-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
            onClick={dismiss}
            type="button"
          >
            <span aria-hidden="true">×</span>
          </button>
        ) : null}
      </div>
    </div>
    <h2 className="mt-5 max-w-sm text-[28px] font-semibold leading-tight tracking-tight text-content-primary">{t("Pay once. Keep building.")}</h2>
    <div className="mt-5 flex flex-wrap items-end gap-x-3 gap-y-1">
      <p className="text-5xl font-semibold leading-none tracking-tight text-content-primary">{price}</p>
      <p className="pb-1 text-xs leading-5 text-content-secondary">USD · {t("one time")}<br/><span className="text-content-muted">{t("No recurring Halv fee.")}</span></p>
    </div>
    <ul className="my-6 space-y-2.5 text-sm text-content-secondary">
      {[t("Halv Cloud for life"), t("Task routing and automatic account rotation"), t("One payment. No renewals.")].map(benefit => <li key={benefit} className="flex items-center gap-2.5"><span aria-hidden="true" className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-accent/15 text-[10px] font-bold text-accent-strong">✓</span>{benefit}</li>)}
    </ul>
    <div className="mt-auto">
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-accent/20 bg-surface-base/55 px-3 py-2.5 text-xs">
        <span className="font-medium text-content-primary">{t("{available} memberships available", {available:offer.available})}</span>
        <span role="timer" aria-live="off" className="font-mono tabular-nums text-content-secondary">{t("Ends in {days}d {time}",{days,time})}</span>
      </div>
      <p className="mt-1.5 text-[10px] leading-4 text-content-muted">{t("Availability includes temporary checkout reservations.")}</p>
      <button type="button" disabled={busy} onClick={()=>void checkout.start("lifetime","month")} className="mt-4 flex min-h-12 w-full items-center justify-center gap-2 rounded-lg bg-accent px-4 py-3 text-sm font-semibold text-accent-contrast transition-colors hover:bg-accent-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-surface-panel disabled:cursor-not-allowed disabled:opacity-60">
        {waiting ? t("Activating your membership…") : selected && checkout.phase === "checkout" ? t("Checkout open") : selected && checkout.phase === "opening" ? t("Opening secure checkout…") : selected && checkout.phase === "openingExternal" ? t("Opening browser…") : t("Get lifetime access · {price}",{price})}
        {!busy && <span aria-hidden="true">→</span>}
      </button>
      <p className="mt-2 text-center text-[11px] text-content-muted">{t("Secure checkout with Stripe")}</p>
      <div role="status" className="text-xs leading-5 text-content-secondary">
        {waiting && <p className="mt-3">{t("This usually takes only a moment. You can leave this screen open.")}</p>}
        {selected && checkout.phase === "checkout" && <p className="mt-3">{t("Complete checkout in the secure Halv window. Closing it brings you back here.")}</p>}
        {timedOut && <div className="mt-3 rounded-lg border border-surface-border/40 bg-surface-panel p-3"><p>{t("Activation is taking longer than expected")}</p><div className="mt-2 flex flex-wrap gap-3"><button type="button" className="font-semibold text-accent-strong underline underline-offset-4" onClick={checkout.retryActivation}>{t("Retry activation")}</button><button type="button" className="underline underline-offset-4" onClick={()=>void checkout.contactSupport()}>{t("Contact support at halv.ai")}</button><button type="button" className="underline underline-offset-4" onClick={checkout.reset}>{t("Choose a plan again")}</button></div></div>}
      </div>
      {selected && checkout.error && <p role="alert" className="mt-3 rounded-lg border border-danger/30 bg-danger-surface/30 p-3 text-xs text-danger">{checkout.error}</p>}
      <button type="button" disabled={busy} className="mt-3 w-full rounded text-xs text-content-muted underline underline-offset-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-50" onClick={()=>void checkout.openInBrowser("lifetime","month")}>{t("Open in browser instead")}</button>
      <p className="mt-4 border-t border-surface-border/25 pt-3 text-[11px] leading-5 text-content-muted">{t("After payment, your existing Halv subscription will stop renewing. Your agent subscriptions remain separate.")}</p>
    </div>
  </section>;
}

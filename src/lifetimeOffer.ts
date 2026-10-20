import type { AccountAuthStatus, LifetimeOfferStatus } from "./types.ts";

type LifetimeOfferStorage = Pick<Storage, "getItem" | "setItem">;
type LifetimeOfferDismissalListener = () => void;

const dismissalListeners = new Set<LifetimeOfferDismissalListener>();

export const LIFETIME_OFFER_DISMISSED_STORAGE_KEY =
  "halv.lifetimeOfferDismissed.v1";

export function readLifetimeOfferDismissed(
  storage: LifetimeOfferStorage | null = browserStorage(),
): boolean {
  try {
    return storage?.getItem(LIFETIME_OFFER_DISMISSED_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

export function writeLifetimeOfferDismissed(
  storage: LifetimeOfferStorage | null = browserStorage(),
): void {
  try {
    storage?.setItem(LIFETIME_OFFER_DISMISSED_STORAGE_KEY, "1");
  } catch {
    // The component keeps the dismissal while storage is unavailable.
  } finally {
    for (const listener of dismissalListeners) {
      listener();
    }
  }
}

export function subscribeLifetimeOfferDismissed(
  listener: LifetimeOfferDismissalListener,
): () => void {
  dismissalListeners.add(listener);
  return () => dismissalListeners.delete(listener);
}

export function canLoadLifetimeOffer(
  account: AccountAuthStatus | null,
  dismissed: boolean,
): boolean {
  if (dismissed || !account?.signedIn || !account.entitlements) {
    return false;
  }
  return !accountHasLifetimeAccess(account);
}

export function canDismissLifetimeOffer(
  account: AccountAuthStatus | null,
): boolean {
  if (
    !account?.signedIn ||
    !account.entitlements ||
    accountHasLifetimeAccess(account)
  ) {
    return false;
  }
  const plan = account.entitlements.plan;
  return plan === "basic" || plan === "unlimited";
}

function accountHasLifetimeAccess(account: AccountAuthStatus): boolean {
  return account.plan === "lifetime" || account.entitlements?.plan === "lifetime";
}

// Anchor to the server clock and elapsed monotonic time, not the device date.
export function remainingOfferSeconds(
  offer: LifetimeOfferStatus,
  elapsedMs: number,
) {
  if (
    !offer.active ||
    offer.owned ||
    !offer.endsAt ||
    !Number.isFinite(elapsedMs) ||
    elapsedMs > 65_000
  ) {
    return 0;
  }
  return Math.max(
    0,
    Math.floor(offer.endsAt - offer.serverNow - Math.max(0, elapsedMs) / 1000),
  );
}

function browserStorage(): LifetimeOfferStorage | null {
  if (typeof window === "undefined") {
    return null;
  }
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

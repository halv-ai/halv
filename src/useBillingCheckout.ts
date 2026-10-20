import { ipc, type UnlistenFn } from "./ipc.ts";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  billingCheckout,
  billingCheckoutInBrowser,
  openBillingSupport,
  type BillingInterval,
  type BillingPlan,
} from "./billing";
import { pollForBillingActivation } from "./billingActivation";
import type { AccountAuthStatus } from "./types";

export type BillingCheckoutPhase =
  | "idle"
  | "opening"
  | "checkout"
  | "openingExternal"
  | "activating"
  | "timedOut";

export type BillingCheckoutController = {
  phase: BillingCheckoutPhase;
  activePlan?: BillingPlan;
  error: string | null;
  start: (
    plan: BillingPlan,
    interval: BillingInterval,
  ) => Promise<void>;
  openInBrowser: (
    plan: BillingPlan,
    interval: BillingInterval,
  ) => Promise<void>;
  retryActivation: () => void;
  contactSupport: () => Promise<void>;
  reset: () => void;
};

type BillingCheckoutEvent = {
  outcome: "success" | "cancelled";
};

const BILLING_CHECKOUT_EVENT = "billing-checkout-event";

export function useBillingCheckout(
  onStatusChange: (status: AccountAuthStatus) => void,
  onCheckoutOpened?: () => void,
): BillingCheckoutController {
  const [phase, setPhase] = useState<BillingCheckoutPhase>("idle");
  const [activePlan, setActivePlan] = useState<BillingPlan | undefined>();
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  const pollGenerationRef = useRef(0);
  const expectedPlanRef = useRef<BillingPlan | undefined>(undefined);
  const onStatusChangeRef = useRef(onStatusChange);
  const onCheckoutOpenedRef = useRef(onCheckoutOpened);

  useEffect(() => {
    onStatusChangeRef.current = onStatusChange;
  }, [onStatusChange]);

  useEffect(() => {
    onCheckoutOpenedRef.current = onCheckoutOpened;
  }, [onCheckoutOpened]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      pollGenerationRef.current += 1;
    };
  }, []);

  const beginActivation = useCallback(() => {
    const generation = pollGenerationRef.current + 1;
    pollGenerationRef.current = generation;
    setError(null);
    setPhase("activating");

    void pollForBillingActivation({
      refresh: () =>
        ipc.auth.refreshEntitlements(),
      onStatus: (status) => onStatusChangeRef.current(status),
      expectedPlan: expectedPlanRef.current,
      cancelled: () =>
        !mountedRef.current || pollGenerationRef.current !== generation,
    }).then((result) => {
      if (!mountedRef.current || pollGenerationRef.current !== generation) {
        return;
      }
      if (result.outcome === "timedOut") {
        setPhase("timedOut");
      } else if (result.outcome === "activated") {
        setPhase("idle");
      }
    });
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;

    void ipc.events.listen<BillingCheckoutEvent>(BILLING_CHECKOUT_EVENT, (payload) => {
      if (payload.outcome === "success") {
        beginActivation();
        return;
      }
      pollGenerationRef.current += 1;
      setError(null);
      setPhase("idle");
    }).then((listener) => {
      if (disposed) {
        listener();
      } else {
        unlisten = listener;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [beginActivation]);

  async function start(
    plan: BillingPlan,
    interval: BillingInterval,
  ) {
    pollGenerationRef.current += 1;
    expectedPlanRef.current = plan;
    setActivePlan(plan);
    setError(null);
    setPhase("opening");
    try {
      await billingCheckout(plan, interval);
      if (mountedRef.current) {
        setPhase((current) => (current === "opening" ? "checkout" : current));
        onCheckoutOpenedRef.current?.();
      }
    } catch (checkoutError) {
      if (mountedRef.current) {
        setError(errorText(checkoutError));
        setPhase("idle");
      }
    }
  }

  async function openInBrowser(
    plan: BillingPlan,
    interval: BillingInterval,
  ) {
    pollGenerationRef.current += 1;
    expectedPlanRef.current = plan;
    setActivePlan(plan);
    setError(null);
    setPhase("openingExternal");
    try {
      await billingCheckoutInBrowser(plan, interval);
      if (mountedRef.current) {
        beginActivation();
        onCheckoutOpenedRef.current?.();
      }
    } catch (checkoutError) {
      if (mountedRef.current) {
        setError(errorText(checkoutError));
        setPhase("idle");
      }
    }
  }

  async function contactSupport() {
    setError(null);
    try {
      await openBillingSupport();
    } catch (supportError) {
      if (mountedRef.current) {
        setError(errorText(supportError));
      }
    }
  }

  function reset() {
    pollGenerationRef.current += 1;
    setError(null);
    setPhase("idle");
  }

  return {
    phase,
    activePlan,
    error,
    start,
    openInBrowser,
    retryActivation: beginActivation,
    contactSupport,
    reset,
  };
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

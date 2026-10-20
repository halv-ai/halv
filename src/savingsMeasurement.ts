import type {
  SavingsMeasurement,
  SavingsPricingSource,
} from "./types.ts";

export type SavingsMeasurementInput = {
  tokensSaved?: number | null;
  savingsUsd?: number | null;
  savingsSource?: string | null;
  pricingSource?: SavingsPricingSource | null;
  headroomTokensSaved?: number | null;
  rtkTokensSaved?: number | null;
  rtkIncludedInTotal?: boolean;
  cruxTokensSaved?: number | null;
  cruxIncludedInTotal?: boolean;
  cacheReadTokens?: number | null;
  savingsSources?: {
    proxy?: boolean;
    rtk?: "missing" | "present" | "instruction";
    crux?: "missing" | "present";
  } | null;
};

export function deriveSavingsMeasurement(
  input: SavingsMeasurementInput,
): SavingsMeasurement {
  const source = input.savingsSource?.trim().toLowerCase() ?? "";
  const pricingSource = input.pricingSource ?? undefined;
  const tokens = finiteNonNegative(input.tokensSaved);
  const usd = finiteNonNegative(input.savingsUsd);
  const rtk = finiteNonNegative(input.rtkTokensSaved);
  const crux = finiteNonNegative(input.cruxTokensSaved);
  const cacheRead = finitePositive(input.cacheReadTokens);
  let headroom = finiteNonNegative(input.headroomTokensSaved);

  if (source === "unmeasured") {
    return {
      state: "unmeasured",
      tokens: null,
      usd: null,
      ...(pricingSource ? { pricingSource } : {}),
    };
  }

  if (tokens !== null && headroom === null) {
    const includedRtk =
      input.rtkIncludedInTotal === true && rtk !== null ? rtk : 0;
    const includedCrux =
      input.cruxIncludedInTotal === true && crux !== null ? crux : 0;
    const remainder = Math.max(0, tokens - includedRtk - includedCrux);
    const completeBreakdown =
      input.rtkIncludedInTotal === true &&
      rtk !== null &&
      input.cruxIncludedInTotal === true &&
      crux !== null;

    if (
      completeBreakdown ||
      source === "headroom" ||
      pricingSource === "headroom"
    ) {
      headroom = remainder;
    } else if (
      input.cruxIncludedInTotal === true &&
      crux === tokens &&
      (rtk === null || rtk === 0)
    ) {
      headroom = 0;
    }
  }

  const composition = compactComposition({
    headroom,
    rtk,
    crux,
    cacheRead,
  });
  const sources = input.savingsSources;
  const measuredEvidence =
    source === "measured" ||
    source === "headroom" ||
    pricingSource === "headroom" ||
    headroom !== null ||
    rtk !== null ||
    sources?.proxy === true ||
    sources?.rtk === "present" ||
    sources?.rtk === "instruction";
  const estimatedEvidence =
    source === "estimated" ||
    source === "usage" ||
    pricingSource === "model_table" ||
    pricingSource === "fallback" ||
    (crux ?? 0) > 0 ||
    (cacheRead !== null && tokens === null) ||
    sources?.crux === "present";
  const hasValue =
    tokens !== null || usd !== null || Object.keys(composition).length > 0;

  if (!hasValue || (!measuredEvidence && !estimatedEvidence)) {
    return {
      state: "unmeasured",
      tokens: null,
      usd: null,
      ...(pricingSource ? { pricingSource } : {}),
    };
  }

  return {
    state: estimatedEvidence ? "estimated" : "measured",
    tokens,
    usd,
    ...(Object.keys(composition).length > 0 ? { composition } : {}),
    ...(pricingSource ? { pricingSource } : {}),
  };
}

export function savingsMeasurementIsEstimated(
  measurement: SavingsMeasurement,
) {
  return measurement.state === "estimated";
}

export type SavingsMeasurementBreakdown = {
  measurement: SavingsMeasurement;
  included: {
    headroom: number;
    rtk: number;
    crux: number;
    other: number;
  };
  separate: {
    rtk: number;
    crux: number;
    cacheRead: number;
  };
};

export function deriveSavingsMeasurementBreakdown(
  input: SavingsMeasurementInput,
): SavingsMeasurementBreakdown {
  const measurement = deriveSavingsMeasurement(input);
  const composition = measurement.composition;
  const total = measurement.tokens;
  const rawHeadroom = finitePositive(composition?.headroom) ?? 0;
  const rawRtk = finitePositive(composition?.rtk) ?? 0;
  const rawCrux = finitePositive(composition?.crux) ?? 0;
  const rawCacheRead = finitePositive(composition?.cacheRead) ?? 0;
  const includedRtk = input.rtkIncludedInTotal === true ? rawRtk : 0;
  const includedCrux = input.cruxIncludedInTotal === true ? rawCrux : 0;
  const includedToolTotal = includedRtk + includedCrux;
  const validIncludedTools = total !== null && includedToolTotal <= total;
  const rtk = validIncludedTools ? includedRtk : 0;
  const crux = validIncludedTools ? includedCrux : 0;
  let remaining = total === null ? 0 : Math.max(0, total - rtk - crux);
  const headroom = Math.min(rawHeadroom, remaining);
  remaining -= headroom;

  return {
    measurement,
    included: {
      headroom,
      rtk,
      crux,
      other: remaining,
    },
    separate: {
      rtk:
        input.rtkIncludedInTotal === true && validIncludedTools ? 0 : rawRtk,
      crux:
        input.cruxIncludedInTotal === true && validIncludedTools ? 0 : rawCrux,
      cacheRead: rawCacheRead,
    },
  };
}

function compactComposition({
  headroom,
  rtk,
  crux,
  cacheRead,
}: {
  headroom: number | null;
  rtk: number | null;
  crux: number | null;
  cacheRead: number | null;
}): NonNullable<SavingsMeasurement["composition"]> {
  return {
    ...(headroom !== null ? { headroom } : {}),
    ...(rtk !== null ? { rtk } : {}),
    ...(crux !== null ? { crux } : {}),
    ...(cacheRead !== null ? { cacheRead } : {}),
  };
}

function finitePositive(value: number | null | undefined) {
  const normalized = finiteNonNegative(value);
  return normalized !== null && normalized > 0 ? normalized : null;
}

function finiteNonNegative(value: number | null | undefined) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : null;
}

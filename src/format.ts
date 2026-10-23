import type {
  AnthropicCredentialStatus,
  AuthStatus,
  SavingsTotals,
  SidecarStatus,
} from "./types";

export function formatAuthStatus(auth: AuthStatus | null) {
  if (!auth) {
    return "loading";
  }

  if ("chatgpt" in auth) {
    const detail = [
      auth.chatgpt.email,
      auth.chatgpt.planType,
      auth.chatgpt.source,
      auth.chatgpt.accessTokenValid ? "token valid" : "token stale",
    ].filter(Boolean);
    return `ChatGPT${detail.length > 0 ? ` (${detail.join(" / ")})` : ""}`;
  }

  if ("apiKey" in auth) {
    return `API key ${auth.apiKey.masked}`;
  }

  if ("anthropicApiKey" in auth) {
    return `Anthropic API key ${auth.anthropicApiKey.masked}`;
  }

  return "not connected";
}

export function hasValidAuth(auth: AuthStatus | null) {
  if (!auth) {
    return false;
  }
  if ("apiKey" in auth) {
    return true;
  }
  return "chatgpt" in auth && auth.chatgpt.accessTokenValid;
}

export function hasProviderCredentials(
  auth: AuthStatus | null,
  anthropic: AnthropicCredentialStatus | null,
) {
  if (anthropic?.configured) {
    return true;
  }
  return auth !== null && !("signedOut" in auth);
}

export function authPrimaryLabel(auth: AuthStatus | null) {
  if (!auth) {
    return "Loading";
  }
  if ("chatgpt" in auth) {
    return auth.chatgpt.email ?? "ChatGPT account";
  }
  if ("apiKey" in auth) {
    return `API key ${auth.apiKey.masked}`;
  }
  if ("anthropicApiKey" in auth) {
    return `Anthropic API key ${auth.anthropicApiKey.masked}`;
  }
  return "Not connected";
}

export function authSecondaryLabel(auth: AuthStatus | null) {
  if (
    !auth ||
    "signedOut" in auth ||
    "apiKey" in auth ||
    "anthropicApiKey" in auth
  ) {
    return null;
  }

  const plan = auth.chatgpt.planType ?? "plan unknown";
  const source = auth.chatgpt.source === "codex-cli" ? "Codex CLI" : "Halv";
  const token = auth.chatgpt.accessTokenValid ? "token valid" : "token stale";
  return `${plan} / ${source} / ${token}`;
}

export function formatEngineStatus(sidecar: SidecarStatus | null) {
  if (!sidecar) {
    return "loading";
  }

  switch (sidecar.state) {
    case "running": {
      const ownership = sidecar.managedByHalv
        ? "managed by Halv"
        : sidecar.spawnedByUs
          ? "managed"
          : "external";
      return `running on :${sidecar.port} (${ownership})`;
    }
    case "installing":
      return `installing on :${sidecar.port}`;
    case "starting":
      return `starting on :${sidecar.port}`;
    case "notInstalled":
      return sidecar.installHint;
    case "error":
      return `error: ${sidecar.message}`;
    case "off":
      return "off";
  }
}

export function formatCompactNumber(value: number | null | undefined) {
  return (value ?? 0).toLocaleString();
}

export function formatCompactTokens(value: number | null | undefined) {
  const amount = value ?? 0;
  const abs = Math.abs(amount);
  if (abs >= 1_000_000) {
    return `${trimOneDecimal(amount / 1_000_000)}M`;
  }
  if (abs >= 1_000) {
    return `${trimOneDecimal(amount / 1_000)}K`;
  }
  return Math.round(amount).toLocaleString();
}

export function formatUsd(value: number | null | undefined) {
  return `$${(value ?? 0).toFixed(2)}`;
}

/**
 * Dollars saved in the compact form of ADR 0015, in the given Intl locale:
 * $0, <$0.01, $0.42, $12.3, $123, $1.2K, $1.2M. A negative value gets the
 * same rules on its absolute value. Intl gives the separators and the
 * symbol position of the locale.
 */
export function formatUsdCompact(
  value: number | null | undefined,
  intlLocale = "en-US",
): string {
  const amount = Number.isFinite(value) ? (value as number) : 0;
  const absolute = Math.abs(amount);
  const sign = amount < 0 ? "-" : "";
  const currency = (options: Intl.NumberFormatOptions, shown = absolute) =>
    new Intl.NumberFormat(intlLocale, {
      style: "currency",
      currency: "USD",
      currencyDisplay: "narrowSymbol",
      ...options,
    }).format(shown);
  if (absolute === 0) {
    return currency({ maximumFractionDigits: 0, minimumFractionDigits: 0 });
  }
  if (absolute < 0.01) {
    return `${sign}<${new Intl.NumberFormat(intlLocale, {
      style: "currency",
      currency: "USD",
      currencyDisplay: "narrowSymbol",
    }).format(0.01)}`;
  }
  // The tier follows the rounded value, so 9.996 shows as $10.
  const body =
    absolute < 9.995
      ? currency({ minimumFractionDigits: 2, maximumFractionDigits: 2 })
      : absolute < 99.95
        ? currency({ minimumFractionDigits: 0, maximumFractionDigits: 1 })
        : absolute < 999.5
          ? currency({ minimumFractionDigits: 0, maximumFractionDigits: 0 })
          : currency(
              {
                notation: "compact",
                minimumFractionDigits: 0,
                maximumFractionDigits: 1,
              },
              Math.max(absolute, 1_000),
            );
  return `${sign}${body}`;
}

export function formatPercent(value: number | null | undefined) {
  if (value === null || value === undefined) {
    return "n/a";
  }
  return `${value.toFixed(1)}%`;
}

export function formatSignedWholePercent(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return "\u2014";
  }
  return `${value > 0 ? "+" : ""}${Math.round(value)}%`;
}

export function formatPlanStretch(savingsPercent: number | null | undefined) {
  if (
    savingsPercent === null ||
    savingsPercent === undefined ||
    savingsPercent <= 0 ||
    savingsPercent >= 100
  ) {
    return "\u2014";
  }
  return `${(1 / (1 - savingsPercent / 100)).toFixed(1)}\u00d7`;
}

export function formatPlanResetDuration(totalMinutes: number) {
  const wholeMinutes = Math.max(0, Math.floor(totalMinutes));
  const minutes = wholeMinutes % 60;
  if (wholeMinutes < 60) {
    return `${String(minutes).padStart(2, "0")}m`;
  }

  const totalHours = Math.floor(wholeMinutes / 60);
  if (totalHours < 24) {
    return `${totalHours}h ${String(minutes).padStart(2, "0")}m`;
  }

  const days = Math.floor(totalHours / 24);
  const hours = totalHours % 24;
  return `${days}d ${String(hours).padStart(2, "0")}h ${String(minutes).padStart(2, "0")}m`;
}

export function formatHalvSavings(totals: SavingsTotals | null) {
  if (!totals) {
    return "loading local savings";
  }

  return `${formatCompactNumber(totals.tokensSaved)} tokens / ${formatUsd(
    totals.savingsUsd,
  )}`;
}

export function formatHalvSessionSavings(totals: SavingsTotals | null) {
  if (!totals) {
    return "loading local savings";
  }

  return `${formatCompactNumber(totals.sessionTokensSaved)} tokens / ${formatUsd(
    totals.sessionSavingsUsd,
  )}`;
}

export function formatRelativeTime(value: string) {
  const timestamp = Date.parse(value);
  if (Number.isNaN(timestamp)) {
    return "unknown";
  }

  const diffSeconds = Math.round((Date.now() - timestamp) / 1000);
  const absolute = Math.abs(diffSeconds);
  const units: Array<[string, number]> = [
    ["y", 60 * 60 * 24 * 365],
    ["mo", 60 * 60 * 24 * 30],
    ["w", 60 * 60 * 24 * 7],
    ["d", 60 * 60 * 24],
    ["h", 60 * 60],
    ["m", 60],
  ];

  if (absolute < 45) {
    return "just now";
  }

  for (const [unit, secondsInUnit] of units) {
    if (absolute >= secondsInUnit) {
      const value = Math.round(absolute / secondsInUnit);
      return diffSeconds >= 0 ? `${value}${unit} ago` : `in ${value}${unit}`;
    }
  }

  return "just now";
}

function trimOneDecimal(value: number) {
  return value.toFixed(1).replace(/\.0$/, "");
}

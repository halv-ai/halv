import type { AppPreferences, BriefSharing } from "./types.ts";

/**
 * The saved answer to the task brief question (ADR 0023). A missing key, a
 * damaged value, or a failed load reads `unset`, so Halv sends no brief.
 */
export function briefSharingAnswer(value: unknown): BriefSharing {
  return value === "allowed" || value === "declined" ? value : "unset";
}

/**
 * The Dot pin shows only when the user turned it on. A missing key, a missing
 * preferences object, or a failed load all read as hidden.
 */
export function dotShortcutVisible(
  preferences: Partial<Pick<AppPreferences, "showDotShortcut">> | null | undefined,
): boolean {
  return preferences?.showDotShortcut === true;
}

export type SubagentConcurrencyParseResult =
  | { valid: true; limit: number | null }
  | { valid: false };

export function parseSubagentConcurrencyLimit(
  value: string,
): SubagentConcurrencyParseResult {
  const trimmed = value.trim();
  if (trimmed.length === 0) {
    return { valid: true, limit: null };
  }

  if (!/^[1-9]\d*$/.test(trimmed)) {
    return { valid: false };
  }

  const limit = Number(trimmed);
  return Number.isSafeInteger(limit)
    ? { valid: true, limit }
    : { valid: false };
}

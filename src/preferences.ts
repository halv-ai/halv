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

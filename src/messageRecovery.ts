export function isProxyRecoveryError(
  normalizedError: string,
  usageLimitReached: boolean,
): boolean {
  return (
    !usageLimitReached &&
    (normalizedError.includes("headroom") ||
      normalizedError.includes("halv engine") ||
      normalizedError.includes("proxy")) &&
    (normalizedError.includes("timed out") ||
      normalizedError.includes("proxy") ||
      normalizedError.includes("exited"))
  );
}

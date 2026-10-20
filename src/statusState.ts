import type { StatusState } from "./types";

export function statusStateEqual(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (left === null || right === null) return false;
  if (typeof left !== "object" || typeof right !== "object") return false;

  if (Array.isArray(left) || Array.isArray(right)) {
    if (!Array.isArray(left) || !Array.isArray(right)) return false;
    if (left.length !== right.length) return false;
    for (let index = 0; index < left.length; index += 1) {
      if (!statusStateEqual(left[index], right[index])) return false;
    }
    return true;
  }

  const leftRecord = left as Record<string, unknown>;
  const rightRecord = right as Record<string, unknown>;
  const keys = new Set([...Object.keys(leftRecord), ...Object.keys(rightRecord)]);
  return [...keys].every((key) =>
    statusStateEqual(leftRecord[key], rightRecord[key]),
  );
}

export function mergeStatus(
  current: StatusState,
  next: StatusState,
): StatusState {
  const merged =
    current.account?.stale && next.account?.signedIn
      ? { ...next, account: current.account }
      : next;
  return statusStateEqual(current, merged) ? current : merged;
}

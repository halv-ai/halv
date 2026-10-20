export const REASONING_SUMMARY_STORAGE_KEY = "halv.reasoning-summary-open";

export type ReasoningSummaryStorage = {
  getItem: (key: string) => string | null;
  setItem: (key: string, value: string) => void;
};

export function readReasoningSummaryOpen(
  storage: ReasoningSummaryStorage | null,
) {
  if (!storage) {
    return true;
  }
  try {
    return storage.getItem(REASONING_SUMMARY_STORAGE_KEY) !== "false";
  } catch {
    return true;
  }
}

export function writeReasoningSummaryOpen(
  storage: ReasoningSummaryStorage | null,
  open: boolean,
) {
  if (!storage) {
    return;
  }
  try {
    storage.setItem(REASONING_SUMMARY_STORAGE_KEY, String(open));
  } catch {
    // Keep the local state when storage is unavailable.
  }
}

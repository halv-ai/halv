type LocalStorageSweep = Pick<Storage, "key" | "length" | "removeItem">;

export const HALV_STORAGE_PREFIX = "halv.";

export function clearHalvLocalStorage(
  storage: LocalStorageSweep | null = browserStorage(),
) {
  try {
    if (!storage) return;

    const matchingKeys: string[] = [];
    for (let index = 0; index < storage.length; index += 1) {
      const key = storage.key(index);
      if (key?.startsWith(HALV_STORAGE_PREFIX)) matchingKeys.push(key);
    }
    for (const key of matchingKeys) storage.removeItem(key);
  } catch {
    // Halv can continue when storage cleanup fails.
  }
}

function browserStorage(): LocalStorageSweep | null {
  if (typeof window === "undefined") return null;
  return window.localStorage;
}

// Feature flags for staged work that lands dark on main (ADR 0010). A flag
// is off by default until its stage ships. The Projects tree and the tabs
// ship, so projectTree, workspaceTabs and shellTabs are on. The Labs commands of the command palette turn a flag
// on or off. The value lives in the localStorage of this viewer only.

export const FEATURE_FLAGS_STORAGE_KEY = "halv.feature-flags.v1";

export const FEATURE_FLAG_NAMES = [
  "projectTree",
  "workspaceTabs",
  "shellTabs",
  "mobileRemote",
] as const;

export type FeatureFlagName = (typeof FEATURE_FLAG_NAMES)[number];
export type FeatureFlags = Readonly<Record<FeatureFlagName, boolean>>;

type FlagStorage = Pick<Storage, "getItem" | "setItem">;

export const DEFAULT_FEATURE_FLAGS: FeatureFlags = Object.freeze({
  projectTree: true,
  workspaceTabs: true,
  shellTabs: true,
  // The phone remote (ADR 0009) is on in development builds. It stays off in
  // releases until the phone app ships.
  mobileRemote: import.meta.env?.DEV === true,
});

export function isFeatureFlagName(value: string): value is FeatureFlagName {
  return (FEATURE_FLAG_NAMES as readonly string[]).includes(value);
}

function browserStorage(): FlagStorage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

// A corrupt value, a value of the wrong type, or a storage that throws
// gives the defaults. Unknown names are ignored.
export function readFeatureFlags(
  storage: FlagStorage | null = browserStorage(),
): FeatureFlags {
  if (!storage) return DEFAULT_FEATURE_FLAGS;
  try {
    const parsed: unknown = JSON.parse(
      storage.getItem(FEATURE_FLAGS_STORAGE_KEY) ?? "null",
    );
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return DEFAULT_FEATURE_FLAGS;
    }
    const flags: Record<FeatureFlagName, boolean> = { ...DEFAULT_FEATURE_FLAGS };
    for (const name of FEATURE_FLAG_NAMES) {
      const value = (parsed as Record<string, unknown>)[name];
      if (typeof value === "boolean") flags[name] = value;
    }
    return Object.freeze(flags);
  } catch {
    return DEFAULT_FEATURE_FLAGS;
  }
}

/** Writes all flags. Returns false when the storage refused the write. */
export function writeFeatureFlags(
  flags: FeatureFlags,
  storage: FlagStorage | null = browserStorage(),
) {
  if (!storage) return false;
  try {
    // Store only the flags that differ from the defaults. A flag that a
    // viewer turned off stays off when its default is on.
    const value: Partial<Record<FeatureFlagName, boolean>> = {};
    for (const name of FEATURE_FLAG_NAMES) {
      if (flags[name] !== DEFAULT_FEATURE_FLAGS[name]) value[name] = flags[name];
    }
    storage.setItem(FEATURE_FLAGS_STORAGE_KEY, JSON.stringify(value));
    return true;
  } catch {
    return false;
  }
}

/** Every flag off: the shape of the app before the staged work. */
export const NO_FEATURE_FLAGS: FeatureFlags = Object.freeze({
  projectTree: false,
  workspaceTabs: false,
  shellTabs: false,
  mobileRemote: false,
});

/**
 * Reads `?flags=projectTree,shellTabs`. The named flags are on and all
 * other flags are off. Unknown names are ignored.
 */
export function featureFlagsFromList(list: string): FeatureFlags {
  const flags: Record<FeatureFlagName, boolean> = { ...NO_FEATURE_FLAGS };
  for (const entry of list.split(",")) {
    const name = entry.trim();
    if (isFeatureFlagName(name)) flags[name] = true;
  }
  return Object.freeze(flags);
}

// One snapshot for the whole app, read with useSyncExternalStore. A
// change in another window arrives as a storage event.
export function createFeatureFlagStore(
  storage: () => FlagStorage | null = browserStorage,
) {
  const listeners = new Set<() => void>();
  let snapshot: FeatureFlags | null = null;
  const notify = () => {
    for (const listener of listeners) listener();
  };
  const onStorage = (event: StorageEvent) => {
    if (event.key !== null && event.key !== FEATURE_FLAGS_STORAGE_KEY) return;
    snapshot = readFeatureFlags(storage());
    notify();
  };
  const getSnapshot = (): FeatureFlags => {
    snapshot ??= readFeatureFlags(storage());
    return snapshot;
  };
  const subscribe = (listener: () => void) => {
    if (listeners.size === 0 && typeof window !== "undefined") {
      window.addEventListener("storage", onStorage);
    }
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0 && typeof window !== "undefined") {
        window.removeEventListener("storage", onStorage);
      }
    };
  };
  // The new value applies also when the storage refuses the write.
  const set = (name: FeatureFlagName, enabled: boolean): FeatureFlags => {
    const current = getSnapshot();
    if (current[name] === enabled) return current;
    const next: FeatureFlags = Object.freeze({ ...current, [name]: enabled });
    writeFeatureFlags(next, storage());
    snapshot = next;
    notify();
    return next;
  };
  const toggle = (name: FeatureFlagName) => set(name, !getSnapshot()[name]);
  return { getSnapshot, subscribe, set, toggle };
}

export const featureFlagStore = createFeatureFlagStore();

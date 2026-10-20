import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  DEFAULT_FEATURE_FLAGS,
  FEATURE_FLAGS_STORAGE_KEY,
  NO_FEATURE_FLAGS,
  createFeatureFlagStore,
  featureFlagsFromList,
  readFeatureFlags,
  writeFeatureFlags,
} from "../src/featureFlags.ts";

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    values,
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
}

const throwingStorage = {
  getItem(): string | null {
    throw new Error("storage is blocked");
  },
  setItem() {
    throw new Error("storage is blocked");
  },
};

test("the shipped projectTree and tab flags are on by default", () => {
  assert.deepEqual(readFeatureFlags(memoryStorage()), {
    projectTree: true,
    workspaceTabs: true,
    shellTabs: true,
    mobileRemote: false,
  });
  assert.deepEqual(readFeatureFlags(null), DEFAULT_FEATURE_FLAGS);
});

test("corrupt or wrongly typed values give the defaults", () => {
  for (const value of ["{", "[]", "true", "\"projectTree\"", "null"]) {
    assert.deepEqual(
      readFeatureFlags(memoryStorage({ [FEATURE_FLAGS_STORAGE_KEY]: value })),
      DEFAULT_FEATURE_FLAGS,
      value,
    );
  }
  assert.deepEqual(
    readFeatureFlags(
      memoryStorage({
        [FEATURE_FLAGS_STORAGE_KEY]: JSON.stringify({
          projectTree: "yes",
          shellTabs: false,
          unknownFlag: true,
        }),
      }),
    ),
    { projectTree: true, workspaceTabs: true, shellTabs: false, mobileRemote: false },
  );
});

test("a throwing storage gives the defaults and refuses writes", () => {
  assert.deepEqual(readFeatureFlags(throwingStorage), DEFAULT_FEATURE_FLAGS);
  assert.equal(
    writeFeatureFlags({ ...DEFAULT_FEATURE_FLAGS, projectTree: true }, throwingStorage),
    false,
  );
});

test("flags round-trip and only values that differ from the defaults are stored", () => {
  const storage = memoryStorage();
  assert.equal(
    writeFeatureFlags({ ...DEFAULT_FEATURE_FLAGS, workspaceTabs: false }, storage),
    true,
  );
  assert.equal(storage.values.get(FEATURE_FLAGS_STORAGE_KEY), "{\"workspaceTabs\":false}");
  assert.equal(readFeatureFlags(storage).workspaceTabs, false);
  assert.equal(readFeatureFlags(storage).shellTabs, true);
});

test("a viewer who turned the tabs on before 0.8.0 keeps them on", () => {
  const storage = memoryStorage({
    [FEATURE_FLAGS_STORAGE_KEY]: JSON.stringify({ workspaceTabs: true, shellTabs: true }),
  });
  assert.equal(readFeatureFlags(storage).workspaceTabs, true);
  assert.equal(readFeatureFlags(storage).shellTabs, true);
});

test("a flag list from the harness URL enables the named flags only", () => {
  assert.deepEqual(featureFlagsFromList("workspaceTabs"), {
    projectTree: false,
    workspaceTabs: true,
    shellTabs: false,
    mobileRemote: false,
  });
  assert.deepEqual(featureFlagsFromList("projectTree, shellTabs,bogus,"), {
    projectTree: true,
    workspaceTabs: false,
    shellTabs: true,
    mobileRemote: false,
  });
  assert.equal(featureFlagsFromList("mobileRemote").mobileRemote, true);
  assert.deepEqual(featureFlagsFromList(""), NO_FEATURE_FLAGS);
});

test("the store toggles a flag, persists it, and notifies subscribers", () => {
  const storage = memoryStorage();
  const store = createFeatureFlagStore(() => storage);
  let calls = 0;
  const unsubscribe = store.subscribe(() => {
    calls += 1;
  });
  const before = store.getSnapshot();
  assert.equal(before.projectTree, true);
  assert.equal(store.getSnapshot(), before, "the snapshot is stable");

  // A flag that is on by default stays off after the viewer turns it off.
  store.toggle("projectTree");
  assert.equal(store.getSnapshot().projectTree, false);
  assert.equal(storage.values.get(FEATURE_FLAGS_STORAGE_KEY), "{\"projectTree\":false}");
  assert.equal(readFeatureFlags(storage).projectTree, false);
  assert.equal(calls, 1);

  store.set("projectTree", false);
  assert.equal(calls, 1, "an unchanged value does not notify");
  unsubscribe();
  store.toggle("projectTree");
  assert.equal(calls, 1);
  assert.equal(store.getSnapshot().projectTree, true);
});

test("the store keeps a toggled value when the storage throws", () => {
  const store = createFeatureFlagStore(() => throwingStorage);
  store.toggle("shellTabs");
  assert.equal(store.getSnapshot().shellTabs, false);
});

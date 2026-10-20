import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { setIpcTransport } from "../src/ipc.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  APP_UPDATE_BEFORE_RESTART_EVENT,
  APP_UPDATE_SNAPSHOT_EVENT,
  AppUpdateStore,
} from "../src/appUpdateStore.ts";
import type { AppUpdateSnapshot, AppUpdateStatus } from "../src/types.ts";

type Invoke = (name: string, args?: Record<string, unknown>) => Promise<unknown>;

function snapshot(status: AppUpdateStatus, change: Partial<AppUpdateSnapshot> = {}) {
  return {
    status,
    currentVersion: "0.7.1",
    autoDownload: true,
    installMode: "replace",
    deferredVersion: null,
    restartWhenIdle: false,
    blockers: null,
    postUpdate: null,
    lastCheckedAt: null,
    sessionsSurviveRestart: true,
    ...change,
  } satisfies AppUpdateSnapshot;
}

const READY: AppUpdateStatus = {
  state: "ready",
  version: "0.7.2",
  notes: null,
  releaseUrl: "https://github.com/pedr0v/halv-releases/releases/tag/v0.7.2",
};

function installTransport(invoke: Invoke) {
  const handlers = new Map<string, (payload: unknown) => void>();
  setIpcTransport({
    invoke: invoke as never,
    listen: async (event: string, handler: (payload: unknown) => void) => {
      handlers.set(event, handler);
      return () => handlers.delete(event);
    },
  } as never);
  return {
    emit(event: string, payload: unknown) {
      const handler = handlers.get(event);
      assert.ok(handler, `no listener for ${event}`);
      handler(payload);
    },
  };
}

test("an event that arrives during the first load wins over the older load result", async () => {
  let releaseLoad: (value: AppUpdateSnapshot) => void = () => undefined;
  const transport = installTransport(async (name) => {
    if (name === "app_update_snapshot") {
      return new Promise<AppUpdateSnapshot>((resolve) => {
        releaseLoad = resolve;
      });
    }
    throw new Error(`unexpected command ${name}`);
  });
  const store = new AppUpdateStore();
  const started = store.ensureStarted();
  await new Promise((resolve) => setImmediate(resolve));

  transport.emit(APP_UPDATE_SNAPSHOT_EVENT, snapshot(READY));
  releaseLoad(snapshot({ state: "idle" }));
  await started;

  assert.equal(store.getSnapshot().snapshot?.status.state, "ready");
});

test("the first load commits when no event arrived", async () => {
  installTransport(async (name) => {
    if (name === "app_update_snapshot") return snapshot(READY);
    throw new Error(`unexpected command ${name}`);
  });
  const store = new AppUpdateStore();
  await store.ensureStarted();
  assert.equal(store.getSnapshot().snapshot?.status.state, "ready");
  // The listeners start once.
  assert.equal(store.ensureStarted(), store.ensureStarted());
});

test("a duplicate snapshot does not notify the views", async () => {
  const transport = installTransport(async () => snapshot({ state: "idle" }));
  const store = new AppUpdateStore();
  await store.ensureStarted();
  let notifications = 0;
  store.subscribe(() => {
    notifications += 1;
  });

  const downloading = snapshot({
    state: "downloading",
    version: "0.7.2",
    downloadedBytes: 10,
    totalBytes: 100,
  });
  transport.emit(APP_UPDATE_SNAPSHOT_EVENT, downloading);
  transport.emit(APP_UPDATE_SNAPSHOT_EVENT, structuredClone(downloading));
  assert.equal(notifications, 1);
});

test("before a restart the store runs the saves and then acknowledges", async () => {
  const calls: Array<[string, unknown]> = [];
  const transport = installTransport(async (name, args) => {
    calls.push([name, args]);
    return name === "app_update_snapshot" ? snapshot(READY) : null;
  });
  const store = new AppUpdateStore();
  await store.ensureStarted();
  const order: string[] = [];
  store.registerFlush(async () => {
    await new Promise((resolve) => setTimeout(resolve, 5));
    order.push("flushed");
  });
  const unregister = store.registerFlush(() => order.push("removed"));
  unregister();

  transport.emit(APP_UPDATE_BEFORE_RESTART_EVENT, { requestId: 7 });
  await new Promise((resolve) => setTimeout(resolve, 30));

  assert.deepEqual(order, ["flushed"]);
  assert.deepEqual(calls.at(-1), ["app_update_restart_ack", { requestId: 7 }]);
});

test("a save that never ends still lets the restart continue", async () => {
  const calls: string[] = [];
  installTransport(async (name) => {
    calls.push(name);
    return null;
  });
  const store = new AppUpdateStore();
  store.registerFlush(() => new Promise(() => undefined));
  store.registerFlush(() => {
    throw new Error("save failed");
  });
  const started = Date.now();
  await store.acknowledgeRestart(3);
  assert.ok(Date.now() - started < 1_000);
  assert.deepEqual(calls, ["app_update_restart_ack"]);
});

test("actions record a rejection and clear the pending state", async () => {
  installTransport(async (name) => {
    if (name === "app_update_snapshot") return snapshot(READY);
    if (name === "app_update_restart") {
      throw "No downloaded Halv update is ready to install.";
    }
    if (name === "app_update_check") return snapshot(READY);
    throw new Error(`unexpected command ${name}`);
  });
  const store = new AppUpdateStore();
  await store.ensureStarted();

  const restart = store.restart();
  assert.equal(store.getSnapshot().pending, "restart");
  assert.equal(await restart, null);
  assert.equal(store.getSnapshot().pending, null);
  assert.equal(
    store.getSnapshot().actionError,
    "No downloaded Halv update is ready to install.",
  );

  await store.check();
  assert.equal(store.getSnapshot().actionError, null);
});

test("restart returns the blockers so the card can show them", async () => {
  const blockers = { chatReplies: 1, guiAgentSessions: 0, inProcessTerminals: 0 };
  const args: unknown[] = [];
  installTransport(async (name, value) => {
    if (name === "app_update_restart") {
      args.push(value);
      return blockers;
    }
    return snapshot(READY);
  });
  const store = new AppUpdateStore();
  assert.deepEqual(await store.restart(), blockers);
  await store.restart(true);
  assert.deepEqual(args, [{ force: false }, { force: true }]);
});

test("the segment toggles the card, and a user check or Later resets the view", async () => {
  const commands: Array<[string, unknown]> = [];
  const transport = installTransport(async (name, args) => {
    commands.push([name, args]);
    return snapshot(READY);
  });
  const store = new AppUpdateStore();
  await store.ensureStarted();
  assert.equal(store.isCardVisible(), true);

  store.toggleCard();
  assert.equal(store.isCardVisible(), false);
  assert.deepEqual(store.getSnapshot().ui, {
    minimizedKey: "ready:0.7.2",
    revealedKey: null,
  });
  store.toggleCard();
  assert.equal(store.isCardVisible(), true);

  // "Later" hides the version, and the segment can still reveal it.
  await store.later("0.7.2");
  assert.deepEqual(commands.at(-1), ["app_update_defer", { version: "0.7.2" }]);
  transport.emit(APP_UPDATE_SNAPSHOT_EVENT, snapshot(READY, { deferredVersion: "0.7.2" }));
  assert.equal(store.isCardVisible(), false);
  store.toggleCard();
  assert.equal(store.isCardVisible(), true);

  store.minimize();
  await store.check();
  assert.deepEqual(commands.at(-1), ["app_update_check", { userInitiated: true }]);
  assert.deepEqual(store.getSnapshot().ui, { minimizedKey: null, revealedKey: null });
});

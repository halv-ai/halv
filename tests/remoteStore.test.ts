import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  setIpcTransport,
  type RemotePairing,
  type RemotePairingRequest,
  type RemoteStatus,
} from "../src/ipc.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  RemoteStore,
  formatConfirmationCode,
  formatCountdown,
  formatTimeAgo,
  remotePlatformLabel,
} from "../src/remoteStore.ts";

const PAIRING: RemotePairing = {
  uri: "halv://pair?p=test",
  qrSvgDataUri: "data:image/svg+xml,",
  expiresAt: 5,
};

const REQUEST: RemotePairingRequest = {
  requestId: "request-1",
  name: "Test iPhone",
  platform: "ios",
  model: "iPhone 17",
  code: "028809",
  expiresAt: 2_000_000,
};

function status(overrides: Partial<RemoteStatus> = {}): RemoteStatus {
  return {
    enabled: true,
    relayUrl: "wss://relay.halv.ai",
    relayUrlIsDefault: true,
    connection: "connected",
    error: null,
    hostId: "host-1",
    hostName: "Test Mac",
    devices: [],
    pairing: null,
    pendingApproval: null,
    ...overrides,
  };
}

type Invoke = (name: string, args?: Record<string, unknown>) => Promise<unknown>;
type Handlers = Map<string, (payload: unknown) => void>;

function installTransport(invoke: Invoke): Handlers {
  const handlers: Handlers = new Map();
  setIpcTransport({
    invoke: invoke as never,
    listen: async (event: string, handler: (payload: unknown) => void) => {
      handlers.set(event, handler);
      return () => handlers.delete(event);
    },
  } as never);
  return handlers;
}

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

test("the confirmation code shows as two groups of three digits", () => {
  assert.equal(formatConfirmationCode("028809"), "028 809");
  assert.equal(formatConfirmationCode("000000"), "000 000");
  assert.equal(formatConfirmationCode("12345"), "12345");
});

test("the countdown rounds up and stops at zero", () => {
  assert.equal(formatCountdown(300_000, 0), "5:00");
  assert.equal(formatCountdown(299_001, 0), "5:00");
  assert.equal(formatCountdown(59_000, 0), "0:59");
  assert.equal(formatCountdown(1_000, 5_000), "0:00");
});

test("the platform label names the platform and the model", () => {
  assert.equal(remotePlatformLabel("ios", "iPhone 17"), "iOS · iPhone 17");
  assert.equal(remotePlatformLabel("android", null), "Android");
  assert.equal(remotePlatformLabel("web", "  "), "web");
});

test("last seen uses the largest whole unit", () => {
  const now = 10 * 24 * 60 * 60_000;
  assert.equal(formatTimeAgo(now - 3 * 60 * 60_000, now, "en-US"), "3 hours ago");
  assert.equal(formatTimeAgo(now - 5 * 60_000, now, "en-US"), "5 minutes ago");
  assert.equal(formatTimeAgo(now, now, "en-US"), "now");
});

test("the pairing request event opens the approval and a status closes it", async () => {
  const handlers = installTransport(async () => {
    throw new Error("no command expected");
  });
  const store = new RemoteStore();
  store.listen();
  await settle();

  handlers.get("remote-pairing-request")?.(REQUEST);
  assert.deepEqual(store.getSnapshot().approval, REQUEST);

  handlers.get("remote-changed")?.(status({ pendingApproval: REQUEST }));
  assert.deepEqual(store.getSnapshot().approval, REQUEST);

  handlers.get("remote-changed")?.(status());
  assert.equal(store.getSnapshot().approval, null);
});

test("a slow status load never replaces a newer event", async () => {
  let release: (value: RemoteStatus) => void = () => undefined;
  const gate = new Promise<RemoteStatus>((resolve) => {
    release = resolve;
  });
  let reads = 0;
  const handlers = installTransport(async (name) => {
    if (name === "remote_status") {
      reads += 1;
      return reads === 1 ? gate : status({ pendingApproval: REQUEST });
    }
    throw new Error(`unexpected command ${name}`);
  });
  const store = new RemoteStore();
  store.ensureLoaded();
  await settle();

  handlers.get("remote-changed")?.(status({ pendingApproval: REQUEST }));
  release(status({ connection: "connecting" }));
  await settle();

  const snapshot = store.getSnapshot();
  assert.equal(snapshot.status?.connection, "connected");
  assert.deepEqual(snapshot.approval, REQUEST);
  assert.equal(reads, 2, "the event reads the full status");
});

test("an event keeps the known offer until the full status arrives", async () => {
  let full = status({ pairing: PAIRING });
  const handlers = installTransport(async (name) => {
    if (name === "remote_status") return full;
    if (name === "remote_pairing_start") return PAIRING;
    throw new Error(`unexpected command ${name}`);
  });
  const store = new RemoteStore();
  store.ensureLoaded();
  await settle();
  await store.startPairing();
  assert.deepEqual(store.getSnapshot().status?.pairing, PAIRING);

  // The event never holds the offer. The store keeps it and reads the full status.
  handlers.get("remote-changed")?.(status({ connection: "connecting" }));
  assert.deepEqual(store.getSnapshot().status?.pairing, PAIRING);
  assert.equal(store.getSnapshot().status?.connection, "connecting");
  await settle();
  assert.deepEqual(store.getSnapshot().status?.pairing, PAIRING);
  assert.equal(store.getSnapshot().status?.connection, "connected");

  // The full status removes an offer that ended.
  full = status();
  handlers.get("remote-changed")?.(status());
  await settle();
  assert.equal(store.getSnapshot().status?.pairing, null);
});

test("the pairing result shows the offer after an event during the call", async () => {
  let finishStart: (value: RemotePairing) => void = () => undefined;
  const handlers = installTransport(async (name) => {
    if (name === "remote_status") throw new Error("Status test error");
    if (name === "remote_pairing_start") {
      return new Promise<RemotePairing>((resolve) => {
        finishStart = resolve;
      });
    }
    throw new Error(`unexpected command ${name}`);
  });
  const store = new RemoteStore();
  store.commit(status());
  store.listen();
  await settle();
  const started = store.startPairing();
  await settle();
  handlers.get("remote-changed")?.(status());
  await settle();
  finishStart(PAIRING);
  await started;
  assert.deepEqual(store.getSnapshot().status?.pairing, PAIRING);
});

test("a command result replaces the status and a failed load keeps the error", async () => {
  installTransport(async (name, args) => {
    if (name === "remote_status") throw new Error("Status test error");
    if (name === "remote_set_enabled") return status({ enabled: Boolean(args?.enabled) });
    if (name === "remote_pairing_start") return PAIRING;
    throw new Error(`unexpected command ${name}`);
  });
  const store = new RemoteStore();
  await assert.rejects(store.refresh(), /Status test error/);
  assert.equal(store.getSnapshot().loadError, "Status test error");

  await store.setEnabled(true);
  assert.equal(store.getSnapshot().status?.enabled, true);
  assert.equal(store.getSnapshot().loadError, null);

  await store.startPairing();
  assert.equal(store.getSnapshot().status?.pairing?.expiresAt, 5);
});

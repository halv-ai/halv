import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { setIpcTransport } from "../src/ipc.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { readFocusRefresh, readModelInputs } from "../src/focusRefresh.ts";

type Invoke = (name: string, args?: Record<string, unknown>) => Promise<unknown>;

const laneOptions = [{ agentKind: "claude", lanes: [] }];
const probe = { claude: { found: true, version: "2.1.300" } };

const responses: Record<string, unknown> = {
  agent_session_lane_options: laneOptions,
  agent_session_probe: probe,
  auth_anthropic_status: { configured: false },
  auth_credential_status: { signedIn: false },
  auth_status: { signedIn: false, freeTurnsUsed: 0 },
  chat_list_models: [],
  sidecar_stats: { requests: 0 },
  sidecar_status: { state: "running" },
  storage_list_credential_profiles: [],
};

function installCountingTransport(): Map<string, number> {
  const calls = new Map<string, number>();
  const invoke: Invoke = async (name) => {
    calls.set(name, (calls.get(name) ?? 0) + 1);
    if (!(name in responses)) throw new Error(`unexpected command ${name}`);
    return structuredClone(responses[name]);
  };
  setIpcTransport({
    invoke: invoke as never,
    listen: async () => () => undefined,
  } as never);
  return calls;
}

test("the focus refresh runs each command once", async () => {
  const calls = installCountingTransport();

  const read = readFocusRefresh();
  await Promise.all([read.agentAccounts, read.status, read.modelInputs]);

  // Before, one focus ran the lane options, the CLI probes, the credential
  // status and the credential profiles twice.
  assert.deepEqual(Object.fromEntries([...calls].sort()), {
    agent_session_lane_options: 1,
    agent_session_probe: 1,
    auth_anthropic_status: 1,
    auth_credential_status: 1,
    auth_status: 1,
    chat_list_models: 1,
    sidecar_stats: 1,
    sidecar_status: 1,
    storage_list_credential_profiles: 1,
  });
});

test("the focus refresh readers share the same results", async () => {
  installCountingTransport();

  const read = readFocusRefresh();
  const [accounts, status, models] = await Promise.all([
    read.agentAccounts,
    read.status,
    read.modelInputs,
  ]);

  assert.deepEqual(accounts.laneOptions, {
    status: "fulfilled",
    value: laneOptions,
  });
  assert.equal(models.laneOptions, (accounts.laneOptions as { value: unknown }).value);
  assert.equal(models.probe, (accounts.probe as { value: unknown }).value);
  assert.equal(models.auth, status.auth);
  assert.deepEqual(status.sidecarStats, { requests: 0 });
});

test("a reader without a shared read starts its own commands", async () => {
  const calls = installCountingTransport();

  const models = await readModelInputs();

  assert.deepEqual(models.laneOptions, laneOptions);
  assert.equal(calls.get("agent_session_lane_options"), 1);
  assert.equal(calls.get("auth_credential_status"), 1);
  assert.equal(calls.get("auth_status"), undefined);
});

test("a failed probe fails the model list but not the gate accounts", async () => {
  installCountingTransport();
  setIpcTransport({
    invoke: (async (name: string) => {
      if (name === "agent_session_probe") throw new Error("probe failed");
      return structuredClone(responses[name]);
    }) as never,
    listen: async () => () => undefined,
  } as never);

  const read = readFocusRefresh();
  const accounts = await read.agentAccounts;

  assert.equal(accounts.probe.status, "rejected");
  assert.equal(accounts.laneOptions.status, "fulfilled");
  await assert.rejects(read.modelInputs, /probe failed/);
  await read.status;
});

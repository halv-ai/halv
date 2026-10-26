import assert from "node:assert/strict";
import test from "node:test";

import {
  BRIEF_SHARING_LEARN_MORE_URL,
  BRIEF_SHARING_NEEDED_EVENT,
  BriefSharingGate,
  UI_MOMENT_WINDOW_MS,
  briefSharingAnswer,
  briefSharingAsk,
  launchCanRoute,
  type BriefSharingAskInput,
} from "../src/briefSharing.ts";
import {
  AGENT_LAUNCH_COMMANDS,
  createIpc,
  setAgentLaunchGate,
  type Transport,
} from "../src/ipc.ts";
import type { BriefSharing } from "../src/types.ts";

const ask = (overrides: Partial<BriefSharingAskInput> = {}) =>
  briefSharingAsk({
    taskRouting: true,
    briefSharing: "unset",
    access: { granted: true },
    launchCanRoute: true,
    uiMoment: true,
    dismissed: false,
    ...overrides,
  });

test("a saved value that is not an answer reads unset", () => {
  assert.equal(briefSharingAnswer("allowed"), "allowed");
  assert.equal(briefSharingAnswer("declined"), "declined");
  for (const value of ["unset", undefined, null, "yes", true, 1]) {
    assert.equal(briefSharingAnswer(value), "unset");
  }
  assert.equal(
    BRIEF_SHARING_LEARN_MORE_URL,
    "https://github.com/halv-ai/halv#task-briefs",
  );
});

test("only Claude, Codex, and duplicated panes can route", () => {
  assert.equal(launchCanRoute("agent_terminal_create", { agentKind: "claude" }), true);
  assert.equal(launchCanRoute("agent_ghostty_launch", { agentKind: "codex" }), true);
  assert.equal(launchCanRoute("agent_terminal_create", { agentKind: "google" }), false);
  assert.equal(launchCanRoute("agent_terminal_create", { agentKind: "kimi" }), false);
  assert.equal(launchCanRoute("agent_terminal_create", undefined), false);
  assert.equal(launchCanRoute("agent_terminal_duplicate", { sessionId: "s" }), true);
});

test("a launch that the user started gets the prompt, any other launch the card", () => {
  assert.equal(ask(), "prompt");
  assert.equal(ask({ uiMoment: false }), "card");
});

test("Halv never asks without routing, after an answer, or without Halv Cloud", () => {
  assert.equal(ask({ taskRouting: false }), "none");
  assert.equal(ask({ briefSharing: "allowed" }), "none");
  assert.equal(ask({ briefSharing: "declined" }), "none");
  // Signed out or no plan: no Halv Cloud access.
  assert.equal(ask({ access: { granted: false } }), "none");
  assert.equal(ask({ access: { granted: false }, uiMoment: false }), "none");
  assert.equal(ask({ launchCanRoute: false }), "none");
  assert.equal(ask({ dismissed: true }), "none");
});

type Harness = {
  gate: BriefSharingGate;
  saved: Array<Exclude<BriefSharing, "unset">>;
  setNow: (value: number) => void;
  setPreferences: (value: { taskRouting: boolean; briefSharing: BriefSharing }) => void;
  failNextSave: (message: string) => void;
};

function harness(): Harness {
  let now = 1_000_000;
  let preferences: { taskRouting: boolean; briefSharing: BriefSharing } = {
    taskRouting: true,
    briefSharing: "unset",
  };
  let failure: string | null = null;
  const saved: Array<Exclude<BriefSharing, "unset">> = [];
  const gate = new BriefSharingGate({
    loadPreferences: async () => ({ ...preferences }),
    save: async (answer) => {
      if (failure) {
        const message = failure;
        failure = null;
        throw new Error(message);
      }
      saved.push(answer);
      preferences = { ...preferences, briefSharing: answer };
      return { briefSharing: answer };
    },
    now: () => now,
  });
  // The app mounts the prompt view.
  gate.attachHost();
  return {
    gate,
    saved,
    setNow: (value) => {
      now = value;
    },
    setPreferences: (value) => {
      preferences = value;
    },
    failNextSave: (message) => {
      failure = message;
    },
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

async function settled(promise: Promise<unknown>) {
  let done = false;
  void promise.then(() => {
    done = true;
  });
  await flush();
  return done;
}

test("the prompt holds a launch until the user allows briefs", async () => {
  const { gate, saved } = harness();
  gate.setAccess({ granted: true });
  gate.noteUserInput();
  const launch = gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" });
  assert.equal(await settled(launch), false);
  assert.equal(gate.getSnapshot().surface, "prompt");

  // A second launch waits for the same answer.
  const second = gate.beforeLaunch("agent_terminal_create", { agentKind: "codex" });
  assert.equal(await settled(second), false);

  await gate.answer("allowed");
  await launch;
  await second;
  assert.deepEqual(saved, ["allowed"]);
  assert.equal(gate.getSnapshot().surface, "none");

  // After the answer, a launch starts at once.
  gate.noteUserInput();
  assert.equal(
    await settled(gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" })),
    true,
  );
  assert.equal(gate.getSnapshot().surface, "none");
});

test("don't send saves a no and lets the launch start", async () => {
  const { gate, saved } = harness();
  gate.setAccess({ granted: true });
  gate.noteUserInput();
  const launch = gate.beforeLaunch("agent_ghostty_launch", { agentKind: "codex" });
  await flush();
  await gate.answer("declined");
  await launch;
  assert.deepEqual(saved, ["declined"]);
  assert.equal(gate.getSnapshot().surface, "none");
});

test("a failed save keeps the prompt open with the error", async () => {
  const { gate, saved, failNextSave } = harness();
  gate.setAccess({ granted: true });
  gate.noteUserInput();
  const launch = gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" });
  await flush();
  failNextSave("disk full");
  await gate.answer("allowed");
  assert.equal(gate.getSnapshot().surface, "prompt");
  assert.equal(gate.getSnapshot().error, "disk full");
  assert.equal(gate.getSnapshot().pending, false);
  assert.equal(await settled(launch), false);
  await gate.answer("allowed");
  await launch;
  assert.deepEqual(saved, ["allowed"]);
});

test("closing the prompt starts the launch without an answer and stops asking", async () => {
  const { gate, saved } = harness();
  gate.setAccess({ granted: true });
  gate.noteUserInput();
  const launch = gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" });
  await flush();
  gate.dismiss();
  await launch;
  assert.deepEqual(saved, []);
  gate.noteUserInput();
  assert.equal(
    await settled(gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" })),
    true,
  );
  assert.equal(gate.getSnapshot().surface, "none");
});

test("a launch without a user action starts at once and shows the card", async () => {
  const { gate, setNow } = harness();
  gate.setAccess({ granted: true });
  gate.noteUserInput(0);
  setNow(UI_MOMENT_WINDOW_MS + 1);
  assert.equal(
    await settled(gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" })),
    true,
  );
  assert.equal(gate.getSnapshot().surface, "card");
  // A launch that the user starts while the card shows gets the prompt.
  gate.noteUserInput();
  const launch = gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" });
  assert.equal(await settled(launch), false);
  assert.equal(gate.getSnapshot().surface, "prompt");
  await gate.answer("allowed");
  await launch;
});

test("a Rust launch without an answer shows the card and never waits", async () => {
  assert.equal(BRIEF_SHARING_NEEDED_EVENT, "brief-sharing-needed");
  const { gate, saved } = harness();
  gate.setAccess({ granted: true });
  // A click just before the event does not turn the card into the prompt.
  gate.noteUserInput();
  assert.equal(await settled(gate.launchedWithoutAnswer()), true);
  assert.equal(gate.getSnapshot().surface, "card");
  await gate.answer("declined");
  assert.deepEqual(saved, ["declined"]);
  assert.equal(gate.getSnapshot().surface, "none");
  // After an answer, the event shows nothing.
  await gate.launchedWithoutAnswer();
  assert.equal(gate.getSnapshot().surface, "none");

  // The event after the IPC gate asked keeps the prompt.
  const second = harness();
  second.gate.setAccess({ granted: true });
  second.gate.noteUserInput();
  const launch = second.gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" });
  assert.equal(await settled(launch), false);
  await second.gate.launchedWithoutAnswer();
  assert.equal(second.gate.getSnapshot().surface, "prompt");
  second.gate.dismiss();
  await launch;
  // After a close, the event does not ask again in this run.
  await second.gate.launchedWithoutAnswer();
  assert.equal(second.gate.getSnapshot().surface, "none");
});

test("a Rust launch asks only an account with Halv Cloud", async () => {
  const signedOut = harness();
  signedOut.gate.setAccess({ granted: false });
  await signedOut.gate.launchedWithoutAnswer();
  assert.equal(signedOut.gate.getSnapshot().surface, "none");

  // Before the account status loads, the card waits for the status.
  for (const granted of [true, false]) {
    const { gate } = harness();
    await gate.launchedWithoutAnswer();
    assert.equal(gate.getSnapshot().surface, "none");
    gate.setAccess({ granted });
    assert.equal(gate.getSnapshot().surface, granted ? "card" : "none");
  }

  const routingOff = harness();
  routingOff.gate.setAccess({ granted: true });
  routingOff.setPreferences({ taskRouting: false, briefSharing: "unset" });
  await routingOff.gate.launchedWithoutAnswer();
  assert.equal(routingOff.gate.getSnapshot().surface, "none");
});

test("a launch before the account status loads shows the card only with Halv Cloud", async () => {
  for (const granted of [true, false]) {
    const { gate } = harness();
    gate.noteUserInput();
    assert.equal(gate.accessKnown(), false);
    assert.equal(
      await settled(gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" })),
      true,
    );
    assert.equal(gate.getSnapshot().surface, "none");
    gate.setAccess({ granted });
    assert.equal(gate.accessKnown(), true);
    assert.equal(gate.getSnapshot().surface, granted ? "card" : "none");
  }
});

test("a signed-out install or an account without Halv Cloud is never asked", async () => {
  const { gate } = harness();
  gate.setAccess({ granted: false });
  gate.noteUserInput();
  assert.equal(
    await settled(gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" })),
    true,
  );
  assert.equal(gate.getSnapshot().surface, "none");
});

test("losing Halv Cloud or an answer in Settings closes the question", async () => {
  const first = harness();
  first.gate.setAccess({ granted: true });
  first.gate.noteUserInput();
  const launch = first.gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" });
  await flush();
  first.gate.setAccess({ granted: false });
  await launch;
  assert.equal(first.gate.getSnapshot().surface, "none");

  const second = harness();
  second.gate.setAccess({ granted: true });
  second.gate.noteUserInput();
  const held = second.gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" });
  await flush();
  second.gate.applyPreferences({ briefSharing: "unset" });
  assert.equal(await settled(held), false);
  second.gate.applyPreferences({ briefSharing: "declined" });
  await held;
  assert.equal(second.gate.getSnapshot().surface, "none");
});

test("routing off, an old answer, or a pane without routing passes the gate", async () => {
  const { gate, setPreferences } = harness();
  gate.setAccess({ granted: true });
  gate.noteUserInput();
  assert.equal(
    await settled(gate.beforeLaunch("agent_terminal_create", { agentKind: "google" })),
    true,
  );
  setPreferences({ taskRouting: false, briefSharing: "unset" });
  assert.equal(
    await settled(gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" })),
    true,
  );
  setPreferences({ taskRouting: true, briefSharing: "declined" });
  assert.equal(
    await settled(gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" })),
    true,
  );
  assert.equal(gate.getSnapshot().surface, "none");
});

function gateWithoutView() {
  const gate = new BriefSharingGate({
    loadPreferences: async () => ({ taskRouting: true, briefSharing: "unset" }),
    save: async (answer) => ({ briefSharing: answer }),
    now: () => 0,
  });
  gate.setAccess({ granted: true });
  gate.noteUserInput(0);
  return gate;
}

test("without a mounted view, a launch gets the card and never waits", async () => {
  const gate = gateWithoutView();
  assert.equal(
    await settled(gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" })),
    true,
  );
  assert.equal(gate.getSnapshot().surface, "card");
});

test("the last view to unmount turns the prompt into the card", async () => {
  const gate = gateWithoutView();
  const detachFirst = gate.attachHost();
  const detachSecond = gate.attachHost();
  const launch = gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" });
  assert.equal(await settled(launch), false);
  assert.equal(gate.getSnapshot().surface, "prompt");
  detachSecond();
  // A second call of the same detach does nothing.
  detachSecond();
  assert.equal(await settled(launch), false, "one view still shows the prompt");
  detachFirst();
  await launch;
  assert.equal(gate.getSnapshot().surface, "card");
});

test("a preferences read failure lets the launch start", async () => {
  const gate = new BriefSharingGate({
    loadPreferences: async () => {
      throw new Error("offline");
    },
    save: async (answer) => ({ briefSharing: answer }),
    now: () => 0,
  });
  gate.setAccess({ granted: true });
  gate.noteUserInput(0);
  await gate.beforeLaunch("agent_terminal_create", { agentKind: "claude" });
  assert.equal(gate.getSnapshot().surface, "none");
});

test("the IPC layer runs the gate before each agent launch command only", async () => {
  const calls: string[] = [];
  const transport: Transport = {
    invoke: (async (name: string) => {
      calls.push(`invoke:${name}`);
      return undefined;
    }) as Transport["invoke"],
    listen: async () => () => undefined,
  };
  const ipc = createIpc(transport);
  let release: () => void = () => undefined;
  setAgentLaunchGate(async (name) => {
    calls.push(`gate:${name}`);
    await new Promise<void>((resolve) => {
      release = resolve;
    });
  });
  try {
    const launch = ipc.agent.terminalCreate({
      agentKind: "claude",
      lane: { kind: "claudeSubscription" },
      cwd: "/tmp",
      cols: 80,
      rows: 24,
    } as never);
    await flush();
    assert.deepEqual(calls, ["gate:agent_terminal_create"]);
    release();
    await launch;
    assert.deepEqual(calls, ["gate:agent_terminal_create", "invoke:agent_terminal_create"]);

    calls.length = 0;
    await ipc.preferences.get();
    assert.deepEqual(calls, ["invoke:preferences_get"]);

    // A failing gate never blocks the launch.
    setAgentLaunchGate(async () => {
      throw new Error("broken gate");
    });
    calls.length = 0;
    await ipc.agent.terminalDuplicate({ sessionId: "s", cols: 80, rows: 24 } as never);
    assert.deepEqual(calls, ["invoke:agent_terminal_duplicate"]);
  } finally {
    setAgentLaunchGate(null);
  }
  assert.deepEqual([...AGENT_LAUNCH_COMMANDS].sort(), [
    "agent_ghostty_launch",
    "agent_terminal_create",
    "agent_terminal_duplicate",
  ]);
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  liveSavingsMeasurementState,
  sessionSavingsMeasurementNote,
  sessionSavingsUnmeasured,
  type AgentSession,
} from "../src/agentSessions.ts";

function liveSession(
  overrides: Partial<AgentSession> = {},
): AgentSession {
  return {
    id: "session-1",
    agentKind: "claude",
    laneKind: "claudeSubscription",
    mode: "terminal_embedded",
    cwd: "/tmp/project",
    pid: 1,
    status: "running",
    createdAt: "2026-08-24T12:00:00Z",
    lastActiveAt: "2026-08-24T12:00:00Z",
    savingsSources: { rtk: "missing", crux: "missing", proxy: false },
    ...overrides,
  };
}

test("subscription sessions explain their measurement scope", () => {
  assert.equal(
    sessionSavingsMeasurementNote("claudeSubscription"),
    "crux + RTK only · proxy off",
  );
  assert.equal(
    sessionSavingsMeasurementNote("kimiSubscription"),
    "crux + RTK (by instruction) · proxy on",
  );
});

test("proxied lanes keep the default meter heading", () => {
  assert.equal(sessionSavingsMeasurementNote("codexChatGpt"), null);
  assert.equal(sessionSavingsMeasurementNote("claudeApiKey"), null);
  assert.equal(sessionSavingsMeasurementNote("custom"), null);
});

test("an unproxied Claude subscription with only Crux is estimated", () => {
  const sources = { rtk: "missing", crux: "present", proxy: false } as const;
  assert.equal(sessionSavingsUnmeasured("claudeSubscription", sources), false);
  assert.equal(
    sessionSavingsMeasurementNote("claudeSubscription", sources),
    "crux only · proxy off",
  );
  assert.equal(
    sessionSavingsUnmeasured("claudeSubscription", {
      ...sources,
      rtk: "present",
    }),
    false,
  );
  assert.equal(
    sessionSavingsMeasurementNote("claudeSubscription", {
      ...sources,
      rtk: "present",
    }),
    "crux + RTK only · proxy off",
  );
  assert.equal(
    sessionSavingsMeasurementNote("claudeSubscription", {
      ...sources,
      proxy: true,
    }),
    "crux + RTK \u00b7 proxy on",
  );
});

test("a proxied Kimi subscription labels RTK instruction guidance", () => {
  const sources = { rtk: "instruction", crux: "present", proxy: true } as const;

  assert.equal(sessionSavingsUnmeasured("kimiSubscription", sources), false);
  assert.equal(
    sessionSavingsMeasurementNote("kimiSubscription", sources),
    "crux + RTK (by instruction) · proxy on",
  );
  assert.equal(
    sessionSavingsUnmeasured("kimiSubscription", {
      ...sources,
      crux: "missing",
    }),
    false,
  );
  assert.equal(
    sessionSavingsMeasurementNote("kimiApiKey", sources),
    "crux + RTK (by instruction) · proxy on",
  );
});

test("Codex sessions label RTK that depends on instructions", () => {
  const sources = {
    rtk: "instruction",
    crux: "missing",
    proxy: true,
  } as const;

  assert.equal(sessionSavingsUnmeasured("codexChatGpt", sources), false);
  assert.equal(
    sessionSavingsMeasurementNote("codexChatGpt", sources),
    "crux + RTK (by instruction) · proxy on",
  );
});

test("weekly live savings is unavailable only when every active session is unmeasured", () => {
  assert.deepEqual(liveSavingsMeasurementState([liveSession()]), {
    active: true,
    state: "unmeasured",
    unmeasured: true,
    note: "RTK hook not active",
  });

  assert.deepEqual(
    liveSavingsMeasurementState([
      liveSession(),
      liveSession({
        id: "session-2",
        laneKind: "codexChatGpt",
        savingsSources: {
          rtk: "instruction",
          crux: "missing",
          proxy: true,
        },
      }),
    ]),
    { active: true, state: "measured", unmeasured: false, note: null },
  );

  assert.deepEqual(
    liveSavingsMeasurementState([
      liveSession({ status: "exited" }),
      liveSession({
        id: "session-2",
        savingsSources: { rtk: "present", crux: "present", proxy: false },
      }),
    ]),
    {
      active: true,
      state: "estimated",
      unmeasured: false,
      note: "crux + RTK only · proxy off",
    },
  );

  assert.deepEqual(
    liveSavingsMeasurementState([liveSession({ mode: "gui" })]),
    {
      active: true,
      state: "unmeasured",
      unmeasured: true,
      note: "RTK hook not active",
    },
  );
});

test("session and weekly meters render the unavailable state and compact note", () => {
  const meter = readFileSync(
    new URL("../src/components/SessionSavingsMeter.tsx", import.meta.url),
    "utf8",
  );
  const panel = readFileSync(
    new URL("../src/components/SavingsPanel.tsx", import.meta.url),
    "utf8",
  );
  const terminal = readFileSync(
    new URL("../src/components/TerminalPanel.tsx", import.meta.url),
    "utf8",
  );
  const styles = readFileSync(
    new URL("../src/styles.css", import.meta.url),
    "utf8",
  );

  assert.match(meter, /savingsSource: unmeasured/);
  assert.match(meter, /"Not measured"/);
  assert.match(meter, /compact && measurementNote/);
  assert.match(meter, /"\{amount\} saved this session"/);
  assert.match(meter, /"\\u2248 \{amount\} saved this session"/);
  assert.match(meter, /"\{source\}: \{amount\} \(\{tokens\} tokens\)"/);
  assert.match(
    meter,
    /"You pay a flat plan\. This is what the same work costs at API prices\."/,
  );
  assert.match(
    meter,
    /"Measured means Halv observed the token reduction\."/,
  );
  assert.match(
    meter,
    /"Estimated means Halv calculated savings from available usage data\."/,
  );
  assert.match(meter, /"Included in total: \{details\}\."/);
  assert.match(meter, /"Observed separately, not added: \{details\}\."/);
  assert.match(panel, /liveMeasurement\.active && liveMeasurement\.unmeasured/);
  assert.match(panel, /liveMeasurementNote \?\? t\("Not measured"\)/);
  assert.match(panel, /"No savings yet"/);
  assert.match(panel, /"Not measured"/);
  assert.match(panel, /"Measured: 0"/);
  assert.match(terminal, /agentEntry\.session\.savingsSources/);
  assert.match(styles, /\.terminal-header \.agent-session-savings-compact-note/);
});

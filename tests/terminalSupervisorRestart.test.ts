import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  attemptSupervisorRestart,
  confirmSupervisorRestart,
} from "../src/terminalSupervisorRestart.ts";
import type { TerminalSupervisorStatus } from "../src/types.ts";

function supervisorStatus(
  runningSessions: number,
): TerminalSupervisorStatus {
  return {
    available: true,
    socketPath: "/tmp/halv-supervisor.sock",
    pid: 42,
    sessions: runningSessions,
    runningSessions,
    supervisorVersion: runningSessions > 0 ? "0.4.15" : "0.4.16",
    supervisorStale: runningSessions > 0,
    appVersion: "0.4.16",
    message:
      runningSessions > 0 ? "terminal sessions are still running" : null,
  };
}

test("supervisor restart requires confirmation before destructive shutdown", async () => {
  const calls: boolean[] = [];
  const restart = async (killSessions: boolean) => {
    calls.push(killSessions);
    return supervisorStatus(killSessions ? 0 : 3);
  };

  await assert.rejects(
    confirmSupervisorRestart(null, restart),
    /requires session confirmation/,
  );
  assert.deepEqual(calls, []);

  const attempt = await attemptSupervisorRestart(restart);
  assert.deepEqual(calls, [false]);
  assert.deepEqual(attempt.confirmation, { runningSessions: 3 });

  await confirmSupervisorRestart(attempt.confirmation, restart);
  assert.deepEqual(calls, [false, true]);
});

test("an unexpected restart error never opens the destructive gate", async () => {
  const calls: boolean[] = [];
  const restart = async (killSessions: boolean) => {
    calls.push(killSessions);
    return {
      ...supervisorStatus(3),
      message: "failed to read supervisor response: connection closed",
    };
  };

  const attempt = await attemptSupervisorRestart(restart);
  assert.equal(attempt.confirmation, null);
  await assert.rejects(
    confirmSupervisorRestart(attempt.confirmation, restart),
    /requires session confirmation/,
  );
  assert.deepEqual(calls, [false]);
});

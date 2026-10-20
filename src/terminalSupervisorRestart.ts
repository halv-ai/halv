import type { TerminalSupervisorStatus } from "./types";

const RUNNING_SESSIONS_ERROR = "terminal sessions are still running";

export type SupervisorRestartConfirmation = {
  runningSessions: number;
};

type RestartSupervisor = (
  killSessions: boolean,
) => Promise<TerminalSupervisorStatus>;

export async function attemptSupervisorRestart(
  restartSupervisor: RestartSupervisor,
): Promise<{
  status: TerminalSupervisorStatus;
  confirmation: SupervisorRestartConfirmation | null;
}> {
  const status = await restartSupervisor(false);
  return {
    status,
    confirmation:
      status.available &&
      status.runningSessions > 0 &&
      status.message === RUNNING_SESSIONS_ERROR
        ? { runningSessions: status.runningSessions }
        : null,
  };
}

export async function confirmSupervisorRestart(
  confirmation: SupervisorRestartConfirmation | null,
  restartSupervisor: RestartSupervisor,
): Promise<TerminalSupervisorStatus> {
  if (!confirmation) {
    throw new Error("supervisor restart requires session confirmation");
  }
  return restartSupervisor(true);
}

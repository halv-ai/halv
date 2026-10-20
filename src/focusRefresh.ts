import {
  listAgentSessionLaneOptions,
  probeAgentSessions,
  type AgentSessionLaneOptions,
  type AgentSessionProbe,
} from "./agentSessions.ts";
import { ipc } from "./ipc.ts";
import type {
  AuthStatus,
  ChatModel,
  CredentialProfile,
  StatusState,
} from "./types";

/** The agent lanes, the CLI probes and the saved credential profiles. */
export type AgentAccountsRead = {
  laneOptions: PromiseSettledResult<AgentSessionLaneOptions>;
  probe: PromiseSettledResult<AgentSessionProbe>;
  profiles: PromiseSettledResult<CredentialProfile[]>;
};

/** What the composer model list needs. */
export type ModelInputsRead = {
  modelRows: ChatModel[];
  laneOptions: AgentSessionLaneOptions;
  probe: AgentSessionProbe;
  auth: AuthStatus | null;
  profiles: CredentialProfile[];
};

/** The reads of one window focus refresh. */
export type FocusRefreshRead = {
  agentAccounts: Promise<AgentAccountsRead>;
  status: Promise<StatusState>;
  modelInputs: Promise<ModelInputsRead>;
};

export async function readAgentAccounts(): Promise<AgentAccountsRead> {
  const [laneOptions, probe, profiles] = await Promise.allSettled([
    listAgentSessionLaneOptions(),
    probeAgentSessions(),
    ipc.storage.listCredentialProfiles(),
  ]);
  return { laneOptions, probe, profiles };
}

export async function readStatusState(
  credentialStatus: Promise<AuthStatus> = ipc.auth.credentialStatus(),
): Promise<StatusState> {
  const [account, auth, anthropic, sidecar] = await Promise.all([
    ipc.auth.status(),
    credentialStatus,
    ipc.auth.anthropicStatus(),
    ipc.sidecar.status(),
  ]);
  const sidecarStats =
    sidecar.state === "running"
      ? await ipc.sidecar.stats().catch(() => null)
      : null;

  return { account, auth, anthropic, sidecar, sidecarStats, error: null };
}

export async function readModelInputs(
  agentAccounts: Promise<AgentAccountsRead> = readAgentAccounts(),
  credentialStatus: Promise<AuthStatus> = ipc.auth.credentialStatus(),
): Promise<ModelInputsRead> {
  const [modelRows, accounts, auth] = await Promise.all([
    ipc.chat.listModels(),
    agentAccounts,
    credentialStatus.catch(() => null),
  ]);
  return {
    modelRows,
    laneOptions: settledValue(accounts.laneOptions),
    probe: settledValue(accounts.probe),
    auth,
    profiles:
      accounts.profiles.status === "fulfilled" ? accounts.profiles.value : [],
  };
}

/**
 * Starts the reads of a window focus refresh.
 *
 * The connect gate, the status bar and the composer model list need some of
 * the same data. Each command runs once, and the three readers share the
 * results. Before, one focus started the CLI probes and the lane options
 * twice.
 */
export function readFocusRefresh(): FocusRefreshRead {
  const agentAccounts = readAgentAccounts();
  const credentialStatus = ipc.auth.credentialStatus();
  return {
    agentAccounts,
    status: readStatusState(credentialStatus),
    modelInputs: readModelInputs(agentAccounts, credentialStatus),
  };
}

function settledValue<T>(result: PromiseSettledResult<T>): T {
  if (result.status === "rejected") throw result.reason;
  return result.value;
}

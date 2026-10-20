import {
  agentSessionAgentLabel,
  buildAgentSessionLane,
  defaultLaneSelectionForAgent,
  laneOptionsForAgent,
  type AgentKind,
  type AgentSessionCreateLane,
  type AgentSessionCreateLaneKind,
  type AgentSessionLaneOptions,
} from "./agentSessions.ts";
import type { TerminalPaneSplitDirection } from "./terminalLayout.ts";

export const TERMINAL_SPLIT_TERMINAL_CHOICE_ID = "terminal";

export type TerminalSplitMenuChoice = {
  id: string;
  label: string;
  ready: boolean;
  readiness: string;
};

export function terminalSplitMenuChoices(
  terminalReady: boolean,
  terminalLabel: string,
  terminalReadiness: string,
  agentChoices: TerminalSplitMenuChoice[],
  otherProjectChoice: TerminalSplitMenuChoice | null,
): TerminalSplitMenuChoice[] {
  return [
    {
      id: TERMINAL_SPLIT_TERMINAL_CHOICE_ID,
      label: terminalLabel,
      ready: terminalReady,
      readiness: terminalReadiness,
    },
    ...agentChoices.filter((choice) => choice.ready),
    ...(otherProjectChoice ? [otherProjectChoice] : []),
  ];
}

export async function openTerminalSplitChoice<T>(
  choiceId: string,
  direction: TerminalPaneSplitDirection,
  openTerminal: () => Promise<T | null>,
): Promise<
  { direction: TerminalPaneSplitDirection; session: T } | null | undefined
> {
  if (choiceId !== TERMINAL_SPLIT_TERMINAL_CHOICE_ID) return undefined;
  const session = await openTerminal();
  return session ? { direction, session } : null;
}

export type TerminalSplitAgentChoice = {
  agentKind: AgentKind;
  label: string;
  ready: boolean;
  readiness: string;
  laneKind: AgentSessionCreateLaneKind;
  lane: AgentSessionCreateLane | null;
};

export function terminalSplitAgentChoice(
  options: AgentSessionLaneOptions,
  agentKind: AgentKind,
): TerminalSplitAgentChoice {
  const label = agentSessionAgentLabel(agentKind);
  const selection = defaultLaneSelectionForAgent(options, agentKind);
  const laneOption = laneOptionsForAgent(options, agentKind).find(
    (lane) => lane.kind === selection.laneKind,
  );
  const storedProfile = laneOption?.credentialProfiles.find(
    (profile) =>
      profile.status === "active" &&
      profile.id === selection.credentialProfileId,
  );
  const subscriptionReady =
    laneOption?.kind === "claudeSubscription" ||
    laneOption?.kind === "codexChatGpt" ||
    laneOption?.kind === "kimiSubscription" ||
    ((agentKind === "gemini" || agentKind === "antigravity") &&
      laneOption?.kind === "custom")
      ? laneOption.loggedIn === true
      : false;
  const profileReady = Boolean(storedProfile);
  if (!laneOption || (!subscriptionReady && !profileReady)) {
    return {
      agentKind,
      label,
      ready: false,
      readiness: "Login or saved key required",
      laneKind: selection.laneKind,
      lane: null,
    };
  }

  return {
    agentKind,
    label,
    ready: true,
    readiness: subscriptionReady ? "Signed in" : storedProfile!.name,
    laneKind: selection.laneKind,
    lane: buildAgentSessionLane(
      selection.laneKind,
      selection.credentialProfileId,
      "",
    ),
  };
}

export function requireTerminalSplitAgentLane(
  options: AgentSessionLaneOptions,
  agentKind: AgentKind,
) {
  const choice = terminalSplitAgentChoice(options, agentKind);
  if (!choice.ready || !choice.lane) {
    throw new Error(
      `${choice.label} needs a signed-in subscription or saved credential profile.`,
    );
  }
  return choice.lane;
}

// A login inside a terminal does not update the cached lanes.
// This helper reads fresh lanes once before it refuses the launch.
export async function resolveTerminalSplitAgentLane(
  options: AgentSessionLaneOptions,
  agentKind: AgentKind,
  refreshLaneOptions: () => Promise<AgentSessionLaneOptions>,
): Promise<AgentSessionCreateLane> {
  const cached = terminalSplitAgentChoice(options, agentKind);
  if (cached.ready && cached.lane) return cached.lane;
  let fresh: AgentSessionLaneOptions;
  try {
    fresh = await refreshLaneOptions();
  } catch {
    return requireTerminalSplitAgentLane(options, agentKind);
  }
  return requireTerminalSplitAgentLane(fresh, agentKind);
}

// A login inside a terminal does not update the cached lanes.
// The split menu hides agents that are not ready, so it reads fresh
// lanes each time it opens. The menu does not wait for the fetch.
export function refreshLanesWhenSplitMenuOpens(
  open: boolean,
  refreshLaneOptions: () => Promise<unknown>,
) {
  if (!open) return;
  // The store records the error. The menu keeps the cached choices.
  void refreshLaneOptions().catch(() => undefined);
}

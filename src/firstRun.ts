import type { OnboardingPreference } from "./onboarding";
import type { ConnectGateConnections } from "./connectGate";
import type { AuthStatus } from "./types";

export type FirstRunVariant =
  | "claude"
  | "codex"
  | "kimi"
  | "zai"
  | "grok"
  | "fresh";

export type FirstRunPhase = "idle" | "prepared" | "waiting" | "success";

export function firstRunVariant(
  auth: AuthStatus | null,
  connections: ConnectGateConnections = {
    anthropic: false,
    claude: false,
    codex: false,
    grok: false,
    kimi: false,
    moonshot: false,
    openai: false,
    zai: false,
  },
): FirstRunVariant {
  if (auth && "chatgpt" in auth) {
    return "codex";
  }
  if (connections.codex) {
    return "codex";
  }
  if (connections.claude || connections.anthropic) {
    return "claude";
  }
  if (connections.kimi || connections.moonshot) {
    return "kimi";
  }
  if (connections.zai) {
    return "zai";
  }
  if (connections.grok) {
    return "grok";
  }
  return "fresh";
}

export function appLooksFresh({
  initialDataLoaded,
  hasConversationMessages,
  hasAssistantReply,
  replayOnboarding = false,
}: {
  initialDataLoaded: boolean;
  hasConversationMessages: boolean;
  hasAssistantReply: boolean;
  replayOnboarding?: boolean;
}) {
  return (
    initialDataLoaded &&
    (replayOnboarding || (!hasConversationMessages && !hasAssistantReply))
  );
}

export function shouldShowFirstRunCard({
  preference,
  appIsFresh,
  phase,
  currentConversationId,
  messageCount,
  noticeCount,
  hasPendingApproval,
}: {
  preference: OnboardingPreference;
  appIsFresh: boolean;
  phase: FirstRunPhase;
  currentConversationId: string | null;
  messageCount: number;
  noticeCount: number;
  hasPendingApproval: boolean;
}) {
  return (
    preference === "available" &&
    appIsFresh &&
    phase === "idle" &&
    currentConversationId === null &&
    messageCount === 0 &&
    noticeCount === 0 &&
    !hasPendingApproval
  );
}

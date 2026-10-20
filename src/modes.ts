import type { ApprovalPolicy, Project } from "./types";

export type ChatMode =
  | "chat"
  | "sandbox"
  | "on_failure"
  | "ask"
  | "full_access"
  | "legacy_restricted";

export function chatModeFor(
  agentMode: boolean,
  approvalPolicy: ApprovalPolicy,
): ChatMode {
  if (!agentMode) {
    return "chat";
  }
  switch (approvalPolicy) {
    case "on_request":
      return "sandbox";
    case "on_failure":
      return "on_failure";
    case "ask":
      return "ask";
    case "skip_all":
      return "full_access";
    case "read_only":
    case "never":
      return "legacy_restricted";
  }
}

export function safeGuidedTourChatMode(
  approvalPolicy: ApprovalPolicy,
): Exclude<ChatMode, "chat" | "full_access" | "legacy_restricted"> {
  const mode = chatModeFor(true, approvalPolicy);
  switch (mode) {
    case "sandbox":
    case "on_failure":
    case "ask":
      return mode;
    case "chat":
    case "full_access":
    case "legacy_restricted":
      return "sandbox";
  }
}

export function approvalPolicyForMode(
  mode: Exclude<ChatMode, "chat" | "legacy_restricted">,
): ApprovalPolicy {
  switch (mode) {
    case "sandbox":
      return "on_request";
    case "on_failure":
      return "on_failure";
    case "ask":
      return "ask";
    case "full_access":
      return "skip_all";
  }
}

export function effectivePolicy(
  stored: ApprovalPolicy | null | undefined,
  globalDefault: ApprovalPolicy | null | undefined,
  trustLevel: Project["trustLevel"],
): ApprovalPolicy {
  if (stored) {
    return stored;
  }
  if (
    globalDefault &&
    trustLevel !== "trusted" &&
    (globalDefault === "skip_all" || globalDefault === "never")
  ) {
    return "on_request";
  }
  if (globalDefault) {
    return globalDefault;
  }
  return "on_request";
}

export function chatModeLabel(mode: ChatMode) {
  switch (mode) {
    case "chat":
      return "Chat only";
    case "sandbox":
      return "Auto";
    case "on_failure":
      return "On failure";
    case "ask":
      return "Ask";
    case "full_access":
      return "Full access";
    case "legacy_restricted":
      return "Restricted (legacy)";
  }
}

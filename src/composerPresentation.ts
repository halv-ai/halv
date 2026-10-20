import type { ChatMode } from "./modes";
import type { WorkflowMode } from "./types";

export type ComposerInputRoute =
  | "chat"
  | "shell"
  | "blocked-empty-shell"
  | "blocked-project-shell";

export type ComposerPrimaryAction = {
  kind: "send" | "steer" | "queue" | "run" | "stop";
  label: string;
  ariaLabel: string;
  disabledReason: string | null;
};

export function composerModePresentation(
  workflowMode: WorkflowMode,
  chatMode: ChatMode,
) {
  if (workflowMode === "plan") {
    return {
      label: "Plan · read-only",
      description: "Explore and propose a plan without changing files.",
    };
  }
  switch (chatMode) {
    case "chat":
      return {
        label: "Chat · no tools",
        description: "Answer without running project tools.",
      };
    case "sandbox":
      return {
        label: "Agent · Auto",
        description:
          "Work sandboxed in the workspace without approval prompts. Ask only when more access is needed.",
      };
    case "on_failure":
      return {
        label: "Agent · On failure",
        description: "Work sandboxed and ask only when a command fails.",
      };
    case "ask":
      return {
        label: "Agent · Ask first",
        description: "Use project tools and confirm write actions.",
      };
    case "full_access":
      return {
        label: "Agent · Full access",
        description: "Use unrestricted machine access without approvals.",
      };
    case "legacy_restricted":
      return {
        label: "Agent · Restricted",
        description: "Legacy policy; choose a current access mode.",
      };
  }
}

export function composerModeAccessLabel(chatMode: ChatMode): string | null {
  switch (chatMode) {
    case "chat":
      return null;
    case "sandbox":
      return "Auto";
    case "on_failure":
      return "On failure";
    case "ask":
      return "Ask first";
    case "full_access":
      return "Full access";
    case "legacy_restricted":
      return "Restricted (legacy)";
  }
}

export function composerPlaceholder(
  workflowMode: WorkflowMode,
  chatMode: ChatMode,
  projectName: string | null,
) {
  if (workflowMode === "plan") {
    return projectName
      ? `Plan work in ${projectName} — no changes will be made`
      : "Describe what to plan — no changes will be made";
  }
  if (chatMode === "chat") {
    return projectName
      ? `Ask about ${projectName} — switch to Agent to use tools`
      : "Ask anything — choose a project to use repository tools";
  }
  return projectName
    ? `Describe a task for ${projectName}`
    : "Choose a project before using Agent tools";
}

export function composerPrimaryAction({
  attachmentCount,
  cancelAndRevertPending,
  hasText,
  inputRoute,
  proxyDown,
  sendPending,
  slashCommand,
  steerAvailable,
}: {
  attachmentCount: number;
  cancelAndRevertPending: boolean;
  hasText: boolean;
  inputRoute: ComposerInputRoute;
  proxyDown: boolean;
  sendPending: boolean;
  slashCommand: boolean;
  steerAvailable: boolean;
}): ComposerPrimaryAction {
  const hasContent = hasText || attachmentCount > 0;
  const canSteer =
    sendPending &&
    steerAvailable &&
    hasText &&
    attachmentCount === 0 &&
    inputRoute === "chat";
  const kind = slashCommand
    ? "run"
    : canSteer
      ? "steer"
      : sendPending && hasContent
        ? "queue"
        : sendPending
          ? "stop"
          : inputRoute !== "chat"
            ? "run"
            : "send";
  const label =
    kind === "run"
      ? slashCommand
        ? "Run"
        : "Run command"
      : kind === "queue"
        ? "Queue"
        : kind === "steer"
          ? "Steer"
          : kind === "stop"
            ? "Stop"
            : "Send";
  const ariaLabel =
    kind === "run"
      ? slashCommand
        ? "Run slash command"
        : "Run direct shell command"
      : kind === "queue"
        ? "Queue message after the current response"
        : kind === "steer"
          ? "Steer the current response"
          : kind === "stop"
            ? "Stop response"
            : "Send message";

  let disabledReason: string | null = null;
  if (cancelAndRevertPending) {
    disabledReason = "Wait while Halv stops and reverts the current turn.";
  } else if (slashCommand && attachmentCount > 0) {
    disabledReason = "Remove image attachments before running a slash command.";
  } else if (inputRoute === "shell" && attachmentCount > 0) {
    disabledReason = "Direct shell commands cannot include image attachments.";
  } else if (inputRoute === "blocked-empty-shell") {
    disabledReason = "Type a command after !, or press Escape to leave shell mode.";
  } else if (inputRoute === "blocked-project-shell") {
    disabledReason = "Choose a project before running a direct shell command.";
  } else if (!sendPending && !slashCommand && inputRoute === "chat" && proxyDown) {
    disabledReason =
      "The proxy is unavailable. Run /restart-proxy or open Settings.";
  } else if (!sendPending && !hasContent) {
    disabledReason = "Type a message or attach an image first.";
  }

  return { kind, label, ariaLabel, disabledReason };
}

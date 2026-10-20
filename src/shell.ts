export function shellCommandFromInput(input: string): string | null {
  const normalized = input.trimStart();
  if (!normalized.startsWith("!")) {
    return null;
  }
  return normalized.slice(1).trim();
}

export type InputRoute =
  | { kind: "chat" }
  | { kind: "shell"; command: string; projectId: number }
  | {
      kind: "blocked_shell";
      command: string;
      reason: "empty_command" | "project_required";
    };

export function resolveInputRoute(
  input: string,
  projectId: number | null,
): InputRoute {
  const command = shellCommandFromInput(input);
  if (command === null) {
    return { kind: "chat" };
  }
  if (!command) {
    return { kind: "blocked_shell", command, reason: "empty_command" };
  }
  if (projectId === null) {
    return { kind: "blocked_shell", command, reason: "project_required" };
  }
  return { kind: "shell", command, projectId };
}

export function pinnedProjectIdForQueuedInput(
  input: string,
  projectId: number | null,
): number | null | undefined {
  return shellCommandFromInput(input) === null ? undefined : projectId;
}

export function requestProjectIdForInput(
  hasExistingConversation: boolean,
  liveProjectId: number | null,
  pinnedProjectId: number | null | undefined,
): number | null {
  return !hasExistingConversation && pinnedProjectId !== undefined
    ? pinnedProjectId
    : liveProjectId;
}

export function exitShellModeDraft(input: string): string {
  const normalized = input.trimStart();
  if (!normalized.startsWith("!")) {
    return input;
  }
  return normalized.slice(1).trimStart();
}

export function shellModePresentation(projectName: string | null) {
  return projectName
    ? {
        status: "direct · no model",
        placeholder: `Run in ${projectName} — model bypassed`,
      }
    : {
        status: "select a project to run",
        placeholder: "Select a project before running a command",
      };
}

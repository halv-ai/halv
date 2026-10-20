import type { TerminalSession } from "./types.ts";

export type PlainTerminalOpenRequest = {
  projectId: number;
  workspaceId: string | null;
  conversationId: string | null;
  /** The shell tab that owns the shell. Never together with a
   *  conversation (ADR 0010). */
  shellTabId?: string | null;
};

export type PlainTerminalInvoker = (
  command: string,
  args: Record<string, unknown>,
) => Promise<TerminalSession>;

export function openPlainTerminalSession(
  invokeTerminal: PlainTerminalInvoker,
  request: PlainTerminalOpenRequest,
) {
  return invokeTerminal("terminal_open", {
    projectId: request.projectId,
    workspaceId: request.workspaceId,
    conversationId: request.conversationId,
    ...(request.shellTabId ? { shellTabId: request.shellTabId } : {}),
    cols: 100,
    rows: 24,
    executionProfile: "full_access",
  });
}

export type TerminalOpeningState =
  | { phase: "idle" }
  | { phase: "opening" }
  | { phase: "attached" }
  | { phase: "error"; message: string };

export type TerminalOpeningEvent =
  | { type: "start" }
  | { type: "attach" }
  | { type: "fail"; message: string }
  | { type: "clear" };

export const IDLE_TERMINAL_OPENING: TerminalOpeningState = { phase: "idle" };

export function reduceTerminalOpening(
  state: TerminalOpeningState,
  event: TerminalOpeningEvent,
): TerminalOpeningState {
  if (event.type === "start") return { phase: "opening" };
  if (event.type === "attach") {
    return state.phase === "opening" ? { phase: "attached" } : state;
  }
  if (event.type === "fail") {
    return state.phase === "opening"
      ? { phase: "error", message: event.message }
      : state;
  }
  return IDLE_TERMINAL_OPENING;
}

export function terminalOpeningNotice(state: TerminalOpeningState) {
  if (state.phase === "opening") return "Opening terminal…";
  if (state.phase === "attached") {
    return "Continued in terminal — resumed session";
  }
  if (state.phase === "error") {
    return `Terminal failed to open — ${state.message}`;
  }
  return null;
}

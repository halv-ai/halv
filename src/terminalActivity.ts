import type { AgentSessionMode, AgentSessionStatus } from "./agentSessions";
import type { TerminalSession } from "./types";

export type TerminalActivityStatus =
  | "running"
  | "responding"
  | "waiting_input";

export function guidedTerminalTurnCompleted(
  phase: "idle" | "prepared" | "waiting" | "success",
  sawActiveStatus: boolean,
  sawSustainedOutput: boolean,
  status: AgentSessionStatus | TerminalSession["status"] | null,
): boolean {
  return (
    phase === "waiting" &&
    sawActiveStatus &&
    sawSustainedOutput &&
    (status === "waiting_input" || status === "exited")
  );
}

export const TERMINAL_RESPONSE_IDLE_MS = 1_500;
export const TERMINAL_TURN_GAP_MS = 8_000;
export const TERMINAL_PROMPT_GRACE_MS = 45_000;
export const TERMINAL_SUSTAINED_OUTPUT_BYTES = 256;

type ActivityListener = (status: TerminalActivityStatus | null) => void;

type TerminalActivityTimingOptions = {
  idleMs?: number;
  turnGapMs?: number;
  promptGraceMs?: number;
  sustainedOutputBytes?: number;
};

type TerminalActivityTracker = {
  notePromptSubmitted(): void;
  noteOutput(byteCount?: number): void;
  noteEmbeddedTurnStarted(turnId?: string | null): boolean;
  noteEmbeddedTurnFinished(turnId?: string | null): boolean;
  noteViewed(): void;
  finish(): void;
  dispose(): void;
};

export function createTerminalActivityTracker(
  onStatus: ActivityListener,
  timingOptions: number | TerminalActivityTimingOptions = {},
): TerminalActivityTracker {
  const idleMs =
    typeof timingOptions === "number"
      ? timingOptions
      : (timingOptions.idleMs ?? TERMINAL_RESPONSE_IDLE_MS);
  const turnGapMs =
    typeof timingOptions === "number"
      ? timingOptions
      : (timingOptions.turnGapMs ?? TERMINAL_TURN_GAP_MS);
  const outputSampleMs = Math.min(idleMs, turnGapMs);
  const promptGraceMs =
    typeof timingOptions === "number"
      ? timingOptions
      : (timingOptions.promptGraceMs ?? TERMINAL_PROMPT_GRACE_MS);
  const sustainedOutputBytes =
    typeof timingOptions === "number"
      ? TERMINAL_SUSTAINED_OUTPUT_BYTES
      : (timingOptions.sustainedOutputBytes ??
        TERMINAL_SUSTAINED_OUTPUT_BYTES);
  let idleTimer: ReturnType<typeof setTimeout> | null = null;
  let turnGapTimer: ReturnType<typeof setTimeout> | null = null;
  let promptGraceTimer: ReturnType<typeof setTimeout> | null = null;
  let turnInFlight = false;
  let sawSustainedOutput = false;
  let outputBytes = 0;
  let samplingOutput = false;
  let suppressRepaints = false;
  let semanticLifecycle = false;
  let semanticTurnActive = false;
  let activeSemanticTurnId: string | null = null;

  const cancelIdleTimer = () => {
    if (idleTimer === null) return;
    clearTimeout(idleTimer);
    idleTimer = null;
  };

  const cancelPromptGraceTimer = () => {
    if (promptGraceTimer === null) return;
    clearTimeout(promptGraceTimer);
    promptGraceTimer = null;
  };

  const cancelTurnGapTimer = () => {
    if (turnGapTimer !== null) clearTimeout(turnGapTimer);
    turnGapTimer = null;
  };

  const settleTurn = () => {
    cancelTurnGapTimer();
    cancelIdleTimer();
    cancelPromptGraceTimer();
    turnInFlight = false;
    sawSustainedOutput = false;
    outputBytes = 0;
    samplingOutput = false;
    suppressRepaints = true;
    onStatus("waiting_input");
  };

  const armIdleTimer = (delayMs: number) => {
    cancelIdleTimer();
    idleTimer = setTimeout(() => {
      idleTimer = null;
      settleTurn();
    }, delayMs);
  };

  const armTurnGap = () => {
    cancelTurnGapTimer();
    turnGapTimer = setTimeout(settleTurn, turnGapMs);
  };

  const armOutputSample = () => {
    cancelIdleTimer();
    idleTimer = setTimeout(() => {
      idleTimer = null;
      if (turnInFlight && outputBytes >= sustainedOutputBytes) {
        const firstSustainedOutput = !sawSustainedOutput;
        sawSustainedOutput = true;
        cancelPromptGraceTimer();
        outputBytes = 0;
        if (firstSustainedOutput) onStatus("responding");
        armTurnGap();
        armOutputSample();
        return;
      }
      outputBytes = 0;
      samplingOutput = false;
      // A quiet sample is not a completed turn. Tool calls and streaming
      // can pause between bursts; only the full turn gap settles activity.
    }, outputSampleMs);
  };

  return {
    noteViewed() {
      if (!turnInFlight) suppressRepaints = true;
    },
    notePromptSubmitted() {
      cancelTurnGapTimer();
      turnInFlight = true;
      sawSustainedOutput = false;
      outputBytes = 0;
      samplingOutput = false;
      suppressRepaints = false;
      onStatus("running");
      cancelIdleTimer();
      cancelPromptGraceTimer();
      if (semanticLifecycle) return;
      promptGraceTimer = setTimeout(() => {
        promptGraceTimer = null;
        settleTurn();
      }, promptGraceMs);
    },
    noteOutput(byteCount = sustainedOutputBytes) {
      // Focus, resize, scrolling and typing can redraw the entire TUI. Once
      // settled, output alone cannot start a new user turn.
      if (suppressRepaints || byteCount <= 0) return;
      onStatus("running");
      if (semanticLifecycle) {
        if (!turnInFlight) return;
        outputBytes += Math.max(0, byteCount);
        if (!sawSustainedOutput && outputBytes >= sustainedOutputBytes) {
          sawSustainedOutput = true;
          outputBytes = 0;
          onStatus("responding");
        }
        return;
      }
      if (!turnInFlight) {
        armIdleTimer(idleMs);
        return;
      }
      outputBytes += Math.max(0, byteCount);
      if (sawSustainedOutput && outputBytes >= sustainedOutputBytes) armTurnGap();
      if (!samplingOutput) {
        samplingOutput = true;
        armOutputSample();
      }
    },
    noteEmbeddedTurnStarted(turnId) {
      const normalizedTurnId = turnId?.trim() || null;
      if (
        semanticTurnActive &&
        normalizedTurnId !== null &&
        normalizedTurnId === activeSemanticTurnId
      ) {
        return false;
      }
      semanticLifecycle = true;
      semanticTurnActive = true;
      activeSemanticTurnId = normalizedTurnId;
      cancelTurnGapTimer();
      cancelIdleTimer();
      cancelPromptGraceTimer();
      turnInFlight = true;
      sawSustainedOutput = false;
      outputBytes = 0;
      samplingOutput = false;
      suppressRepaints = false;
      onStatus("running");
      return true;
    },
    noteEmbeddedTurnFinished(turnId) {
      semanticLifecycle = true;
      const normalizedTurnId = turnId?.trim() || null;
      if (
        !semanticTurnActive ||
        (activeSemanticTurnId !== null &&
          normalizedTurnId !== null &&
          activeSemanticTurnId !== normalizedTurnId)
      ) {
        return false;
      }
      semanticTurnActive = false;
      activeSemanticTurnId = null;
      settleTurn();
      return true;
    },
    finish() {
      cancelTurnGapTimer();
      cancelIdleTimer();
      cancelPromptGraceTimer();
      turnInFlight = false;
      sawSustainedOutput = false;
      outputBytes = 0;
      samplingOutput = false;
      suppressRepaints = true;
      semanticTurnActive = false;
      activeSemanticTurnId = null;
      onStatus(null);
    },
    dispose() {
      cancelTurnGapTimer();
      cancelIdleTimer();
      cancelPromptGraceTimer();
    },
  };
}

export function effectiveTerminalStatus(
  processStatus: TerminalSession["status"],
  agentStatus: AgentSessionStatus | null | undefined,
  activityStatus: TerminalActivityStatus | null | undefined,
  agentMode?: AgentSessionMode | null,
): AgentSessionStatus | TerminalSession["status"] {
  if (processStatus !== "running") return processStatus;
  const displayActivityStatus =
    activityStatus === "responding" ? "running" : activityStatus;
  if (agentMode === "gui") {
    return agentStatus ?? displayActivityStatus ?? processStatus;
  }
  return displayActivityStatus ?? processStatus;
}

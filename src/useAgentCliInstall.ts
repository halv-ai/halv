import { ipc, type UnlistenFn } from "./ipc.ts";
import { useCallback, useEffect, useRef, useState } from "react";

export type AgentCliKind = "claude" | "codex";

export type AgentCliInstallOutcome = {
  agentKind: string;
  version: string;
  binary: string;
};

export type AgentCliInstallError = {
  code:
    | "unsupportedAgent"
    | "unsupportedPlatform"
    | "appDataUnavailable"
    | "resolveFailed"
    | "downloadFailed"
    | "checksumMismatch"
    | "extractFailed"
    | "writeFailed"
    | "alreadyInstalling";
  message: string;
  details: string[];
  manualHint: string;
};

export type AgentCliInstallProgress = {
  agentKind: string;
  stage:
    | "resolving"
    | "downloading"
    | "verifying"
    | "extracting"
    | "success"
    | "error";
  line: string;
  downloadedBytes?: number | null;
  totalBytes?: number | null;
};

type ActiveInstall = {
  agentKind: AgentCliKind;
  generation: number;
};

const AGENT_CLI_INSTALL_PROGRESS_EVENT = "agent-cli-install-progress";

export function useAgentCliInstall() {
  const [pendingAgent, setPendingAgent] = useState<AgentCliKind | null>(null);
  const [progress, setProgress] = useState<AgentCliInstallProgress | null>(null);
  const [error, setError] = useState<AgentCliInstallError | null>(null);
  const mountedRef = useRef(true);
  const generationRef = useRef(0);
  const activeInstallRef = useRef<ActiveInstall | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    let disposed = false;
    let unlisten: UnlistenFn | null = null;

    void ipc.events.listen<AgentCliInstallProgress>(
      AGENT_CLI_INSTALL_PROGRESS_EVENT,
      (payload) => {
        const activeInstall = activeInstallRef.current;
        if (
          disposed ||
          !mountedRef.current ||
          activeInstall === null ||
          generationRef.current !== activeInstall.generation ||
          payload.agentKind !== activeInstall.agentKind
        ) {
          return;
        }
        setProgress((current) => {
          const latestInstall = activeInstallRef.current;
          if (
            !mountedRef.current ||
            latestInstall === null ||
            latestInstall.generation !== activeInstall.generation ||
            generationRef.current !== activeInstall.generation
          ) {
            return current;
          }
          return payload;
        });
      },
    ).then((listener) => {
      if (disposed) {
        listener();
      } else {
        unlisten = listener;
      }
    });

    return () => {
      disposed = true;
      mountedRef.current = false;
      generationRef.current += 1;
      activeInstallRef.current = null;
      unlisten?.();
    };
  }, []);

  const install = useCallback(async (agentKind: AgentCliKind) => {
    if (activeInstallRef.current !== null) {
      return false;
    }

    const generation = generationRef.current + 1;
    generationRef.current = generation;
    activeInstallRef.current = { agentKind, generation };
    setPendingAgent(agentKind);
    setProgress(null);
    setError(null);
    const installIsCurrent = () => {
      const activeInstall = activeInstallRef.current;
      return (
        mountedRef.current &&
        activeInstall !== null &&
        activeInstall.agentKind === agentKind &&
        activeInstall.generation === generation &&
        generationRef.current === generation
      );
    };

    try {
      await ipc.agent.cliInstall({ agentKind });
      if (!installIsCurrent()) {
        return false;
      }
      activeInstallRef.current = null;
      setPendingAgent(null);
      return true;
    } catch (cause) {
      if (!installIsCurrent()) {
        return false;
      }
      activeInstallRef.current = null;
      setPendingAgent(null);
      setError(normalizeInstallError(cause, agentKind));
      return false;
    }
  }, []);

  const reset = useCallback(() => {
    generationRef.current += 1;
    activeInstallRef.current = null;
    if (!mountedRef.current) {
      return;
    }
    setPendingAgent(null);
    setProgress(null);
    setError(null);
  }, []);

  return { install, pendingAgent, progress, error, reset };
}

function normalizeInstallError(
  cause: unknown,
  agentKind: AgentCliKind,
): AgentCliInstallError {
  if (isAgentCliInstallError(cause)) {
    return cause;
  }
  const message =
    cause instanceof Error
      ? cause.message
      : typeof cause === "string"
        ? cause
        : "The CLI download failed.";
  return {
    code: "downloadFailed",
    message: message.trim() || "The CLI download failed.",
    details: [],
    manualHint:
      agentKind === "claude"
        ? "npm i -g @anthropic-ai/claude-code"
        : "npm i -g @openai/codex",
  };
}

function isAgentCliInstallError(value: unknown): value is AgentCliInstallError {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Record<string, unknown>;
  return (
    isAgentCliInstallErrorCode(candidate.code) &&
    typeof candidate.message === "string" &&
    Array.isArray(candidate.details) &&
    candidate.details.every((detail) => typeof detail === "string") &&
    typeof candidate.manualHint === "string"
  );
}

function isAgentCliInstallErrorCode(
  value: unknown,
): value is AgentCliInstallError["code"] {
  return (
    value === "unsupportedAgent" ||
    value === "unsupportedPlatform" ||
    value === "appDataUnavailable" ||
    value === "resolveFailed" ||
    value === "downloadFailed" ||
    value === "checksumMismatch" ||
    value === "extractFailed" ||
    value === "writeFailed" ||
    value === "alreadyInstalling"
  );
}

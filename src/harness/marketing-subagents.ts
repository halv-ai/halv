import type { StoredMessage, SubagentThread } from "../types.ts";

export const MARKETING_SUBAGENT_STATE_NAMES = [
  "marketing-subagents-dark",
  "marketing-subagents-light",
] as const;

export const MARKETING_SUBAGENT_CONVERSATION_ID = "audit-marketing";
export const MARKETING_SUBAGENT_TURN_ID = "marketing-subagents-user";
export const MARKETING_SUBAGENT_CONVERSATION_TITLE =
  "Parallel release readiness";

export const MARKETING_SUBAGENT_MESSAGES: StoredMessage[] = [
  {
    id: MARKETING_SUBAGENT_TURN_ID,
    conversationId: MARKETING_SUBAGENT_CONVERSATION_ID,
    role: "user",
    content:
      "Fan this release audit into parallel code, test, documentation, and migration checks.",
    createdAt: "2026-08-13T14:54:00.000Z",
    model: null,
    inputTokens: null,
    outputTokens: null,
    error: false,
    mentions: ["src/auth", "tests/auth", "docs"],
    tokensSaved: null,
    savingsUsd: null,
  },
];

export const MARKETING_SUBAGENT_THREADS: SubagentThread[] = [
  subagent({
    agentId: "/root/callers",
    taskName: "callers",
    task: "Map every caller of the session rotation boundary.",
    status: "running",
    result: "Mapped 18 call sites. Checking two indirect middleware paths…",
  }),
  subagent({
    agentId: "/root/regression_tests",
    taskName: "regression_tests",
    task: "Run focused authentication and recovery tests.",
    status: "completed",
    result: "12 focused tests pass. No response-contract changes found.",
  }),
  subagent({
    agentId: "/root/docs_review",
    taskName: "docs_review",
    task: "Review public authentication documentation.",
    status: "pending",
    result: "Queued behind the implementation diff.",
  }),
  subagent({
    agentId: "/root/migration_audit",
    taskName: "migration_audit",
    task: "Validate rollback behavior against stored sessions.",
    status: "errored",
    error: "Fixture snapshot mismatch at migration boundary line 84.",
  }),
];

export function prepareMarketingSubagentsHarness(stateName: string) {
  if (!MARKETING_SUBAGENT_STATE_NAMES.some((name) => name === stateName)) return;
  window.localStorage.setItem(
    "halv.workspace.last-active-conversation.v1",
    MARKETING_SUBAGENT_CONVERSATION_ID,
  );
}

function subagent(input: {
  agentId: string;
  taskName: string;
  task: string;
  status: SubagentThread["status"];
  result?: string;
  error?: string;
}): SubagentThread {
  const updatedAt =
    input.status === "completed" || input.status === "errored"
      ? "2026-08-13T14:58:30.000Z"
      : "2026-08-13T14:59:30.000Z";
  return {
    ...input,
    conversationId: MARKETING_SUBAGENT_CONVERSATION_ID,
    turnId: MARKETING_SUBAGENT_TURN_ID,
    result: input.result ?? null,
    error: input.error ?? null,
    generation: 1,
    createdAt: "2026-08-13T14:55:00.000Z",
    updatedAt,
    messages: [
      {
        id: `${input.agentId}-tail`,
        agentId: input.agentId,
        role: input.error ? "error" : "assistant",
        content: input.error ?? input.result ?? "Waiting for coordinator input.",
        generation: 1,
        createdAt: updatedAt,
      },
    ],
  };
}

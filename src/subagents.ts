import type { SubagentThread, ToolCallUi } from "./types";

export type SubagentActivityStatus =
  | "pending"
  | "running"
  | "completed"
  | "errored"
  | "interrupted"
  | "unknown";

export type SubagentActivity = {
  agentId: string;
  taskName: string;
  task: string;
  status: SubagentActivityStatus;
  output: string;
  active: boolean;
  failed: boolean;
};

const activeStatuses = new Set(["pending", "running", "starting", "queued"]);

function normalizeStatus(status: string): SubagentActivityStatus {
  const normalized = status.trim().toLowerCase();
  if (normalized === "pending" || normalized === "running") {
    return normalized;
  }
  if (normalized === "completed" || normalized === "succeeded" || normalized === "done") {
    return "completed";
  }
  if (normalized === "errored" || normalized === "error" || normalized === "failed") {
    return "errored";
  }
  if (normalized === "interrupted" || normalized === "cancelled" || normalized === "canceled") {
    return "interrupted";
  }
  return "unknown";
}

export function isSubagentActivityToolCall(call: ToolCallUi) {
  return call.toolKind === "subagent" && call.command.startsWith("/root/");
}

export function subagentActivities(
  toolCalls: ToolCallUi[],
  threads: SubagentThread[] = [],
): SubagentActivity[] {
  const records = new Map<string, SubagentActivity>();
  const order: string[] = [];

  for (const thread of threads) {
    const status = normalizeStatus(thread.status);
    records.set(thread.agentId, {
      agentId: thread.agentId,
      taskName: thread.taskName,
      task: thread.task,
      status,
      output: thread.result?.trim() || thread.error?.trim() || "",
      active: status === "pending" || status === "running",
      failed: status === "errored" || status === "interrupted",
    });
    order.push(thread.agentId);
  }

  for (const call of toolCalls) {
    if (!isSubagentActivityToolCall(call)) {
      continue;
    }
    const status = normalizeStatus(call.status);
    const active =
      status === "pending" ||
      status === "running" ||
      activeStatuses.has(call.status.trim().toLowerCase());
    const persisted = records.get(call.callId);
    const activity: SubagentActivity = {
      agentId: call.callId,
      taskName: call.command,
      task: call.justification?.trim() || persisted?.task || "",
      status,
      output: call.tail.trim() || persisted?.output || "",
      active,
      failed: status === "errored" || status === "interrupted",
    };
    if (!records.has(call.callId)) {
      order.push(call.callId);
    }
    records.set(call.callId, activity);
  }

  return order.map((id) => records.get(id)).filter((record): record is SubagentActivity => Boolean(record));
}

export function subagentStatusLabel(status: SubagentActivityStatus) {
  switch (status) {
    case "pending":
      return "Queued";
    case "running":
      return "Running";
    case "completed":
      return "Completed";
    case "errored":
      return "Failed";
    case "interrupted":
      return "Stopped";
    default:
      return "Unknown";
  }
}

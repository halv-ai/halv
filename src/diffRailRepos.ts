import type { AgentSessionTranscriptItem } from "./agentSessions";

type JsonObject = Record<string, unknown>;

export const GIT_REPO_DISCOVERY_TTL_MS = 5_000;

type GitRepoDiscoveryCacheEntry = {
  generation: number;
  expiresAt: number;
  request: Promise<string[]>;
};

export function createGitRepoDiscoveryCache(
  load: (directory: string) => Promise<string[]>,
  ttlMs = GIT_REPO_DISCOVERY_TTL_MS,
  now: () => number = Date.now,
) {
  const entries = new Map<string, GitRepoDiscoveryCacheEntry>();
  return (directory: string, generation: number) => {
    const requestedAt = now();
    const cached = entries.get(directory);
    if (
      cached?.generation === generation &&
      requestedAt < cached.expiresAt
    ) {
      return cached.request;
    }
    const request = load(directory);
    entries.set(directory, {
      generation,
      expiresAt: requestedAt + ttlMs,
      request,
    });
    return request;
  };
}

export function collectEditedPaths(
  transcript: readonly AgentSessionTranscriptItem[],
): string[] {
  const paths = new Set<string>();
  for (const item of transcript) {
    if (item.kind !== "toolCall" || !item.summaryJson) continue;
    const summary = parseObject(item.summaryJson);
    if (!summary) continue;

    addAbsolutePath(paths, summary.file_path);
    addAbsolutePath(paths, summary.notebook_path);
    if (Array.isArray(summary.edits)) {
      for (const edit of summary.edits) {
        if (isObject(edit)) addAbsolutePath(paths, edit.file_path);
      }
    }
    if (item.name === "applyPatch") {
      if (isObject(summary.changes)) {
        for (const path of Object.keys(summary.changes)) {
          addAbsolutePath(paths, path);
        }
      } else if (Array.isArray(summary.changes)) {
        for (const change of summary.changes) {
          if (isObject(change)) addAbsolutePath(paths, change.path);
        }
      }
    }
  }
  return [...paths];
}

export function collectWorkedDirectories(
  transcript: readonly AgentSessionTranscriptItem[],
): string[] {
  const directories = new Set<string>();
  for (const item of transcript) {
    if (item.kind !== "toolCall" || !item.summaryJson) continue;
    const summary = parseObject(item.summaryJson);
    if (summary) addAbsolutePath(directories, summary.cwd);
  }
  return [...directories];
}

function parseObject(value: string): JsonObject | null {
  try {
    const parsed: unknown = JSON.parse(value);
    return isObject(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function addAbsolutePath(paths: Set<string>, value: unknown) {
  if (typeof value !== "string") return;
  const path = value.trim();
  if (path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path)) paths.add(path);
}

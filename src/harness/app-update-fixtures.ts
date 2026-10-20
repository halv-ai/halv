// Update card states for the UX harness: ?update=<state>.
import type {
  AppUpdateRestartBlockers,
  AppUpdateSnapshot,
  AppUpdateStatus,
} from "../types";
import { FIXED_VISUAL_NOW, type AppUpdateHarnessState } from "./states.ts";

const CURRENT_VERSION = "0.7.1";
const NEXT_VERSION = "0.7.2";
const RELEASE_URL = `https://github.com/pedr0v/halv-releases/releases/tag/v${NEXT_VERSION}`;
// The shape of a real latest.json note: a heading and bullet lines.
const NOTES = [
  "## What's new",
  "",
  "- Updates download in the background. A small card asks before Halv restarts.",
  "- Agent sessions keep running while Halv restarts, and the card says how many reconnected.",
  "- Settings shows the terminal supervisor and an automatic download switch.",
  "- Quitting Halv installs a downloaded update.",
].join("\n");
const ARCHIVE_BYTES = 176_883_936;

export const HARNESS_RESTART_BLOCKERS: AppUpdateRestartBlockers = {
  chatReplies: 1,
  guiAgentSessions: 2,
  inProcessTerminals: 0,
};

const READY: AppUpdateStatus = {
  state: "ready",
  version: NEXT_VERSION,
  notes: NOTES,
  releaseUrl: RELEASE_URL,
};

export function appUpdateHarnessSnapshot(
  state: AppUpdateHarnessState | undefined,
): AppUpdateSnapshot {
  const base: AppUpdateSnapshot = {
    status: { state: "idle" },
    currentVersion: CURRENT_VERSION,
    autoDownload: true,
    installMode: "replace",
    deferredVersion: null,
    restartWhenIdle: false,
    blockers: null,
    postUpdate: null,
    lastCheckedAt: FIXED_VISUAL_NOW,
    sessionsSurviveRestart: true,
  };
  switch (state) {
    case undefined:
      return base;
    case "checking":
      return { ...base, status: { state: "checking", userInitiated: true } };
    case "uptodate":
      return { ...base, status: { state: "upToDate", userInitiated: true } };
    case "available":
      return {
        ...base,
        autoDownload: false,
        status: {
          state: "available",
          version: NEXT_VERSION,
          notes: NOTES,
          releaseUrl: RELEASE_URL,
        },
      };
    case "downloading":
      return {
        ...base,
        status: {
          state: "downloading",
          version: NEXT_VERSION,
          downloadedBytes: 72_450_000,
          totalBytes: ARCHIVE_BYTES,
        },
      };
    case "ready":
    case "busy":
      return { ...base, status: READY };
    case "blockers":
      return { ...base, status: READY, blockers: HARNESS_RESTART_BLOCKERS };
    case "waiting":
      return {
        ...base,
        status: READY,
        restartWhenIdle: true,
        blockers: HARNESS_RESTART_BLOCKERS,
      };
    case "installing":
      return {
        ...base,
        status: { state: "installing", version: NEXT_VERSION, trigger: "restart" },
      };
    case "error":
      return {
        ...base,
        status: {
          state: "error",
          operation: "download",
          message: "the download timed out: no data arrived for 60 seconds",
          version: NEXT_VERSION,
          retryable: true,
          userInitiated: true,
        },
      };
    case "updated":
      return {
        ...base,
        currentVersion: NEXT_VERSION,
        postUpdate: {
          fromVersion: CURRENT_VERSION,
          toVersion: NEXT_VERSION,
          reconnectedSessions: 3,
          releaseUrl: RELEASE_URL,
        },
      };
    case "windows":
      return {
        ...base,
        installMode: "installer",
        sessionsSurviveRestart: false,
        status: READY,
      };
    case "no-supervisor":
      return { ...base, sessionsSurviveRestart: false, status: READY };
    case "read-only":
      return {
        ...base,
        installMode: "readOnly",
        status: {
          state: "available",
          version: NEXT_VERSION,
          notes: NOTES,
          releaseUrl: RELEASE_URL,
        },
      };
  }
}

/** The release that a harness check finds, or null when Halv is current. */
export function appUpdateHarnessRelease(
  state: AppUpdateHarnessState | undefined,
): AppUpdateStatus | null {
  return state === undefined || state === "checking" || state === "uptodate"
    ? null
    : READY;
}

/** The work that stops a harness restart. Only "busy" has any. */
export function appUpdateHarnessBlockers(
  state: AppUpdateHarnessState | undefined,
): AppUpdateRestartBlockers {
  return state === "busy"
    ? HARNESS_RESTART_BLOCKERS
    : { chatReplies: 0, guiAgentSessions: 0, inProcessTerminals: 0 };
}

// Pure helpers for update messages. Rust owns the update flow (ADR 0012).
import type { Translate } from "./i18n/runtime";
import type { AppUpdateOperation } from "./types";

export function appUpdateDefaultReleaseNotesLabel(t: Translate) {
  return t(
    "This update includes reliability improvements, bug fixes, and interface polish.",
  );
}

const INTERNAL_RELEASE_NOTE_PATTERNS = [
  /\b(?:TAURI|APPLE|HEADROOM|GH|GITHUB)_[A-Z0-9_]+\b/,
  /\b(?:private key|signing secret|desktop artifacts?)\b/i,
];

type AppUpdateDisplayOptions = {
  debug?: boolean;
};

const FAILURE_PREFIXES: Record<AppUpdateOperation, string> = {
  check: "Could not check for updates",
  download: "Could not download the update",
  install: "Could not install the update",
  relaunch: "Could not restart Halv",
};

// The update commands reject with these sentences from Rust.
const COMMAND_ERRORS = {
  unsupported: "This build of Halv cannot install updates.",
  notReady: "No downloaded Halv update is ready to install.",
  busy: "Halv is already downloading or installing an update.",
  noRelease: "No Halv update is available.",
} as const;

export function isInternalReleaseNote(notes: string) {
  return INTERNAL_RELEASE_NOTE_PATTERNS.some((pattern) => pattern.test(notes));
}

function updaterErrorMessage(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, " ").trim().slice(0, 220);
}

export function appUpdateFailureMessage(
  error: unknown,
  operation: AppUpdateOperation,
  { debug = false }: AppUpdateDisplayOptions = {},
) {
  const detail = updaterErrorMessage(error);
  const normalized = detail.toLowerCase();

  if (
    (normalized.includes("target") &&
      (normalized.includes("not found") ||
        normalized.includes("not in the archive"))) ||
    normalized.includes("none of the fallback platforms")
  ) {
    return "This release does not include an update for this system.";
  }
  if (
    normalized.includes("unsupported application architecture") ||
    normalized.includes("unsupported os")
  ) {
    return "Automatic updates are not supported on this system.";
  }
  if (
    normalized.includes("error sending request") ||
    normalized.includes("network") ||
    normalized.includes("timed out") ||
    normalized.includes("timeout") ||
    normalized.includes("connection")
  ) {
    return "Could not reach the update server. Check your connection and try again.";
  }
  if (
    normalized.includes("404") ||
    normalized.includes("release not found") ||
    normalized.includes("could not fetch a valid release json")
  ) {
    return "Update information is temporarily unavailable. Try again later.";
  }
  if (
    normalized.includes("signature") ||
    normalized.includes("minisign") ||
    normalized.includes("base64") ||
    normalized.includes("invalid byte") ||
    normalized.includes("invalid length") ||
    normalized.includes("public key") ||
    normalized.includes("pubkey")
  ) {
    return operation === "check"
      ? "The update information is invalid. Try again later."
      : "Update verification failed, so Halv was not changed.";
  }
  if (
    normalized.includes("admin privileges") ||
    normalized.includes("permission denied") ||
    normalized.includes("access is denied")
  ) {
    return "Halv could not install the update with the current system permissions.";
  }

  const prefix = FAILURE_PREFIXES[operation];
  return detail && debug ? `${prefix}: ${detail}` : `${prefix}. Try again later.`;
}


export function appUpdateFailureLabel(message: string, t: Translate) {
  switch (message) {
    case "This release does not include an update for this system.":
      return t("This release does not include an update for this system.");
    case "Automatic updates are not supported on this system.":
      return t("Automatic updates are not supported on this system.");
    case "Could not reach the update server. Check your connection and try again.":
      return t(
        "Could not reach the update server. Check your connection and try again.",
      );
    case "Update information is temporarily unavailable. Try again later.":
      return t("Update information is temporarily unavailable. Try again later.");
    case "Update verification failed, so Halv was not changed.":
      return t("Update verification failed, so Halv was not changed.");
    case "The update information is invalid. Try again later.":
      return t("The update information is invalid. Try again later.");
    case "Halv could not install the update with the current system permissions.":
      return t(
        "Halv could not install the update with the current system permissions.",
      );
    case "Could not install the update. Try again later.":
      return t("Could not install the update. Try again later.");
    case "Could not check for updates. Try again later.":
      return t("Could not check for updates. Try again later.");
    case "Could not download the update. Try again later.":
      return t("Could not download the update. Try again later.");
    case "Could not restart Halv. Try again later.":
      return t("Could not restart Halv. Try again later.");
  }
  const installPrefix = "Could not install the update: ";
  if (message.startsWith(installPrefix)) {
    return t("Could not install the update: {detail}", {
      detail: message.slice(installPrefix.length),
    });
  }
  const checkPrefix = "Could not check for updates: ";
  if (message.startsWith(checkPrefix)) {
    return t("Could not check for updates: {detail}", {
      detail: message.slice(checkPrefix.length),
    });
  }
  const downloadPrefix = "Could not download the update: ";
  if (message.startsWith(downloadPrefix)) {
    return t("Could not download the update: {detail}", {
      detail: message.slice(downloadPrefix.length),
    });
  }
  const relaunchPrefix = "Could not restart Halv: ";
  if (message.startsWith(relaunchPrefix)) {
    return t("Could not restart Halv: {detail}", {
      detail: message.slice(relaunchPrefix.length),
    });
  }
  return message;
}

/** Translates the error of an update command. */
export function appUpdateCommandErrorLabel(error: unknown, t: Translate) {
  const message = error instanceof Error ? error.message : String(error);
  switch (message) {
    case COMMAND_ERRORS.unsupported:
      return t("This build of Halv cannot install updates.");
    case COMMAND_ERRORS.notReady:
      return t("No downloaded Halv update is ready to install.");
    case COMMAND_ERRORS.busy:
      return t("Halv is already downloading or installing an update.");
    case COMMAND_ERRORS.noRelease:
      return t("No Halv update is available.");
  }
  return message.trim() || t("Something went wrong. Try again.");
}

import { ipc } from "./ipc.ts";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import type { AppPreferences } from "./types";

let permissionRequested = false;
let permissionGranted: boolean | null = null;
let preferencesPromise: Promise<AppPreferences | null> | null = null;

export type NotificationKind = "completion" | "permission" | "question";

export function setNotificationPreferencesCache(preferences: AppPreferences) {
  preferencesPromise = Promise.resolve(preferences);
}

function notificationPreferences() {
  if (!preferencesPromise) {
    preferencesPromise = ipc.preferences.get().catch(() => null);
  }
  return preferencesPromise;
}

// `focused` means the user sees the subject: the focused window for chat,
// and the focused window with that pane active for a terminal pane.
export function notificationEnabled(
  preferences: AppPreferences | null,
  kind: NotificationKind,
  focused: boolean,
) {
  if (!preferences) {
    return !focused;
  }
  if (kind === "permission") {
    return preferences.permissionNotifications && !focused;
  }
  if (kind === "question") {
    return preferences.questionNotifications && !focused;
  }
  if (preferences.completionNotifications === "never") {
    return false;
  }
  return preferences.completionNotifications === "always" || !focused;
}

async function canNotify() {
  if (permissionGranted === true) {
    return true;
  }

  try {
    const alreadyGranted = await isPermissionGranted();
    if (alreadyGranted) {
      permissionGranted = true;
      return true;
    }
    if (permissionRequested) {
      return false;
    }

    permissionRequested = true;
    const permission = await requestPermission();
    permissionGranted = permission === "granted";
    return permissionGranted;
  } catch {
    return false;
  }
}

export async function notify(
  title: string,
  body: string,
  kind: NotificationKind = "completion",
) {
  try {
    const preferences = await notificationPreferences();
    if (!notificationEnabled(preferences, kind, document.hasFocus())) {
      return;
    }
    if (!(await canNotify())) {
      return;
    }
    sendNotification({ title, body });
  } catch {
    // Notifications are best effort; browser dev mode and denied OS APIs should not interrupt chat.
  }
}

/**
 * Notifies about a terminal pane that the user does not see. Rust shows the
 * banner, so a click on it can focus the pane.
 */
export async function notifyTerminalPane(
  sessionId: string,
  title: string,
  body: string,
  kind: NotificationKind,
) {
  try {
    const preferences = await notificationPreferences();
    if (!notificationEnabled(preferences, kind, false)) {
      return;
    }
    await ipc.terminal.notify({ sessionId, title, body });
  } catch {
    // A failed banner must not interrupt the terminal.
  }
}

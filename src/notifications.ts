import { ipc } from "./ipc.ts";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import type { AppPreferences, NotificationPermissionState } from "./types";

let preferencesPromise: Promise<AppPreferences | null> | null = null;

/**
 * "completion" is a finished turn, "error" a failed turn, "permission" an
 * approval wait, and "question" any other wait for the user.
 */
export type NotificationKind =
  | "completion"
  | "error"
  | "permission"
  | "question";

export type { NotificationPermissionState };

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
  if (kind === "error") {
    // A failure needs the user, so "never" for finished turns keeps it.
    return preferences.completionNotifications === "always" || !focused;
  }
  if (preferences.completionNotifications === "never") {
    return false;
  }
  return preferences.completionNotifications === "always" || !focused;
}

/**
 * The fix that the Settings notice offers for a macOS state, or null when
 * nothing needs a fix.
 */
export function notificationPermissionAction(
  state: NotificationPermissionState,
): "request" | "openSettings" | null {
  if (state === "notDetermined") return "request";
  if (state === "denied" || state === "bannersOff") return "openSettings";
  return null;
}

/**
 * The time between a click on "Send test notification" and the banner.
 * macOS shows no banner while Halv is the frontmost app, and the user clicks
 * the button inside Halv. The delay lets the user put another app in front.
 */
export const TEST_NOTIFICATION_DELAY_MS = 5_000;

/**
 * Sends the Settings test banner after TEST_NOTIFICATION_DELAY_MS. Halv does
 * not override shouldPresentNotification, because the "background" setting
 * depends on macOS hiding banners while Halv is in front.
 */
export async function sendTestNotificationLater({
  send,
  wait = (ms) => new Promise<void>((resolve) => setTimeout(resolve, ms)),
}: {
  send: () => Promise<void>;
  wait?: (ms: number) => Promise<void>;
}) {
  await wait(TEST_NOTIFICATION_DELAY_MS);
  await send();
}

export type DeliveryGateDependencies = {
  read(): Promise<NotificationPermissionState>;
  request(): Promise<NotificationPermissionState>;
  /** The answer of the notification plugin, for a state that is unknown. */
  fallback(): Promise<boolean>;
};

/**
 * True when a banner can reach the user. The plugin reports "granted" on
 * desktop even when macOS blocks Halv, so the real macOS state decides.
 * "bannersOff" still sends, because Notification Center keeps the entry.
 */
export async function allowsDelivery({
  read,
  request,
  fallback,
}: DeliveryGateDependencies) {
  let state: NotificationPermissionState;
  try {
    state = await read();
  } catch {
    state = "unknown";
  }
  if (state === "notDetermined") {
    try {
      state = await request();
    } catch {
      state = "unknown";
    }
  }
  if (state === "unknown" || state === "notDetermined") {
    return fallback();
  }
  return state !== "denied";
}

let pluginPermissionRequested = false;
let pluginPermissionGranted: boolean | null = null;

async function pluginAllows() {
  if (pluginPermissionGranted === true) {
    return true;
  }
  try {
    if (await isPermissionGranted()) {
      pluginPermissionGranted = true;
      return true;
    }
    if (pluginPermissionRequested) {
      return false;
    }
    pluginPermissionRequested = true;
    pluginPermissionGranted = (await requestPermission()) === "granted";
    return pluginPermissionGranted;
  } catch {
    return false;
  }
}

/** Reads the macOS state of Halv notifications. */
export function readNotificationPermission() {
  return ipc.notification.permissionState();
}

function canNotify() {
  return allowsDelivery({
    read: readNotificationPermission,
    request: () => ipc.notification.requestPermission(),
    fallback: pluginAllows,
  });
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
  /** Names the pane signal. Rust writes it to the banner log. */
  trace?: string,
) {
  try {
    const preferences = await notificationPreferences();
    if (!notificationEnabled(preferences, kind, false)) {
      return;
    }
    await ipc.terminal.notify({ sessionId, title, body, kind, trace });
  } catch {
    // A failed banner must not interrupt the terminal.
  }
}

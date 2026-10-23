import { useCallback, useEffect, useRef, useState } from "react";
import { ipc } from "../ipc.ts";
import {
  notificationPermissionAction,
  sendTestNotificationLater,
} from "../notifications";
import { useT, type Translate } from "../i18n/runtime";
import type { NotificationPermissionState } from "../types";

// The test banner needs a moment to reach macOS before the state is read again.
const TEST_SETTLE_MS = 1_200;

const buttonClass =
  "rounded-md border border-surface-border/30 px-3 py-2 text-sm text-content-secondary transition-colors duration-150 hover:bg-surface-raised hover:text-content-primary focus:outline-none focus:ring-2 focus:ring-accent/30 disabled:cursor-not-allowed disabled:opacity-60";

type Pending = "request" | "settings" | "test" | null;

/**
 * Shows what macOS lets Halv notify, and offers the fix. The notification
 * plugin reports "granted" even when macOS blocks Halv, so without this
 * notice a blocked user sees no banner and no reason.
 */
export function NotificationPermissionNotice() {
  const t = useT();
  const [state, setState] = useState<NotificationPermissionState | null>(null);
  const [pending, setPending] = useState<Pending>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await ipc.notification.permissionState();
      if (mounted.current) setState(next);
      return next;
    } catch {
      if (mounted.current) setState("unknown");
      return "unknown" as const;
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    // The user changes the setting in System Settings, then comes back.
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => {
      mounted.current = false;
      window.removeEventListener("focus", onFocus);
    };
  }, [refresh]);

  async function run(kind: Exclude<Pending, null>, action: () => Promise<void>) {
    setPending(kind);
    setError(null);
    setNotice(null);
    try {
      await action();
    } catch (cause) {
      if (mounted.current) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      if (mounted.current) setPending(null);
    }
  }

  const requestPermission = () =>
    run("request", async () => {
      const next = await ipc.notification.requestPermission();
      if (mounted.current) setState(next);
    });

  const openSettings = () =>
    run("settings", () => ipc.notification.openSystemSettings());

  const sendTest = () =>
    run("test", async () => {
      // macOS shows no banner while Halv is in front, so the banner waits
      // until the user puts another app in front.
      setNotice(
        t(
          "macOS shows Halv banners only while another app is in front. Switch to another app now. The test banner arrives in 5 seconds.",
        ),
      );
      // An empty pane id makes a banner that only brings Halv forward.
      // The send does not depend on this view: the user may close Settings.
      await sendTestNotificationLater({
        send: () =>
          ipc.terminal.notify({
            sessionId: "",
            title: t("Halv test notification"),
            body: t("If you see this banner, Halv notifications work."),
            kind: "test",
            trace: "settings-test",
          }),
      });
      await new Promise((resolve) => setTimeout(resolve, TEST_SETTLE_MS));
      const next = await refresh();
      if (!mounted.current) return;
      setNotice(
        notificationPermissionAction(next) === null
          ? t(
              "Test notification sent. If no banner appeared while another app was in front, check System Settings, Notifications, Halv.",
            )
          : null,
      );
    });

  const action = state ? notificationPermissionAction(state) : null;
  const message = state ? stateMessage(state, t) : null;

  return (
    <div
      className="space-y-2 rounded-md border border-surface-border/30 p-3"
      data-testid="notification-permission-notice"
    >
      {message ? (
        <p
          className={`text-sm ${
            state === "denied" ? "text-danger" : "text-content-secondary"
          }`}
          data-testid="notification-permission-message"
          role={state === "denied" ? "alert" : "status"}
        >
          {message}
        </p>
      ) : null}
      <div className="flex flex-wrap gap-2">
        {action === "request" ? (
          <button
            className={buttonClass}
            data-testid="notification-permission-request"
            disabled={pending !== null}
            onClick={() => void requestPermission()}
            type="button"
          >
            {t("Allow notifications")}
          </button>
        ) : null}
        {action === "openSettings" ? (
          <button
            className={buttonClass}
            data-testid="notification-permission-open-settings"
            disabled={pending !== null}
            onClick={() => void openSettings()}
            type="button"
          >
            {t("Open System Settings")}
          </button>
        ) : null}
        <button
          className={buttonClass}
          data-testid="notification-permission-test"
          disabled={pending !== null}
          onClick={() => void sendTest()}
          type="button"
        >
          {pending === "test"
            ? t("Sending...")
            : t("Send test notification")}
        </button>
      </div>
      {notice ? (
        <p className="text-xs text-accent" role="status">
          {notice}
        </p>
      ) : null}
      {error ? (
        <p className="text-xs text-danger" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

function stateMessage(state: NotificationPermissionState, t: Translate) {
  switch (state) {
    case "granted":
      return t("macOS allows notifications from Halv.");
    case "denied":
      return t(
        "macOS blocks notifications from Halv, so no banner or sound appears. Turn on Allow Notifications for Halv in System Settings.",
      );
    case "notDetermined":
      return t("macOS has not asked yet whether Halv may send notifications.");
    case "bannersOff":
      return t(
        "macOS allows Halv notifications, but the alert style is None, so no banner appears. Choose Banners or Alerts in System Settings.",
      );
    case "unknown":
      return null;
  }
}

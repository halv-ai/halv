// The update card and the status bar segment as pure data. Rust owns the
// update state (ADR 0012); these functions only decide what the views show.
// The layout follows Orca's UpdateCard (stablyai/orca, MIT).
import {
  appUpdateCommandErrorLabel,
  appUpdateDefaultReleaseNotesLabel,
  appUpdateFailureLabel,
  appUpdateFailureMessage,
  isInternalReleaseNote,
} from "./appUpdater.ts";
import type { Translate } from "./i18n/runtime.tsx";
import type {
  AppUpdateInstallMode,
  AppUpdateRestartBlockers,
  AppUpdateSnapshot,
  AppUpdateStatus,
} from "./types";

export const APP_UPDATE_UP_TO_DATE_HIDE_MS = 3_000;
// Pixels from the window edges. The status bar is 24 px high, so the card
// keeps a 12 px gap above it.
export const APP_UPDATE_CARD_LAYOUT = {
  right: 16,
  bottom: 36,
  gap: 12,
  width: 360,
  narrowWidth: 480,
} as const;
export const APP_UPDATE_POST_UPDATE_HIDE_MS = 6_000;
const NOTES_MAX_ITEMS = 3;
const NOTES_MAX_LENGTH = 220;

/** View state that only the webview keeps. Keys come from appUpdateCardKey. */
export type AppUpdateCardUi = {
  // The user hid the card for this key. A new key shows the card again.
  minimizedKey: string | null;
  // The user opened the card for this key, also when "Later" hid it.
  revealedKey: string | null;
};

export const APP_UPDATE_CARD_UI_DEFAULT: AppUpdateCardUi = {
  minimizedKey: null,
  revealedKey: null,
};

export type AppUpdateCardKind =
  | "checking"
  | "upToDate"
  | "available"
  | "downloading"
  | "ready"
  | "blockers"
  | "waiting"
  | "installing"
  | "restarting"
  | "error"
  | "postUpdate";

export type AppUpdateCardActionId =
  | "download"
  | "later"
  | "hide"
  | "restart"
  | "restartWhenDone"
  | "restartAnyway"
  | "cancelRestart"
  | "retry"
  | "dismiss";

export type AppUpdateCardAction = {
  id: AppUpdateCardActionId;
  label: string;
  emphasis: "primary" | "secondary" | "quiet";
};

export type AppUpdateCardModel = {
  key: string;
  kind: AppUpdateCardKind;
  tone: "neutral" | "accent" | "warning" | "danger";
  title: string | null;
  body: string | null;
  notes: string[];
  list: string[];
  progress: { percent: number | null; label: string } | null;
  busy: boolean;
  actions: AppUpdateCardAction[];
  link: { label: string; url: string } | null;
  // The close button minimizes the card, or clears the post-update note.
  close: "minimize" | "dismissPostUpdate" | null;
  alert: string | null;
  autoHideMs: number | null;
};

export type AppUpdateSegmentModel = {
  label: string;
  title: string;
  tone: "accent" | "danger" | "neutral";
};

type NumberFormatter = (value: number, options?: Intl.NumberFormatOptions) => string;

export type AppUpdateCardDeps = {
  t: Translate;
  formatNumber?: NumberFormatter;
  debug?: boolean;
  // The rejection of the last card action, if any.
  actionError?: unknown;
};

const defaultFormatNumber: NumberFormatter = (value, options) =>
  new Intl.NumberFormat("en-US", options).format(value);

type VersionedStatus = Extract<AppUpdateStatus, { version: string }>;

function statusVersion(status: AppUpdateStatus): string | null {
  return "version" in status && typeof status.version === "string"
    ? status.version
    : null;
}

// Background checks and their results do not show a card.
function statusIsQuiet(status: AppUpdateStatus) {
  return (
    status.state === "idle" ||
    ((status.state === "checking" || status.state === "upToDate") &&
      !status.userInitiated)
  );
}

function hasWorkInProgress(blockers: AppUpdateRestartBlockers) {
  return blockers.chatReplies + blockers.guiAgentSessions > 0;
}

/**
 * Names what the card shows. A minimize or a reveal applies only while
 * the key stays the same, so a new step shows the card again.
 */
export function appUpdateCardKey(snapshot: AppUpdateSnapshot): string | null {
  const { status } = snapshot;
  if (statusIsQuiet(status)) {
    return snapshot.postUpdate ? `postUpdate:${snapshot.postUpdate.toVersion}` : null;
  }
  switch (status.state) {
    case "checking":
      return "checking";
    case "upToDate":
      return `upToDate:${snapshot.lastCheckedAt ?? ""}`;
    case "ready":
      if (snapshot.restartWhenIdle) return `waiting:${status.version}`;
      if (snapshot.blockers) return `blockers:${status.version}`;
      return `ready:${status.version}`;
    case "error":
      return `error:${status.operation}:${status.version ?? ""}`;
    default:
      return `${status.state}:${statusVersion(status) ?? ""}`;
  }
}

/** "Later" hides the card for this version until the next start. */
function deferredByLater(snapshot: AppUpdateSnapshot) {
  const { status } = snapshot;
  if (
    status.state !== "available" &&
    status.state !== "downloading" &&
    status.state !== "ready"
  ) {
    return false;
  }
  if (snapshot.blockers || snapshot.restartWhenIdle) return false;
  return snapshot.deferredVersion === status.version;
}

export function isAppUpdateCardVisible(
  snapshot: AppUpdateSnapshot,
  ui: AppUpdateCardUi,
) {
  const key = appUpdateCardKey(snapshot);
  if (key === null) return false;
  const { status } = snapshot;
  // The user asked for the restart, so the install always shows.
  if (status.state === "installing" || status.state === "restarting") return true;
  if (ui.minimizedKey === key) return false;
  if (ui.revealedKey === key) return true;
  // A failed background download shows only in the status bar.
  if (status.state === "error" && !status.userInitiated) return false;
  return !deferredByLater(snapshot);
}

/** The first bullets of the Markdown release notes, as plain text. */
export function appUpdateNotesExcerpt(
  notes: string | null,
  { t, debug = false }: Pick<AppUpdateCardDeps, "t" | "debug">,
  maxItems = NOTES_MAX_ITEMS,
): string[] {
  const text = notes?.trim();
  if (!text) return [];
  if (!debug && isInternalReleaseNote(text)) {
    return [appUpdateDefaultReleaseNotesLabel(t)];
  }
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !/^#{1,6}\s/.test(line));
  const bullets = lines
    .map((line) => /^(?:[-*+]|\d+[.)])\s+(.+)$/.exec(line)?.[1])
    .filter((line): line is string => Boolean(line));
  const items = bullets.length > 0 ? bullets : [lines.join(" ")];
  return items
    .slice(0, maxItems)
    .map((item) => truncate(plainMarkdown(item), NOTES_MAX_LENGTH))
    .filter(Boolean);
}

function plainMarkdown(text: string) {
  return text
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\*\*|__|`/g, "")
    .replace(/\*(\S[^*]*)\*/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

function truncate(text: string, maxLength: number) {
  return text.length <= maxLength
    ? text
    : `${text.slice(0, maxLength - 1).trimEnd()}…`;
}

function megabytes(bytes: number, formatNumber: NumberFormatter) {
  return formatNumber(bytes / 1_000_000, {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  });
}

export function appUpdateProgressPercent(status: AppUpdateStatus) {
  if (status.state !== "downloading" || !status.totalBytes) return null;
  return Math.min(
    100,
    Math.max(0, Math.round((status.downloadedBytes / status.totalBytes) * 100)),
  );
}

export function appUpdateProgressLabel(
  status: Extract<AppUpdateStatus, { state: "downloading" }>,
  t: Translate,
  formatNumber: NumberFormatter = defaultFormatNumber,
) {
  const percent = appUpdateProgressPercent(status);
  const done = megabytes(status.downloadedBytes, formatNumber);
  if (percent === null || status.totalBytes === null) {
    return t("{done} MB downloaded", { done });
  }
  return t("{percent}% · {done} of {total} MB", {
    percent: formatNumber(percent),
    done,
    total: megabytes(status.totalBytes, formatNumber),
  });
}

/** Download and install work only where Halv can replace itself. */
export function appUpdateCanInstall(installMode: AppUpdateInstallMode) {
  return installMode !== "unsupported" && installMode !== "readOnly";
}

/** The sentence that says what a restart does to the running sessions. */
export function appUpdateRestartCopy(
  snapshot: Pick<AppUpdateSnapshot, "installMode" | "sessionsSurviveRestart">,
  t: Translate,
) {
  if (snapshot.installMode === "installer") {
    return t("Halv closes to install the update. Open terminals and agents will restart.");
  }
  // The supervisor is not available, so the terminals run inside Halv.
  if (!snapshot.sessionsSurviveRestart) {
    return t("Open terminals and agents close when Halv restarts.");
  }
  return t("Your agent sessions keep running while Halv restarts.");
}

/** Why this copy of Halv cannot install the update itself. */
export function appUpdateCannotInstallCopy(installMode: AppUpdateInstallMode, t: Translate) {
  return installMode === "readOnly"
    ? t("Halv cannot update itself in this folder. Move Halv to your Applications folder, then open it again.")
    : t("This build of Halv cannot install updates.");
}

/** One line for each kind of work that a restart stops. */
export function appUpdateBlockerLines(
  blockers: AppUpdateRestartBlockers,
  t: Translate,
  { includeTerminals = true }: { includeTerminals?: boolean } = {},
) {
  const lines: string[] = [];
  if (blockers.chatReplies === 1) lines.push(t("1 chat reply"));
  else if (blockers.chatReplies > 1) {
    lines.push(t("{count} chat replies", { count: blockers.chatReplies }));
  }
  if (blockers.guiAgentSessions === 1) lines.push(t("1 agent turn"));
  else if (blockers.guiAgentSessions > 1) {
    lines.push(t("{count} agent turns", { count: blockers.guiAgentSessions }));
  }
  if (includeTerminals) {
    if (blockers.inProcessTerminals === 1) lines.push(t("1 terminal inside Halv"));
    else if (blockers.inProcessTerminals > 1) {
      lines.push(
        t("{count} terminals inside Halv", { count: blockers.inProcessTerminals }),
      );
    }
  }
  return lines;
}

function errorTitle(operation: Extract<AppUpdateStatus, { state: "error" }>["operation"], t: Translate) {
  switch (operation) {
    case "check":
      return t("Could not check for updates");
    case "download":
      return t("The update did not download");
    case "install":
      return t("The update did not install");
    case "relaunch":
      return t("Halv did not restart");
  }
}

function whatsNew(url: string, t: Translate) {
  return { label: t("What's new"), url };
}

type CardBase = Omit<AppUpdateCardModel, "kind" | "tone" | "title">;

function cardBase(key: string, alert: string | null): CardBase {
  return {
    key,
    body: null,
    notes: [],
    list: [],
    progress: null,
    busy: false,
    actions: [],
    link: null,
    close: "minimize",
    alert,
    autoHideMs: null,
  };
}

/** Builds the card for a snapshot. Visibility is a separate decision. */
export function appUpdateCardModel(
  snapshot: AppUpdateSnapshot,
  deps: AppUpdateCardDeps,
): AppUpdateCardModel | null {
  const key = appUpdateCardKey(snapshot);
  if (key === null) return null;
  const { t, debug = false, formatNumber = defaultFormatNumber } = deps;
  const alert =
    deps.actionError === undefined || deps.actionError === null
      ? null
      : appUpdateCommandErrorLabel(deps.actionError, t);
  const base = cardBase(key, alert);
  const { status } = snapshot;

  if (statusIsQuiet(status) && snapshot.postUpdate) {
    const { postUpdate } = snapshot;
    const count = postUpdate.reconnectedSessions;
    return {
      ...base,
      kind: "postUpdate",
      tone: "accent",
      title: t("Updated to Halv {version}", { version: postUpdate.toVersion }),
      body:
        count === 1
          ? t("1 agent session reconnected.")
          : count > 1
            ? t("{count} agent sessions reconnected.", { count: formatNumber(count) })
            : null,
      link: whatsNew(postUpdate.releaseUrl, t),
      close: "dismissPostUpdate",
      autoHideMs: APP_UPDATE_POST_UPDATE_HIDE_MS,
    };
  }

  switch (status.state) {
    case "idle":
      return null;
    case "checking":
      return {
        ...base,
        kind: "checking",
        tone: "neutral",
        title: null,
        body: t("Checking for updates…"),
        busy: true,
      };
    case "upToDate":
      return {
        ...base,
        kind: "upToDate",
        tone: "accent",
        title: t("Halv is up to date."),
        body: t("You have version {version}.", { version: snapshot.currentVersion }),
        autoHideMs: APP_UPDATE_UP_TO_DATE_HIDE_MS,
      };
    case "available": {
      const canInstall = appUpdateCanInstall(snapshot.installMode);
      return {
        ...base,
        kind: "available",
        tone: "accent",
        title: t("Halv {version} is available", { version: status.version }),
        body: canInstall ? null : appUpdateCannotInstallCopy(snapshot.installMode, t),
        notes: appUpdateNotesExcerpt(status.notes, { t, debug }),
        actions: canInstall
          ? [
              { id: "download", label: t("Download"), emphasis: "primary" },
              { id: "later", label: t("Later"), emphasis: "quiet" },
            ]
          : [{ id: "later", label: t("Later"), emphasis: "quiet" }],
        link: whatsNew(status.releaseUrl, t),
      };
    }
    case "downloading":
      return {
        ...base,
        kind: "downloading",
        tone: "accent",
        title: t("Downloading Halv {version}", { version: status.version }),
        body: t("Keep working. Halv asks before it restarts."),
        progress: {
          percent: appUpdateProgressPercent(status),
          label: appUpdateProgressLabel(status, t, formatNumber),
        },
        actions: [{ id: "hide", label: t("Hide"), emphasis: "quiet" }],
      };
    case "ready":
      return readyModel(snapshot, status, base, t, debug);
    case "installing":
      return {
        ...base,
        kind: "installing",
        tone: "accent",
        title: t("Installing Halv {version}…", { version: status.version }),
        body:
          status.trigger === "quit"
            ? t("Halv quits when the install is done.")
            : t("Halv restarts in a moment."),
        busy: true,
        close: null,
      };
    case "restarting":
      return {
        ...base,
        kind: "restarting",
        tone: "accent",
        title: t("Restarting Halv…"),
        busy: true,
        close: null,
      };
    case "error":
      return {
        ...base,
        kind: "error",
        tone: "danger",
        title: errorTitle(status.operation, t),
        body: appUpdateFailureLabel(
          appUpdateFailureMessage(status.message, status.operation, { debug }),
          t,
        ),
        actions: [
          ...(status.retryable
            ? [{ id: "retry", label: t("Try again"), emphasis: "primary" } as const]
            : []),
          { id: "dismiss", label: t("Dismiss"), emphasis: "quiet" },
        ],
      };
  }
}

function readyModel(
  snapshot: AppUpdateSnapshot,
  status: Extract<VersionedStatus, { state: "ready" }>,
  base: CardBase,
  t: Translate,
  debug: boolean,
): AppUpdateCardModel {
  const { blockers } = snapshot;
  if (snapshot.restartWhenIdle) {
    return {
      ...base,
      kind: "waiting",
      tone: "accent",
      title: t("Halv restarts when the work is done"),
      body: t("Waiting for:"),
      list: blockers ? appUpdateBlockerLines(blockers, t, { includeTerminals: false }) : [],
      busy: true,
      actions: [
        { id: "restartAnyway", label: t("Restart now"), emphasis: "secondary" },
        { id: "cancelRestart", label: t("Cancel"), emphasis: "quiet" },
      ],
    };
  }
  if (blockers) {
    const canWait = hasWorkInProgress(blockers);
    return {
      ...base,
      kind: "blockers",
      tone: "warning",
      title: t("Work is still running"),
      body: t("Restarting now stops this work:"),
      list: appUpdateBlockerLines(blockers, t),
      actions: canWait
        ? [
            { id: "restartWhenDone", label: t("Restart when done"), emphasis: "primary" },
            { id: "restartAnyway", label: t("Restart now"), emphasis: "secondary" },
            { id: "cancelRestart", label: t("Cancel"), emphasis: "quiet" },
          ]
        : [
            { id: "restartAnyway", label: t("Restart now"), emphasis: "primary" },
            { id: "cancelRestart", label: t("Cancel"), emphasis: "quiet" },
          ],
    };
  }
  return {
    ...base,
    kind: "ready",
    tone: "accent",
    title: t("Halv {version} is ready", { version: status.version }),
    body: appUpdateRestartCopy(snapshot, t),
    notes: appUpdateNotesExcerpt(status.notes, { t, debug }),
    actions: [
      { id: "restart", label: t("Restart now"), emphasis: "primary" },
      { id: "later", label: t("Later"), emphasis: "quiet" },
    ],
    link: whatsNew(status.releaseUrl, t),
  };
}

/**
 * The status bar segment. It stays while the update downloads, is ready,
 * installs, or failed, so a hidden card is never lost.
 */
export function appUpdateStatusSegment(
  snapshot: AppUpdateSnapshot,
  { t, formatNumber = defaultFormatNumber }: Pick<AppUpdateCardDeps, "t" | "formatNumber">,
): AppUpdateSegmentModel | null {
  const { status } = snapshot;
  switch (status.state) {
    case "downloading": {
      const percent = appUpdateProgressPercent(status);
      return {
        label:
          percent === null
            ? t("↓ {done} MB", { done: megabytes(status.downloadedBytes, formatNumber) })
            : `↓ ${formatNumber(percent / 100, { style: "percent" })}`,
        title: t("Downloading Halv {version}", { version: status.version }),
        tone: "neutral",
      };
    }
    case "ready":
      return {
        label: t("Update ready"),
        title: t("Halv {version} is ready", { version: status.version }),
        tone: "accent",
      };
    case "installing":
    case "restarting":
      return {
        label: t("Installing update…"),
        title: t("Installing Halv {version}…", { version: status.version }),
        tone: "accent",
      };
    case "error":
      return {
        label: t("Update failed"),
        title: errorTitle(status.operation, t),
        tone: "danger",
      };
    default:
      return null;
  }
}

export type AppUpdateSettingsStatus = {
  text: string;
  tone: "muted" | "accent" | "danger";
};

/** One line for Settings > System > Updates. */
export function appUpdateSettingsStatus(
  snapshot: AppUpdateSnapshot,
  { t, debug = false }: Pick<AppUpdateCardDeps, "t" | "debug">,
): AppUpdateSettingsStatus | null {
  const { status } = snapshot;
  switch (status.state) {
    case "idle":
      return null;
    case "checking":
      return { text: t("Checking for updates…"), tone: "muted" };
    case "upToDate":
      return { text: t("Halv is up to date."), tone: "muted" };
    case "available":
      return {
        text: t("Version {version} is available.", { version: status.version }),
        tone: "accent",
      };
    case "downloading":
      return {
        text: t("Downloading Halv {version}", { version: status.version }),
        tone: "muted",
      };
    case "ready":
      return {
        text: t("Halv {version} is ready. Restart to finish the update.", {
          version: status.version,
        }),
        tone: "accent",
      };
    case "installing":
      return {
        text: t("Installing Halv {version}…", { version: status.version }),
        tone: "accent",
      };
    case "restarting":
      return { text: t("Restarting Halv…"), tone: "accent" };
    case "error":
      return {
        text: appUpdateFailureLabel(
          appUpdateFailureMessage(status.message, status.operation, { debug }),
          t,
        ),
        tone: "danger",
      };
  }
}

export type AppUpdateCardRect = {
  left: number;
  top: number;
  right: number;
  bottom: number;
};

/**
 * The bottom offset of the card. The card moves above the chat composer
 * when it would cover it, so the send button stays usable. When the card
 * cannot fit above the composer, it keeps its corner.
 */
export function appUpdateCardBottom(
  viewport: { width: number; height: number },
  cardHeight: number,
  avoid: AppUpdateCardRect | null,
) {
  const { right, bottom, gap, width, narrowWidth } = APP_UPDATE_CARD_LAYOUT;
  if (!avoid || avoid.right <= avoid.left || avoid.bottom <= avoid.top) return bottom;
  const cardWidth =
    viewport.width < narrowWidth
      ? viewport.width - 2 * right
      : Math.min(width, viewport.width - 2 * right);
  const cardRight = viewport.width - right;
  const cardBottom = viewport.height - bottom;
  const card = {
    left: cardRight - cardWidth,
    top: cardBottom - cardHeight,
    right: cardRight,
    bottom: cardBottom,
  };
  if (!rectsOverlap(card, avoid)) return bottom;
  const lifted = viewport.height - avoid.top + gap;
  return lifted + cardHeight + gap > viewport.height ? bottom : Math.max(bottom, lifted);
}

/**
 * The card gives way to an app menu that opens under it, such as the
 * composer command list ([data-update-card-yield]). Then the menu shows in
 * full, and a click on the menu never lands on the card.
 */
export function appUpdateCardYields(
  card: AppUpdateCardRect | null,
  menus: readonly AppUpdateCardRect[],
) {
  return card !== null && menus.some((menu) => rectsOverlap(card, menu));
}

function rectsOverlap(a: AppUpdateCardRect, b: AppUpdateCardRect) {
  return a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
}

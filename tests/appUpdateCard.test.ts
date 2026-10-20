import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  APP_UPDATE_CARD_UI_DEFAULT,
  APP_UPDATE_POST_UPDATE_HIDE_MS,
  APP_UPDATE_UP_TO_DATE_HIDE_MS,
  appUpdateBlockerLines,
  appUpdateCardBottom,
  appUpdateCardKey,
  appUpdateCardModel,
  appUpdateCardYields,
  appUpdateNotesExcerpt,
  appUpdateSettingsStatus,
  appUpdateStatusSegment,
  isAppUpdateCardVisible,
  type AppUpdateCardUi,
} from "../src/appUpdateCard.ts";
import type {
  AppUpdateSnapshot,
  AppUpdateStatus,
} from "../src/types.ts";

const t = (source: string, values?: Record<string, string | number>) =>
  source.replace(/\{(\w+)\}/g, (_, key: string) => String(values?.[key] ?? ""));

// The notes of the real 0.7.1 release (latest.json on halv-releases).
const NOTES_0_7_1 =
  "## What's new\n\n- Orca accounts: when Orca is installed, Halv uses the same Claude and Codex accounts as Orca. Add or switch an account in one app, and the other app follows.\n- New status bar at the bottom of the window. It shows the plan usage of each provider and the tokens Halv saved.\n- Select a provider in the status bar to see its usage windows and to switch, add, or manage accounts.\n";
const RELEASE_URL = "https://github.com/pedr0v/halv-releases/releases/tag/v0.7.2";

const READY: AppUpdateStatus = {
  state: "ready",
  version: "0.7.2",
  notes: NOTES_0_7_1,
  releaseUrl: RELEASE_URL,
};

function snapshot(change: Partial<AppUpdateSnapshot> = {}): AppUpdateSnapshot {
  return {
    status: { state: "idle" },
    currentVersion: "0.7.1",
    autoDownload: true,
    installMode: "replace",
    deferredVersion: null,
    restartWhenIdle: false,
    blockers: null,
    postUpdate: null,
    lastCheckedAt: "2026-10-14T12:00:00Z",
    sessionsSurviveRestart: true,
    ...change,
  };
}

function visible(value: AppUpdateSnapshot, ui: AppUpdateCardUi = APP_UPDATE_CARD_UI_DEFAULT) {
  return isAppUpdateCardVisible(value, ui);
}

function model(value: AppUpdateSnapshot, actionError?: unknown) {
  const result = appUpdateCardModel(value, { t, actionError });
  assert.ok(result, "the snapshot must have a card");
  return result;
}

test("background checks never show a card", () => {
  for (const status of [
    { state: "idle" },
    { state: "checking", userInitiated: false },
    { state: "upToDate", userInitiated: false },
  ] satisfies AppUpdateStatus[]) {
    assert.equal(appUpdateCardKey(snapshot({ status })), null);
    assert.equal(visible(snapshot({ status })), false);
    assert.equal(appUpdateCardModel(snapshot({ status }), { t }), null);
  }
});

test("a user check shows progress and then hides the up-to-date note after 3 seconds", () => {
  const checking = snapshot({ status: { state: "checking", userInitiated: true } });
  assert.equal(visible(checking), true);
  assert.equal(model(checking).body, "Checking for updates…");
  assert.equal(model(checking).busy, true);

  const upToDate = snapshot({ status: { state: "upToDate", userInitiated: true } });
  const card = model(upToDate);
  assert.equal(card.title, "Halv is up to date.");
  assert.equal(card.body, "You have version 0.7.1.");
  assert.equal(card.autoHideMs, APP_UPDATE_UP_TO_DATE_HIDE_MS);
  assert.equal(visible(upToDate, { minimizedKey: card.key, revealedKey: null }), false);
});

test("an available release offers the download, Later, and the release notes", () => {
  const available = snapshot({
    autoDownload: false,
    status: { ...READY, state: "available" },
  });
  const card = model(available);
  assert.equal(card.title, "Halv 0.7.2 is available");
  assert.deepEqual(
    card.actions.map((action) => action.id),
    ["download", "later"],
  );
  assert.deepEqual(card.link, { label: "What's new", url: RELEASE_URL });
  assert.equal(card.notes.length, 3);

  const devBuild = model({ ...available, installMode: "unsupported" });
  assert.equal(devBuild.body, "This build of Halv cannot install updates.");
  assert.deepEqual(devBuild.actions.map((action) => action.id), ["later"]);

  // A disk image or a read-only folder: the card asks the user to move Halv.
  const readOnly = model({ ...available, installMode: "readOnly" });
  assert.equal(
    readOnly.body,
    "Halv cannot update itself in this folder. Move Halv to your Applications folder, then open it again.",
  );
  assert.deepEqual(readOnly.actions.map((action) => action.id), ["later"]);
  assert.deepEqual(readOnly.link, { label: "What's new", url: RELEASE_URL });
});

test("the download shows percent and megabytes, or only megabytes without a size", () => {
  const downloading = snapshot({
    status: {
      state: "downloading",
      version: "0.7.2",
      downloadedBytes: 72_450_000,
      totalBytes: 176_883_936,
    },
  });
  const card = model(downloading);
  assert.equal(card.title, "Downloading Halv 0.7.2");
  assert.deepEqual(card.progress, { percent: 41, label: "41% · 72.5 of 176.9 MB" });
  assert.deepEqual(card.actions.map((action) => action.id), ["hide"]);

  const unknownSize = model(
    snapshot({
      status: {
        state: "downloading",
        version: "0.7.2",
        downloadedBytes: 1_250_000,
        totalBytes: null,
      },
    }),
  );
  assert.deepEqual(unknownSize.progress, { percent: null, label: "1.3 MB downloaded" });
});

test("the ready card says what a restart does on each platform", () => {
  const ready = snapshot({ status: READY });
  const card = model(ready);
  assert.equal(card.kind, "ready");
  assert.equal(card.title, "Halv 0.7.2 is ready");
  assert.equal(card.body, "Your agent sessions keep running while Halv restarts.");
  assert.deepEqual(card.actions.map((action) => action.id), ["restart", "later"]);
  assert.equal(model({ ...ready, installMode: "elevated" }).body, card.body);
  assert.equal(
    model({ ...ready, installMode: "installer", sessionsSurviveRestart: false }).body,
    "Halv closes to install the update. Open terminals and agents will restart.",
  );
  // Without the supervisor, the terminals run inside Halv and close.
  assert.equal(
    model({ ...ready, sessionsSurviveRestart: false }).body,
    "Open terminals and agents close when Halv restarts.",
  );
});

test("Later hides the card for that version, and a newer version shows again", () => {
  const ready = snapshot({ status: READY, deferredVersion: "0.7.2" });
  assert.equal(visible(ready), false);
  // The status bar segment stays, and a click reveals the card.
  assert.equal(appUpdateStatusSegment(ready, { t })?.label, "Update ready");
  assert.equal(
    visible(ready, { minimizedKey: null, revealedKey: appUpdateCardKey(ready) }),
    true,
  );
  assert.equal(
    visible({ ...ready, status: { ...READY, version: "0.7.3" } }),
    true,
  );
  // A blocked restart is an answer to the user, so Later does not hide it.
  assert.equal(
    visible({
      ...ready,
      blockers: { chatReplies: 1, guiAgentSessions: 0, inProcessTerminals: 0 },
    }),
    true,
  );
});

test("a hidden card comes back for the next step but not for progress", () => {
  const downloading = snapshot({
    status: { state: "downloading", version: "0.7.2", downloadedBytes: 10, totalBytes: 100 },
  });
  const ui = { minimizedKey: appUpdateCardKey(downloading), revealedKey: null };
  assert.equal(visible(downloading, ui), false);
  assert.equal(
    visible(
      {
        ...downloading,
        status: { state: "downloading", version: "0.7.2", downloadedBytes: 90, totalBytes: 100 },
      },
      ui,
    ),
    false,
  );
  assert.equal(visible(snapshot({ status: READY }), ui), true);
});

test("the install always shows, also after the user hid the card", () => {
  const installing = snapshot({
    status: { state: "installing", version: "0.7.2", trigger: "restart" },
  });
  const ui = { minimizedKey: appUpdateCardKey(installing), revealedKey: null };
  assert.equal(visible(installing, ui), true);
  assert.equal(model(installing).body, "Halv restarts in a moment.");
  assert.equal(model(installing).close, null);
  assert.equal(
    model({ ...installing, status: { state: "installing", version: "0.7.2", trigger: "quit" } })
      .body,
    "Halv quits when the install is done.",
  );
});

test("the blockers step waits for replies and agent turns but only names terminals", () => {
  const blockers = { chatReplies: 1, guiAgentSessions: 2, inProcessTerminals: 3 };
  const card = model(snapshot({ status: READY, blockers }));
  assert.equal(card.kind, "blockers");
  assert.equal(card.tone, "warning");
  assert.deepEqual(card.list, [
    "1 chat reply",
    "2 agent turns",
    "3 terminals inside Halv",
  ]);
  assert.deepEqual(
    card.actions.map((action) => [action.id, action.emphasis]),
    [
      ["restartWhenDone", "primary"],
      ["restartAnyway", "secondary"],
      ["cancelRestart", "quiet"],
    ],
  );

  const terminalsOnly = model(
    snapshot({
      status: READY,
      blockers: { chatReplies: 0, guiAgentSessions: 0, inProcessTerminals: 1 },
    }),
  );
  assert.deepEqual(terminalsOnly.list, ["1 terminal inside Halv"]);
  assert.deepEqual(
    terminalsOnly.actions.map((action) => action.id),
    ["restartAnyway", "cancelRestart"],
  );

  const waiting = model(snapshot({ status: READY, blockers, restartWhenIdle: true }));
  assert.equal(waiting.kind, "waiting");
  assert.equal(waiting.busy, true);
  assert.deepEqual(waiting.list, ["1 chat reply", "2 agent turns"]);
  assert.deepEqual(
    appUpdateBlockerLines(
      { chatReplies: 2, guiAgentSessions: 1, inProcessTerminals: 0 },
      t,
    ),
    ["2 chat replies", "1 agent turn"],
  );
});

test("errors map the raw message, offer a retry, and hide background failures", () => {
  const failed = snapshot({
    status: {
      state: "error",
      operation: "download",
      message: "the download timed out: no data arrived for 60 seconds",
      version: "0.7.2",
      retryable: true,
      userInitiated: true,
    },
  });
  const card = model(failed);
  assert.equal(card.title, "The update did not download");
  assert.equal(
    card.body,
    "Could not reach the update server. Check your connection and try again.",
  );
  assert.deepEqual(card.actions.map((action) => action.id), ["retry", "dismiss"]);
  assert.equal(visible(failed), true);

  const background = {
    ...failed,
    status: { ...failed.status, userInitiated: false } as AppUpdateStatus,
  };
  assert.equal(visible(background), false);
  assert.equal(appUpdateStatusSegment(background, { t })?.label, "Update failed");
  assert.equal(appUpdateStatusSegment(background, { t })?.tone, "danger");
});

test("an action error shows on the card in the user's words", () => {
  const card = model(
    snapshot({ status: READY }),
    "No downloaded Halv update is ready to install.",
  );
  assert.equal(card.alert, "No downloaded Halv update is ready to install.");
});

test("after a restart the card says how many agent sessions reconnected", () => {
  const updated = snapshot({
    currentVersion: "0.7.2",
    postUpdate: {
      fromVersion: "0.7.1",
      toVersion: "0.7.2",
      reconnectedSessions: 11,
      releaseUrl: RELEASE_URL,
    },
  });
  const card = model(updated);
  assert.equal(card.kind, "postUpdate");
  assert.equal(card.title, "Updated to Halv 0.7.2");
  assert.equal(card.body, "11 agent sessions reconnected.");
  assert.equal(card.close, "dismissPostUpdate");
  assert.equal(card.autoHideMs, APP_UPDATE_POST_UPDATE_HIDE_MS);
  assert.equal(visible(updated), true);

  const one = model({
    ...updated,
    postUpdate: { ...updated.postUpdate!, reconnectedSessions: 1 },
  });
  assert.equal(one.body, "1 agent session reconnected.");
  const none = model({
    ...updated,
    postUpdate: { ...updated.postUpdate!, reconnectedSessions: 0 },
  });
  assert.equal(none.body, null);

  // A newer release takes the card; the note waits behind it.
  assert.equal(model({ ...updated, status: READY }).kind, "ready");
});

test("the notes excerpt keeps the first three bullets of the real 0.7.1 notes", () => {
  const excerpt = appUpdateNotesExcerpt(NOTES_0_7_1, { t });
  assert.equal(excerpt.length, 3);
  assert.ok(excerpt[0].startsWith("Orca accounts: when Orca is installed"));
  assert.ok(excerpt.every((item) => !item.startsWith("#") && !item.startsWith("-")));

  assert.deepEqual(
    appUpdateNotesExcerpt("## Fixes\n- **Faster** `restart` with [notes](https://x.test)", { t }),
    ["Faster restart with notes"],
  );
  assert.deepEqual(
    appUpdateNotesExcerpt("A faster, smoother update\nfor everyone.", { t }),
    ["A faster, smoother update for everyone."],
  );
  assert.deepEqual(
    appUpdateNotesExcerpt("- one\n- two\n- three\n- four\n- five", { t }, 2),
    ["one", "two"],
  );
  assert.equal(appUpdateNotesExcerpt(`- ${"x".repeat(400)}`, { t })[0].length, 220);
  assert.deepEqual(appUpdateNotesExcerpt(null, { t }), []);
  assert.deepEqual(
    appUpdateNotesExcerpt("Signatures need TAURI_SIGNING_PRIVATE_KEY.", { t }),
    ["This update includes reliability improvements, bug fixes, and interface polish."],
  );
});

test("the status bar segment shows progress, ready, installing, and failure only", () => {
  const segment = (status: AppUpdateStatus) =>
    appUpdateStatusSegment(snapshot({ status }), { t });
  assert.equal(segment({ state: "idle" }), null);
  assert.equal(segment({ ...READY, state: "available" }), null);
  assert.equal(
    segment({
      state: "downloading",
      version: "0.7.2",
      downloadedBytes: 41,
      totalBytes: 100,
    })?.label,
    "↓ 41%",
  );
  assert.equal(
    segment({
      state: "downloading",
      version: "0.7.2",
      downloadedBytes: 72_450_000,
      totalBytes: null,
    })?.label,
    "↓ 72.5 MB",
  );
  // The unit goes through the translation.
  const french = (source: string, values?: Record<string, string | number>) =>
    t(source === "↓ {done} MB" ? "↓ {done} Mo" : source, values);
  assert.equal(
    appUpdateStatusSegment(
      snapshot({
        status: {
          state: "downloading",
          version: "0.7.2",
          downloadedBytes: 72_450_000,
          totalBytes: null,
        },
      }),
      { t: french },
    )?.label,
    "↓ 72.5 Mo",
  );
  assert.deepEqual(segment(READY), {
    label: "Update ready",
    title: "Halv 0.7.2 is ready",
    tone: "accent",
  });
  assert.equal(
    segment({ state: "restarting", version: "0.7.2" })?.label,
    "Installing update…",
  );
});

test("Settings shows one status line for each state", () => {
  const line = (status: AppUpdateStatus) =>
    appUpdateSettingsStatus(snapshot({ status }), { t });
  assert.equal(line({ state: "idle" }), null);
  assert.deepEqual(line(READY), {
    text: "Halv 0.7.2 is ready. Restart to finish the update.",
    tone: "accent",
  });
  assert.deepEqual(line({ ...READY, state: "available" }), {
    text: "Version 0.7.2 is available.",
    tone: "accent",
  });
  assert.equal(
    line({
      state: "error",
      operation: "check",
      message: "error sending request",
      retryable: true,
      userInitiated: true,
    })?.tone,
    "danger",
  );
});

test("the card moves above the chat composer only when it would cover it", () => {
  const viewport = { width: 1280, height: 800 };
  // The composer of the default chat layout at 1280 x 800.
  const composer = { left: 432, top: 653, right: 1185, bottom: 776 };
  assert.equal(appUpdateCardBottom(viewport, 200, null), 36);
  assert.equal(appUpdateCardBottom(viewport, 200, composer), 800 - 653 + 12);
  // A wide window leaves the corner free.
  assert.equal(
    appUpdateCardBottom({ width: 1920, height: 1080 }, 200, {
      left: 700,
      top: 930,
      right: 1480,
      bottom: 1056,
    }),
    36,
  );
  // A card that does not fit above the composer keeps its corner.
  assert.equal(appUpdateCardBottom({ width: 1280, height: 400 }, 300, composer), 36);
  // A hidden composer has an empty rectangle.
  assert.equal(
    appUpdateCardBottom(viewport, 200, { left: 0, top: 0, right: 0, bottom: 0 }),
    36,
  );
});

test("the card gives way to a menu that opens under it", () => {
  // The card at 1024 x 680 and the composer command list from the review.
  const card = { left: 648, top: 431, right: 1008, bottom: 644 };
  const slashMenu = { left: 280, top: 300, right: 846, bottom: 520 };
  assert.equal(appUpdateCardYields(card, [slashMenu]), true);
  assert.equal(appUpdateCardYields(card, []), false);
  // A wide window keeps the menu clear of the card.
  assert.equal(
    appUpdateCardYields(card, [{ left: 100, top: 300, right: 640, bottom: 520 }]),
    false,
  );
  assert.equal(appUpdateCardYields(null, [slashMenu]), false);
});

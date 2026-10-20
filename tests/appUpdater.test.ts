import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";

import {
  appUpdateCommandErrorLabel,
  appUpdateFailureLabel,
  appUpdateFailureMessage,
  isInternalReleaseNote,
} from "../src/appUpdater.ts";
import { appUpdateNotesExcerpt } from "../src/appUpdateCard.ts";

const t = (source: string, values?: Record<string, string | number>) =>
  source.replace(/\{(\w+)\}/g, (_, key: string) => String(values?.[key] ?? ""));

test("shows actionable updater failures without hiding the cause category", () => {
  assert.equal(
    appUpdateFailureMessage(
      new Error(
        'None of the fallback platforms ["linux-x86_64-appimage"] were found',
      ),
      "check",
    ),
    "This release does not include an update for this system.",
  );
  assert.equal(
    appUpdateFailureMessage(new Error("error sending request: timed out"), "check"),
    "Could not reach the update server. Check your connection and try again.",
  );
  assert.equal(
    appUpdateFailureMessage(new Error("signature verification failed"), "install"),
    "Update verification failed, so Halv was not changed.",
  );
  assert.equal(
    appUpdateFailureMessage(new Error("unexpected updater response"), "check"),
    "Could not check for updates. Try again later.",
  );
  assert.equal(
    appUpdateFailureMessage(new Error("unexpected updater response"), "check", {
      debug: true,
    }),
    "Could not check for updates: unexpected updater response",
  );
});

test("keeps release-engineering notes behind the debug flag", () => {
  const internal =
    "Halv desktop artifacts. Updater signatures are attached when TAURI_SIGNING_PRIVATE_KEY is configured.";
  assert.equal(isInternalReleaseNote(internal), true);
  assert.equal(isInternalReleaseNote("A faster, smoother update for everyone."), false);
  assert.deepEqual(appUpdateNotesExcerpt(internal, { t }), [
    "This update includes reliability improvements, bug fixes, and interface polish.",
  ]);
  assert.deepEqual(appUpdateNotesExcerpt(internal, { t, debug: true }), [internal]);
});

test("maps download and restart failures to their own sentences", () => {
  assert.equal(
    appUpdateFailureMessage(
      "the download timed out: no data arrived for 60 seconds",
      "download",
    ),
    "Could not reach the update server. Check your connection and try again.",
  );
  assert.equal(
    appUpdateFailureMessage("minisign signature mismatch", "download"),
    "Update verification failed, so Halv was not changed.",
  );
  assert.equal(
    appUpdateFailureMessage("helper exited", "relaunch"),
    "Could not restart Halv. Try again later.",
  );
  assert.equal(
    appUpdateFailureMessage("disk full", "download", { debug: true }),
    "Could not download the update: disk full",
  );
  assert.equal(
    appUpdateFailureLabel("Could not restart Halv: helper exited", t),
    "Could not restart Halv: helper exited",
  );
});

test("translates the errors of the update commands", () => {
  assert.equal(
    appUpdateCommandErrorLabel("This build of Halv cannot install updates.", t),
    "This build of Halv cannot install updates.",
  );
  assert.equal(
    appUpdateCommandErrorLabel(new Error("custom failure"), t),
    "custom failure",
  );
  assert.equal(appUpdateCommandErrorLabel("  ", t), "Something went wrong. Try again.");
});

test("Rust owns the update flow and the webview never calls the plugin", () => {
  const app = readFileSync("src/App.tsx", "utf8");
  const settings = readFileSync("src/components/SettingsPanel.tsx", "utf8");
  const gate = readFileSync("src/components/ConnectGate.tsx", "utf8");
  const capability = readFileSync("src-tauri/capabilities/default.json", "utf8");
  const main = readFileSync("src/main.tsx", "utf8");

  assert.equal(existsSync("src/useAppUpdateInstall.ts"), false);
  for (const source of [app, settings, gate]) {
    assert.doesNotMatch(source, /@tauri-apps\/plugin-updater/);
    assert.doesNotMatch(source, /useAppUpdateInstall/);
  }
  assert.doesNotMatch(app, /startAppUpdateChecks/);
  assert.doesNotMatch(capability, /updater:default/);
  assert.match(capability, /"process:default"/);
  assert.match(main, /<AppUpdateCard \/>/);
});

test("macOS app menu opens the update card after Rust starts the check", () => {
  const appMenu = readFileSync("src-tauri/src/app_menu.rs", "utf8");
  const app = readFileSync("src/App.tsx", "utf8");

  assert.match(appMenu, /"Check for Updates…"/);
  assert.match(appMenu, /app_submenu\.insert\(&check_for_updates, 1\)/);
  assert.match(appMenu, /"halv:\/\/menu\/check-for-updates"/);
  assert.match(appMenu, /app_update::check_from_menu\(app\)/);
  assert.match(app, /"halv:\/\/menu\/check-for-updates"/);
  assert.match(app, /appUpdateStore\.reveal\(\)/);
});

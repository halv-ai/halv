// The App wires the analytics v2 parts together (ADR 0022). A component test
// would need the whole App, so these tests read the source and check each
// wire: the language, the notice, the pitch hold, and the trial surfaces.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

function source(path: string) {
  return readFileSync(new URL(path, import.meta.url), "utf8");
}

const app = source("../src/App.tsx");
const settings = source("../src/components/SettingsPanel.tsx");
const connections = source("../src/components/SettingsConnections.tsx");
const taskBoard = source("../src/components/TaskBoard.tsx");
const cloudCard = source("../src/components/HalvCloudCard.tsx");

// The text of the first `useEffect` call that contains `marker`.
function effectWith(code: string, marker: string) {
  const at = code.indexOf(marker);
  assert.notEqual(at, -1, `${marker} is in the source`);
  const start = code.lastIndexOf("useEffect(", at);
  const end = code.indexOf("\n  }, [", at);
  assert.ok(start !== -1 && end !== -1, "the marker sits in an effect");
  return code.slice(start, end);
}

test("App sends the UI language once at start and again on each change", () => {
  const effect = effectWith(app, "setAnalyticsLocale(getActiveLocale())");
  assert.match(effect, /const sendLocale = \(\) => void setAnalyticsLocale\(getActiveLocale\(\)\);/);
  // The start call, then the subscription whose cleanup the effect returns.
  assert.match(effect, /sendLocale\(\);\s*return subscribeActiveLocale\(sendLocale\);/);
  // The effect has no dependency, so it runs once.
  const at = app.indexOf(effect) + effect.length;
  assert.match(app.slice(at, at + 20), /^\n {2}\}, \[\]\);/);
});

test("App shows the notice of list version 2 only where the consent banner shows", () => {
  assert.match(app, /const showConsentNotice = shouldShowConsentNotice\(\{/);
  const decision = app.slice(app.indexOf("const showConsentNotice = "));
  const call = decision.slice(0, decision.indexOf("});"));
  assert.match(call, /consent: anonymousUsagePreference\.consent,/);
  assert.match(call, /asked: anonymousUsagePreference\.asked,/);
  assert.match(call, /welcomeScreenShown,/);
  assert.match(call, /firstTaskShown: showFirstRunCard \|\| guidedTaskPhase === "prepared",/);
  assert.match(app, /\{showConsentNotice \? <ConsentNoticeBanner \/> : null\}/);
  assert.match(app, /ConsentNoticeBanner,/);
});

test("the Halv Cloud pitch waits for the consent banner and the notice", () => {
  const start = app.indexOf("const showCloudPitch =");
  const pitch = app.slice(start, app.indexOf("cloudPitchVisible({", start));
  assert.match(pitch, /!showConsentBanner &&/);
  assert.match(pitch, /!showConsentNotice &&/);
  // The notice banner sits above the pitch card in the chat column.
  assert.ok(
    app.indexOf("<ConsentNoticeBanner />") < app.indexOf("{showCloudPitch ? ("),
  );
});

test("each Halv Cloud card names the surface of its trial_prompted event", () => {
  // The pitch in App records "pitch" itself.
  assert.match(app, /recordTrialPrompted\("pitch"\);/);
  // The inline card records the surface, or else the feature.
  assert.match(cloudCard, /const trialSurface = surface \?\? feature;/);
  assert.match(cloudCard, /recordTrialPrompted\(trialSurface\)/);
  assert.match(cloudCard, /placement === "inline" && !access\.granted/);
  // Each point of use passes its place.
  assert.match(taskBoard, /feature="routing"[\s\S]*?surface="task_board"/);
  assert.match(settings, /feature="routing"/);
  assert.match(connections, /feature="rotation"/);
});

test("account_connected carries the facts of the sign-ins that have loaded", () => {
  // The effect runs from its name to the closing of its dependency list.
  const start = app.indexOf("function recordFunnelEventsForStateChange()");
  assert.notEqual(start, -1);
  const effect = app.slice(start, app.indexOf("\n  );\n", start));
  assert.match(effect, /analyticsEventProps\(event, nextState\)/);
  assert.match(effect, /signIns: signInsKnown/);
  // No account counts as connected until the lanes and the status answered.
  assert.match(effect, /accountsResolved: connectGateAccountsResolved,/);
  assert.match(effect, /statusError: status\.error,/);
  assert.match(effect, /accountConnected:\s*signInsKnown &&/);
  const deps = effect.slice(effect.indexOf("\n    [\n"));
  assert.match(deps, /connectGateAccountsResolved,/);
  assert.match(deps, /status\.error,/);
});

test("Settings, Privacy shows the saved list version inside the disclosure", () => {
  assert.match(settings, /<ConsentDetails>[\s\S]*?<ConsentRecord \/>[\s\S]*?<\/ConsentDetails>/);
});

// The Halv Cloud gate (ADR 0021). Local work never waits for an account.
// Halv Cloud asks only at the point of use, in an inline card.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { accountStatusStore, gateDecision } from "../src/gate.ts";
// @ts-expect-error Node's test runner needs the source extension.
import * as trialPrompt from "../src/trialPrompt.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { autoRotationActive, rotationCloudCardVisible } from "../src/accountAutoRotation.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { routingCloudCardPlacement, taskRoutingPreferenceAfterToggle } from "../src/taskRouting.ts";
// @ts-expect-error Node's test runner needs the source extension.
import type { CloudAccess } from "../src/halvCloud.ts";
// @ts-expect-error Node's test runner needs the source extension.
import type { AccountAuthStatus, Entitlements } from "../src/types.ts";

const {
  CLOUD_PITCH_DISMISSED_STORAGE_KEY,
  CLOUD_PITCH_SHOWN_STORAGE_KEY,
  cloudPitchVisible,
  readCloudPitchDismissed,
  readCloudPitchShown,
  writeCloudPitchDismissed,
  writeCloudPitchShown,
} = trialPrompt;

const NOW_MS = Date.parse("2026-10-21T12:00:00Z");

const ENTITLED: Entitlements = {
  beta: false,
  billingRequired: true,
  canUseApp: true,
  offlineGraceHours: 72,
  plan: "unlimited",
  message: null,
  checkoutUrl: null,
  billingPortalUrl: null,
  savings: { period: "2026-10", tokensSaved: 0, capTokens: 500_000, exhausted: false },
};

function status(
  signedIn: boolean,
  entitlements: Entitlements | null,
  syncedAt: string | null = "2026-10-21T11:00:00Z",
): AccountAuthStatus {
  return {
    signedIn,
    email: signedIn ? "user@example.com" : null,
    plan: entitlements?.plan ?? null,
    serverUrl: "https://api.halv.ai",
    entitlements,
    syncedAt: signedIn ? syncedAt : null,
    firstLaunchAt: "2026-10-01T00:00:00Z",
    freeTurnsUsed: 0,
    stale: false,
  };
}

// Every account state that the app can meet.
const ACCOUNT_STATES: Record<string, AccountAuthStatus> = {
  signedOut: status(false, null),
  noEntitlements: status(true, null, null),
  noPlan: status(true, { ...ENTITLED, canUseApp: false }),
  expiredTrial: status(true, { ...ENTITLED, canUseApp: false, trialConfirmed: true }),
  paymentIssue: status(true, { ...ENTITLED, canUseApp: false, paymentIssue: "past_due" }),
  unpaid: status(true, { ...ENTITLED, canUseApp: false, paymentIssue: "unpaid" }),
  trial: status(true, { ...ENTITLED, trialEnd: NOW_MS / 1000 + 86_400 }),
  paid: status(true, { ...ENTITLED, paymentConfirmed: true }),
  beta: status(true, { ...ENTITLED, plan: "beta_free" }),
  staleSync: status(true, ENTITLED, "2026-10-01T00:00:00Z"),
};

const NO_ACCESS: CloudAccess[] = [
  { granted: false, reason: "signed_out" },
  { granted: false, reason: "no_plan" },
  { granted: false, reason: "payment_issue" },
];
const WITH_ACCESS: CloudAccess[] = [
  { granted: true, reason: "trial" },
  { granted: true, reason: "paid" },
  { granted: true, reason: "beta" },
];

function source(path: string) {
  return readFileSync(new URL(path, import.meta.url), "utf8");
}

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  };
}

// --- The send path never blocks -------------------------------------------

test("no account state returns a wall", () => {
  for (const [name, account] of Object.entries(ACCOUNT_STATES)) {
    const decision = gateDecision(account, NOW_MS);
    assert.ok(
      decision === "app" || decision === "verify",
      `${name} returned ${decision}`,
    );
  }
});

test("the trial send gate is gone", () => {
  assert.equal("trialGateForSend" in trialPrompt, false);
  const app = source("../src/App.tsx");
  assert.doesNotMatch(app, /trialGateForSend|sendGate\(|openSendGate/);
  assert.doesNotMatch(app, /<TrialPromptCard/);
});

test("a terminal prompt never waits for an account", () => {
  const app = source("../src/App.tsx");
  const gate = app.match(/onPromptSubmitGate=\{\(\) => \{([\s\S]*?)\n\s*\}\}/);
  assert.ok(gate, "App passes onPromptSubmitGate");
  assert.doesNotMatch(gate[1], /return true/);
  assert.match(gate[1], /return false;/);
});

test("no account state renders a full-screen wall", () => {
  const app = source("../src/App.tsx");
  assert.doesNotMatch(app, /<VerificationGate/);
  assert.doesNotMatch(app, /accountGateDecision === "paywall"/);
  // The plans page opens only on a click, and it always has a way back.
  const paywalls = app.match(/<Paywall[\s\S]*?\/>/g) ?? [];
  assert.equal(paywalls.length, 1);
  assert.match(paywalls[0], /onBack=\{\(\) => setAccountFlowOpen\(null\)\}/);
  assert.match(source("../src/components/Paywall.tsx"), /onBack: \(\) => void;/);
});

test("the account status store tells the status bar about each change", () => {
  const seen: Array<AccountAuthStatus | null> = [];
  const stop = accountStatusStore.subscribe(() =>
    seen.push(accountStatusStore.getSnapshot()),
  );
  accountStatusStore.publish(ACCOUNT_STATES.paid);
  accountStatusStore.publish(ACCOUNT_STATES.paid);
  accountStatusStore.publish(null);
  stop();
  assert.deepEqual(seen, [ACCOUNT_STATES.paid, null]);
});

// --- The pitch card ----------------------------------------------------------

test("the pitch waits for the first result", () => {
  for (const access of NO_ACCESS) {
    assert.equal(
      cloudPitchVisible({ valueDelivered: false, access, dismissed: false }),
      false,
    );
  }
});

test("the pitch shows after the first result to an install without Halv Cloud", () => {
  assert.equal(
    cloudPitchVisible({ valueDelivered: true, access: NO_ACCESS[0], dismissed: false }),
    true,
  );
  assert.equal(
    cloudPitchVisible({ valueDelivered: true, access: NO_ACCESS[1], dismissed: false }),
    true,
  );
  // A payment issue gets the payment card, not a trial pitch.
  assert.equal(
    cloudPitchVisible({ valueDelivered: true, access: NO_ACCESS[2], dismissed: false }),
    false,
  );
  for (const access of WITH_ACCESS) {
    assert.equal(
      cloudPitchVisible({ valueDelivered: true, access, dismissed: false }),
      false,
    );
  }
});

test("the pitch never shows after a dismissal", () => {
  const storage = memoryStorage();
  assert.equal(readCloudPitchDismissed(storage), false);
  writeCloudPitchDismissed(storage);
  assert.equal(storage.getItem("halv.cloudPitchDismissed"), "1");
  assert.equal(CLOUD_PITCH_DISMISSED_STORAGE_KEY, "halv.cloudPitchDismissed");
  const dismissed = readCloudPitchDismissed(storage);
  for (const access of NO_ACCESS) {
    assert.equal(
      cloudPitchVisible({ valueDelivered: true, access, dismissed }),
      false,
    );
  }
});

test("the pitch shows on one launch only", () => {
  const storage = memoryStorage();
  assert.equal(readCloudPitchShown(storage), false);
  writeCloudPitchShown(storage);
  assert.equal(CLOUD_PITCH_SHOWN_STORAGE_KEY, "halv.cloudPitchShown");
  assert.equal(storage.getItem(CLOUD_PITCH_SHOWN_STORAGE_KEY), "1");
  const shownOnEarlierLaunch = readCloudPitchShown(storage);
  for (const access of NO_ACCESS) {
    assert.equal(
      cloudPitchVisible({
        valueDelivered: true,
        access,
        dismissed: false,
        shownOnEarlierLaunch,
      }),
      false,
    );
  }
  // App reads the flag once at launch and writes it when the card shows.
  const app = source("../src/App.tsx");
  assert.match(app, /shownOnEarlierLaunch: cloudPitchShownOnEarlierLaunch/);
  assert.match(app, /if \(!showCloudPitch\) return;\s*writeCloudPitchShown\(\);/);
});

test("a payment issue never opens a modal at launch", () => {
  const app = source("../src/App.tsx");
  assert.doesNotMatch(app, /title: tRef\.current\("Payment needs attention"\)/);
  assert.doesNotMatch(app, /shouldPromptForPaymentIssue/);
});

test("a storage that throws keeps the pitch logic safe", () => {
  const broken = {
    getItem: () => {
      throw new Error("blocked");
    },
    setItem: () => {
      throw new Error("blocked");
    },
  };
  assert.equal(readCloudPitchDismissed(broken), false);
  assert.doesNotThrow(() => writeCloudPitchDismissed(broken));
});

test("App shows one pitch card, and checkout or dismissal hides it for good", () => {
  const app = source("../src/App.tsx");
  const pitches = app.match(/<HalvCloudCard[\s\S]*?placement="pitch"[\s\S]*?\/>/g) ?? [];
  assert.equal(pitches.length, 1);
  assert.match(pitches[0], /onDismiss=\{dismissCloudPitch\}/);
  assert.match(pitches[0], /onCheckoutOpened=\{\(\) => writeCloudPitchDismissed\(\)\}/);
  assert.match(
    app,
    /if \(!showCloudPitch\) return;\s*writeCloudPitchShown\(\);\s*void recordAnalyticsEvent\("trial_prompted"\)/,
  );
  const card = source("../src/components/HalvCloudCard.tsx");
  assert.match(card, /recordAnalyticsEvent\("checkout_opened"\)/);
  assert.match(card, /checkout\.openInBrowser\(/);
  assert.match(card, /\{t\("Keep going without it"\)\}/);
  // The card is inline: no dialog, no overlay.
  assert.doesNotMatch(card, /role="dialog"|aria-modal|fixed inset-0/);
});

// --- Automatic rotation --------------------------------------------------------

test("automatic rotation does nothing without Halv Cloud access", () => {
  for (const access of NO_ACCESS) {
    assert.equal(autoRotationActive({ enabled: true }, access), false);
  }
  for (const access of WITH_ACCESS) {
    assert.equal(autoRotationActive({ enabled: true }, access), true);
    assert.equal(autoRotationActive({ enabled: false }, access), false);
  }
  const hook = source("../src/useAccountAutoRotation.ts");
  assert.match(hook, /const active = autoRotationActive\(settings, cloudAccess\);/);
  assert.match(hook, /if \(!loaded \|\| !active\) return;/);
  assert.match(hook, /notice: active \? notice : null/);
  assert.match(
    source("../src/components/AccountSwapNotice.tsx"),
    /const autoEnabled = autoRotation\.active;/,
  );
});

test("the rotation checkbox keeps the choice and shows the card without access", () => {
  const base = { ready: true, settings: { enabled: true }, dismissedThisVisit: false };
  for (const access of NO_ACCESS) {
    assert.equal(rotationCloudCardVisible({ ...base, access }), true);
  }
  for (const access of WITH_ACCESS) {
    assert.equal(rotationCloudCardVisible({ ...base, access }), false);
  }
  assert.equal(
    rotationCloudCardVisible({ ...base, access: NO_ACCESS[0], settings: { enabled: false } }),
    false,
  );
  assert.equal(
    rotationCloudCardVisible({ ...base, access: NO_ACCESS[0], ready: false }),
    false,
  );
  assert.equal(
    rotationCloudCardVisible({ ...base, access: NO_ACCESS[0], dismissedThisVisit: true }),
    false,
  );
  const connections = source("../src/components/SettingsConnections.tsx");
  // The checkbox stays enabled. Access never writes the setting.
  assert.doesNotMatch(connections, /autoRotationSetting\.set\(\{ enabled: false \}\)/);
  assert.match(connections, /feature="rotation"/);
});

// --- Task routing ------------------------------------------------------------

test("the routing toggle keeps the preference of the user", () => {
  for (const access of [...NO_ACCESS, ...WITH_ACCESS]) {
    assert.equal(taskRoutingPreferenceAfterToggle(true, access), true);
    assert.equal(taskRoutingPreferenceAfterToggle(false, access), false);
  }
  const settings = source("../src/components/SettingsPanel.tsx");
  assert.match(
    settings,
    /taskRouting: taskRoutingPreferenceAfterToggle\(\s*nextEnabled,\s*halvCloudAccess,\s*\)/,
  );
});

test("routing on without access shows the card under the toggle", () => {
  const base = { ready: true, routingOn: true, dismissedThisVisit: false };
  for (const access of NO_ACCESS) {
    assert.equal(routingCloudCardPlacement({ ...base, access }), "toggle");
    // Routing off is not a point of use, so no card shows.
    assert.equal(
      routingCloudCardPlacement({ ...base, access, routingOn: false }),
      null,
    );
  }
  for (const access of WITH_ACCESS) {
    assert.equal(routingCloudCardPlacement({ ...base, access }), null);
    assert.equal(
      routingCloudCardPlacement({ ...base, access, routingOn: false }),
      null,
    );
  }
});

test("the routing card shows once per visit and never flashes before load", () => {
  const access = NO_ACCESS[1];
  assert.equal(
    routingCloudCardPlacement({ ready: false, access, routingOn: true, dismissedThisVisit: false }),
    null,
  );
  assert.equal(
    routingCloudCardPlacement({ ready: true, access, routingOn: true, dismissedThisVisit: true }),
    null,
  );
  const settings = source("../src/components/SettingsPanel.tsx");
  assert.match(
    settings,
    /useEffect\(\(\) => \{\s*if \(routingSectionOpen\) setRoutingCloudDismissed\(false\);\s*\}, \[routingSectionOpen\]\);/,
  );
  assert.match(settings, /routingCloudCard === "toggle" \? routingCloudCardElement : null/);
  assert.doesNotMatch(settings, /routingCloudCard === "header"/);
});

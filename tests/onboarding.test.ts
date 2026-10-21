import assert from "node:assert/strict";
import test from "node:test";
import {
  appLooksFresh,
  firstRunVariant,
  shouldShowFirstRunCard,
} from "../src/firstRun.ts";
import {
  CLOUD_INFO_SEEN_STORAGE_KEY,
  CONSENT_BANNER_SHOWN_STORAGE_KEY,
  ONBOARDING_STORAGE_KEY,
  SAFE_FIRST_TASK_PROMPT,
  WELCOME_SEEN_STORAGE_KEY,
  conversationIdToRestore,
  onboardingPayoffCopy,
  onboardingReplayEnabled,
  readCloudInfoSeen,
  readConsentBannerShown,
  readOnboardingPreference,
  readWelcomeSeen,
  shouldAskConsent,
  shouldShowConsentBanner,
  shouldShowWelcomeScreen,
  writeCloudInfoSeen,
  writeConsentBannerShown,
  writeOnboardingPreference,
  writeWelcomeSeen,
} from "../src/onboarding.ts";

function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    removeItem(key: string) {
      values.delete(key);
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
}

test("guided onboarding dismissal and completion persist", () => {
  const storage = memoryStorage();
  assert.equal(readOnboardingPreference(storage), "available");

  writeOnboardingPreference("dismissed", storage);
  assert.equal(storage.getItem(ONBOARDING_STORAGE_KEY), "dismissed");
  assert.equal(readOnboardingPreference(storage), "dismissed");

  writeOnboardingPreference("completed", storage);
  assert.equal(readOnboardingPreference(storage), "completed");
});

test("welcome visibility persists after the welcome screen is seen", () => {
  const storage = memoryStorage();
  assert.equal(readWelcomeSeen(storage), false);

  writeWelcomeSeen(storage);

  assert.equal(storage.getItem(WELCOME_SEEN_STORAGE_KEY), "1");
  assert.equal(readWelcomeSeen(storage), true);
});

test("welcome visibility ignores unknown stored values", () => {
  const storage = memoryStorage();
  storage.setItem(WELCOME_SEEN_STORAGE_KEY, "true");
  assert.equal(readWelcomeSeen(storage), false);
});

test("the development flag enables onboarding replay safely", () => {
  assert.equal(
    onboardingReplayEnabled({ VITE_HALV_RESET_ONBOARDING: "1" }),
    true,
  );
  assert.equal(
    onboardingReplayEnabled({ VITE_HALV_RESET_ONBOARDING: "0" }),
    false,
  );
  assert.equal(onboardingReplayEnabled({ VITE_HALV_FRESH_STATE: "1" }), true);
  assert.equal(onboardingReplayEnabled(undefined), false);
});

test("onboarding replay shows welcome with existing connections", () => {
  assert.equal(
    shouldShowWelcomeScreen({
      welcomeSeen: false,
      connectionRequired: false,
      replayOnboarding: true,
    }),
    true,
  );
  assert.equal(
    shouldShowWelcomeScreen({
      welcomeSeen: false,
      connectionRequired: false,
      replayOnboarding: false,
    }),
    false,
  );
  assert.equal(
    shouldShowWelcomeScreen({
      welcomeSeen: true,
      connectionRequired: true,
      replayOnboarding: true,
    }),
    false,
  );
});

test("onboarding replay skips the last conversation restore", () => {
  assert.equal(
    conversationIdToRestore({
      lastActiveConversationId: "thread-1",
      replayOnboarding: true,
    }),
    null,
  );
  assert.equal(
    conversationIdToRestore({
      lastActiveConversationId: "thread-1",
      replayOnboarding: false,
    }),
    "thread-1",
  );
  assert.equal(
    conversationIdToRestore({
      lastActiveConversationId: null,
      replayOnboarding: false,
    }),
    null,
  );
});

test("the first-run variant identifies one connected agent", () => {
  assert.equal(
    firstRunVariant({
      chatgpt: {
        source: "codex-cli",
        accountId: "account-1",
        accessTokenValid: true,
      },
    }),
    "codex",
  );
  assert.equal(firstRunVariant(null, connections("claude")), "claude");
  assert.equal(firstRunVariant(null, connections("kimi")), "kimi");
  assert.equal(firstRunVariant(null, connections("zai")), "zai");
  assert.equal(firstRunVariant(null, connections("grok")), "grok");
  assert.equal(
    firstRunVariant(null, connections("claude", "kimi")),
    "claude",
  );
  assert.equal(firstRunVariant(null, connections("codex", "kimi")), "codex");
  assert.equal(
    firstRunVariant(
      {
        chatgpt: {
          source: "codex-cli",
          accountId: "account-1",
          accessTokenValid: true,
        },
      },
      connections("claude", "codex"),
    ),
    "codex",
  );
  assert.equal(
    firstRunVariant({
      chatgpt: {
        source: "app",
        accountId: "account-1",
        accessTokenValid: true,
      },
    }),
    "codex",
  );
  assert.equal(firstRunVariant({ apiKey: { masked: "sk-...1234" } }), "fresh");
  assert.equal(firstRunVariant(null), "fresh");
});

function connections(
  ...connected: Array<"claude" | "codex" | "kimi" | "zai" | "grok">
) {
  return {
    anthropic: false,
    claude: connected.includes("claude"),
    codex: connected.includes("codex"),
    grok: connected.includes("grok"),
    kimi: connected.includes("kimi"),
    moonshot: false,
    openai: false,
    zai: connected.includes("zai"),
  };
}

test("fresh app detection keeps the existing message-based rules", () => {
  assert.equal(
    appLooksFresh({
      initialDataLoaded: true,
      hasConversationMessages: false,
      hasAssistantReply: false,
    }),
    true,
  );
  assert.equal(
    appLooksFresh({
      initialDataLoaded: false,
      hasConversationMessages: false,
      hasAssistantReply: false,
    }),
    false,
  );
  assert.equal(
    appLooksFresh({
      initialDataLoaded: true,
      hasConversationMessages: true,
      hasAssistantReply: false,
    }),
    false,
  );
  assert.equal(
    appLooksFresh({
      initialDataLoaded: true,
      hasConversationMessages: false,
      hasAssistantReply: true,
    }),
    false,
  );
});

test("onboarding replay makes an initialized app look fresh", () => {
  assert.equal(
    appLooksFresh({
      initialDataLoaded: true,
      hasConversationMessages: true,
      hasAssistantReply: true,
      replayOnboarding: true,
    }),
    true,
  );
  assert.equal(
    appLooksFresh({
      initialDataLoaded: false,
      hasConversationMessages: true,
      hasAssistantReply: true,
      replayOnboarding: true,
    }),
    false,
  );
});

test("the first-run card only replaces a fully idle fresh message list", () => {
  const visible = {
    preference: "available" as const,
    appIsFresh: true,
    phase: "idle" as const,
    currentConversationId: null,
    messageCount: 0,
    noticeCount: 0,
    hasPendingApproval: false,
  };

  assert.equal(shouldShowFirstRunCard(visible), true);
  assert.equal(
    shouldShowFirstRunCard({ ...visible, preference: "dismissed" }),
    false,
  );
  assert.equal(
    shouldShowFirstRunCard({ ...visible, preference: "completed" }),
    false,
  );
  assert.equal(shouldShowFirstRunCard({ ...visible, appIsFresh: false }), false);
  assert.equal(
    shouldShowFirstRunCard({ ...visible, phase: "prepared" }),
    false,
  );
  assert.equal(
    shouldShowFirstRunCard({ ...visible, currentConversationId: "thread-1" }),
    false,
  );
  assert.equal(shouldShowFirstRunCard({ ...visible, messageCount: 1 }), false);
  assert.equal(shouldShowFirstRunCard({ ...visible, noticeCount: 1 }), false);
  assert.equal(
    shouldShowFirstRunCard({ ...visible, hasPendingApproval: true }),
    false,
  );
});

test("the first task is useful and explicitly read-only", () => {
  assert.match(SAFE_FIRST_TASK_PROMPT, /read-only tour/i);
  // A broad read shows the context that Halv removes (ADR 0018).
  assert.match(SAFE_FIRST_TASK_PROMPT, /largest modules/i);
  assert.match(SAFE_FIRST_TASK_PROMPT, /main entry points/i);
  assert.match(SAFE_FIRST_TASK_PROMPT, /run its tests/i);
  assert.match(SAFE_FIRST_TASK_PROMPT, /low-risk next task/i);
  assert.match(SAFE_FIRST_TASK_PROMPT, /Do not edit files/i);
  assert.match(SAFE_FIRST_TASK_PROMPT, /destructive commands/i);
});

test("the completion payoff prefers turn savings and degrades clearly", () => {
  assert.equal(
    onboardingPayoffCopy(43.6, 1_200),
    "Halv reduced the input context for this reply by 44%.",
  );
  assert.equal(
    onboardingPayoffCopy(null, 1_200),
    "Halv has saved 1,200 tokens today.",
  );
  assert.equal(
    onboardingPayoffCopy(null, 1_200, 0.42),
    "Halv has saved $0.42 today at API prices.",
  );
  assert.match(onboardingPayoffCopy(null, 0), /tracking the context saved/i);
  assert.equal(
    onboardingPayoffCopy(0, 0),
    "This reply had little context to trim. Savings grow with longer tasks.",
  );
});

test("consent is asked only while the saved answer is unset", () => {
  for (const welcomeSeen of [false, true]) {
    assert.equal(shouldAskConsent({ consent: "unset", welcomeSeen }), true);
    assert.equal(shouldAskConsent({ consent: "on", welcomeSeen }), false);
    assert.equal(shouldAskConsent({ consent: "off", welcomeSeen }), false);
  }
});

test("consent is not asked before the saved answer loads", () => {
  assert.equal(shouldAskConsent({ consent: null, welcomeSeen: true }), false);
  assert.equal(
    shouldAskConsent({ consent: undefined, welcomeSeen: false }),
    false,
  );
});

test("the consent banner asks only in the main app and only once at a time", () => {
  const main = { welcomeScreenShown: false, firstTaskShown: false };
  assert.equal(shouldShowConsentBanner({ consent: "unset", ...main }), true);
  assert.equal(shouldShowConsentBanner({ consent: "on", ...main }), false);
  assert.equal(shouldShowConsentBanner({ consent: "off", ...main }), false);
  assert.equal(shouldShowConsentBanner({ consent: null, ...main }), false);
  // The welcome screen and the first-run card have their own ask card.
  assert.equal(
    shouldShowConsentBanner({
      consent: "unset",
      welcomeScreenShown: true,
      firstTaskShown: false,
    }),
    false,
  );
  assert.equal(
    shouldShowConsentBanner({
      consent: "unset",
      welcomeScreenShown: false,
      firstTaskShown: true,
    }),
    false,
  );
});

test("the consent banner shows on one launch only, and the answer stays unset", () => {
  const storage = memoryStorage();
  const main = { welcomeScreenShown: false, firstTaskShown: false };
  assert.equal(readConsentBannerShown(storage), false);
  assert.equal(
    shouldShowConsentBanner({
      consent: "unset",
      ...main,
      shownOnEarlierLaunch: readConsentBannerShown(storage),
    }),
    true,
  );
  writeConsentBannerShown(storage);
  assert.equal(storage.getItem(CONSENT_BANNER_SHOWN_STORAGE_KEY), "1");
  assert.equal(
    shouldShowConsentBanner({
      consent: "unset",
      ...main,
      shownOnEarlierLaunch: readConsentBannerShown(storage),
    }),
    false,
  );
  // The answer stays unset, so Halv still sends nothing.
  assert.equal(shouldAskConsent({ consent: "unset", welcomeSeen: true }), true);
});

test("the Halv Cloud information block shows once", () => {
  const storage = memoryStorage();
  assert.equal(readCloudInfoSeen(storage), false);
  writeCloudInfoSeen(storage);
  assert.equal(storage.getItem(CLOUD_INFO_SEEN_STORAGE_KEY), "1");
  assert.equal(readCloudInfoSeen(storage), true);
});

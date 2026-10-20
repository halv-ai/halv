export type OnboardingPreference = "available" | "dismissed" | "completed";

type OnboardingStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export const ONBOARDING_STORAGE_KEY = "halv.guidedFirstTask";
export const WELCOME_SEEN_STORAGE_KEY = "halv.welcomeSeen";

export function onboardingReplayEnabled(
  env: { [key: string]: unknown } | undefined = import.meta.env,
) {
  return (
    env?.VITE_HALV_RESET_ONBOARDING === "1" ||
    env?.VITE_HALV_FRESH_STATE === "1"
  );
}

export function shouldShowWelcomeScreen({
  welcomeSeen,
  connectionRequired,
  replayOnboarding,
}: {
  welcomeSeen: boolean;
  connectionRequired: boolean;
  replayOnboarding: boolean;
}) {
  return !welcomeSeen && (connectionRequired || replayOnboarding);
}

export function conversationIdToRestore({
  lastActiveConversationId,
  replayOnboarding,
}: {
  lastActiveConversationId: string | null;
  replayOnboarding: boolean;
}) {
  return replayOnboarding ? null : lastActiveConversationId;
}

// The first task reads broadly on purpose: Halv measures the context it
// removes from each reply, so a task that reads many files shows the
// promise of the product (ADR 0018). Every prompt stays read-only.
export const SAFE_FIRST_TASK_PROMPT =
  "Give me a read-only tour of this repository. Read the README, the main entry points, the largest modules, and the test setup before you answer. Explain how the pieces connect, how to run its tests, and suggest one small, low-risk next task. Do not edit files or run destructive commands.";

export type FirstTaskGoal = "tour" | "quick-win" | "tests";

export function firstTaskPrompt(goal: FirstTaskGoal): string {
  if (goal === "quick-win") {
    return "Review this project and find one small, useful improvement. Read the README, the main entry points, and the modules they import, so the suggestion fits how the code already works. Cite the relevant files, explain why the change matters, and give me a short implementation plan. Do not edit files or run destructive commands.";
  }
  if (goal === "tests") {
    return "Give me a read-only overview of testing in this project. Read the test configuration, the test helpers, and a representative test for each main area. Explain how to run them, identify one important behavior that may lack coverage, and propose a small test for it. Cite the relevant files. Do not edit files, install dependencies, or run commands.";
  }
  return SAFE_FIRST_TASK_PROMPT;
}

export function firstTaskFollowUp(goal: FirstTaskGoal): string {
  if (goal === "tests") {
    return "Draft the test you suggested and explain what it would verify. Show me the code before making any file changes.";
  }
  return "Turn your suggested improvement into a small implementation plan. Show the relevant files and how we would verify the change. Do not edit files yet.";
}

export function readOnboardingPreference(
  storage: OnboardingStorage | null = browserStorage(),
): OnboardingPreference {
  try {
    const value = storage?.getItem(ONBOARDING_STORAGE_KEY);
    return value === "dismissed" || value === "completed" ? value : "available";
  } catch {
    return "available";
  }
}

export function writeOnboardingPreference(
  preference: OnboardingPreference,
  storage: OnboardingStorage | null = browserStorage(),
) {
  try {
    if (preference === "available") {
      storage?.removeItem(ONBOARDING_STORAGE_KEY);
      return;
    }
    storage?.setItem(ONBOARDING_STORAGE_KEY, preference);
  } catch {
    // Onboarding persistence is optional; the in-memory experience still works.
  }
}

export function readWelcomeSeen(
  storage: OnboardingStorage | null = browserStorage(),
): boolean {
  try {
    return storage?.getItem(WELCOME_SEEN_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

export function writeWelcomeSeen(
  storage: OnboardingStorage | null = browserStorage(),
) {
  try {
    storage?.setItem(WELCOME_SEEN_STORAGE_KEY, "1");
  } catch {
    // Welcome persistence is optional; the in-memory experience still works.
  }
}

export function onboardingPayoffCopy(
  turnSavingsPercent: number | null,
  tokensSavedToday: number,
  dollarsSavedToday = 0,
  formatAmount: (value: number) => string = (value) => `$${value.toFixed(2)}`,
) {
  if (turnSavingsPercent !== null && Number.isFinite(turnSavingsPercent)) {
    if (turnSavingsPercent <= 0)
      return "This reply had little context to trim. Savings grow with longer tasks.";
    return `Halv reduced the input context for this reply by ${Math.max(
      0,
      Math.round(turnSavingsPercent),
    )}%.`;
  }
  // Dollars lead once the dollar metric (ADR 0015) has a value.
  if (dollarsSavedToday > 0) {
    return `Halv has saved ${formatAmount(dollarsSavedToday)} today at API prices.`;
  }
  if (tokensSavedToday > 0) {
    return `Halv has saved ${Math.round(tokensSavedToday).toLocaleString()} tokens today.`;
  }
  return "Halv is now tracking the context saved by each successful reply.";
}

function browserStorage(): OnboardingStorage | null {
  if (typeof window === "undefined") {
    return null;
  }
  return window.localStorage;
}

import type { CloudAccess } from "./halvCloud.ts";

// The Halv Cloud pitch card (ADR 0021). It replaces the trial card of
// ADR 0018. The card never blocks a send. It shows after the first result,
// and its dismissal is permanent.
export const CLOUD_PITCH_DISMISSED_STORAGE_KEY = "halv.cloudPitchDismissed";
// The launch that first shows the card writes this key. Later launches
// read it and keep the card hidden, so the card shows on one launch only.
export const CLOUD_PITCH_SHOWN_STORAGE_KEY = "halv.cloudPitchShown";

type PitchStorage = Pick<Storage, "getItem" | "setItem">;

/**
 * True when the pitch card shows: Halv delivered value, the account has no
 * Halv Cloud plan yet, the user did not dismiss the card, and no earlier
 * launch showed it. An account
 * with a payment issue gets the payment card in Settings instead.
 */
export function cloudPitchVisible({
  valueDelivered,
  access,
  dismissed,
  shownOnEarlierLaunch = false,
}: {
  valueDelivered: boolean;
  access: Pick<CloudAccess, "granted" | "reason">;
  dismissed: boolean;
  shownOnEarlierLaunch?: boolean;
}) {
  if (!valueDelivered || dismissed || shownOnEarlierLaunch || access.granted) {
    return false;
  }
  return access.reason === "signed_out" || access.reason === "no_plan";
}

export function readCloudPitchDismissed(
  storage: PitchStorage | null = browserStorage(),
) {
  try {
    return storage?.getItem(CLOUD_PITCH_DISMISSED_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

/** Records the dismissal. A start of checkout also dismisses the card. */
export function writeCloudPitchDismissed(
  storage: PitchStorage | null = browserStorage(),
) {
  try {
    storage?.setItem(CLOUD_PITCH_DISMISSED_STORAGE_KEY, "1");
  } catch {
    // The in-memory flag still hides the card for this launch.
  }
}

export function readCloudPitchShown(
  storage: PitchStorage | null = browserStorage(),
) {
  try {
    return storage?.getItem(CLOUD_PITCH_SHOWN_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

/** Records that the card showed. App calls it when the card first renders. */
export function writeCloudPitchShown(
  storage: PitchStorage | null = browserStorage(),
) {
  try {
    storage?.setItem(CLOUD_PITCH_SHOWN_STORAGE_KEY, "1");
  } catch {
    // The card then shows again on the next launch.
  }
}

function browserStorage(): PitchStorage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

export type TerminalPromptSubmissionTracker = {
  noteInput(data: string): boolean;
  hasDraft(): boolean;
  rejectSubmission(): void;
};

export function createTerminalPromptSubmissionTracker(): TerminalPromptSubmissionTracker {
  let printableCharacters = 0;
  let submittedCharacters = 0;
  let csiParameters = "";
  let escapeState: "normal" | "escape" | "csi" | "osc" | "osc_escape" =
    "normal";

  return {
    hasDraft: () => printableCharacters > 0,
    rejectSubmission() { printableCharacters = Math.max(printableCharacters, submittedCharacters); },
    noteInput(data) {
      let submitted = false;

      for (const character of data) {
        const codePoint = character.codePointAt(0) ?? 0;
        if (escapeState === "osc") {
          if (character === "\u0007") escapeState = "normal";
          else if (character === "\u001b") escapeState = "osc_escape";
          continue;
        }
        if (escapeState === "osc_escape") {
          escapeState = character === "\\" ? "normal" : "osc";
          continue;
        }
        if (escapeState === "csi") {
          if (codePoint >= 0x40 && codePoint <= 0x7e) {
            // History can restore text without printable input from xterm.
            if ((character === "A" || character === "B") && csiParameters === "") {
              printableCharacters = Math.max(1, printableCharacters);
            }
            escapeState = "normal";
          } else {
            csiParameters += character;
          }
          continue;
        }
        if (escapeState === "escape") {
          csiParameters = "";
          escapeState = character === "[" ? "csi" : character === "]" ? "osc" : "normal";
          continue;
        }
        if (character === "\u001b") {
          escapeState = "escape";
          continue;
        }
        if (character === "\r" || character === "\n") {
          if (printableCharacters > 0) {
            submitted = true;
            submittedCharacters = printableCharacters;
            printableCharacters = 0;
          }
          continue;
        }
        if (character === "\u0003" || character === "\u0015") { printableCharacters = 0; submittedCharacters = 0; continue; }
        if (character === "\u0010" || character === "\u000e") { printableCharacters = Math.max(1, printableCharacters); continue; }
        if (character === "\u007f" || character === "\b") { printableCharacters = Math.max(0, printableCharacters - 1); continue; }
        if (codePoint >= 0x20 && codePoint !== 0x7f && !(codePoint >= 0x80 && codePoint <= 0x9f)) {
          printableCharacters += 1;
        }
      }

      return submitted;
    },
  };
}

// The trial card opens only after Halv delivered value: a first completed
// task on any lane, or a first measured saving (ADR 0018). Before that
// moment, an anonymous user sends freely in any conversation.
export function trialGateForSend({
  signedIn,
  valueDelivered,
}: {
  signedIn: boolean;
  valueDelivered: boolean;
}) {
  return !signedIn && valueDelivered;
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

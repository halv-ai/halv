import { expect, type Locator, type Page } from "@playwright/test";

/**
 * Replays WebKit (macOS WKWebView) input event orders on the xterm helper
 * textarea of a harness terminal. Real NSEvents in a native WKWebView gave
 * these orders. After a main-thread stall, WebKit sends composition and text
 * events before their keydowns, and some keydowns after their keyups.
 *
 * Synthetic events have no default action. Each text step applies the
 * WebKit edit itself, between its beforeinput and input events. A canceled
 * beforeinput skips the edit and the input event, as in the browser.
 */
type Modifiers = { composing?: boolean; shift?: boolean; meta?: boolean };
export type Step =
  // The composition replaces the selection that the browser reads after
  // compositionstart.
  | ["compStart"]
  | ["compUpdate", string]
  // insertCompositionText: replace the marked text.
  | ["mark", string]
  // deleteCompositionText: remove the marked text.
  | ["markDelete"]
  // insertFromComposition: insert the commit at the start of the marked text.
  | ["commit", string]
  | ["compEnd", string]
  // One insertText per character. The keydown of each key comes later.
  | ["insert", string]
  // A paste event. xterm does not cancel it, so the browser inserts the text.
  | ["paste", string]
  // Edit > Select All (Cmd+A) selects the whole textarea.
  | ["selectAll"]
  // Cmd+Left and Cmd+Up move the caret to the start of the textarea.
  | ["caretToStart"]
  // Cmd+X (deleteByCut) removes the selection.
  | ["cut"]
  | ["down", string, string, number, Modifiers?]
  | ["up", string, string, number, Modifiers?]
  // A keypress. Its charCode is the given key code.
  | ["press", string, string, number, Modifiers?]
  // One timer turn: xterm and the guard read the textarea in setTimeout(0).
  | ["tick"];

export async function openHarnessTerminal(page: Page) {
  await page.goto("/ux-audit.html?state=marketing-app-dark&ghostty=off");
  const pane = page.locator('.terminal-pane[data-terminal-scene-complete="true"]').first();
  // The first load waits for the dev server to compile the app.
  await expect(pane).toBeVisible({ timeout: 30_000 });
  const sessionId = (await pane.getAttribute("data-terminal-session-id"))!;
  const textarea = pane.locator(".xterm-helper-textarea");
  await textarea.focus();
  // Start as after Enter: xterm clears the textarea on Enter.
  await textarea.evaluate((node) => { (node as HTMLTextAreaElement).value = ""; });
  await page.evaluate(() => { (window as any).__harnessTerminalWrites = []; });
  return { pane, sessionId, textarea };
}

/** The PTY writes of one session, one entry for each write. */
export async function writes(page: Page, sessionId: string): Promise<string[]> {
  return page.evaluate((id) => ((window as any).__harnessTerminalWrites ?? [])
    .filter((write: { sessionId: string }) => write.sessionId === id)
    .map((write: { data: string }) => write.data), sessionId);
}

/**
 * Keys that queue behind a stall. WebKit sends each keydown with keyCode 229,
 * then the keyup with the real keyCode.
 */
export function queuedKeydowns(text: string): Step[] {
  const steps: Step[] = [];
  for (const character of text) {
    const upper = character.toUpperCase();
    const code = character === " " ? "Space" : character === "\r" ? "Enter" : `Key${upper}`;
    const key = character === "\r" ? "Enter" : character;
    const keyCode = character === " " ? 32 : character === "\r" ? 13 : upper.charCodeAt(0);
    steps.push(["down", key, code, 229], ["up", key, code, keyCode]);
  }
  return steps;
}

export async function runBurst(textarea: Locator, steps: Step[]) {
  await textarea.evaluate(async (node, steps) => {
    const textarea = node as HTMLTextAreaElement;
    let markStart = -1;
    let markLength = 0;
    // WebKit keeps a trailing space in xterm's nowrap textarea as U+00A0. It
    // changes it back to a space when text follows it.
    const withWebKitSpaces = (value: string) => {
      const spaces = value.replace(/\u00a0/g, " ");
      return spaces.endsWith(" ") ? `${spaces.slice(0, -1)}\u00a0` : spaces;
    };
    const setValue = (value: string, caret: number) => {
      textarea.value = withWebKitSpaces(value);
      textarea.setSelectionRange(caret, caret);
    };
    const input = (
      inputType: string,
      data: string | null,
      isComposing: boolean,
      edit: () => void,
    ) => {
      const proceed = textarea.dispatchEvent(new InputEvent("beforeinput", {
        bubbles: true, cancelable: true, composed: true, inputType, data, isComposing,
      }));
      if (!proceed) return;
      edit();
      textarea.dispatchEvent(new InputEvent("input", {
        bubbles: true, cancelable: false, composed: true, inputType, data, isComposing,
      }));
    };
    const insertAtSelection = (text: string) => {
      const start = textarea.selectionStart;
      const value = textarea.value;
      setValue(value.slice(0, start) + text + value.slice(textarea.selectionEnd), start + text.length);
    };
    const key = (
      type: string,
      [name, code, keyCode, modifiers = {}]: [string, string, number, Modifiers?],
    ) => {
      const event = new KeyboardEvent(type, {
        bubbles: true, cancelable: true, composed: true, key: name, code,
        isComposing: !!modifiers.composing,
        shiftKey: !!modifiers.shift,
        metaKey: !!modifiers.meta,
      });
      // KeyboardEvent init ignores the legacy key codes.
      Object.defineProperty(event, "keyCode", { get: () => keyCode });
      Object.defineProperty(event, "which", { get: () => keyCode });
      Object.defineProperty(event, "charCode", { get: () => (type === "keypress" ? keyCode : 0) });
      textarea.dispatchEvent(event);
    };
    const timerTurn = () => new Promise((resolve) => setTimeout(resolve, 0));

    for (const step of steps) {
      switch (step[0]) {
        case "compStart":
          textarea.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
          markStart = textarea.selectionStart;
          markLength = textarea.selectionEnd - textarea.selectionStart;
          break;
        case "compUpdate":
          textarea.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: step[1] }));
          break;
        case "mark": {
          const text = step[1];
          input("insertCompositionText", text, true, () => {
            const value = textarea.value;
            setValue(
              value.slice(0, markStart) + text + value.slice(markStart + markLength),
              markStart + text.length,
            );
            markLength = text.length;
          });
          break;
        }
        case "markDelete":
          input("deleteCompositionText", null, true, () => {
            const value = textarea.value;
            setValue(value.slice(0, markStart) + value.slice(markStart + markLength), markStart);
            markLength = 0;
          });
          break;
        case "commit": {
          const text = step[1];
          input("insertFromComposition", text, true, () => {
            const value = textarea.value;
            setValue(value.slice(0, markStart) + text + value.slice(markStart), markStart + text.length);
          });
          break;
        }
        case "compEnd":
          textarea.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: step[1] }));
          markStart = -1;
          break;
        case "insert":
          for (const character of step[1]) {
            input("insertText", character, false, () => insertAtSelection(character));
          }
          break;
        case "paste": {
          const text = step[1];
          const clipboardData = new DataTransfer();
          clipboardData.setData("text/plain", text);
          const proceed = textarea.dispatchEvent(new ClipboardEvent("paste", {
            bubbles: true, cancelable: true, clipboardData,
          }));
          if (proceed) input("insertFromPaste", null, false, () => insertAtSelection(text));
          break;
        }
        case "selectAll":
          textarea.select();
          break;
        case "caretToStart":
          textarea.setSelectionRange(0, 0);
          break;
        case "cut":
          input("deleteByCut", null, false, () => insertAtSelection(""));
          break;
        case "down":
          key("keydown", step.slice(1) as [string, string, number, Modifiers?]);
          break;
        case "up":
          key("keyup", step.slice(1) as [string, string, number, Modifiers?]);
          break;
        case "press":
          key("keypress", step.slice(1) as [string, string, number, Modifiers?]);
          break;
        case "tick":
          await timerTurn();
          break;
      }
    }
    // Let every pending xterm and guard read run. No read schedules another.
    for (let turn = 0; turn < 3; turn += 1) await timerTurn();
  }, steps);
}

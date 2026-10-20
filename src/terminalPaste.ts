/** C0 controls except TAB, LF and CR, and the C1 controls U+0080 to U+009F. */
const PASTE_CONTROLS = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u0080-\u009f]/g;

/**
 * Remove control characters from text before a terminal paste.
 *
 * xterm.js 6 puts pasted text between ESC[200~ and ESC[201~ as is. Text that
 * contains ESC[201~ ends the bracketed paste early, and the shell runs the
 * rest as typed input. An 8-bit CSI (U+009B) can do the same in a program
 * that reads C1 controls. TAB, LF and CR stay, so a multi-line paste keeps
 * its lines.
 */
export function sanitizeTerminalPaste(text: string): string {
  return text.replace(PASTE_CONTROLS, "");
}

/**
 * Take each clipboard paste into one terminal from xterm.js, and paste its
 * text without control characters.
 *
 * xterm.js reads the clipboard in a paste listener on its textarea and in one
 * on its root element. The capture listeners of this filter run before them.
 * The composition guard also listens on the owner of the textarea, and it
 * sends a pending edit before the paste. Create this filter after the guard,
 * so that the guard listener runs first.
 */
export function createTerminalPasteFilter(
  root: EventTarget,
  textarea: HTMLTextAreaElement,
  paste: (text: string) => void,
) {
  const owner: EventTarget = textarea.parentElement ?? textarea;
  const takePaste = (event: Event) => {
    // The browser must not insert the text, and xterm.js must not paste it.
    event.preventDefault();
    event.stopImmediatePropagation();
    // xterm.js pastes an empty text too. A CLI can then read an image from
    // the clipboard.
    const text = (event as ClipboardEvent).clipboardData?.getData("text/plain");
    if (text !== undefined) paste(sanitizeTerminalPaste(text));
  };
  // A paste into the textarea passes the owner, and the owner listener takes
  // it. The root listener takes only the other pastes.
  const takeOtherPaste = (event: Event) => {
    const target = event.target as Node | null;
    if (target && (owner as Partial<Node>).contains?.(target)) return;
    takePaste(event);
  };
  owner.addEventListener("paste", takePaste, true);
  root.addEventListener("paste", takeOtherPaste, true);

  return {
    dispose() {
      owner.removeEventListener("paste", takePaste, true);
      root.removeEventListener("paste", takeOtherPaste, true);
    },
  };
}

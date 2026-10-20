const BACKSPACE = "\u007f";
const NO_BREAK_SPACE = /\u00a0/g;
const LINE_BREAK = /\r\n?|\n/g;
/** WebKit sends a stale keydown at most this long after the keyup of the same key. */
const LATE_KEYDOWN_MS = 150;
/** How long the guard keeps the text that it sent, to match a stale keydown. */
const SENT_TEXT_MS = 1000;
/** Edits whose text xterm.js already sent (paste) or that are not terminal input. */
const RESYNC_ONLY = new Set([
  "insertFromPaste",
  "insertFromPasteAsQuotation",
  "insertFromDrop",
  // Cmd+X cuts a textarea selection that the terminal line does not show.
  "deleteByCut",
  "historyUndo",
  "historyRedo",
]);

/** Translate a revised textarea value into edits for the terminal input line. */
export function terminalEditForTextareaChange(previous: string, next: string) {
  // WebKit keeps a trailing space in xterm's nowrap textarea as U+00A0 and
  // changes it back to a space when text follows it. That swap is not an edit.
  const previousCharacters = Array.from(previous.replace(NO_BREAK_SPACE, " "));
  const nextCharacters = Array.from(next.replace(NO_BREAK_SPACE, " "));
  let commonPrefixLength = 0;
  while (
    commonPrefixLength < previousCharacters.length &&
    commonPrefixLength < nextCharacters.length &&
    previousCharacters[commonPrefixLength] === nextCharacters[commonPrefixLength]
  ) {
    commonPrefixLength += 1;
  }

  return (
    BACKSPACE.repeat(previousCharacters.length - commonPrefixLength) +
    nextCharacters.slice(commonPrefixLength).join("")
  );
}

export type CompositionPreviewStyle = { fontFamily?: string; fontSize?: number };

/** The textarea value and selection before an edit. */
type TextareaRange = { value: string; start: number; end: number };

/**
 * Own every text edit in xterm's helper textarea and forward each edit once.
 *
 * xterm.js 6 reads this textarea in three places: the insertText handler, a
 * deferred composition read, and the keyCode 229 diff. After a main-thread
 * stall, WebKit delivers composition and text events before their keydowns.
 * The readers then overlap. The deferred read resends the accent and all text
 * after it. A stale composition end resends the text before the accent.
 *
 * This guard hides composition and input events from xterm. It forwards each
 * edit in DOM order, as a diff against the value the terminal already has.
 * The diff is correct only for edits at the end of the textarea. xterm puts
 * selected terminal text into the textarea and selects it, and Cmd+A or
 * Cmd+Left move the selection. The guard sends only the typed text of an edit
 * away from the end, and it moves the caret back to the end.
 */
export function createTerminalCompositionGuard(
  textarea: HTMLTextAreaElement,
  forwardInput: (data: string) => void = () => undefined,
  previewStyle: CompositionPreviewStyle = {},
) {
  let disposed = false;
  let composing = false;
  // The textarea value that the terminal input line already reflects.
  let forwarded = textarea.value;
  // xterm sends typed keys itself, and their text does not enter the
  // textarea. Text of `forwarded` before this character index is not at the
  // end of the terminal line, so an edit must not delete it.
  let directInputAt = Array.from(forwarded).length;
  // The textarea before a non-composing edit away from its end.
  let editAwayFromEnd: TextareaRange | null = null;
  let readPending = false;
  let readTimer: ReturnType<typeof setTimeout> | undefined;
  // After a stall, WebKit can deliver the text and the keyup of a key before
  // its keydown. These records find that late keydown.
  const heldKeys = new Set<string>();
  const keyUpsWithoutKeyDown = new Map<string, number>();
  let sentText: { character: string; time: number }[] = [];
  const owner: EventTarget = textarea.parentElement ?? textarea;
  const preview =
    (owner as Partial<ParentNode>).querySelector?.<HTMLElement>(".composition-view") ?? null;

  const rememberSentText = (data: string) => {
    const time = performance.now();
    sentText = sentText.filter((entry) => time - entry.time < SENT_TEXT_MS);
    for (const character of data) {
      if (character !== BACKSPACE) sentText.push({ character, time });
    }
  };
  const send = (edit: string) => {
    const data = edit.replace(LINE_BREAK, "\r");
    rememberSentText(data);
    forwardInput(data);
  };
  const forwardChange = () => {
    const next = textarea.value;
    const edit = terminalEditForTextareaChange(forwarded, next);
    forwarded = next;
    // A deletion can shorten the text before the typed keys.
    directInputAt = Math.min(directInputAt, Array.from(next).length);
    if (edit) send(edit);
  };
  // Use the textarea value as the base of the next diff. The terminal line
  // can lack that text, so no later edit deletes it.
  const adopt = () => {
    if (textarea.value === forwarded) return;
    forwarded = textarea.value;
    directInputAt = Array.from(forwarded).length;
  };
  const resync = () => {
    if (!composing && !readPending) adopt();
  };
  const caretAtEnd = () => {
    const end = textarea.value.length;
    return textarea.selectionStart === end && textarea.selectionEnd === end;
  };
  const moveCaretToEnd = () => {
    if (caretAtEnd()) return;
    const end = textarea.value.length;
    textarea.setSelectionRange(end, end);
  };
  /** True when the edit deletes text that typed keys moved away from the end of the line. */
  const deletesBeforeDirectInput = () => {
    const edit = terminalEditForTextareaChange(forwarded, textarea.value);
    const deleted = edit.length - edit.replace(/^\u007f+/, "").length;
    return Array.from(forwarded).length - deleted < directInputAt;
  };
  const cancelRead = () => {
    readPending = false;
    clearTimeout(readTimer);
  };
  // Send the edit of a keyCode 229 key before a later event changes the
  // textarea. xterm clears it on Enter, Ctrl+C and paste.
  const flushRead = () => {
    if (!readPending) return;
    cancelRead();
    if (!composing) forwardChange();
  };
  // One deferred read at a time. It catches edits that arrive without input
  // events, and commits whose compositionend carries no data.
  const readLater = () => {
    if (readPending) return;
    readPending = true;
    readTimer = setTimeout(() => {
      readPending = false;
      if (!disposed && !composing) forwardChange();
    }, 0);
  };
  /**
   * True for a keydown that arrives after the keyup of the same key, when this
   * guard already sent the text of that key. xterm would send the text again.
   */
  const isLateKeyDown = (event: KeyboardEvent) => {
    const time = performance.now();
    const keyUpTime = event.code ? keyUpsWithoutKeyDown.get(event.code) : undefined;
    const character = Array.from(event.key ?? "").length === 1 ? event.key : undefined;
    const index = sentText.findIndex(
      (entry) => entry.character === character && time - entry.time < SENT_TEXT_MS,
    );
    // Each keydown consumes the text that it typed.
    if (index >= 0) sentText.splice(index, 1);
    return (
      keyUpTime !== undefined &&
      time - keyUpTime < LATE_KEYDOWN_MS &&
      event.keyCode !== 229 &&
      index >= 0
    );
  };
  const showPreview = (text: string) => {
    if (!preview) return;
    preview.textContent = text;
    preview.classList.toggle("active", text.length > 0);
    if (!text) return;
    // xterm keeps the textarea on the cursor cell when it is not composing.
    preview.style.left = textarea.style.left;
    preview.style.top = textarea.style.top;
    preview.style.height = textarea.style.height;
    preview.style.lineHeight = textarea.style.lineHeight || textarea.style.height;
    if (previewStyle.fontFamily) preview.style.fontFamily = previewStyle.fontFamily;
    if (previewStyle.fontSize) preview.style.fontSize = `${previewStyle.fontSize}px`;
  };
  const own = (event: Event) => {
    if (disposed || event.target !== textarea) return false;
    // xterm must not read this edit from the textarea.
    event.stopImmediatePropagation();
    return true;
  };

  const compositionStart = (event: Event) => {
    if (!own(event)) return;
    // Send an edit that a keyCode 229 key made without an input event.
    flushRead();
    // The browser reads the selection for the composition after this event.
    moveCaretToEnd();
    resync();
    composing = true;
    showPreview("");
  };
  const compositionUpdate = (event: Event) => {
    if (!own(event)) return;
    showPreview((event as CompositionEvent).data ?? "");
  };
  const compositionEnd = (event: Event) => {
    if (!own(event)) return;
    composing = false;
    showPreview("");
    // The browser has already written the commit into the textarea.
    if ((event as CompositionEvent).data) forwardChange();
    // No data: a focus change cancels the composition, and blur then clears
    // the textarea before this read runs. Other commits are read then.
    else readLater();
  };
  const beforeInput = (event: Event) => {
    if (disposed || event.target !== textarea) return;
    const inputEvent = event as InputEvent;
    if (composing || inputEvent.isComposing) return;
    flushRead();
    resync();
    // The browser has already set the range of this edit. A dictation
    // revision selects the dictated text that it replaces.
    editAwayFromEnd =
      inputEvent.inputType === "insertReplacementText" || caretAtEnd()
        ? null
        : { value: textarea.value, start: textarea.selectionStart, end: textarea.selectionEnd };
  };
  /** Send only the text that an edit away from the end put in place of its range. */
  const forwardInsertedText = (before: TextareaRange) => {
    const previous = before.value.replace(NO_BREAK_SPACE, " ");
    const next = textarea.value.replace(NO_BREAK_SPACE, " ");
    const kept = previous.slice(before.end);
    if (next.startsWith(previous.slice(0, before.start)) && next.endsWith(kept)) {
      const inserted = next.slice(before.start, next.length - kept.length);
      if (inserted) send(inserted);
    }
    // The terminal line does not show the replaced text. A deletion of it
    // sends nothing.
    adopt();
    moveCaretToEnd();
  };
  const input = (event: Event) => {
    if (!own(event)) return;
    const inputEvent = event as InputEvent;
    if (composing || inputEvent.isComposing) return;
    const before = editAwayFromEnd;
    editAwayFromEnd = null;
    if (RESYNC_ONLY.has(inputEvent.inputType)) {
      adopt();
      return;
    }
    if (before) {
      forwardInsertedText(before);
      return;
    }
    if (inputEvent.inputType === "insertReplacementText" && deletesBeforeDirectInput()) {
      // Keys typed after the dictated text keep their place. The revision is lost.
      adopt();
      return;
    }
    forwardChange();
  };
  // xterm clears the textarea on Enter, Ctrl+C, blur and paste, without input
  // events. These listeners run after xterm's own listeners on the textarea.
  const afterKeyDown = (event: Event) => {
    if (disposed) return;
    const { code } = event as KeyboardEvent;
    // A late keydown pairs with a keyup that has already arrived.
    if (code && !keyUpsWithoutKeyDown.delete(code)) heldKeys.add(code);
    resync();
  };
  const afterKeyUp = (event: Event) => {
    const keyEvent = event as KeyboardEvent;
    if (disposed || !keyEvent.code || heldKeys.delete(keyEvent.code)) return;
    // The app menu takes Cmd and Ctrl shortcuts. Only their keyup arrives here.
    if (keyEvent.metaKey || keyEvent.ctrlKey) return;
    keyUpsWithoutKeyDown.set(keyEvent.code, performance.now());
  };
  const beforePaste = (event: Event) => {
    if (!disposed && event.target === textarea) flushRead();
  };
  const afterPaste = () => {
    if (!disposed) resync();
  };
  const afterBlur = () => {
    if (disposed) return;
    cancelRead();
    composing = false;
    showPreview("");
    adopt();
  };

  // The owner listeners run before xterm's listeners on the textarea.
  owner.addEventListener("paste", beforePaste, true);
  owner.addEventListener("compositionstart", compositionStart, true);
  owner.addEventListener("compositionupdate", compositionUpdate, true);
  owner.addEventListener("compositionend", compositionEnd, true);
  owner.addEventListener("beforeinput", beforeInput, true);
  owner.addEventListener("input", input, true);
  textarea.addEventListener("keydown", afterKeyDown, true);
  textarea.addEventListener("keyup", afterKeyUp, true);
  textarea.addEventListener("blur", afterBlur);
  textarea.addEventListener("paste", afterPaste);

  return {
    accept(event: KeyboardEvent): boolean {
      if (disposed) return true;
      if (event.type === "keypress") {
        // WebKit: after a dead key that cannot combine, one keypress carries
        // both characters ("'d") and the dead character as charCode. The
        // browser then inserts only "d", and the input event forwards it.
        if (Array.from(event.key ?? "").length > 1) return false;
        // xterm sends keypress text itself. Keep that text out of the
        // textarea, so the textarea holds only edits that this guard sends.
        event.preventDefault?.();
        directInputAt = Array.from(forwarded).length;
        return true;
      }
      if (event.type !== "keydown") return true;
      if (isLateKeyDown(event)) {
        // The input event already sent this text. Block a second send.
        event.preventDefault?.();
        return false;
      }
      if (event.keyCode !== 229) {
        // xterm handles the key after this call. It sends the key past the
        // textarea, and Enter or Ctrl+C clear the textarea.
        flushRead();
        directInputAt = Array.from(forwarded).length;
        return true;
      }
      if (!composing && !event.isComposing && !readPending) {
        adopt();
        readLater();
      }
      // xterm's keyCode 229 path diffs whole textarea values. Composition
      // and input events forward these edits.
      return false;
    },
    dispose() {
      disposed = true;
      cancelRead();
      showPreview("");
      owner.removeEventListener("paste", beforePaste, true);
      owner.removeEventListener("compositionstart", compositionStart, true);
      owner.removeEventListener("compositionupdate", compositionUpdate, true);
      owner.removeEventListener("compositionend", compositionEnd, true);
      owner.removeEventListener("beforeinput", beforeInput, true);
      owner.removeEventListener("input", input, true);
      textarea.removeEventListener("keydown", afterKeyDown, true);
      textarea.removeEventListener("keyup", afterKeyUp, true);
      textarea.removeEventListener("blur", afterBlur);
      textarea.removeEventListener("paste", afterPaste);
    },
  };
}

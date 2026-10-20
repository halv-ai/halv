import assert from "node:assert/strict";
import test from "node:test";
import {
  createTerminalCompositionGuard,
  terminalEditForTextareaChange,
} from "../src/terminalComposition.ts";

/** A textarea with a selection. A new value puts the caret at its end, as in the browser. */
function textareaStub(value = "") {
  const textarea = new EventTarget() as HTMLTextAreaElement;
  let text = value;
  let selection = [value.length, value.length];
  Object.defineProperties(textarea, {
    value: {
      get: () => text,
      set: (next: string) => {
        text = next;
        selection = [next.length, next.length];
      },
    },
    selectionStart: { get: () => selection[0] },
    selectionEnd: { get: () => selection[1] },
  });
  Object.assign(textarea, {
    parentElement: null,
    style: {},
    setSelectionRange: (start: number, end: number) => {
      selection = [start, end];
    },
    select: () => {
      selection = [0, text.length];
    },
  });
  return textarea;
}

function keyEvent(
  keyCode: number,
  options: { isComposing?: boolean; type?: string } = {},
) {
  return {
    isComposing: options.isComposing ?? false,
    keyCode,
    type: options.type ?? "keydown",
  } as KeyboardEvent;
}

function domEvent(type: string, fields: Record<string, unknown> = {}) {
  return Object.assign(new Event(type), fields);
}

/** Replace the selection with text and put the caret after it. */
function replaceSelection(textarea: HTMLTextAreaElement, text: string) {
  const { value, selectionStart, selectionEnd } = textarea;
  textarea.value = value.slice(0, selectionStart) + text + value.slice(selectionEnd);
  textarea.setSelectionRange(selectionStart + text.length, selectionStart + text.length);
}

/**
 * WebKit order for one dead key: the textarea holds the commit at
 * compositionend. The composition replaces the selection that the browser
 * reads after compositionstart.
 */
function commitDeadKey(
  textarea: HTMLTextAreaElement,
  marked: string,
  committed: string,
) {
  textarea.dispatchEvent(domEvent("compositionstart", { data: "" }));
  const { value, selectionStart, selectionEnd } = textarea;
  const head = value.slice(0, selectionStart);
  const tail = value.slice(selectionEnd);
  textarea.dispatchEvent(domEvent("compositionupdate", { data: marked }));
  textarea.value = head + marked + tail;
  textarea.setSelectionRange(head.length + marked.length, head.length + marked.length);
  textarea.dispatchEvent(domEvent("input", {
    inputType: "insertCompositionText", data: marked, isComposing: true,
  }));
  textarea.value = head + committed + tail;
  textarea.setSelectionRange(head.length + committed.length, head.length + committed.length);
  textarea.dispatchEvent(domEvent("input", {
    inputType: "insertFromComposition", data: committed, isComposing: true,
  }));
  textarea.dispatchEvent(domEvent("compositionend", { data: committed }));
}

/** One non-composing edit at the selection, with its beforeinput and input events. */
function edit(
  textarea: HTMLTextAreaElement,
  inputType: string,
  data: string | null,
  change: () => void,
) {
  const fields = { inputType, data, isComposing: false };
  textarea.dispatchEvent(domEvent("beforeinput", fields));
  change();
  textarea.dispatchEvent(domEvent("input", fields));
}

function insertText(textarea: HTMLTextAreaElement, text: string) {
  for (const character of text) {
    edit(textarea, "insertText", character, () => replaceSelection(textarea, character));
  }
}

function applyTerminalEdits(data: string) {
  const text: string[] = [];
  for (const character of data) {
    if (character === "\u007f") text.pop();
    else text.push(character);
  }
  return text.join("");
}

async function flushTextareaRead() {
  await new Promise((resolve) => setTimeout(resolve, 5));
}

test("translates revised textarea text into terminal line edits", () => {
  const previous = "EH SOH UM ATALHO PRA ABRIR";
  const next = "EH SOH UM ATALHO PARA ABRIR";
  const edits = terminalEditForTextareaChange(previous, next);

  assert.equal(applyTerminalEdits(previous + edits), next);
  assert.equal(terminalEditForTextareaChange("ação", "ação"), "");
  assert.equal(terminalEditForTextareaChange("👍a", "👍b"), "\u007fb");
  // WebKit changes a trailing U+00A0 back to a space when text follows it.
  assert.equal(terminalEditForTextareaChange("O\u00a0", "O á"), "á");
});

test("coalesces fallback keys and forwards the final textarea value", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => {
    forwarded.push(data);
  });
  const fallbackKey = keyEvent(229);

  assert.equal(guard.accept(fallbackKey), false);
  textarea.value = "hello";
  assert.equal(guard.accept(fallbackKey), false);
  textarea.value = "hello world";
  await flushTextareaRead();

  assert.deepEqual(forwarded, ["hello world"]);
  guard.dispose();
});

test("forwards normal keys and owns composition, so the commit is sent once", async () => {
  const textarea = textareaStub("O\u00a0");
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  assert.equal(guard.accept(keyEvent(65)), true);
  assert.equal(guard.accept(keyEvent(65, { type: "keyup" })), true);
  commitDeadKey(textarea, "'", "á");
  // xterm's keyCode 229 path diffs whole textarea values. It must not run.
  assert.equal(guard.accept(keyEvent(229, { isComposing: true })), false);
  assert.equal(guard.accept(keyEvent(229)), false);
  await flushTextareaRead();

  assert.deepEqual(forwarded, ["á"]);
  guard.dispose();
});

test("stall: text that WebKit inserts after a commit is sent once, in order", async () => {
  const textarea = textareaStub("O\u00a0");
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  // Queued keys: the composition and insertText events arrive before any keydown.
  commitDeadKey(textarea, "'", "á");
  insertText(textarea, "gora tudo bem");
  for (const _ of "gora tudo bem") assert.equal(guard.accept(keyEvent(229)), false);
  await flushTextareaRead();

  assert.equal(forwarded.join(""), "ágora tudo bem");
  guard.dispose();
});

test("stall: Enter that WebKit inserts as text is sent once", async () => {
  const textarea = textareaStub("O\u00a0");
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  commitDeadKey(textarea, "'", "á");
  insertText(textarea, "bc\rxy");
  await flushTextareaRead();

  assert.equal(forwarded.join(""), "ábc\rxy");
  guard.dispose();
});

test("a focus change cancels a pending dead key without sending it", async () => {
  const textarea = textareaStub("O\u00a0");
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  textarea.dispatchEvent(domEvent("compositionstart", { data: "" }));
  textarea.value = "O '";
  textarea.dispatchEvent(domEvent("input", {
    inputType: "insertCompositionText", data: "'", isComposing: true,
  }));
  // WebKit sends compositionend without data, then blur. xterm clears the
  // textarea on blur.
  textarea.dispatchEvent(domEvent("compositionend", { data: "" }));
  textarea.value = "";
  textarea.dispatchEvent(domEvent("blur"));
  await flushTextareaRead();

  assert.deepEqual(forwarded, []);
  guard.dispose();
});

test("text that xterm already sent is not forwarded again", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));
  let prevented = false;
  const keypress = {
    type: "keypress",
    keyCode: 79,
    isComposing: false,
    preventDefault: () => { prevented = true; },
  };

  // xterm sends the keypress text. The guard keeps it out of the textarea.
  assert.equal(guard.accept(keypress as unknown as KeyboardEvent), true);
  assert.equal(prevented, true);
  // xterm's paste handler sends the clipboard text. Then the browser
  // inserts the same text into the textarea.
  textarea.dispatchEvent(domEvent("paste"));
  textarea.dispatchEvent(domEvent("beforeinput", {
    inputType: "insertFromPaste", isComposing: false,
  }));
  textarea.value = "ação ação";
  textarea.dispatchEvent(domEvent("input", {
    inputType: "insertFromPaste", isComposing: false,
  }));
  await flushTextareaRead();

  assert.deepEqual(forwarded, []);
  guard.dispose();
});

test("a dead key before a consonant sends the dead character once and keeps the consonant", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));
  let prevented = false;

  commitDeadKey(textarea, "'", "'");
  // Native WebKit order: one keypress with key "'d" and charCode 39.
  const keypress = {
    type: "keypress",
    key: "'d",
    keyCode: 39,
    isComposing: false,
    preventDefault: () => { prevented = true; },
  };
  assert.equal(guard.accept(keypress as unknown as KeyboardEvent), false);
  assert.equal(prevented, false);
  insertText(textarea, "d");
  await flushTextareaRead();

  assert.equal(forwarded.join(""), "'d");
  guard.dispose();
});

test("a textarea cleared by xterm on Enter is not read as deletions", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  commitDeadKey(textarea, "ˆ", "ê");
  textarea.value = "";
  textarea.dispatchEvent(domEvent("keydown"));
  assert.equal(guard.accept(keyEvent(229)), false);
  textarea.value = "x";
  await flushTextareaRead();

  assert.deepEqual(forwarded, ["ê", "x"]);
  guard.dispose();
});

test("cancels a pending read when the terminal pane disposes", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => {
    forwarded.push(data);
  });

  assert.equal(guard.accept(keyEvent(229)), false);
  textarea.value = "discarded";
  guard.dispose();
  await flushTextareaRead();

  assert.deepEqual(forwarded, []);
  assert.equal(guard.accept(keyEvent(229)), true);
});

function keyboard(
  type: string,
  fields: { key: string; code: string; keyCode: number; metaKey?: boolean },
) {
  let prevented = false;
  const event = {
    type,
    isComposing: false,
    metaKey: false,
    ctrlKey: false,
    ...fields,
    preventDefault: () => {
      prevented = true;
    },
  };
  return { event: event as unknown as KeyboardEvent, prevented: () => prevented };
}

/** xterm's keydown listener calls accept first. The guard's own listener runs after it. */
function keyDown(
  textarea: HTMLTextAreaElement,
  guard: ReturnType<typeof createTerminalCompositionGuard>,
  fields: { key: string; code: string; keyCode: number },
) {
  const down = keyboard("keydown", fields);
  const accepted = guard.accept(down.event);
  textarea.dispatchEvent(domEvent("keydown", { code: fields.code }));
  return { accepted, prevented: down.prevented() };
}

function keyUp(textarea: HTMLTextAreaElement, code: string, metaKey = false) {
  textarea.dispatchEvent(domEvent("keyup", { code, metaKey, ctrlKey: false }));
}

test("stall: a keydown that WebKit sends after its own keyup does not send the text again", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  // Native WebKit order after a stall: the commit and the queued text arrive
  // first, then the keyups, then the late keydowns.
  commitDeadKey(textarea, "'", "á");
  insertText(textarea, " Bc ");
  keyUp(textarea, "Quote");
  keyUp(textarea, "KeyA");
  for (const [key, code] of [[" ", "Space"], ["B", "KeyB"], ["c", "KeyC"], [" ", "Space"]]) {
    assert.equal(keyDown(textarea, guard, { key, code, keyCode: 229 }).accepted, false);
    keyUp(textarea, code);
  }
  assert.equal(
    keyDown(textarea, guard, { key: "Dead", code: "Quote", keyCode: 229 }).accepted,
    false,
  );
  // The user typed "D" after the stall. xterm sends it, as usual.
  assert.equal(
    keyDown(textarea, guard, { key: "D", code: "KeyD", keyCode: 68 }).accepted,
    true,
  );
  // The vowel of the dead key arrives last, as key "á" with keyCode 65.
  const late = keyDown(textarea, guard, { key: "á", code: "KeyA", keyCode: 65 });
  await flushTextareaRead();

  assert.equal(late.accepted, false);
  assert.equal(late.prevented, true);
  assert.equal(forwarded.join(""), "á Bc ");
  guard.dispose();
});

test("a keyup from a Cmd shortcut does not block the next keystroke", () => {
  const textarea = textareaStub();
  const guard = createTerminalCompositionGuard(textarea);

  // The app menu takes Cmd+V. Only the keyup gets to the textarea.
  keyUp(textarea, "KeyV", true);
  assert.equal(
    keyDown(textarea, guard, { key: "v", code: "KeyV", keyCode: 86 }).accepted,
    true,
  );
  guard.dispose();
});

test("a fast keystroke after a lone keyup is sent when the guard did not send its text", () => {
  const textarea = textareaStub();
  const guard = createTerminalCompositionGuard(textarea);

  // Focus moved between keydown and keyup, so this pane gets only the keyup.
  keyUp(textarea, "KeyA");
  assert.equal(
    keyDown(textarea, guard, { key: "a", code: "KeyA", keyCode: 65 }).accepted,
    true,
  );
  guard.dispose();
});

test("a normal-speed dead key does not block a later direct key with the same text", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  // Normal order: the vowel keydown (keyCode 229) arrives after the commit.
  commitDeadKey(textarea, "'", "á");
  assert.equal(
    keyDown(textarea, guard, { key: "á", code: "KeyA", keyCode: 229 }).accepted,
    false,
  );
  keyUp(textarea, "KeyA");
  keyUp(textarea, "Quote");
  // A layout with a direct key for the same character.
  assert.equal(
    keyDown(textarea, guard, { key: "á", code: "KeyQ", keyCode: 81 }).accepted,
    true,
  );
  await flushTextareaRead();

  assert.deepEqual(forwarded, ["á"]);
  guard.dispose();
});

/** xterm's right-click handler puts the selected word into the textarea and selects it. */
function rightClickWord(textarea: HTMLTextAreaElement, word: string) {
  textarea.value = word;
  textarea.select();
}

test("a right-clicked word in the textarea is not deleted by the next commit or emoji", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  rightClickWord(textarea, "arquivo");
  commitDeadKey(textarea, "'", "é");
  rightClickWord(textarea, "Add");
  insertText(textarea, "👍");
  await flushTextareaRead();

  assert.deepEqual(forwarded, ["é", "👍"]);
  guard.dispose();
});

test("a commit after Cmd+A or Cmd+Left sends only the typed text", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  commitDeadKey(textarea, "˜", "ã");
  commitDeadKey(textarea, "'", "é");
  // Edit > Select All selects the textarea.
  textarea.select();
  commitDeadKey(textarea, "ˆ", "ê");
  // Cmd+Left moves the caret to the start of the textarea.
  textarea.setSelectionRange(0, 0);
  commitDeadKey(textarea, "'", "á");
  textarea.setSelectionRange(0, 0);
  insertText(textarea, "xy");
  await flushTextareaRead();

  assert.deepEqual(forwarded, ["ã", "é", "ê", "á", "x", "y"]);
  guard.dispose();
});

test("Cmd+X of a textarea selection sends nothing", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  commitDeadKey(textarea, "˜", "ã");
  textarea.select();
  edit(textarea, "deleteByCut", null, () => replaceSelection(textarea, ""));
  commitDeadKey(textarea, "'", "é");
  await flushTextareaRead();

  assert.deepEqual(forwarded, ["ã", "é"]);
  guard.dispose();
});

test("a dictation revision replaces the dictated text", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  edit(textarea, "insertText", "olá mundo", () => replaceSelection(textarea, "olá mundo"));
  // The revision selects the dictated word that it replaces.
  textarea.setSelectionRange(4, 9);
  edit(textarea, "insertReplacementText", null, () => replaceSelection(textarea, "Mundo"));
  await flushTextareaRead();

  assert.equal(applyTerminalEdits(forwarded.join("")), "olá Mundo");
  guard.dispose();
});

test("a dictation revision does not delete keys typed after the dictated text", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  edit(textarea, "insertText", "olá mundo", () => replaceSelection(textarea, "olá mundo"));
  // xterm sends " sim" from the keydowns. The text does not enter the textarea.
  for (const [key, code, keyCode] of [[" ", "Space", 32], ["s", "KeyS", 83]] as const) {
    assert.equal(keyDown(textarea, guard, { key, code, keyCode }).accepted, true);
  }
  textarea.setSelectionRange(4, 9);
  edit(textarea, "insertReplacementText", null, () => replaceSelection(textarea, "Mundo"));
  await flushTextareaRead();

  assert.deepEqual(forwarded, ["olá mundo"]);
  guard.dispose();
});

test("Enter between a keyCode 229 key and its read does not send deletions", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  commitDeadKey(textarea, "'", "á");
  insertText(textarea, " Bc");
  // One burst after a stall, with no timer turn: a late keyCode 229 keydown,
  // then Enter, then "x".
  assert.equal(guard.accept(keyEvent(229)), false);
  assert.equal(guard.accept(keyEvent(13)), true);
  // xterm sends the Enter and clears the textarea.
  textarea.value = "";
  textarea.dispatchEvent(domEvent("keydown", { code: "Enter" }));
  assert.equal(guard.accept(keyEvent(88)), true);
  textarea.dispatchEvent(domEvent("keydown", { code: "KeyX" }));
  await flushTextareaRead();

  assert.equal(forwarded.join(""), "á Bc");
  guard.dispose();
});

test("a paste between a keyCode 229 key and its read does not send deletions", async () => {
  const textarea = textareaStub();
  const forwarded: string[] = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => forwarded.push(data));

  commitDeadKey(textarea, "'", "á");
  assert.equal(guard.accept(keyEvent(229)), false);
  // The paste listener of the guard runs first. Then xterm sends the
  // clipboard text and clears the textarea.
  const paste = domEvent("paste");
  textarea.addEventListener("paste", () => { textarea.value = ""; }, { once: true });
  textarea.dispatchEvent(paste);
  await flushTextareaRead();

  assert.deepEqual(forwarded, ["á"]);
  guard.dispose();
});

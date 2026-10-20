import { expect, test, type Page } from "@playwright/test";
import {
  openHarnessTerminal,
  queuedKeydowns,
  runBurst,
  writes,
  type Step,
} from "./webkitInput";

// Dead keys in macOS WKWebView, with the event orders that real NSEvents
// gave in a native WKWebView. When the app main thread stalls, WebKit sends
// the queued keys as text before their keydowns. xterm.js then sent the
// accent and the text after it again, or the text before it. Each typed
// character must reach the PTY once. Run this file with --browser=webkit too.

// Dead acute + "a" (Brazilian - Pro) behind a stall: the composition comes
// first, before any keydown.
const acuteA: Step[] = [
  ["compStart"], ["compUpdate", "'"], ["mark", "'"],
  ["up", "'", "Quote", 222, { composing: true }],
  ["markDelete"], ["commit", "á"], ["compEnd", "á"],
];
// Dead acute + Shift+E behind a stall.
const acuteCapitalE: Step[] = [
  ["compStart"], ["compUpdate", "'"], ["mark", "'"],
  ["markDelete"], ["commit", "É"], ["compEnd", "É"],
];
const circumflexE: Step[] = [
  ["compStart"], ["compUpdate", "ˆ"], ["mark", "ˆ"],
  ["markDelete"], ["commit", "ê"], ["compEnd", "ê"],
];

async function sent(page: Page, sessionId: string) {
  return (await writes(page, sessionId)).join("");
}

/** WebKit can keep a space in the textarea as U+00A0. */
function withSpaces(text: string) {
  return text.replace(/\u00a0/g, " ");
}

/** Compare the characters only, when WebKit itself changed the key order. */
function sortedCharacters(text: string) {
  return Array.from(withSpaces(text)).sort().join("");
}

function lineBreaks(text: string) {
  return text.match(/[\r\n]/g)?.length ?? 0;
}

test("a dead key at normal pace sends the accented character once", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Oi ");
  // The native order of dead acute + "a", with a timer turn between events.
  await runBurst(textarea, [
    ["compStart"], ["compUpdate", "'"], ["mark", "'"],
    ["down", "'", "Quote", 229, { composing: true }], ["tick"],
    ["up", "'", "Quote", 222, { composing: true }], ["tick"],
    ["markDelete"], ["commit", "á"], ["compEnd", "á"],
    ["down", "a", "KeyA", 229], ["tick"], ["up", "a", "KeyA", 65], ["tick"],
  ]);
  await page.keyboard.type("gora tudo bem");

  expect(await sent(page, sessionId)).toBe("Oi ágora tudo bem");
});

test("text queued behind a stall after a dead key is sent once", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Oi ");
  const rest = "gora tudo bem com voce";
  await runBurst(textarea, [
    ...acuteA,
    // The queued text arrives before its keydowns.
    ["insert", rest],
    // The deferred composition read of xterm.js runs here.
    ["tick"],
    ["up", "a", "KeyA", 65],
    ...queuedKeydowns(rest),
    ["down", "Dead", "Quote", 229], ["down", "á", "KeyA", 229],
  ]);

  expect(await sent(page, sessionId)).toBe("Oi ágora tudo bem com voce");
});

test("a timer turn after the commit keeps the queued text once", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Oi ");
  const rest = "gora tudo bem com voce";
  await runBurst(textarea, [
    ...acuteA, ["tick"],
    ["insert", rest], ["tick"],
    ["up", "a", "KeyA", 65],
    ...queuedKeydowns(rest),
  ]);

  expect(await sent(page, sessionId)).toBe("Oi ágora tudo bem com voce");
});

test("an Enter queued behind a stall after a dead key submits once", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Oi ");
  await runBurst(textarea, [
    ...acuteA,
    // WebKit sends the queued Enter as insertText "\r".
    ["insert", "bc\rxy"],
    ["tick"],
    ["up", "a", "KeyA", 65],
    ...queuedKeydowns("bc\rxy"),
    ["down", "Dead", "Quote", 229], ["down", "á", "KeyA", 229],
  ]);
  const data = await sent(page, sessionId);

  expect(lineBreaks(data)).toBe(1);
  expect(data.replace(/\n/g, "\r")).toBe("Oi ábc\rxy");
});

test("a late composition end does not resend the text before the accent", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  // xterm.js keeps capitals and spaces in its textarea until Enter.
  const prefix = "Pedro, Tudo Bem? Oi ";
  await page.keyboard.type(prefix);
  const rest = " pode ver isso";
  // The "voc" keydowns queued before the dead key. WebKit sends them, with
  // their real key codes, after the text of the later keys.
  await runBurst(textarea, [
    ...circumflexE,
    ["insert", rest],
    ["down", "v", "KeyV", 86], ["up", "v", "KeyV", 86],
    ["down", "o", "KeyO", 79], ["up", "o", "KeyO", 79],
    ["down", "c", "KeyC", 67], ["up", "c", "KeyC", 67],
    ["tick"],
    ["up", "ˆ", "Digit6", 54], ["up", "e", "KeyE", 69],
    ...queuedKeydowns(rest),
    ["down", "Dead", "Digit6", 229], ["down", "ê", "KeyE", 229],
  ]);

  expect(sortedCharacters(await sent(page, sessionId)))
    .toBe(sortedCharacters(`${prefix}você pode ver isso`));
});

test("a late composition end does not resend an all-caps prompt", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  const prefix = "REVISE O PR AGORA ";
  await page.keyboard.type(prefix);
  await runBurst(textarea, [
    ...circumflexE,
    ["insert", "s"],
    ["down", "v", "KeyV", 86], ["up", "v", "KeyV", 86],
    ["tick"],
    ...queuedKeydowns("s"),
  ]);

  expect(sortedCharacters(await sent(page, sessionId)))
    .toBe(sortedCharacters(`${prefix}êsv`));
});

test("Cmd during a dead key, then an in-place commit, sends the commit once", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("OI PEDRO ");
  await runBurst(textarea, [
    ["compStart"], ["compUpdate", "'"], ["mark", "'"],
    ["down", "Meta", "MetaLeft", 91, { meta: true }], ["up", "Meta", "MetaLeft", 91],
    // The native traces do not show this order: the dead keydown after Cmd.
    ["down", "'", "Quote", 229, { composing: true }],
    ["markDelete"], ["commit", "á"], ["compEnd", "á"],
    ["tick"],
    ["down", "a", "KeyA", 229], ["up", "a", "KeyA", 65],
  ]);

  expect(withSpaces(await sent(page, sessionId))).toBe("OI PEDRO á");
});

test("Cmd during a pending dead key sends the dead character once", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Ola ");
  // The native order of a dead acute and a Cmd shortcut, at normal pace.
  await runBurst(textarea, [
    ["compStart"], ["compUpdate", "'"], ["mark", "'"],
    ["down", "'", "Quote", 229, { composing: true }], ["tick"],
    ["up", "'", "Quote", 222, { composing: true }], ["tick"],
    ["down", "Meta", "MetaLeft", 91, { meta: true }], ["tick"],
    ["markDelete"], ["commit", "'"], ["compEnd", "'"], ["tick"],
    ["up", "Meta", "MetaLeft", 91], ["tick"],
  ]);

  expect(await sent(page, sessionId)).toBe("Ola '");
});

test("a stall does not send the whole prompt again after a dead key", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Pedro, Tudo Bem? ");
  const rest = " isso mesmo voce pode corrigir o teste";
  // The native order: the commit, all queued text, the deferred composition
  // read of xterm.js, then the keyups and the late keydowns.
  await runBurst(textarea, [
    ...acuteCapitalE,
    ["insert", rest],
    ["tick"],
    ["up", "'", "Quote", 222], ["up", "E", "KeyE", 69, { shift: true }],
    ...queuedKeydowns(rest),
    ["down", "Dead", "Quote", 229], ["down", "É", "KeyE", 229, { shift: true }],
  ]);
  const chunks = await writes(page, sessionId);

  expect(withSpaces(chunks.join(""))).toBe(`Pedro, Tudo Bem? É${rest}`);
  // No single PTY write carries the typed text a second time.
  expect(chunks.filter((chunk) => chunk.length > 1 && chunk.includes("isso mesmo voce")))
    .toEqual([]);
});

test("an Enter queued after a dead key submits the prompt once", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Pedro, Tudo Bem? ");
  const rest = " isso mesmo\r";
  await runBurst(textarea, [
    ...acuteCapitalE,
    ["insert", rest],
    ["tick"],
    ["up", "'", "Quote", 222], ["up", "E", "KeyE", 69, { shift: true }],
    ...queuedKeydowns(rest),
    ["down", "Dead", "Quote", 229], ["down", "É", "KeyE", 229, { shift: true }],
  ]);
  const data = await sent(page, sessionId);

  expect(lineBreaks(data)).toBe(1);
  expect(withSpaces(data).replace(/\n/g, "\r")).toBe("Pedro, Tudo Bem? É isso mesmo\r");
});

test("a keydown that WebKit sends after its own keyup does not resend the vowel", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Oi ");
  await runBurst(textarea, [
    ["compStart"], ["compUpdate", "'"], ["mark", "'"],
    ["markDelete"], ["commit", "á"], ["compEnd", "á"],
    ["insert", " Bc "],
    ["tick"],
    ["up", "'", "Quote", 222], ["up", "a", "KeyA", 65],
    ...queuedKeydowns(" Bc "),
    ["down", "Dead", "Quote", 229],
    // The user typed "D" after the stall. Its keydown and keypress arrive in order.
    ["down", "D", "KeyD", 68, { shift: true }], ["press", "D", "KeyD", 68, { shift: true }],
    // The late vowel keydown has the key "á" and keyCode 65, not 229.
    ["down", "á", "KeyA", 65],
    ["up", "D", "KeyD", 68, { shift: true }],
  ]);
  await page.keyboard.type("e");

  expect(withSpaces(await sent(page, sessionId))).toBe("Oi á Bc De");
});

test("ordinary typing and special keys send their usual bytes", async ({ page }) => {
  const { sessionId } = await openHarnessTerminal(page);
  await page.keyboard.type("Pedro, Tudo Bem? ");
  // The US layout has no "é". Playwright inserts it as text, without a keydown.
  await page.keyboard.type("é");
  await page.keyboard.press("Backspace");
  await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("Tab");
  await page.keyboard.type("ABC x 123 !@#");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Control+C");

  expect(await sent(page, sessionId))
    .toBe("Pedro, Tudo Bem? é\u007f\u001b[D\tABC x 123 !@#\r\u0003");
});

test("a pending dead key shows the composition preview", async ({ page }) => {
  const { pane, textarea } = await openHarnessTerminal(page);
  const preview = pane.locator(".composition-view");
  await page.keyboard.type("Oi ");
  await runBurst(textarea, [["compStart"], ["compUpdate", "'"], ["mark", "'"]]);

  await expect(preview).toBeVisible();
  await expect(preview).toHaveText("'");
  await runBurst(textarea, [["markDelete"], ["commit", "á"], ["compEnd", "á"]]);
  await expect(preview).toBeHidden();
});

test("text that the browser inserts on paste is not sent again", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Oi ");
  await runBurst(textarea, [["paste", "ação ação"]]);
  // A dead key after the paste does not resend the pasted text.
  await runBurst(textarea, [
    ["compStart"], ["compUpdate", "'"], ["mark", "'"],
    ["markDelete"], ["commit", "é"], ["compEnd", "é"],
    ["insert", "x"],
  ]);
  const data = await sent(page, sessionId);

  expect(data.replace(/\u001b\[20[01]~/g, "")).toBe("Oi ação açãoéx");
});

// The textarea of xterm.js can hold a selection that the terminal line does
// not show. A commit or text that replaces it must not send deletions.
const acuteAtPace = (vowel: string, committed: string): Step[] => [
  ["compStart"], ["compUpdate", "'"], ["mark", "'"],
  ["down", "'", "Quote", 229, { composing: true }], ["tick"],
  ["up", "'", "Quote", 222, { composing: true }], ["tick"],
  ["markDelete"], ["commit", committed], ["compEnd", committed],
  ["down", vowel, `Key${vowel.toUpperCase()}`, 229], ["tick"],
  ["up", vowel, `Key${vowel.toUpperCase()}`, vowel.toUpperCase().charCodeAt(0)], ["tick"],
];

test("a right-clicked word is not deleted by the next accent or emoji", async ({ page }) => {
  const { pane, sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("ok arquivo ");
  // xterm's right-click handler puts the word into the textarea and selects it.
  const word = await pane.evaluate((node) => {
    for (const row of node.querySelectorAll(".xterm-rows > div")) {
      const text = row.textContent ?? "";
      const index = text.search(/[A-Za-z]{3}/);
      if (index < 0) continue;
      const bounds = row.getBoundingClientRect();
      const cell = bounds.width / Math.max(1, text.length);
      return { x: bounds.left + cell * (index + 1.5), y: bounds.top + bounds.height / 2 };
    }
    return null;
  });
  expect(word).not.toBeNull();
  await page.mouse.click(word!.x, word!.y, { button: "right" });
  expect(await textarea.evaluate((node) => {
    const { selectionStart, selectionEnd } = node as HTMLTextAreaElement;
    return selectionEnd > selectionStart;
  })).toBe(true);
  await page.keyboard.type("e ");
  await runBurst(textarea, acuteAtPace("e", "é"));
  await page.keyboard.type(" ");
  await page.mouse.click(word!.x, word!.y, { button: "right" });
  await runBurst(textarea, [["insert", "👍"]]);
  await page.keyboard.type("x");

  expect(withSpaces(await sent(page, sessionId))).toBe("ok arquivo e é 👍x");
});

test("Cmd+A, Cmd+Left and Cmd+X in the textarea do not delete typed text", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("n");
  await runBurst(textarea, [["insert", "ã"]]);
  await page.keyboard.type("o ");
  await runBurst(textarea, acuteAtPace("e", "é"));
  await page.keyboard.type(" isso ");
  await runBurst(textarea, [["selectAll"], ...acuteAtPace("a", "á")]);
  await page.keyboard.type(" ");
  await runBurst(textarea, [["caretToStart"], ["insert", "ç"], ["selectAll"], ["cut"]]);
  await page.keyboard.type("x");

  expect(withSpaces(await sent(page, sessionId))).toBe("não é isso á çx");
});

test("an Enter between a keyCode 229 key and its read sends no deletions", async ({ page }) => {
  const { sessionId, textarea } = await openHarnessTerminal(page);
  await page.keyboard.type("Oi ");
  // One burst after a stall, with no timer turn: the late keyCode 229
  // keydown, then the Enter and the "x" that the user typed after the stall.
  await runBurst(textarea, [
    ["compStart"], ["compUpdate", "'"], ["mark", "'"],
    ["markDelete"], ["commit", "á"], ["compEnd", "á"],
    ["insert", " Bc"],
    ["up", "'", "Quote", 222], ["up", "a", "KeyA", 65],
    ["down", " ", "Space", 229], ["up", " ", "Space", 32],
    ["down", "Enter", "Enter", 13], ["up", "Enter", "Enter", 13],
    ["down", "x", "KeyX", 88], ["up", "x", "KeyX", 88],
  ]);

  expect(withSpaces(await sent(page, sessionId))).toBe("Oi á Bc\rx");
});

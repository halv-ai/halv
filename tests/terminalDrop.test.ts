import assert from "node:assert/strict";
import test from "node:test";

import {
  createTerminalDropBridge,
  droppedPathsText,
  quoteDroppedPath,
  terminalQuoteStyle,
} from "../src/terminalDrop.ts";

test("picks the quote style from the session shell", () => {
  assert.equal(terminalQuoteStyle("/bin/zsh"), "posix");
  assert.equal(terminalQuoteStyle("/usr/local/bin/fish"), "posix");
  assert.equal(terminalQuoteStyle("C:\\Windows\\System32\\cmd.exe"), "windows");
  assert.equal(terminalQuoteStyle("C:\\Program Files\\PowerShell\\7\\pwsh.exe"), "windows");
  assert.equal(terminalQuoteStyle("powershell"), "windows");
});

test("quotes a POSIX path only when the shell needs it", () => {
  assert.equal(quoteDroppedPath("/work/halv/src/main.ts", "posix"), "/work/halv/src/main.ts");
  assert.equal(quoteDroppedPath("/Users/me/My Docs/a.txt", "posix"), "'/Users/me/My Docs/a.txt'");
  assert.equal(quoteDroppedPath("/tmp/it's.txt", "posix"), "'/tmp/it'\\''s.txt'");
  assert.equal(quoteDroppedPath("/tmp/$HOME", "posix"), "'/tmp/$HOME'");
  assert.equal(quoteDroppedPath("~/notes", "posix"), "'~/notes'");
});

test("quotes a Windows path with double quotes when it has spaces", () => {
  assert.equal(quoteDroppedPath("C:\\work\\a.txt", "windows"), "C:\\work\\a.txt");
  assert.equal(quoteDroppedPath("C:\\My Work\\a.txt", "windows"), '"C:\\My Work\\a.txt"');
});

test("joins dropped paths with spaces and ends with one space", () => {
  assert.equal(
    droppedPathsText(["/a/b.txt", " /c d/e.txt "], "posix"),
    "/a/b.txt '/c d/e.txt' ",
  );
  assert.equal(droppedPathsText([], "posix"), "");
  assert.equal(droppedPathsText(["", "  "], "posix"), "");
  // A path with a control character never reaches the shell.
  assert.equal(droppedPathsText(["/tmp/a\u001b[31mb"], "posix"), "");
});

function bounds(left: number, top: number, right: number, bottom: number) {
  return () => ({ left, top, right, bottom });
}

test("drops into the smallest visible pane under the point", () => {
  const bridge = createTerminalDropBridge();
  const pastes: Array<[string, string]> = [];
  bridge.register("wide", {
    bounds: bounds(0, 0, 800, 600),
    shell: "/bin/zsh",
    paste: (text) => pastes.push(["wide", text]),
  });
  bridge.register("split", {
    bounds: bounds(400, 0, 800, 600),
    shell: "/bin/zsh",
    paste: (text) => pastes.push(["split", text]),
  });
  bridge.register("hidden", {
    bounds: () => null,
    shell: "/bin/zsh",
    paste: () => assert.fail("hidden pane"),
  });
  assert.equal(bridge.targetAt(100, 100), "wide");
  assert.equal(bridge.targetAt(600, 100), "split");
  assert.equal(bridge.targetAt(900, 100), null);
  assert.equal(bridge.drop(600, 100, ["/tmp/x y.txt"]), true);
  assert.equal(bridge.drop(900, 100, ["/tmp/x.txt"]), false);
  assert.equal(bridge.drop(100, 100, []), false);
  assert.deepEqual(pastes, [["split", "'/tmp/x y.txt' "]]);
});

test("hover marks one pane at a time and unregister clears it", () => {
  const bridge = createTerminalDropBridge();
  const hover: string[] = [];
  const unregister = bridge.register("a", {
    bounds: bounds(0, 0, 100, 100),
    shell: "/bin/bash",
    paste: () => undefined,
    setHover: (value) => hover.push(`a:${value}`),
  });
  bridge.register("b", {
    bounds: bounds(100, 0, 200, 100),
    shell: "/bin/bash",
    paste: () => undefined,
    setHover: (value) => hover.push(`b:${value}`),
  });
  bridge.hover(50, 50);
  bridge.hover(60, 50);
  bridge.hover(150, 50);
  bridge.clearHover();
  assert.deepEqual(hover, ["a:true", "a:false", "b:true", "b:false"]);
  unregister();
  assert.equal(bridge.targetAt(50, 50), null);
  assert.equal(bridge.size, 1);
});

test("an old pane cleanup cannot unregister a new pane for the same session", () => {
  const bridge = createTerminalDropBridge();
  const unregisterOld = bridge.register("s", {
    bounds: bounds(0, 0, 10, 10),
    shell: "/bin/zsh",
    paste: () => assert.fail("old pane"),
  });
  let pasted = "";
  bridge.register("s", {
    bounds: bounds(0, 0, 10, 10),
    shell: "/bin/zsh",
    paste: (text) => {
      pasted = text;
    },
  });
  unregisterOld();
  assert.equal(bridge.drop(5, 5, ["/tmp/a"]), true);
  assert.equal(pasted, "/tmp/a ");
});

import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  MAX_SCANNED_LINE_LENGTH,
  MAX_TERMINAL_PATH_CANDIDATES,
  parseTerminalPathCandidates,
  rangeForCandidate,
  readLogicalLine,
  terminalLinkGesture,
} from "../src/terminalLinks.ts";

type Candidate = {
  text: string;
  pathText: string;
  line: number | null;
  column: number | null;
  startIndex: number;
  endIndex: number;
};

function paths(lineText: string): string[] {
  return parseTerminalPathCandidates(lineText).map((candidate: Candidate) => candidate.pathText);
}

function only(lineText: string): Candidate {
  const candidates = parseTerminalPathCandidates(lineText);
  assert.equal(candidates.length, 1, `one candidate in ${JSON.stringify(lineText)}`);
  return candidates[0];
}

test("finds relative, absolute, home and Windows paths", () => {
  assert.deepEqual(paths("./a/b.ts"), ["./a/b.ts"]);
  assert.deepEqual(paths("cd ../x"), ["../x"]);
  assert.deepEqual(paths("wrote /abs/p today"), ["/abs/p"]);
  assert.deepEqual(paths("ls ~/dir"), ["~/dir"]);
  assert.deepEqual(paths("open C:\\dir\\f.rs now"), ["C:\\dir\\f.rs"]);
  assert.deepEqual(paths("D:/work/f.rs"), ["D:/work/f.rs"]);
  assert.deepEqual(paths("app/(shop)/[id]/page.tsx"), ["app/(shop)/[id]/page.tsx"]);
  assert.deepEqual(paths("../../lib"), ["../../lib"]);
  assert.deepEqual(paths("cd src/.."), ["src/.."]);
  assert.deepEqual(paths("src/"), ["src/"]);
});

test("accepts Unicode names and path punctuation", () => {
  assert.deepEqual(paths("docs/café/naïve-ü.md"), ["docs/café/naïve-ü.md"]);
  assert.deepEqual(paths("資料/ファイル.txt"), ["資料/ファイル.txt"]);
  assert.deepEqual(paths("node_modules/@scope/pkg+1/a%20b~.js"), [
    "node_modules/@scope/pkg+1/a%20b~.js",
  ]);
});

test("gives the offsets of the full match", () => {
  const lineText = "error in src/foo.ts:12:3 here";
  const candidate = only(lineText);
  assert.deepEqual(candidate, {
    text: "src/foo.ts:12:3",
    pathText: "src/foo.ts",
    line: 12,
    column: 3,
    startIndex: 9,
    endIndex: 24,
  });
  assert.equal(lineText.slice(candidate.startIndex, candidate.endIndex), candidate.text);
});

test("parses line and column suffixes", () => {
  assert.deepEqual(
    { ...only("src/foo.ts:12") },
    { text: "src/foo.ts:12", pathText: "src/foo.ts", line: 12, column: null, startIndex: 0, endIndex: 13 },
  );
  const noSuffix = only("src/foo.ts");
  assert.equal(noSuffix.line, null);
  assert.equal(noSuffix.column, null);
  const drive = only("C:\\dir\\f.rs:7:9");
  assert.equal(drive.pathText, "C:\\dir\\f.rs");
  assert.equal(drive.line, 7);
  assert.equal(drive.column, 9);
  // A word after the path is not a line number.
  const word = only("src/foo.ts:abc");
  assert.equal(word.pathText, "src/foo.ts");
  assert.equal(word.text, "src/foo.ts");
});

test("strips trailing punctuation and wrapping brackets", () => {
  assert.equal(only("see src/a.ts.").text, "src/a.ts");
  assert.equal(only("see src/a.ts, then").text, "src/a.ts");
  assert.equal(only("see src/a.ts;").text, "src/a.ts");
  assert.equal(only("see src/a.ts:").text, "src/a.ts");
  assert.equal(only("see src/foo...").text, "src/foo");
  assert.equal(only("(see src/a.ts)").text, "src/a.ts");
  assert.equal(only("(src/a.ts:12)").text, "src/a.ts:12");
  assert.equal(only("[./a/b.ts]").text, "./a/b.ts");
  assert.equal(only('"./a/b.ts"').text, "./a/b.ts");
  assert.equal(only("'~/dir'").text, "~/dir");
  assert.equal(only("`src/a.ts`").text, "src/a.ts");
  assert.equal(only("src/foo.ts:12:3.").text, "src/foo.ts:12:3");
  assert.equal(only("(app/(shop)/page.tsx)").text, "app/(shop)/page.tsx");
  assert.equal(only("(shop)/page.tsx").text, "(shop)/page.tsx");
  const wrapped = only("x (./a/b.ts) y");
  assert.equal(wrapped.startIndex, 3);
  assert.equal(wrapped.endIndex, 11);
});

test("finds the path in a tool call header", () => {
  const update = only("\u23fa Update(src/components/TerminalPane.tsx)");
  assert.equal(update.pathText, "src/components/TerminalPane.tsx");
  assert.equal(update.text, "src/components/TerminalPane.tsx");
  const lineText = "Read(src/terminalLinks.ts)";
  const read = only(lineText);
  assert.equal(lineText.slice(read.startIndex, read.endIndex), "src/terminalLinks.ts");
  assert.equal(only("Read(/abs/a.ts)").pathText, "/abs/a.ts");
  assert.equal(only("Write(/Users/me/proj/src/ipc.ts)").pathText, "/Users/me/proj/src/ipc.ts");
  assert.equal(only("Read(~/notes/a.md)").pathText, "~/notes/a.md");
  assert.equal(only("Read(C:\\proj\\a.ts)").pathText, "C:\\proj\\a.ts");
  assert.equal(only("Read(../a.ts)").pathText, "../a.ts");
  const withLine = only("Edit(src/a.ts:12:3)");
  assert.equal(withLine.pathText, "src/a.ts");
  assert.equal(withLine.line, 12);
  assert.equal(withLine.column, 3);
  assert.deepEqual(paths("Read(src/a.ts) and Read(src/b.ts)"), ["src/a.ts", "src/b.ts"]);
  assert.deepEqual(paths("Read(src/a.ts), Read(src/b.ts)"), ["src/a.ts", "src/b.ts"]);
  assert.equal(only("(Update(src/a.ts))").pathText, "src/a.ts");
  assert.equal(only("Update([id]/page.tsx)").pathText, "[id]/page.tsx");
  assert.equal(only("Update(src/a(1).ts)").pathText, "src/a(1).ts");
  assert.equal(only("List[src/a.ts]").pathText, "src/a.ts");
});

test("keeps a bracket that belongs to a name", () => {
  assert.equal(only("app/(shop)/[id]/page.tsx").pathText, "app/(shop)/[id]/page.tsx");
  assert.equal(only("(shop)/page.tsx").pathText, "(shop)/page.tsx");
  assert.equal(only("[id]/page.tsx").pathText, "[id]/page.tsx");
  assert.equal(only("foo(1)/bar.ts").pathText, "foo(1)/bar.ts");
  assert.equal(only("src/a(1).ts").pathText, "src/a(1).ts");
});

test("finds more than one path in a line", () => {
  assert.deepEqual(paths("M src/a.ts\tM src/b.ts"), ["src/a.ts", "src/b.ts"]);
  assert.deepEqual(paths("PATH=/usr/bin:/bin"), ["/usr/bin", "/bin"]);
});

test("skips URLs", () => {
  assert.deepEqual(paths("https://x/a/b"), []);
  assert.deepEqual(paths("see <https://example.com/a/b.ts:3> now"), []);
  assert.deepEqual(paths("file:///Users/me/a.txt"), []);
  assert.deepEqual(paths("https://x/a/b and ./c/d"), ["./c/d"]);
});

test("skips a path inside the query of a web URL", () => {
  assert.deepEqual(
    paths("Open http://localhost:5173/__open-in-editor?file=src/App.tsx:12:3 now"),
    [],
  );
  assert.deepEqual(paths("Open https://x.dev/open?file=/abs/existing/path now"), []);
  // A path outside the URL stays a candidate.
  assert.deepEqual(
    paths("http://localhost:5173/open?file=src/App.tsx and ./c/d.ts"),
    ["./c/d.ts"],
  );
  assert.deepEqual(paths("see ./a/b.ts (http://x.dev/o?f=src/a.ts)"), ["./a/b.ts"]);
});

test("skips text without a path", () => {
  assert.deepEqual(paths(""), []);
  assert.deepEqual(paths("README.md"), []);
  assert.deepEqual(paths("just some words"), []);
  assert.deepEqual(paths("/"), []);
  assert.deepEqual(paths("// comment"), []);
  assert.deepEqual(paths("./"), []);
  assert.deepEqual(paths("../"), []);
  assert.deepEqual(paths("..."), []);
  assert.deepEqual(paths("../.."), []);
});

test("does not treat numbers as paths", () => {
  assert.deepEqual(paths("1/2"), []);
  assert.deepEqual(paths("done 10/20 tasks"), []);
  assert.deepEqual(paths("2026/10/21"), []);
  assert.deepEqual(paths("1.5/2.0"), []);
  assert.deepEqual(paths("v1/2"), ["v1/2"]);
});

test("caps the scanned line and the candidate count", () => {
  const late = `${" ".repeat(MAX_SCANNED_LINE_LENGTH)} src/late.ts`;
  assert.deepEqual(paths(late), []);
  const many = Array.from({ length: 100 }, (_, index) => `d/f${index}.ts`).join(" ");
  assert.equal(parseTerminalPathCandidates(many).length, MAX_TERMINAL_PATH_CANDIDATES);
});

test("runs in linear time on hostile input", () => {
  const inputs = [
    "a".repeat(100_000),
    "a/".repeat(50_000),
    "(".repeat(50_000) + "a/b" + ")".repeat(50_000),
    ":".repeat(100_000) + "/",
    "a:".repeat(50_000) + "/",
    ".".repeat(100_000) + "/x",
    "/a" + "]".repeat(100_000),
  ];
  for (const input of inputs) {
    const started = performance.now();
    parseTerminalPathCandidates(input);
    const elapsed = performance.now() - started;
    assert.ok(elapsed < 200, `parse took ${elapsed} ms`);
  }
});

// A small fake of the xterm buffer. Each row is a list of cells. A cell is
// [chars, width]. A wide character uses [char, 2] and then ["", 0].
type FakeCell = [string, number];
type FakeRow = { wrapped: boolean; cells: FakeCell[] };

function textCells(text: string): FakeCell[] {
  return Array.from(text, (char) => [char, 1] as FakeCell);
}

function fakeBuffer(rows: FakeRow[]) {
  return {
    getLine(y: number) {
      const row = rows[y];
      if (!row) {
        return undefined;
      }
      return {
        isWrapped: row.wrapped,
        length: row.cells.length,
        getCell(x: number) {
          const cell = row.cells[x];
          return cell ? { getChars: () => cell[0], getWidth: () => cell[1] } : undefined;
        },
      };
    },
  };
}

test("reads a logical line over wrapped rows from any row", () => {
  const buffer = fakeBuffer([
    { wrapped: false, cells: textCells("before    ") },
    { wrapped: false, cells: textCells("see src/fo") },
    { wrapped: true, cells: textCells("o.ts:12 ok") },
    { wrapped: false, cells: textCells("after     ") },
  ]);
  for (const y of [1, 2]) {
    const line = readLogicalLine(buffer, y);
    assert.ok(line);
    assert.equal(line.text, "see src/foo.ts:12 ok");
    assert.equal(line.cells.length, line.text.length);
  }
  assert.equal(readLogicalLine(buffer, 0)?.text, "before    ");
  assert.equal(readLogicalLine(buffer, 3)?.text, "after     ");
});

test("returns null for an empty or missing line", () => {
  const buffer = fakeBuffer([{ wrapped: false, cells: [["", 1], ["", 1]] }]);
  assert.equal(readLogicalLine(buffer, 0), null);
  assert.equal(readLogicalLine(buffer, 5), null);
});

test("maps wide characters and empty cells", () => {
  const buffer = fakeBuffer([
    {
      wrapped: false,
      cells: [
        ["資", 2],
        ["", 0],
        ["/", 1],
        ["a", 1],
        ["", 1],
        ["b", 1],
      ],
    },
  ]);
  const line = readLogicalLine(buffer, 0);
  assert.ok(line);
  assert.equal(line.text, "資/a b");
  assert.deepEqual(
    line.cells.map((cell: { x: number }) => cell.x),
    [0, 2, 3, 4, 5],
  );
  // The wide character covers columns 1 and 2 (1-based).
  assert.deepEqual(rangeForCandidate(line, 0, 1), {
    start: { x: 1, y: 1 },
    end: { x: 2, y: 1 },
  });
  assert.deepEqual(rangeForCandidate(line, 0, 3), {
    start: { x: 1, y: 1 },
    end: { x: 4, y: 1 },
  });
});

test("skips the empty last cell when a wide character wraps", () => {
  const buffer = fakeBuffer([
    { wrapped: false, cells: [...textCells("ab/"), ["", 1]] },
    { wrapped: true, cells: [["資", 2], ["", 0], ...textCells(".md")] },
  ]);
  const line = readLogicalLine(buffer, 1);
  assert.ok(line);
  assert.equal(line.text, "ab/資.md");
  const candidate = only(line.text);
  assert.deepEqual(rangeForCandidate(line, candidate.startIndex, candidate.endIndex), {
    start: { x: 1, y: 1 },
    end: { x: 5, y: 2 },
  });
});

test("gives a range across a wrap for a parsed candidate", () => {
  const buffer = fakeBuffer([
    { wrapped: false, cells: textCells("") },
    { wrapped: false, cells: textCells("see src/fo") },
    { wrapped: true, cells: textCells("o.ts:12 ok") },
  ]);
  const line = readLogicalLine(buffer, 2);
  assert.ok(line);
  const candidate = only(line.text);
  assert.equal(candidate.text, "src/foo.ts:12");
  // Buffer line 1 (0-based) is y 2 (1-based). "s" is at column 5 (1-based).
  // The last "2" is at column 7 of the next row.
  assert.deepEqual(rangeForCandidate(line, candidate.startIndex, candidate.endIndex), {
    start: { x: 5, y: 2 },
    end: { x: 7, y: 3 },
  });
});

test("caps a logical line at 64 rows", () => {
  const rows: FakeRow[] = [{ wrapped: false, cells: textCells("ab") }];
  for (let index = 0; index < 100; index += 1) {
    rows.push({ wrapped: true, cells: textCells("cd") });
  }
  const buffer = fakeBuffer(rows);
  const first = readLogicalLine(buffer, 0);
  assert.equal(first?.text.length, 128);
  const last = readLogicalLine(buffer, 100);
  assert.equal(last?.text.length, 128);
  assert.equal(last?.cells.at(-1)?.y, 100);
});

test("rejects bad offsets", () => {
  const line = readLogicalLine(fakeBuffer([{ wrapped: false, cells: textCells("a/b") }]), 0);
  assert.ok(line);
  assert.equal(rangeForCandidate(line, -1, 2), null);
  assert.equal(rangeForCandidate(line, 2, 2), null);
  assert.equal(rangeForCandidate(line, 0, 4), null);
  assert.equal(rangeForCandidate(line, 0.5, 2), null);
});

test("reads the click gesture", () => {
  const click = { button: 0, detail: 1, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false };
  assert.equal(terminalLinkGesture({ ...click, metaKey: true }, "MacIntel"), "open");
  assert.equal(terminalLinkGesture({ ...click, metaKey: true, shiftKey: true }, "MacIntel"), "reveal");
  assert.equal(terminalLinkGesture({ ...click, metaKey: true, altKey: true }, "MacIntel"), null);
  assert.equal(terminalLinkGesture({ ...click, ctrlKey: true }, "MacIntel"), null);
  assert.equal(terminalLinkGesture(click, "MacIntel"), null);
  assert.equal(terminalLinkGesture({ ...click, metaKey: true, detail: 2 }, "MacIntel"), null);
  assert.equal(terminalLinkGesture({ ...click, metaKey: true, button: 2 }, "MacIntel"), null);
  assert.equal(terminalLinkGesture({ ...click, ctrlKey: true }, "Win32"), "open");
  assert.equal(terminalLinkGesture({ ...click, ctrlKey: true, shiftKey: true }, "Linux x86_64"), "reveal");
  assert.equal(terminalLinkGesture({ ...click, metaKey: true }, "Win32"), null);
});

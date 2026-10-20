import assert from "node:assert/strict";
import test from "node:test";

import {
  MAX_PARSED_DIFF_LINES,
  MAX_WORD_DIFF_TOTAL_CELLS,
  parseUnifiedDiff,
  parseUnifiedDiffWithLimit,
  toSideBySideRows,
} from "../src/diff.ts";

const fileA = [
  "diff --git a/a.ts b/a.ts",
  "--- a/a.ts",
  "+++ b/a.ts",
  "@@ -1 +1 @@",
  "-const a = 1;",
  "+const a = 2;",
  "",
].join("\n");

const fileB = [
  "diff --git a/src/b.ts b/src/b.ts",
  "--- a/src/b.ts",
  "+++ b/src/b.ts",
  "@@ -4,2 +4,3 @@ export function b() {",
  "   return 1;",
  "+  return 2;",
  " }",
  "",
].join("\n");

test("bounds newline-heavy diffs before creating review lines", () => {
  const lineCount = MAX_PARSED_DIFF_LINES * 100;
  const parsed = parseUnifiedDiffWithLimit(
    [
      "diff --git a/large.txt b/large.txt",
      "--- /dev/null",
      "+++ b/large.txt",
      `@@ -0,0 +1,${lineCount} @@`,
      `${"+x\n".repeat(lineCount)}`,
    ].join("\n"),
  );

  assert.equal(parsed.truncated, true);
  assert.ok(parsed.files[0].hunks[0].lines.length < MAX_PARSED_DIFF_LINES);
});

test("handles empty and incomplete diff input", () => {
  assert.deepEqual(parseUnifiedDiff(""), []);
  const [file] = parseUnifiedDiff(
    "diff --git a/odd.txt b/odd.txt\n--- a/odd.txt",
  );
  assert.equal(file.path, "odd.txt");
  assert.deepEqual(file.hunks, []);
});

test("parses file-aware unified diff metadata", () => {
  const files = parseUnifiedDiff(`${fileA}${fileB}`);

  assert.equal(files.length, 2);
  assert.equal(files[0]?.path, "a.ts");
  assert.equal(files[0]?.status, "modified");
  assert.equal(files[0]?.additions, 1);
  assert.equal(files[0]?.deletions, 1);
  assert.equal(files[1]?.path, "src/b.ts");
  assert.equal(files[1]?.hunks[0]?.header, "@@ -4,2 +4,3 @@ export function b() {");
});

test("keeps inline-comment targets stable when another file leaves the diff", () => {
  const together = parseUnifiedDiff(`${fileA}${fileB}`);
  const alone = parseUnifiedDiff(fileB);
  const before = together[1]?.hunks[0]?.lines.find((line) => line.kind === "add");
  const after = alone[0]?.hunks[0]?.lines.find((line) => line.kind === "add");

  assert.equal(before?.id, after?.id);
  assert.match(before?.id ?? "", /^src\/b\.ts:/);
});

test("parses added-file status and line numbers", () => {
  const files = parseUnifiedDiff(`diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
-old
+new
 same
diff --git a/b.txt b/b.txt
new file mode 100644
--- /dev/null
+++ b/b.txt
@@ -0,0 +1 @@
+hello
`);

  assert.equal(files[0]?.hunks[0]?.lines[0]?.oldLine, 1);
  assert.equal(files[0]?.hunks[0]?.lines[1]?.newLine, 1);
  assert.deepEqual(
    files.map(({ path, status, additions, deletions }) => ({
      path,
      status,
      additions,
      deletions,
    })),
    [
      { path: "a.txt", status: "modified", additions: 1, deletions: 1 },
      { path: "b.txt", status: "added", additions: 1, deletions: 0 },
    ],
  );
});

test("parses deleted and binary files", () => {
  const files = parseUnifiedDiff(`diff --git a/old.bin b/old.bin
deleted file mode 100644
Binary files a/old.bin and /dev/null differ
--- a/old.bin
+++ /dev/null
`);

  assert.equal(files[0]?.path, "old.bin");
  assert.equal(files[0]?.status, "binary");
  assert.equal(files[0]?.binary, true);
});

test("adds word-level segments to paired delete and add lines", () => {
  const [file] = parseUnifiedDiff(`diff --git a/a.ts b/a.ts
--- a/a.ts
+++ b/a.ts
@@ -1 +1 @@
-const total = oldValue + 1;
+const total = newValue + 1;
`);
  const [deleted, added] = file.hunks[0].lines;

  assert.deepEqual(deleted.segments, [
    { text: "const total = ", changed: false },
    { text: "oldValue", changed: true },
    { text: " + 1;", changed: false },
  ]);
  assert.deepEqual(added.segments, [
    { text: "const total = ", changed: false },
    { text: "newValue", changed: true },
    { text: " + 1;", changed: false },
  ]);
  assert.equal(
    deleted.segments?.map((segment) => segment.text).join(""),
    deleted.text,
  );
  assert.equal(
    added.segments?.map((segment) => segment.text).join(""),
    added.text,
  );
});

test("pairs only adjacent delete and add runs", () => {
  const [file] = parseUnifiedDiff(`diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,4 @@
-first old
-second old
+first new
+second new
 unchanged
-unpaired delete
 unchanged again
+unpaired add
`);
  const lines = file.hunks[0].lines;

  assert.equal(lines[0].segments?.find((segment) => segment.changed)?.text, "old");
  assert.equal(lines[1].segments?.find((segment) => segment.changed)?.text, "old");
  assert.equal(lines[2].segments?.find((segment) => segment.changed)?.text, "new");
  assert.equal(lines[3].segments?.find((segment) => segment.changed)?.text, "new");
  assert.equal(lines[5].segments, undefined);
  assert.equal(lines[7].segments, undefined);
});

test("skips word-level segments for lines over 400 characters", () => {
  const longOld = `old${"a".repeat(398)}`;
  const longNew = `new${"a".repeat(398)}`;
  const [file] = parseUnifiedDiff(`diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-${longOld}
+${longNew}
`);

  assert.equal(file.hunks[0].lines[0].segments, undefined);
  assert.equal(file.hunks[0].lines[1].segments, undefined);
});

test("skips word-level segments for runs over 200 lines", () => {
  const deleted = Array.from({ length: 201 }, (_, index) => `-old ${index}`);
  const added = Array.from({ length: 201 }, (_, index) => `+new ${index}`);
  const [file] = parseUnifiedDiff(
    [
      "diff --git a/a.txt b/a.txt",
      "--- a/a.txt",
      "+++ b/a.txt",
      "@@ -1,201 +1,201 @@",
      ...deleted,
      ...added,
      "",
    ].join("\n"),
  );

  assert.equal(
    file.hunks[0].lines.some((line) => line.segments !== undefined),
    false,
  );
});

test("runs word-level segments at the line and run limits", () => {
  const longOld = `old${"a".repeat(397)}`;
  const longNew = `new${"a".repeat(397)}`;
  const deleted = [longOld, ...Array.from({ length: 199 }, () => "old")];
  const added = [longNew, ...Array.from({ length: 199 }, () => "new")];
  const [file] = parseUnifiedDiff(
    [
      "diff --git a/a.txt b/a.txt",
      "--- a/a.txt",
      "+++ b/a.txt",
      "@@ -1,200 +1,200 @@",
      ...deleted.map((line) => `-${line}`),
      ...added.map((line) => `+${line}`),
      "",
    ].join("\n"),
  );
  const lines = file.hunks[0].lines;

  assert.equal(lines[0].text.length, 400);
  assert.notEqual(lines[0].segments, undefined);
  assert.notEqual(lines[199].segments, undefined);
  assert.notEqual(lines[200].segments, undefined);
  assert.notEqual(lines[399].segments, undefined);
});

test("builds side-by-side rows from hunk line runs", () => {
  const [file] = parseUnifiedDiff(`diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,4 @@
 context before
-old one
-old two
+new one
 context after
+new tail
`);
  const rows = toSideBySideRows(file.hunks[0]);

  assert.deepEqual(
    rows.map(({ left, right }) => [left?.kind ?? null, right?.kind ?? null]),
    [
      ["context", "context"],
      ["delete", "add"],
      ["delete", null],
      ["context", "context"],
      [null, "add"],
    ],
  );
  assert.equal(rows[1].left?.text, "old one");
  assert.equal(rows[1].right?.text, "new one");
  assert.notEqual(rows[0].left?.id, rows[0].right?.id);
  assert.match(rows[0].left?.id ?? "", /:l$/);
  assert.match(rows[0].right?.id ?? "", /:r$/);
});

test("leaves the left side empty for extra additions in a paired run", () => {
  const [file] = parseUnifiedDiff(`diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1,2 @@
-old
+new
+extra
`);
  const rows = toSideBySideRows(file.hunks[0]);

  assert.equal(rows[0].left?.kind, "delete");
  assert.equal(rows[0].right?.kind, "add");
  assert.equal(rows[1].left, null);
  assert.equal(rows[1].right?.text, "extra");
});


function wordDiffBudgetDiff(pairs: number) {
  const longOld = `old ${"a".repeat(396)}`;
  const longNew = `new ${"a".repeat(396)}`;
  return [
    "diff --git a/a.txt b/a.txt",
    "--- a/a.txt",
    "+++ b/a.txt",
    `@@ -1,${pairs} +1,${pairs} @@`,
    ...Array.from({ length: pairs }, () => `-${longOld}`),
    ...Array.from({ length: pairs }, () => `+${longNew}`),
    "",
  ].join("\n");
}

test("stops word-level segments once the parse budget is spent", () => {
  const pairCost = (400 + 1) * (400 + 1);
  const budgetPairs = Math.floor(MAX_WORD_DIFF_TOTAL_CELLS / pairCost);
  const pairs = budgetPairs + 3;
  const [file] = parseUnifiedDiff(wordDiffBudgetDiff(pairs));
  const lines = file.hunks[0].lines;

  assert.notEqual(lines[0].segments, undefined);
  assert.notEqual(lines[budgetPairs - 1].segments, undefined);
  assert.equal(lines[budgetPairs].segments, undefined);
  assert.equal(lines[pairs - 1].segments, undefined);
  assert.notEqual(lines[pairs].segments, undefined);
  assert.notEqual(lines[pairs + budgetPairs - 1].segments, undefined);
  assert.equal(lines[pairs + budgetPairs].segments, undefined);
});

test("resets the word-diff budget for each parse call", () => {
  const pairCost = (400 + 1) * (400 + 1);
  const pairs = Math.floor(MAX_WORD_DIFF_TOTAL_CELLS / pairCost) + 3;
  parseUnifiedDiff(wordDiffBudgetDiff(pairs));

  const [file] = parseUnifiedDiff(fileA);
  assert.notEqual(file.hunks[0].lines[0].segments, undefined);
});

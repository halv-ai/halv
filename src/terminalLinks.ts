import type { IBufferRange } from "@xterm/xterm";
import { isTerminalLinkClick } from "./externalLinks.ts";

// Finds file paths in terminal text, maps them to xterm buffer cells and reads
// the click gesture. The ideas come from Orca and VS Code (both MIT): the
// terminal local link detector and the wrapped-row cell walk. Halv does not
// copy their code.

// A path that the parser found in one logical line.
// - text: the full match, for example "src/foo.ts:12:3".
// - pathText: the path only, without the ":line:col" suffix.
// - line, column: the 1-based numbers from the suffix, or null.
// - startIndex, endIndex: UTF-16 offsets of text in the line. endIndex is
//   exclusive.
export type TerminalPathCandidate = {
  text: string;
  pathText: string;
  line: number | null;
  column: number | null;
  startIndex: number;
  endIndex: number;
};

// The parser reads only this number of characters from a line. The cap keeps
// the work small when a program prints a very long line.
export const MAX_SCANNED_LINE_LENGTH = 4096;

// The parser returns this number of candidates or fewer. The Rust command
// terminal_link_targets accepts the same number.
export const MAX_TERMINAL_PATH_CANDIDATES = 64;

// A run is a maximal sequence of characters that can be in a path, a
// ":line:col" suffix or a drive letter. Whitespace, quotes, commas, angle
// brackets and similar characters end a run. The class has no nested
// quantifier, so the regex is linear in the line length.
const RUN_PATTERN = /[\p{L}\p{N}\p{M}._~\-/\\%+@()[\]:]+/gu;
const DIGITS = /^\d+$/;
const DRIVE_LETTER = /^[A-Za-z]$/;
const ONLY_SEPARATORS_AND_DOTS = /^[\\/.]+$/;
const NUMERIC_SEGMENT = /^[\d.]+$/;
// The same pattern as the xterm web links addon, so a URL span here is the
// span that the addon links. Each class has no nested quantifier.
const URL_PATTERN = /https?:\/\/[^\s"'!*(){}|\\^<>`]*[^\s"':,.!?{}|\\^~[\]`()<>]/gi;

function isSeparator(char: string | undefined) {
  return char === "/" || char === "\\";
}

function hasSeparator(text: string) {
  return text.includes("/") || text.includes("\\");
}

// Returns true when each name between separators has only digits and dots.
// "1/2", "10/20" and "2026/10/21" are fractions or dates, not paths.
function isNumericOnly(path: string) {
  const names = path.split(/[\\/]/).filter((name) => name.length > 0);
  return names.length > 0 && names.every((name) => NUMERIC_SEGMENT.test(name));
}

// Returns true when the bracket at run[open] closes before the next
// separator. Then the bracket is part of a name, for example "(shop)/page.tsx".
// Else the bracket wraps the path, for example "(src/a.ts:12)".
function closesBeforeSeparator(run: string, open: number, closer: string) {
  for (let index = open + 1; index < run.length; index += 1) {
    if (run[index] === closer) return true;
    if (isSeparator(run[index])) return false;
  }
  return false;
}

// Returns the offset after the first "(" or "[" of a run, when the bracket
// follows a name and no separator is before it. Returns -1 otherwise. This is
// the call form that tools print: "Update(src/a.ts)" or "Read(/abs/a.ts)".
// The bracket does not wrap a name when it closes before the first separator,
// for example "(shop)/page.tsx". Then the function returns -1.
function callPrefixEnd(run: string, start: number, end: number) {
  for (let index = start; index < end; index += 1) {
    const char = run[index];
    if (isSeparator(char)) {
      return -1;
    }
    if (char === "(" || char === "[") {
      if (index === start || closesBeforeSeparator(run, index, char === "(" ? ")" : "]")) {
        return -1;
      }
      return index + 1;
    }
  }
  return -1;
}

// Removes characters at the two ends of a run that are not part of the path.
// The function keeps brackets in names, so "app/(shop)/[id]/page.tsx" stays
// whole. It removes a leading bracket that wraps the path and a trailing
// bracket without a pair. It removes the name and the bracket of a call such
// as "Update(src/a.ts)". It removes a trailing ".", but not the dots of a "."
// or ".." name.
// Returns the start and end offsets of the trimmed run in the run.
function trimRun(run: string): { start: number; end: number } {
  let start = 0;
  let end = run.length;
  let openParens = 0;
  let closeParens = 0;
  let openSquare = 0;
  let closeSquare = 0;
  for (const char of run) {
    if (char === "(") openParens += 1;
    else if (char === ")") closeParens += 1;
    else if (char === "[") openSquare += 1;
    else if (char === "]") closeSquare += 1;
  }

  while (start < end) {
    const char = run[start];
    if (char === "(" && !closesBeforeSeparator(run, start, ")")) {
      openParens -= 1;
    } else if (char === "[" && !closesBeforeSeparator(run, start, "]")) {
      openSquare -= 1;
    } else if (char === ":") {
      // A leading ":" never starts a path.
    } else {
      break;
    }
    start += 1;
  }

  // The bracket of a call is not in the path. Then the closing bracket of the
  // call has no pair, and the trailing trim below removes it.
  const afterCall = callPrefixEnd(run, start, end);
  if (afterCall !== -1) {
    if (run[afterCall - 1] === "(") openParens -= 1;
    else openSquare -= 1;
    start = afterCall;
  }

  while (end > start) {
    const char = run[end - 1];
    if (char === ")" && closeParens > openParens) {
      closeParens -= 1;
    } else if (char === "]" && closeSquare > openSquare) {
      closeSquare -= 1;
    } else if (char === ":") {
      // A trailing ":" is punctuation, for example "see src/a.ts:".
    } else if (char === ".") {
      // Keep "." and ".." names, for example "../..". Remove other trailing
      // dots, for example "src/a.ts." or "src/foo...".
      let nameStart = end;
      while (nameStart > start && !isSeparator(run[nameStart - 1])) {
        nameStart -= 1;
      }
      const name = run.slice(nameStart, end);
      if ((name === "." || name === "..") && nameStart > start) {
        break;
      }
    } else {
      break;
    }
    end -= 1;
  }
  return { start, end };
}

type Segment = { text: string; start: number };

// Splits a trimmed run at each ":" and keeps the offset of each segment.
// A one-letter segment before a segment that starts with a separator is a
// Windows drive, so the function joins the two: "C:\dir\f.rs".
function splitSegments(text: string, offset: number): Segment[] {
  const raw: Segment[] = [];
  let start = 0;
  for (let index = 0; index <= text.length; index += 1) {
    if (index === text.length || text[index] === ":") {
      raw.push({ text: text.slice(start, index), start: offset + start });
      start = index + 1;
    }
  }
  const joined: Segment[] = [];
  for (let index = 0; index < raw.length; index += 1) {
    const segment = raw[index];
    const next = raw[index + 1];
    if (DRIVE_LETTER.test(segment.text) && next && isSeparator(next.text[0])) {
      joined.push({ text: `${segment.text}:${next.text}`, start: segment.start });
      index += 1;
    } else {
      joined.push(segment);
    }
  }
  return joined;
}

type Span = { start: number; end: number };

// Finds the http and https URLs in a line. The text must contain "://".
function findUrlSpans(text: string): Span[] {
  if (!text.includes("://")) {
    return [];
  }
  const spans: Span[] = [];
  for (const match of text.matchAll(URL_PATTERN)) {
    const start = match.index ?? 0;
    spans.push({ start, end: start + match[0].length });
  }
  return spans;
}

function insideUrlSpan(spans: Span[], start: number, end: number) {
  return spans.some((span) => start >= span.start && end <= span.end);
}

function isPathText(path: string) {
  return (
    hasSeparator(path) &&
    !ONLY_SEPARATORS_AND_DOTS.test(path) &&
    !isNumericOnly(path)
  );
}

// Finds file paths in one logical terminal line.
// - A path must contain "/" or "\". Bare names such as "README.md" do not match.
// - A path can have a ":line" or ":line:col" suffix. The "(12,3)" compiler
//   form is not supported.
// - The parser skips URLs: a run that contains "://" gives no candidate. A
//   path inside the query of an http or https URL also gives no candidate,
//   because the web links addon links the whole URL.
// - The parser skips text with only separators and dots, and numbers such as
//   "1/2" or "2026/10/21".
// - The parser reads the first MAX_SCANNED_LINE_LENGTH characters only.
export function parseTerminalPathCandidates(lineText: string): TerminalPathCandidate[] {
  const text =
    lineText.length > MAX_SCANNED_LINE_LENGTH
      ? lineText.slice(0, MAX_SCANNED_LINE_LENGTH)
      : lineText;
  if (!hasSeparator(text)) {
    return [];
  }
  const urlSpans = findUrlSpans(text);
  const candidates: TerminalPathCandidate[] = [];
  for (const match of text.matchAll(RUN_PATTERN)) {
    const run = match[0];
    const runStart = match.index ?? 0;
    if (!hasSeparator(run) || run.includes("://")) {
      continue;
    }
    const { start, end } = trimRun(run);
    if (end <= start) {
      continue;
    }
    const segments = splitSegments(run.slice(start, end), runStart + start);
    for (let index = 0; index < segments.length; index += 1) {
      const segment = segments[index];
      if (!isPathText(segment.text)) {
        continue;
      }
      let line: number | null = null;
      let column: number | null = null;
      let endIndex = segment.start + segment.text.length;
      const lineSegment = segments[index + 1];
      if (lineSegment && DIGITS.test(lineSegment.text)) {
        line = Number(lineSegment.text);
        endIndex = lineSegment.start + lineSegment.text.length;
        index += 1;
        const columnSegment = segments[index + 1];
        if (columnSegment && DIGITS.test(columnSegment.text)) {
          column = Number(columnSegment.text);
          endIndex = columnSegment.start + columnSegment.text.length;
          index += 1;
        }
      }
      if (insideUrlSpan(urlSpans, segment.start, endIndex)) {
        continue;
      }
      candidates.push({
        text: text.slice(segment.start, endIndex),
        pathText: segment.text,
        line,
        column,
        startIndex: segment.start,
        endIndex,
      });
      if (candidates.length >= MAX_TERMINAL_PATH_CANDIDATES) {
        return candidates;
      }
    }
  }
  return candidates;
}

// The name of the parser in the shared contract.
export const findTerminalPathCandidates = parseTerminalPathCandidates;

// The buffer fields that readLogicalLine uses. The xterm IBuffer type has
// these fields, and tests can give a small fake.
export type TerminalBufferCellLike = { getChars(): string; getWidth(): number };
export type TerminalBufferLineLike = {
  readonly isWrapped: boolean;
  readonly length: number;
  getCell(x: number): TerminalBufferCellLike | undefined;
};
export type TerminalBufferLike = {
  getLine(y: number): TerminalBufferLineLike | undefined;
};

// The buffer cell of one UTF-16 unit in a LogicalLine. x is the 0-based
// column and y is the 0-based buffer line. width is the cell width: 2 for
// a wide character.
export type LogicalLineCell = { x: number; y: number; width: number };

// The text of one or more buffer rows that soft wrap makes one line.
// cells[i] is the buffer cell of text[i], so cells.length === text.length.
export type LogicalLine = { text: string; cells: LogicalLineCell[] };

// The maximum number of buffer rows in one LogicalLine.
export const MAX_LOGICAL_LINE_ROWS = 64;

// Reads the logical line that contains buffer line y (0-based). The function
// goes up while the row has isWrapped, and goes down while
// getLine(row + 1)?.isWrapped. It reads MAX_LOGICAL_LINE_ROWS rows or fewer.
// It skips cells with width 0 (the second half of a wide character) and
// writes a space for an empty cell. Returns null when the line has no text.
export function readLogicalLine(buffer: TerminalBufferLike, y: number): LogicalLine | null {
  if (!buffer.getLine(y)) {
    return null;
  }
  let first = y;
  while (first > 0 && y - first < MAX_LOGICAL_LINE_ROWS - 1 && buffer.getLine(first)?.isWrapped) {
    first -= 1;
  }
  let last = y;
  while (last - first < MAX_LOGICAL_LINE_ROWS - 1 && buffer.getLine(last + 1)?.isWrapped) {
    last += 1;
  }

  let text = "";
  const cells: LogicalLineCell[] = [];
  for (let row = first; row <= last; row += 1) {
    const bufferLine = buffer.getLine(row);
    if (!bufferLine) {
      break;
    }
    let rowEnd = bufferLine.length;
    if (row < last) {
      // xterm leaves the last cell empty when a wide character does not fit
      // and wraps to the next row. The empty cell is not a space in the text.
      while (rowEnd > 0 && bufferLine.getCell(rowEnd - 1)?.getChars() === "") {
        rowEnd -= 1;
      }
    }
    for (let x = 0; x < rowEnd; x += 1) {
      const cell = bufferLine.getCell(x);
      if (!cell) {
        break;
      }
      const width = cell.getWidth();
      if (width === 0) {
        continue;
      }
      const chars = cell.getChars() || " ";
      text += chars;
      for (let unit = 0; unit < chars.length; unit += 1) {
        cells.push({ x, y: row, width });
      }
    }
  }
  return text.trim().length > 0 ? { text, cells } : null;
}

// Converts the text offsets [startIndex, endIndex) of a LogicalLine to an
// xterm link range. In the range, x is the 1-based column, y is the 1-based
// buffer line, and end is the last cell of the link (inclusive). A wide last
// character ends on its second cell. Returns null for a bad offset.
export function rangeForCandidate(
  line: LogicalLine,
  startIndex: number,
  endIndex: number,
): IBufferRange | null {
  if (
    !Number.isInteger(startIndex) ||
    !Number.isInteger(endIndex) ||
    startIndex < 0 ||
    endIndex <= startIndex ||
    endIndex > line.cells.length
  ) {
    return null;
  }
  const first = line.cells[startIndex];
  const last = line.cells[endIndex - 1];
  return {
    start: { x: first.x + 1, y: first.y + 1 },
    end: { x: last.x + last.width, y: last.y + 1 },
  };
}

// The action for a click on a path link. "open" opens the path in the system
// default app, and "reveal" shows it in Finder or Explorer.
export type TerminalLinkGesture = "open" | "reveal";

export type TerminalLinkGestureEvent = Pick<
  MouseEvent,
  "button" | "detail" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey"
>;

// Returns the action for a mouse event on a path link, or null.
// The click must pass isTerminalLinkClick (Cmd on macOS, Ctrl elsewhere) and
// must not have Alt. Shift selects "reveal". Other clicks select "open".
export function terminalLinkGesture(
  event: TerminalLinkGestureEvent,
  platform?: string,
): TerminalLinkGesture | null {
  if (!isTerminalLinkClick(event, platform) || event.altKey) {
    return null;
  }
  return event.shiftKey ? "reveal" : "open";
}

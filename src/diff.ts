import type {
  ReviewFile,
  ReviewHunk,
  ReviewLine,
  ReviewLineSegment,
} from "./types";

const MAX_WORD_DIFF_LINE_LENGTH = 400;
const MAX_WORD_DIFF_RUN_LENGTH = 200;
export const MAX_WORD_DIFF_TOTAL_CELLS = 2_000_000;
export const MAX_PARSED_DIFF_CHARACTERS = 1024 * 1024;
export const MAX_PARSED_DIFF_LINES = 10_000;

// One budget caps the total word-diff table cells for one parse call.
type WordDiffBudget = { remaining: number };

export type ParsedUnifiedDiff = {
  files: ReviewFile[];
  truncated: boolean;
};

export type SideBySideRow = {
  left: ReviewLine | null;
  right: ReviewLine | null;
};

export function sourceReviewLineId(lineId: string) {
  return lineId.replace(/:[lr]$/, "");
}

export function parseUnifiedDiff(diff: string): ReviewFile[] {
  return parseUnifiedDiffWithLimit(diff).files;
}

export function parseUnifiedDiffWithLimit(
  diff: string,
  maxCharacters = MAX_PARSED_DIFF_CHARACTERS,
  maxLines = MAX_PARSED_DIFF_LINES,
): ParsedUnifiedDiff {
  const limited = limitUnifiedDiff(diff, maxCharacters, maxLines);
  const budget: WordDiffBudget = { remaining: MAX_WORD_DIFF_TOTAL_CELLS };
  const starts = [...limited.diff.matchAll(/^diff --git .+$/gm)].map(
    (match) => match.index ?? 0,
  );
  if (starts.length === 0) {
    return {
      files: limited.diff.trim() ? [parseFileDiff(limited.diff, 0, budget)] : [],
      truncated: limited.truncated,
    };
  }
  return {
    files: starts.map((start, index) =>
      parseFileDiff(
        limited.diff.slice(
          start,
          starts[index + 1] ?? limited.diff.length,
        ),
        index,
        budget,
      ),
    ),
    truncated: limited.truncated,
  };
}

function limitUnifiedDiff(
  diff: string,
  maxCharacters: number,
  maxLines: number,
) {
  const characterLimit = Math.max(1, Math.floor(maxCharacters));
  const lineLimit = Math.max(1, Math.floor(maxLines));
  let end = diff.length;
  let truncated = false;

  if (end > characterLimit) {
    const lineEnd = diff.lastIndexOf("\n", characterLimit - 1);
    end = lineEnd >= 0 ? lineEnd + 1 : characterLimit;
    truncated = true;
  }

  let offset = 0;
  for (let line = 0; line < lineLimit; line += 1) {
    const next = diff.indexOf("\n", offset);
    if (next < 0 || next >= end) {
      return { diff: diff.slice(0, end), truncated };
    }
    offset = next + 1;
  }
  if (offset < end) {
    end = offset;
    truncated = true;
  }
  return { diff: diff.slice(0, end), truncated };
}

function parseFileDiff(
  raw: string,
  fileIndex: number,
  budget: WordDiffBudget,
): ReviewFile {
  const lines = raw.replace(/\r\n/g, "\n").split("\n");
  const oldHeader = lines.find((line) => line.startsWith("--- "))?.slice(4);
  const newHeader = lines.find((line) => line.startsWith("+++ "))?.slice(4);
  const headerPaths = parseGitHeader(lines[0] ?? "");
  const oldPath = cleanDiffPath(oldHeader) ?? headerPaths.oldPath;
  const newPath = cleanDiffPath(newHeader) ?? headerPaths.newPath;
  const path = newPath ?? oldPath ?? `File ${fileIndex + 1}`;
  const renamed = lines.some((line) => line.startsWith("rename from "));
  const binary = lines.some(
    (line) => line.startsWith("Binary files ") || line === "GIT binary patch",
  );
  const status = binary
    ? "binary"
    : renamed
      ? "renamed"
      : oldHeader === "/dev/null"
        ? "added"
        : newHeader === "/dev/null"
          ? "deleted"
          : "modified";
  const fileIdentity = path;
  const hunks: ReviewHunk[] = [];
  let additions = 0;
  let deletions = 0;

  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(
      /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/,
    );
    if (!match) {
      continue;
    }
    const header = lines[index];
    const hunkIndex = hunks.length;
    const hunkLines: ReviewLine[] = [];
    let oldLine = Number(match[1]);
    let newLine = Number(match[3]);
    index += 1;
    while (index < lines.length && !lines[index].startsWith("@@ ")) {
      const line = lines[index];
      if (line.startsWith("diff --git ")) {
        break;
      }
      const lineIndex = hunkLines.length;
      if (line.startsWith("+")) {
        additions += 1;
        hunkLines.push({
          id: `${fileIdentity}:${hunkIndex}:${lineIndex}:add:${newLine}`,
          kind: "add",
          newLine,
          text: line.slice(1),
        });
        newLine += 1;
      } else if (line.startsWith("-")) {
        deletions += 1;
        hunkLines.push({
          id: `${fileIdentity}:${hunkIndex}:${lineIndex}:delete:${oldLine}`,
          kind: "delete",
          oldLine,
          text: line.slice(1),
        });
        oldLine += 1;
      } else if (line.startsWith(" ")) {
        hunkLines.push({
          id: `${fileIdentity}:${hunkIndex}:${lineIndex}:context:${oldLine}:${newLine}`,
          kind: "context",
          oldLine,
          newLine,
          text: line.slice(1),
        });
        oldLine += 1;
        newLine += 1;
      } else if (line) {
        hunkLines.push({
          id: `${fileIdentity}:${hunkIndex}:${lineIndex}:meta`,
          kind: "meta",
          text: line,
        });
      }
      index += 1;
    }
    index -= 1;
    addIntraLineSegments(hunkLines, budget);
    hunks.push({
      id: `${fileIdentity}:${hunkIndex}:${match[1]}:${match[3]}`,
      header: header ?? `@@ -${match[1]} +${match[3]} @@`,
      oldStart: Number(match[1]),
      newStart: Number(match[3]),
      lines: hunkLines,
    });
  }

  return {
    id: fileIdentity,
    path,
    oldPath: oldPath && oldPath !== path ? oldPath : undefined,
    status,
    additions,
    deletions,
    binary,
    hunks,
    raw,
  };
}

export function toSideBySideRows(hunk: ReviewHunk): SideBySideRow[] {
  const rows: SideBySideRow[] = [];

  for (let index = 0; index < hunk.lines.length; ) {
    const line = hunk.lines[index];
    if (line.kind === "delete") {
      const deletesStart = index;
      while (hunk.lines[index]?.kind === "delete") {
        index += 1;
      }
      const additionsStart = index;
      while (hunk.lines[index]?.kind === "add") {
        index += 1;
      }
      const deleteCount = additionsStart - deletesStart;
      const additionCount = index - additionsStart;
      const rowCount = Math.max(deleteCount, additionCount);

      for (let offset = 0; offset < rowCount; offset += 1) {
        const left =
          offset < deleteCount
            ? (hunk.lines[deletesStart + offset] ?? null)
            : null;
        const right =
          offset < additionCount
            ? (hunk.lines[additionsStart + offset] ?? null)
            : null;
        rows.push({
          left: splitLine(left, "l"),
          right: splitLine(right, "r"),
        });
      }
      continue;
    }

    if (line.kind === "add") {
      rows.push({ left: null, right: splitLine(line, "r") });
    } else {
      rows.push({ left: splitLine(line, "l"), right: splitLine(line, "r") });
    }
    index += 1;
  }

  return rows;
}

function splitLine(line: ReviewLine | null, side: "l" | "r") {
  return line ? { ...line, id: `${line.id}:${side}` } : null;
}

function addIntraLineSegments(lines: ReviewLine[], budget: WordDiffBudget) {
  for (let index = 0; index < lines.length; ) {
    if (lines[index]?.kind !== "delete") {
      index += 1;
      continue;
    }

    const deletesStart = index;
    while (lines[index]?.kind === "delete") {
      index += 1;
    }
    const additionsStart = index;
    while (lines[index]?.kind === "add") {
      index += 1;
    }

    const deleteCount = additionsStart - deletesStart;
    const additionCount = index - additionsStart;
    if (
      additionCount === 0 ||
      deleteCount > MAX_WORD_DIFF_RUN_LENGTH ||
      additionCount > MAX_WORD_DIFF_RUN_LENGTH
    ) {
      continue;
    }

    const pairCount = Math.min(deleteCount, additionCount);
    for (let offset = 0; offset < pairCount; offset += 1) {
      const deleted = lines[deletesStart + offset];
      const added = lines[additionsStart + offset];
      if (
        deleted.text.length > MAX_WORD_DIFF_LINE_LENGTH ||
        added.text.length > MAX_WORD_DIFF_LINE_LENGTH
      ) {
        continue;
      }
      // The character product bounds the LCS cell count for this pair.
      const cost = (deleted.text.length + 1) * (added.text.length + 1);
      if (cost > budget.remaining) {
        // The budget is spent. Later pairs keep the plain line diff.
        budget.remaining = 0;
        continue;
      }
      budget.remaining -= cost;
      const [left, right] = wordDiffSegments(deleted.text, added.text);
      deleted.segments = left;
      added.segments = right;
    }
  }
}

function wordDiffSegments(
  leftText: string,
  rightText: string,
): [ReviewLineSegment[], ReviewLineSegment[]] {
  const leftTokens = tokenize(leftText);
  const rightTokens = tokenize(rightText);
  const rowLength = rightTokens.length + 1;
  const lengths = new Uint16Array((leftTokens.length + 1) * rowLength);

  for (let left = leftTokens.length - 1; left >= 0; left -= 1) {
    for (let right = rightTokens.length - 1; right >= 0; right -= 1) {
      const cell = left * rowLength + right;
      lengths[cell] =
        leftTokens[left] === rightTokens[right]
          ? lengths[(left + 1) * rowLength + right + 1] + 1
          : Math.max(
              lengths[(left + 1) * rowLength + right],
              lengths[left * rowLength + right + 1],
            );
    }
  }

  const leftSegments: ReviewLineSegment[] = [];
  const rightSegments: ReviewLineSegment[] = [];
  let left = 0;
  let right = 0;
  while (left < leftTokens.length || right < rightTokens.length) {
    if (
      left < leftTokens.length &&
      right < rightTokens.length &&
      leftTokens[left] === rightTokens[right]
    ) {
      appendSegment(leftSegments, leftTokens[left], false);
      appendSegment(rightSegments, rightTokens[right], false);
      left += 1;
      right += 1;
    } else if (
      left < leftTokens.length &&
      (right === rightTokens.length ||
        lengths[(left + 1) * rowLength + right] >=
          lengths[left * rowLength + right + 1])
    ) {
      appendSegment(leftSegments, leftTokens[left], true);
      left += 1;
    } else {
      appendSegment(rightSegments, rightTokens[right], true);
      right += 1;
    }
  }

  return [leftSegments, rightSegments];
}

function tokenize(text: string) {
  return text.match(/[\p{L}\p{N}_]+|\s+|[^\s\p{L}\p{N}_]+/gu) ?? [];
}

function appendSegment(
  segments: ReviewLineSegment[],
  text: string,
  changed: boolean,
) {
  const previous = segments[segments.length - 1];
  if (previous?.changed === changed) {
    previous.text += text;
  } else {
    segments.push({ text, changed });
  }
}

function parseGitHeader(header: string) {
  const match = header.match(/^diff --git a\/(.+) b\/(.+)$/);
  return {
    oldPath: match?.[1],
    newPath: match?.[2],
  };
}

function cleanDiffPath(value: string | undefined) {
  if (!value || value === "/dev/null") {
    return undefined;
  }
  const valueWithoutTimestamp = value.split("\t", 1)[0];
  if (valueWithoutTimestamp.startsWith("a/") || valueWithoutTimestamp.startsWith("b/")) {
    return valueWithoutTimestamp.slice(2);
  }
  return valueWithoutTimestamp;
}

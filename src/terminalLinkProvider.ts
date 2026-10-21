import type { ILink, ILinkProvider } from "@xterm/xterm";
import { ipc, type TerminalLinkTarget } from "./ipc.ts";
import {
  findTerminalPathCandidates,
  rangeForCandidate,
  readLogicalLine,
  terminalLinkGesture,
  type TerminalBufferLike,
  type TerminalLinkGesture,
  type TerminalPathCandidate,
} from "./terminalLinks.ts";

// Makes file paths in terminal output into links. A Cmd-click on macOS or a
// Ctrl-click elsewhere opens the path. Shift with the same click reveals the
// path in Finder or Explorer. The backend resolves relative paths against
// the cwd of the shell, so a path becomes a link only when it exists.

// A target that the backend found on disk. path is the absolute path.
export type ResolvedTerminalLinkTarget = TerminalLinkTarget & {
  path: string;
  kind: "file" | "dir";
};

export type TerminalPathLinkProviderDeps = {
  sessionId: string;
  // Returns the active buffer of the terminal at the time of the call.
  buffer: () => TerminalBufferLike;
  // Resolves path texts to targets: one result for each path text, in the
  // same order. The default sends terminal_link_targets to the backend.
  resolve?: (candidates: string[]) => Promise<TerminalLinkTarget[]>;
  // Does the action of a link click. The default sends app_open_path or
  // app_reveal_path to the backend.
  open?: (target: ResolvedTerminalLinkTarget, gesture: TerminalLinkGesture) => void;
  onHover?: (target: ResolvedTerminalLinkTarget, event: MouseEvent) => void;
  onLeave?: () => void;
  platform?: string;
  now?: () => number;
};

// The cache keeps a "none" result for a short time. A program can print a
// path before it writes the file, and the path must become a link soon.
export const MISSING_TARGET_TTL_MS = 10_000;
export const FOUND_TARGET_TTL_MS = 60_000;
// A relative path depends on the cwd of the shell, and the cwd changes with
// each "cd". The cache keeps a relative result only for a hover burst.
export const RELATIVE_TARGET_TTL_MS = 1_000;
// The cache keeps this number of path texts or fewer.
export const MAX_CACHED_TARGETS = 512;

type CacheEntry = { target: TerminalLinkTarget; at: number };

function noneTarget(text: string): TerminalLinkTarget {
  return { text, path: null, kind: "none" };
}

// Returns a safe target for one backend result. A bad result becomes "none".
function sanitizeTarget(text: string, value: unknown): TerminalLinkTarget {
  if (!value || typeof value !== "object") return noneTarget(text);
  const { path, kind } = value as Partial<TerminalLinkTarget>;
  if ((kind === "file" || kind === "dir") && typeof path === "string" && path.length > 0) {
    return { text, path, kind };
  }
  return noneTarget(text);
}

// Returns true when the path text does not depend on the cwd: an absolute
// path, a path under the home directory, or a Windows drive or UNC path.
function isCwdIndependent(pathText: string): boolean {
  return /^(?:[/\\]|[A-Za-z]:[/\\]|~(?:$|[/\\]))/.test(pathText);
}

function isResolved(target: TerminalLinkTarget): target is ResolvedTerminalLinkTarget {
  return target.kind !== "none" && typeof target.path === "string";
}

// Sends the click action to the backend. "open" opens a directory in the file
// manager and a file in its default app. The backend reveals a launchable
// file and does not run it. "reveal" shows the path in the file manager.
export function openTerminalPathTarget(
  target: ResolvedTerminalLinkTarget,
  gesture: TerminalLinkGesture,
): void {
  const request =
    gesture === "reveal"
      ? ipc.app.revealPath({ path: target.path })
      : ipc.app.openPath({ path: target.path });
  void Promise.resolve(request).catch((error) => {
    console.error("Terminal path link open failed", error);
  });
}

type Found = { candidate: TerminalPathCandidate; target: ResolvedTerminalLinkTarget };

// Keeps the longest of overlapping links. Returns the links in line order.
function dropOverlaps(found: Found[]): Found[] {
  const byLength = [...found].sort(
    (a, b) =>
      b.candidate.endIndex - b.candidate.startIndex -
      (a.candidate.endIndex - a.candidate.startIndex),
  );
  const kept: Found[] = [];
  for (const item of byLength) {
    const overlaps = kept.some(
      (other) =>
        item.candidate.startIndex < other.candidate.endIndex &&
        other.candidate.startIndex < item.candidate.endIndex,
    );
    if (!overlaps) kept.push(item);
  }
  return kept.sort((a, b) => a.candidate.startIndex - b.candidate.startIndex);
}

export function createTerminalPathLinkProvider(
  deps: TerminalPathLinkProviderDeps,
): ILinkProvider {
  const now = deps.now ?? Date.now;
  const resolve =
    deps.resolve ??
    ((candidates: string[]) =>
      ipc.terminal.linkTargets({ sessionId: deps.sessionId, candidates }));
  const open = deps.open ?? openTerminalPathTarget;
  const cache = new Map<string, CacheEntry>();
  const inFlight = new Map<string, Promise<Map<string, TerminalLinkTarget>>>();
  let loggedError = false;

  function cached(pathText: string, at: number): TerminalLinkTarget | null {
    const entry = cache.get(pathText);
    if (!entry) return null;
    let ttl = entry.target.kind === "none" ? MISSING_TARGET_TTL_MS : FOUND_TARGET_TTL_MS;
    if (!isCwdIndependent(pathText)) ttl = Math.min(ttl, RELATIVE_TARGET_TTL_MS);
    if (at - entry.at < ttl) return entry.target;
    cache.delete(pathText);
    return null;
  }

  function remember(pathText: string, target: TerminalLinkTarget) {
    cache.delete(pathText);
    cache.set(pathText, { target, at: now() });
    while (cache.size > MAX_CACHED_TARGETS) {
      const oldest = cache.keys().next().value;
      if (oldest === undefined) break;
      cache.delete(oldest);
    }
  }

  // Sends one batch for the path texts that the cache does not have.
  async function request(pathTexts: string[]): Promise<Map<string, TerminalLinkTarget>> {
    try {
      // The extra tick makes sure that targetsFor stores the batch in
      // inFlight before the finally block below removes it.
      const results = await Promise.resolve().then(() => resolve(pathTexts));
      const targets = new Map<string, TerminalLinkTarget>();
      pathTexts.forEach((pathText, index) => {
        const target = sanitizeTarget(
          pathText,
          Array.isArray(results) ? results[index] : undefined,
        );
        targets.set(pathText, target);
        remember(pathText, target);
      });
      return targets;
    } finally {
      for (const pathText of pathTexts) inFlight.delete(pathText);
    }
  }

  async function targetsFor(pathTexts: string[]): Promise<Map<string, TerminalLinkTarget>> {
    const at = now();
    const targets = new Map<string, TerminalLinkTarget>();
    const waits = new Set<Promise<Map<string, TerminalLinkTarget>>>();
    const missing: string[] = [];
    for (const pathText of new Set(pathTexts)) {
      const hit = cached(pathText, at);
      if (hit) {
        targets.set(pathText, hit);
        continue;
      }
      const pending = inFlight.get(pathText);
      if (pending) {
        waits.add(pending);
        continue;
      }
      missing.push(pathText);
    }
    if (missing.length > 0) {
      const batch = request(missing);
      for (const pathText of missing) inFlight.set(pathText, batch);
      waits.add(batch);
    }
    for (const result of await Promise.all(waits)) {
      for (const [pathText, target] of result) {
        if (!targets.has(pathText)) targets.set(pathText, target);
      }
    }
    return targets;
  }

  function linkFor({ candidate, target }: Found, range: ILink["range"]): ILink {
    return {
      range,
      text: candidate.text,
      activate(event) {
        const gesture = terminalLinkGesture(event, deps.platform);
        if (!gesture) return;
        open(target, gesture);
      },
      hover(event) {
        deps.onHover?.(target, event);
      },
      leave() {
        deps.onLeave?.();
      },
    };
  }

  async function provide(bufferLineNumber: number): Promise<ILink[] | undefined> {
    // xterm gives a 1-based buffer line. readLogicalLine reads a 0-based row.
    const row = bufferLineNumber - 1;
    const line = readLogicalLine(deps.buffer(), row);
    if (!line) return undefined;
    const candidates = findTerminalPathCandidates(line.text);
    if (candidates.length === 0) return undefined;

    const targets = await targetsFor(candidates.map((candidate) => candidate.pathText));

    // The terminal can write new text while the backend works. Then the
    // offsets do not match the cells, so the provider gives no links.
    if (readLogicalLine(deps.buffer(), row)?.text !== line.text) return undefined;

    const found: Found[] = [];
    for (const candidate of candidates) {
      const target = targets.get(candidate.pathText);
      if (target && isResolved(target)) found.push({ candidate, target });
    }
    const links: ILink[] = [];
    for (const item of dropOverlaps(found)) {
      const range = rangeForCandidate(line, item.candidate.startIndex, item.candidate.endIndex);
      // A wrapped line has links on other rows. xterm counts the cells of a
      // link on another row as cells of the hovered row. It then removes the
      // links of the hovered row that meet those cells. So the provider
      // gives only the links that cover the hovered row.
      if (range && range.start.y <= bufferLineNumber && bufferLineNumber <= range.end.y) {
        links.push(linkFor(item, range));
      }
    }
    return links.length > 0 ? links : undefined;
  }

  // Counts the provideLinks calls. xterm asks again when the mouse moves to
  // another line, and a reply for the old line then writes over the reply
  // for the new line. So a call that a newer call replaced never replies.
  let latestCall = 0;

  return {
    provideLinks(bufferLineNumber, callback) {
      const call = (latestCall += 1);
      let called = false;
      const reply = (links: ILink[] | undefined) => {
        if (called) return;
        called = true;
        if (call !== latestCall) return;
        callback(links);
      };
      provide(bufferLineNumber).then(reply, (error: unknown) => {
        if (!loggedError) {
          loggedError = true;
          console.error("Terminal path links failed", error);
        }
        reply(undefined);
      });
    },
  };
}

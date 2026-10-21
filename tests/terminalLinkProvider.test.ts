import assert from "node:assert/strict";
import test from "node:test";
import type { ILink, ILinkProvider } from "@xterm/xterm";

// @ts-expect-error Node's test runner needs the source extension.
import { setIpcTransport } from "../src/ipc.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  createTerminalPathLinkProvider,
  FOUND_TARGET_TTL_MS,
  MISSING_TARGET_TTL_MS,
  RELATIVE_TARGET_TTL_MS,
} from "../src/terminalLinkProvider.ts";

type Target = { text: string; path: string | null; kind: "file" | "dir" | "none" };

// A small buffer: each row is a string of one-cell characters. A row with
// wrapped: true continues the row above it.
type FakeRow = { text: string; wrapped?: boolean };

function fakeBuffer(rows: FakeRow[], columns = 40) {
  return {
    rows,
    getLine(y: number) {
      const row = rows[y];
      if (!row) return undefined;
      return {
        isWrapped: row.wrapped === true,
        length: columns,
        getCell(x: number) {
          if (x >= columns) return undefined;
          const char = row.text[x] ?? "";
          return { getChars: () => char, getWidth: () => 1 };
        },
      };
    },
  };
}

// Calls provideLinks and returns the links and the number of callback calls.
async function provide(provider: ILinkProvider, bufferLineNumber: number) {
  let calls = 0;
  let links: ILink[] | undefined;
  await new Promise<void>((resolve) => {
    provider.provideLinks(bufferLineNumber, (value) => {
      calls += 1;
      links = value;
      resolve();
    });
  });
  // Let a second callback call show itself, if there is one.
  await new Promise((resolve) => setTimeout(resolve, 5));
  return { links, calls };
}

// Calls provideLinks and records each callback call, without waiting.
function replies(provider: ILinkProvider, bufferLineNumber: number) {
  const calls: Array<ILink[] | undefined> = [];
  provider.provideLinks(bufferLineNumber, (value) => {
    calls.push(value);
  });
  return { calls };
}

// Lets the promise work of the provider finish.
function settle() {
  return new Promise((resolve) => setTimeout(resolve, 5));
}

// Resolves each path text from a table. Unknown paths give "none".
function tableResolver(table: Record<string, Target["kind"]>) {
  const requests: string[][] = [];
  const resolve = async (candidates: string[]): Promise<Target[]> => {
    requests.push([...candidates]);
    return candidates.map((text) => {
      const kind = table[text] ?? "none";
      return { text, path: kind === "none" ? null : `/work/${text}`, kind };
    });
  };
  return { resolve, requests };
}

function click(fields: Partial<MouseEvent>): MouseEvent {
  return {
    button: 0,
    detail: 1,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    ...fields,
  } as MouseEvent;
}

test("provides links only for paths that exist", async () => {
  const buffer = fakeBuffer([{ text: "edit src/a.ts:3 and src/gone.ts now" }]);
  const { resolve, requests } = tableResolver({ "src/a.ts": "file" });
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve,
  });

  const { links, calls } = await provide(provider, 1);

  assert.equal(calls, 1);
  assert.deepEqual(requests, [["src/a.ts", "src/gone.ts"]]);
  assert.equal(links?.length, 1);
  assert.equal(links?.[0].text, "src/a.ts:3");
  // "src/a.ts:3" starts at 0-based column 5 and ends at column 14.
  assert.deepEqual(links?.[0].range, { start: { x: 6, y: 1 }, end: { x: 15, y: 1 } });
});

test("maps a path on a wrapped line to both rows", async () => {
  const buffer = fakeBuffer(
    [{ text: "$ cat src/lo" }, { text: "ng/file.ts", wrapped: true }],
    12,
  );
  const { resolve } = tableResolver({ "src/long/file.ts": "file" });
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve,
  });

  const { links } = await provide(provider, 2);

  assert.equal(links?.length, 1);
  assert.equal(links?.[0].text, "src/long/file.ts");
  assert.deepEqual(links?.[0].range, { start: { x: 7, y: 1 }, end: { x: 10, y: 2 } });
});

test("gives no links and sends no request when the line has no path", async () => {
  const buffer = fakeBuffer([{ text: "hello world" }, { text: "" }]);
  const { resolve, requests } = tableResolver({});
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve,
  });

  assert.deepEqual(await provide(provider, 1), { links: undefined, calls: 1 });
  assert.deepEqual(await provide(provider, 2), { links: undefined, calls: 1 });
  assert.deepEqual(await provide(provider, 9), { links: undefined, calls: 1 });
  assert.deepEqual(requests, []);
});

test("caches found and missing targets with different lifetimes", async () => {
  let clock = 1_000;
  // Absolute paths do not depend on the cwd, so the cache keeps them longer.
  const buffer = fakeBuffer([{ text: "/work/src/a.ts /work/src/later.ts" }], 60);
  const table: Record<string, Target["kind"]> = { "/work/src/a.ts": "file" };
  const { resolve, requests } = tableResolver(table);
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve,
    now: () => clock,
  });

  assert.equal((await provide(provider, 1)).links?.length, 1);
  assert.equal((await provide(provider, 1)).links?.length, 1);
  assert.equal(requests.length, 1);

  // The program writes the second file. The "none" result expires first.
  table["/work/src/later.ts"] = "file";
  clock += MISSING_TARGET_TTL_MS;
  const second = await provide(provider, 1);
  assert.deepEqual(requests.at(-1), ["/work/src/later.ts"]);
  assert.deepEqual(
    second.links?.map((link) => link.text),
    ["/work/src/a.ts", "/work/src/later.ts"],
  );

  // A found result expires after its own lifetime.
  clock += FOUND_TARGET_TTL_MS;
  await provide(provider, 1);
  assert.deepEqual(requests.at(-1), ["/work/src/a.ts", "/work/src/later.ts"]);
});

test("keeps a relative target only for a hover burst, so a cd changes the target", async () => {
  let clock = 1_000;
  let cwd = "/work/one";
  const buffer = fakeBuffer([{ text: "see docs/b.txt and ~/c.txt and /abs/d.txt" }], 60);
  const requests: string[][] = [];
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    now: () => clock,
    resolve: async (candidates: string[]) => {
      requests.push([...candidates]);
      return candidates.map((text) => ({
        text,
        path: text.startsWith("/") || text.startsWith("~") ? `/fixed/${text}` : `${cwd}/${text}`,
        kind: "file" as const,
      }));
    },
  });
  const first = await provide(provider, 1);
  assert.equal(first.links?.length, 3);
  assert.equal(requests.length, 1);

  // Inside the burst, the cache answers.
  clock += RELATIVE_TARGET_TTL_MS - 1;
  await provide(provider, 1);
  assert.equal(requests.length, 1);

  // The shell changes directory. Only the relative path asks the backend again.
  cwd = "/work/two";
  clock += 1;
  await provide(provider, 1);
  assert.deepEqual(requests.at(-1), ["docs/b.txt"]);
});

test("opens the new directory after a cd for a relative path", async () => {
  let clock = 1_000;
  let cwd = "/work/one";
  const buffer = fakeBuffer([{ text: "see docs/b.txt" }]);
  const opened: string[] = [];
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    now: () => clock,
    platform: "MacIntel",
    open: (target: { path: string }) => opened.push(target.path),
    resolve: async (candidates: string[]) =>
      candidates.map((text) => ({ text, path: `${cwd}/${text}`, kind: "file" as const })),
  });

  const before = (await provide(provider, 1)).links?.[0];
  assert.ok(before);
  before.activate(click({ metaKey: true }), before.text);

  cwd = "/work/two";
  clock += RELATIVE_TARGET_TTL_MS;
  const after = (await provide(provider, 1)).links?.[0];
  assert.ok(after);
  after.activate(click({ metaKey: true }), after.text);

  assert.deepEqual(opened, ["/work/one/docs/b.txt", "/work/two/docs/b.txt"]);
});

test("sends one request for two lines that resolve at the same time", async () => {
  const buffer = fakeBuffer([{ text: "src/a.ts" }, { text: "see src/a.ts" }]);
  const { resolve, requests } = tableResolver({ "src/a.ts": "dir" });
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve,
  });

  const stale = replies(provider, 1);
  const latest = replies(provider, 2);
  await settle();

  assert.equal(requests.length, 1);
  // The second call replaced the first, so only the second line replies.
  assert.equal(stale.calls.length, 0);
  assert.equal(latest.calls.length, 1);
  assert.equal(latest.calls[0]?.length, 1);
});

test("does not reply for an earlier line after a newer line asked", async () => {
  const buffer = fakeBuffer([
    { text: "see docs/a.txt" },
    { text: "see docs/b.txt" },
    { text: "see docs/c.txt" },
    { text: "see docs/d.txt" },
  ]);
  const gates = new Map<string, () => void>();
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve: (candidates: string[]) =>
      new Promise<Target[]>((resolve) => {
        gates.set(candidates[0], () =>
          resolve(candidates.map((text) => ({ text, path: `/work/${text}`, kind: "file" }))),
        );
      }),
  });

  // The pointer enters line 1 and moves to line 2 before the backend replies.
  const first = replies(provider, 1);
  await settle();
  const second = replies(provider, 2);
  await settle();

  // The reply for line 2 comes first. Then the late reply for line 1 comes.
  gates.get("docs/b.txt")?.();
  await settle();
  gates.get("docs/a.txt")?.();
  await settle();

  assert.equal(first.calls.length, 0);
  assert.equal(second.calls.length, 1);
  assert.deepEqual(
    second.calls[0]?.map((link) => link.text),
    ["docs/b.txt"],
  );

  // The reverse order must give the same result: no reply for the old line.
  const third = replies(provider, 3);
  await settle();
  const fourth = replies(provider, 4);
  await settle();
  gates.get("docs/c.txt")?.();
  await settle();
  gates.get("docs/d.txt")?.();
  await settle();
  assert.equal(third.calls.length, 0);
  assert.equal(fourth.calls.length, 1);
});

test("gives only the links that cover the hovered row of a wrapped line", async () => {
  // One logical line on three rows of 10 cells. Each row has one path.
  const buffer = fakeBuffer(
    [
      { text: "src/a.ts  " },
      { text: "src/b.ts  ", wrapped: true },
      { text: "src/c.ts", wrapped: true },
    ],
    10,
  );
  const { resolve } = tableResolver({ "src/a.ts": "file", "src/b.ts": "file", "src/c.ts": "file" });
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve,
  });

  const row1 = await provide(provider, 1);
  const row2 = await provide(provider, 2);
  const row3 = await provide(provider, 3);

  assert.deepEqual(row1.links?.map((link) => link.text), ["src/a.ts"]);
  assert.deepEqual(row2.links?.map((link) => link.text), ["src/b.ts"]);
  assert.deepEqual(row3.links?.map((link) => link.text), ["src/c.ts"]);
  assert.deepEqual(row2.links?.[0].range, { start: { x: 1, y: 2 }, end: { x: 8, y: 2 } });
});

test("keeps a link that starts on the row above and ends on the hovered row", async () => {
  const buffer = fakeBuffer(
    [{ text: "src/a.ts  src/l" }, { text: "ong.ts z/d.ts", wrapped: true }],
    15,
  );
  const { resolve } = tableResolver({ "src/a.ts": "file", "src/long.ts": "file", "z/d.ts": "file" });
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve,
  });

  const row1 = await provide(provider, 1);
  const row2 = await provide(provider, 2);

  assert.deepEqual(row1.links?.map((link) => link.text), ["src/a.ts", "src/long.ts"]);
  assert.deepEqual(row2.links?.map((link) => link.text), ["src/long.ts", "z/d.ts"]);
});

test("drops the links when the line changes during the request", async () => {
  const buffer = fakeBuffer([{ text: "src/a.ts" }]);
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve: async (candidates: string[]) => {
      buffer.rows[0] = { text: "src/a.ts changed" };
      return candidates.map((text) => ({ text, path: `/work/${text}`, kind: "file" as const }));
    },
  });

  assert.deepEqual(await provide(provider, 1), { links: undefined, calls: 1 });
});

test("calls the callback once and logs once when the request fails", async () => {
  const buffer = fakeBuffer([{ text: "src/a.ts" }, { text: "src/b.ts" }]);
  const errors: unknown[] = [];
  let requests = 0;
  const originalError = console.error;
  console.error = (...args: unknown[]) => {
    errors.push(args);
  };
  try {
    const provider = createTerminalPathLinkProvider({
      sessionId: "s1",
      buffer: () => buffer,
      resolve: () => {
        requests += 1;
        throw new Error("backend down");
      },
    });
    assert.deepEqual(await provide(provider, 1), { links: undefined, calls: 1 });
    assert.deepEqual(await provide(provider, 2), { links: undefined, calls: 1 });
    // A failed request does not stay in the cache or block a new request.
    assert.deepEqual(await provide(provider, 1), { links: undefined, calls: 1 });
    assert.equal(requests, 3);
    assert.equal(errors.length, 1);
  } finally {
    console.error = originalError;
  }
});

test("treats a bad backend result as no target", async () => {
  const buffer = fakeBuffer([{ text: "src/a.ts src/b.ts" }]);
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve: async () => [{ text: "src/a.ts", path: null, kind: "file" }] as never,
  });

  assert.deepEqual(await provide(provider, 1), { links: undefined, calls: 1 });
});

test("sends the click gesture to the right backend command", async () => {
  const invoked: Array<{ name: string; args: unknown }> = [];
  setIpcTransport({
    invoke: async (name: string, args?: { candidates?: string[] }) => {
      invoked.push({ name, args });
      if (name === "terminal_link_targets") {
        return (args?.candidates ?? []).map((text) => ({
          text,
          path: `/work/${text}`,
          kind: "file",
        }));
      }
      return undefined;
    },
    listen: async () => () => undefined,
  } as never);
  const buffer = fakeBuffer([{ text: "open report/index.html" }]);
  const provider = createTerminalPathLinkProvider({
    sessionId: "session-7",
    buffer: () => buffer,
    platform: "MacIntel",
  });

  const { links } = await provide(provider, 1);
  assert.deepEqual(invoked, [
    {
      name: "terminal_link_targets",
      args: { sessionId: "session-7", candidates: ["report/index.html"] },
    },
  ]);
  const link = links?.[0];
  assert.ok(link);
  invoked.length = 0;

  link.activate(click({ metaKey: true }), link.text);
  link.activate(click({ metaKey: true, shiftKey: true }), link.text);
  // These gestures do nothing: no modifier, Ctrl on macOS, Alt, a double
  // click and a right click.
  link.activate(click({}), link.text);
  link.activate(click({ ctrlKey: true }), link.text);
  link.activate(click({ metaKey: true, altKey: true }), link.text);
  link.activate(click({ metaKey: true, detail: 2 }), link.text);
  link.activate(click({ metaKey: true, button: 2 }), link.text);

  assert.deepEqual(invoked, [
    { name: "app_open_path", args: { path: "/work/report/index.html" } },
    { name: "app_reveal_path", args: { path: "/work/report/index.html" } },
  ]);
});

test("calls the hover and leave callbacks with the target", async () => {
  const buffer = fakeBuffer([{ text: "cd src/lib" }]);
  const { resolve } = tableResolver({ "src/lib": "dir" });
  const hovered: Target[] = [];
  let left = 0;
  const provider = createTerminalPathLinkProvider({
    sessionId: "s1",
    buffer: () => buffer,
    resolve,
    onHover: (target: Target) => hovered.push(target),
    onLeave: () => {
      left += 1;
    },
  });

  const { links } = await provide(provider, 1);
  const link = links?.[0];
  assert.ok(link);
  link.hover?.(click({}), link.text);
  link.leave?.(click({}), link.text);

  assert.deepEqual(hovered, [{ text: "src/lib", path: "/work/src/lib", kind: "dir" }]);
  assert.equal(left, 1);
});

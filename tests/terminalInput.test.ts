import assert from "node:assert/strict";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import { setIpcTransport } from "../src/ipc.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { createTerminalInputStream } from "../src/terminalInput.ts";

type WriteArgs = {
  sessionId: string;
  data: string;
  writer?: string;
  sequence?: number;
};

test("a pane numbers its writes from 1 in onData order", async () => {
  const writes: WriteArgs[] = [];
  const write = createTerminalInputStream("session-1", async (args) => {
    writes.push(args);
  });

  await Promise.all([write("O"), write("i"), write(" "), write("é")]);

  assert.deepEqual(
    writes.map(({ data, sequence }) => [data, sequence]),
    [
      ["O", 1],
      ["i", 2],
      [" ", 3],
      ["é", 4],
    ],
  );
  assert.ok(writes.every((args) => args.sessionId === "session-1"));
  assert.equal(new Set(writes.map((args) => args.writer)).size, 1);
});

test("each pane mount gets its own writer id", async () => {
  const writers = new Set<string | undefined>();
  for (let mount = 0; mount < 3; mount += 1) {
    const write = createTerminalInputStream("session-1", async (args) => {
      writers.add(args.writer);
      assert.equal(args.sequence, 1, "a new stream starts at 1");
    });
    await write("x");
  }
  assert.equal(writers.size, 3);
  assert.ok(![...writers].includes(undefined));
});

test("the stream sends terminal_write with the writer and the sequence", async () => {
  const invoked: Array<{ name: string; args: unknown }> = [];
  setIpcTransport({
    invoke: (async (name: string, args: unknown) => {
      invoked.push({ name, args });
      return null;
    }) as never,
    listen: async () => () => undefined,
  } as never);

  const write = createTerminalInputStream("session-2");
  await write("\r");

  assert.equal(invoked.length, 1);
  assert.equal(invoked[0].name, "terminal_write");
  const args = invoked[0].args as WriteArgs;
  assert.equal(args.sessionId, "session-2");
  assert.equal(args.data, "\r");
  assert.equal(args.sequence, 1);
  assert.equal(typeof args.writer, "string");
});

function settleCalls(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

test("a pane has one terminal_write call in flight at a time", async () => {
  const calls: Array<{ args: WriteArgs; settle: () => void }> = [];
  const write = createTerminalInputStream("session-1", (args) =>
    new Promise<void>((resolve) => {
      calls.push({ args, settle: resolve });
    }),
  );

  const first = write("a");
  const second = write("b");
  await settleCalls();
  assert.deepEqual(calls.map(({ args }) => args.data), ["a"]);

  calls[0].settle();
  await first;
  await settleCalls();
  assert.deepEqual(
    calls.map(({ args }) => [args.data, args.sequence]),
    [
      ["a", 1],
      ["b", 2],
    ],
  );
  calls[1].settle();
  await second;
});

test("a failed write fails only its caller and the next write still goes out", async () => {
  const sent: string[] = [];
  const write = createTerminalInputStream("session-1", async (args) => {
    sent.push(args.data);
    if (args.data === "a") throw new Error("terminal session is not running");
  });

  const first = write("a");
  const second = write("b");
  await assert.rejects(first, /terminal session is not running/);
  await second;
  assert.deepEqual(sent, ["a", "b"]);
});

test("a call that never settles holds the next write only until the watchdog", async () => {
  const sent: WriteArgs[] = [];
  let settleFirst = () => {};
  const write = createTerminalInputStream(
    "session-1",
    (args) => {
      sent.push(args);
      if (args.sequence !== 1) return Promise.resolve();
      return new Promise<void>((resolve) => {
        settleFirst = resolve;
      });
    },
    20,
  );

  const first = write("a");
  const second = write("b");
  await settleCalls();
  assert.equal(sent.length, 1);
  await second;

  // The first call settles late. It does not send a write a second time.
  settleFirst();
  await first;
  await write("c");
  assert.deepEqual(
    sent.map(({ data, sequence }) => [data, sequence]),
    [
      ["a", 1],
      ["b", 2],
      ["c", 3],
    ],
  );
});

import { ipc } from "./ipc.ts";

export type TerminalInputWrite = (data: string) => Promise<void>;

/**
 * How long a write waits for the previous call of its pane. A lost IPC answer
 * must not stop the typing, so the next write goes out after this time.
 */
export const TERMINAL_INPUT_WATCHDOG_MS = 1_000;

type QueuedWrite = {
  data: string;
  resolve: () => void;
  reject: (cause: unknown) => void;
};

/**
 * Sends the input of one pane as an ordered stream.
 *
 * Tauri runs each `terminal_write` call as its own async task, so two calls
 * can reach the backend in either order. A pane therefore sends one call at a
 * time: the next write goes out when the previous call settles, or after the
 * watchdog time. Each write carries the writer id of the stream and the next
 * sequence number, from 1. The backend writes the stream in sequence order. It
 * drops a number that it already received, for example when Tauri sends a call
 * again after a failed IPC fetch. A call that arrives after the backend stopped
 * its wait still goes out, after the later writes, so a slow call does not
 * lose typed input.
 */
export function createTerminalInputStream(
  sessionId: string,
  write: typeof ipc.terminal.write = (args) => ipc.terminal.write(args),
  watchdogMs = TERMINAL_INPUT_WATCHDOG_MS,
): TerminalInputWrite {
  const writer = terminalInputWriterId();
  const queued: QueuedWrite[] = [];
  let sequence = 0;
  let sending = false;

  const sendNext = () => {
    const next = queued.shift();
    if (!next) {
      sending = false;
      return;
    }
    sending = true;
    sequence += 1;
    // Each chunk stays one call. The backend reads a lone Esc or answer key
    // as an agent signal, so the queue never joins chunks.
    let released = false;
    const release = () => {
      if (released) return;
      released = true;
      clearTimeout(watchdog);
      sendNext();
    };
    const watchdog = setTimeout(release, watchdogMs);
    let call: Promise<void>;
    try {
      call = write({ sessionId, data: next.data, writer, sequence });
    } catch (cause) {
      call = Promise.reject(cause);
    }
    void call.then(next.resolve, next.reject).finally(release);
  };

  return (data) =>
    new Promise<void>((resolve, reject) => {
      queued.push({ data, resolve, reject });
      if (!sending) sendNext();
    });
}

function terminalInputWriterId(): string {
  return globalThis.crypto?.randomUUID?.() ??
    `input-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

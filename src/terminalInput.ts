import { ipc } from "./ipc.ts";

export type TerminalInputWrite = (data: string) => Promise<void>;

/**
 * Sends the input of one pane as an ordered stream.
 *
 * Tauri runs each `terminal_write` call as its own async task, so two calls
 * can reach the backend in either order. Each write carries the writer id of
 * the stream and the next sequence number, from 1. The backend writes the
 * stream in sequence order. It drops a number that it already received, for
 * example when Tauri sends a call again after a failed IPC fetch.
 */
export function createTerminalInputStream(
  sessionId: string,
  write: typeof ipc.terminal.write = (args) => ipc.terminal.write(args),
): TerminalInputWrite {
  const writer = terminalInputWriterId();
  let sequence = 0;
  return (data) => {
    sequence += 1;
    return write({ sessionId, data, writer, sequence });
  };
}

function terminalInputWriterId(): string {
  return globalThis.crypto?.randomUUID?.() ??
    `input-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

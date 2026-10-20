export type TerminalDraftFailure = "unavailable" | "occupied" | "invalid";
export class TerminalDraftError extends Error {
  readonly reason: TerminalDraftFailure;
  constructor(reason: TerminalDraftFailure) {
    super(reason);
    this.reason = reason;
  }
}

// This bridge addresses an exact mounted PTY. It prepares text through the
// same input path as paste, never writes an Enter and never picks another pane.
export function createTerminalDraftBridge() {
  const handlers = new Map<string, (text: string) => Promise<void>>();
  return {
    register(id: string, handler: (text: string) => Promise<void>) {
      handlers.set(id, handler);
      return () => { if (handlers.get(id) === handler) handlers.delete(id); };
    },
    async prepare(id: string, text: string) {
      // A multi-line prompt must not turn into multiple terminal submissions.
      const plain = text.replace(/[\r\n\t]+/g, " ").trim();
      if (!plain || /[\u0000-\u001f\u007f-\u009f]/.test(plain)) throw new TerminalDraftError("invalid");
      const handler = handlers.get(id);
      if (!handler) throw new TerminalDraftError("unavailable");
      await handler(plain);
    },
  };
}
export const terminalDraftBridge = createTerminalDraftBridge();

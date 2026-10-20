// One PTY has one size, but more than one pane can show a session. The
// retained conversation surface and the Agent sessions overlay can both
// mount a pane for it. The pane that became visible last owns the size.
//
// Only the owner sends terminal_resize, and only when its grid differs from
// the last size sent for the session. Each sent size makes the agent repaint
// its full screen. When the owner hides or closes, the next visible pane
// becomes the owner and sends its own grid again.

export type TerminalGridSize = { cols: number; rows: number };

export type TerminalSizeClaim = {
  // Records the grid of the pane. The owner sends it when it changed.
  resize: (size: TerminalGridSize) => void;
  // Records the size that the PTY has, when no pane sent a size yet.
  ptySize: (size: TerminalGridSize) => void;
  setVisible: (visible: boolean) => void;
  dispose: () => void;
};

type PaneSize = {
  size: TerminalGridSize | null;
  send: (size: TerminalGridSize) => Promise<unknown>;
  onError: (cause: unknown) => void;
  // False when the session process ended. The PTY is gone then.
  canSend: () => boolean;
};

type SessionSize = {
  panes: Set<PaneSize>;
  // Visible panes in the order they became visible. The last one is the owner.
  visible: PaneSize[];
  sent: TerminalGridSize | null;
};

function ownerOf(session: SessionSize): PaneSize | undefined {
  return session.visible[session.visible.length - 1];
}

export function createTerminalSizeRegistry() {
  const sessions = new Map<string, SessionSize>();

  const sendIfChanged = (session: SessionSize, pane: PaneSize) => {
    const size = pane.size;
    if (!size || ownerOf(session) !== pane || !pane.canSend()) return;
    const sent = session.sent;
    if (sent && sent.cols === size.cols && sent.rows === size.rows) return;
    session.sent = size;
    void pane.send(size).catch((cause) => {
      // The PTY size is not known now, so the next grid is sent again.
      if (session.sent === size) session.sent = null;
      pane.onError(cause);
    });
  };

  const claim = (
    sessionId: string,
    send: PaneSize["send"],
    onError: PaneSize["onError"],
    canSend: PaneSize["canSend"] = () => true,
  ): TerminalSizeClaim => {
    const session: SessionSize = sessions.get(sessionId) ?? {
      panes: new Set(),
      visible: [],
      sent: null,
    };
    sessions.set(sessionId, session);
    const pane: PaneSize = { size: null, send, onError, canSend };
    session.panes.add(pane);
    let disposed = false;

    const hide = () => {
      const index = session.visible.indexOf(pane);
      if (index < 0) return;
      const wasOwner = index === session.visible.length - 1;
      session.visible.splice(index, 1);
      const owner = ownerOf(session);
      if (wasOwner && owner) sendIfChanged(session, owner);
    };

    return {
      resize(size) {
        if (disposed) return;
        pane.size = { cols: size.cols, rows: size.rows };
        sendIfChanged(session, pane);
      },
      ptySize(size) {
        if (disposed || session.sent) return;
        session.sent = { cols: size.cols, rows: size.rows };
      },
      setVisible(visible) {
        if (disposed) return;
        if (!visible) {
          hide();
          return;
        }
        if (session.visible.includes(pane)) return;
        session.visible.push(pane);
        sendIfChanged(session, pane);
      },
      dispose() {
        if (disposed) return;
        disposed = true;
        hide();
        session.panes.delete(pane);
        // A new pane for the session sends its first grid again.
        if (session.panes.size === 0 && sessions.get(sessionId) === session) {
          sessions.delete(sessionId);
        }
      },
    };
  };

  return { claim };
}

export const terminalSizeRegistry = createTerminalSizeRegistry();

// One webview drag-drop listener serves every mounted terminal pane. Panes
// retain it while mounted; the last pane to unmount releases it.
import { getCurrentWebview } from "@tauri-apps/api/webview";

import { nativeDropPoint } from "./nativeDropPoint";
import { terminalDropBridge } from "./terminalDrop";

let holders = 0;
let generation = 0;
let unlisten: (() => void) | undefined;

function start(currentGeneration: number) {
  if (!("__TAURI_INTERNALS__" in window)) return;
  try {
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "leave") {
          terminalDropBridge.clearHover();
          return;
        }
        const position = nativeDropPoint(payload.position);
        if (payload.type === "drop") {
          terminalDropBridge.drop(position.x, position.y, payload.paths);
          return;
        }
        terminalDropBridge.hover(position.x, position.y);
      })
      .then((stopListening) => {
        if (currentGeneration !== generation || holders === 0) {
          stopListening();
        } else {
          unlisten = stopListening;
        }
      })
      .catch(() => undefined);
  } catch {
    // The webview API is unavailable outside Tauri; DOM drops still work.
  }
}

function stop() {
  generation += 1;
  unlisten?.();
  unlisten = undefined;
  terminalDropBridge.clearHover();
}

/** Keeps the native drop listener alive. The return value releases it. */
export function retainNativeTerminalDrop(): () => void {
  holders += 1;
  if (holders === 1) {
    generation += 1;
    start(generation);
  }
  let released = false;
  return () => {
    if (released) return;
    released = true;
    holders -= 1;
    if (holders === 0) stop();
  };
}

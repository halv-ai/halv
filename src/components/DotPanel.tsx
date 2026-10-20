import { useEffect, useRef, useState } from "react";
import { ipc } from "../ipc";
import { useT } from "../i18n/runtime";

// Serialize layout/hide requests, including unmount/remount during fast switching.
let viewQueue: Promise<unknown> = Promise.resolve();
function queueView(args: Parameters<typeof ipc.dot.setView>[0]) {
  const next = viewQueue.catch(() => {}).then(() => ipc.dot.setView(args));
  viewQueue = next;
  return next;
}

export function DotPanel({ suspended }: { suspended: boolean }) {
  const t = useT();
  const host = useRef<HTMLDivElement>(null);
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const native = !location.pathname.includes("ux-audit.html");

  useEffect(() => {
    if (!native) return;
    let disposed = false;
    let frame = 0;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        if (disposed || !host.current) return;
        const rect = host.current.getBoundingClientRect();
        void queueView({visible: !suspended && rect.width > 0 && rect.height > 0,
          x: rect.x, y: rect.y, width: rect.width, height: rect.height,
        }).then(() => { if (!disposed) setError(false); })
          .catch(() => { if (!disposed) setError(true); });
      });
    };
    const observer = new ResizeObserver(update);
    if (host.current) observer.observe(host.current);
    window.addEventListener("resize", update);
    update();
    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", update);
      void queueView({visible:false, x:0, y:0, width:1, height:1}).catch(() => {});
    };
  }, [suspended, attempt, native]);

  return <section className="dot-panel" aria-label="Dot">
    <div ref={host} className="dot-panel-host">
      {!native ? <p className="m-auto max-w-md p-6 text-sm text-content-muted">{t("Dot runs inside the desktop app. This browser preview does not load your account.")}</p>
        : error ? <div role="alert" className="m-auto max-w-md p-6 text-sm">
          <p>{t("Could not load Dot. Check your connection and try again.")}</p>
          <button className="settings-secondary-action mt-3" onClick={() => setAttempt(n => n + 1)}>{t("Retry")}</button>
        </div> : <p className="m-auto text-sm text-content-muted">{t("Loading Dot…")}</p>}
    </div>
  </section>;
}

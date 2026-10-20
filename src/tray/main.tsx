import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { ipc } from "../ipc";
import { LocaleProvider, setActiveLocale } from "../i18n/runtime";
import { getLocaleDefinition } from "../i18n/config";
import { readLanguagePreference, readSystemLanguageTags, resolveLanguagePreference } from "../i18n/language";
import { readThemePreference, resolveThemePreference } from "../theme";
import { EMPTY_SNAPSHOT, type TrayAction, type TraySnapshot } from "./model";
import { TrayPanel } from "./TrayPanel";
import "./tray.css";

function syncAppearance(): boolean {
  const dark = resolveThemePreference(readThemePreference(), matchMedia("(prefers-color-scheme: dark)").matches) === "dark";
  document.documentElement.classList.toggle("dark", dark);
  const locale = resolveLanguagePreference(readLanguagePreference(), readSystemLanguageTags());
  setActiveLocale(locale);
  document.documentElement.lang = getLocaleDefinition(locale).htmlLang;
  return dark;
}

const initialDark = syncAppearance();

function TrayApp() {
  const [dark, setDark] = useState(initialDark);
  const [snapshot, setSnapshot] = useState<TraySnapshot>(EMPTY_SNAPSHOT);
  const [now, setNow] = useState(Date.now);
  const [error, setError] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let disposed = false;
    const listeners: (() => void)[] = [];
    const subscribe = async <T,>(event: string, handler: (value: T) => void) => {
      const stop = await ipc.events.listen<T>(event, handler);
      if (disposed) stop(); else listeners.push(stop);
    };
    const accept = (value: TraySnapshot) => {
      if (disposed) return;
      setSnapshot(value); setNow(Date.now()); setError(false);
    };
    const appearance = () => { setDark(syncAppearance()); setNow(Date.now()); };
    const read = () => ipc.tray.getSnapshot().then(accept).catch(() => { if (!disposed) setError(true); });
    void (async () => {
      await subscribe<TraySnapshot>("halv://tray/snapshot", accept);
      await subscribe("halv://tray/opened", () => { appearance(); void read(); });
      await subscribe<{ arrowX: number; below: boolean }>("halv://tray/position", value => {
        document.documentElement.style.setProperty("--arrow-x", `${value.arrowX - 12}px`);
        document.documentElement.dataset.edge = value.below ? "top" : "bottom";
      });
      await read();
    })().catch(() => { if (!disposed) setError(true); });
    const timer = window.setInterval(() => setNow(Date.now()), 15_000);
    const media = matchMedia("(prefers-color-scheme: dark)");
    media.addEventListener("change", appearance);
    window.addEventListener("storage", appearance);
    window.addEventListener("languagechange", appearance);
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.preventDefault(); void ipc.tray.action({ action: "dismiss" }); }
    };
    window.addEventListener("keydown", key);
    const panel = document.querySelector(".panel");
    let previousHeight = 0;
    const observer = new ResizeObserver(() => {
      const height = Math.ceil(panel?.getBoundingClientRect().height ?? 512) + 24;
      if (height === previousHeight) return;
      previousHeight = height;
      void ipc.tray.resize({ height }).catch(console.warn);
    });
    if (panel) observer.observe(panel);
    return () => {
      disposed = true; listeners.forEach(stop => stop()); observer.disconnect(); clearInterval(timer);
      media.removeEventListener("change", appearance);
      window.removeEventListener("storage", appearance);
      window.removeEventListener("languagechange", appearance);
      window.removeEventListener("keydown", key);
    };
  }, []);

  async function action(action: TrayAction) {
    if (busy) return;
    setBusy(true);
    try { await ipc.tray.action({ action }); } catch (error) { console.warn(error); setError(true); }
    finally { setBusy(false); }
  }
  return <TrayPanel snapshot={snapshot} now={now} dark={dark} error={error} busy={busy} onAction={action} />;
}

ReactDOM.createRoot(document.getElementById("root")!).render(<LocaleProvider><TrayApp /></LocaleProvider>);

import "./freshStateBoot";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { AppErrorBoundary } from "./components/AppErrorBoundary";
import {
  AppUpdateCard,
  AppUpdateCardBoundary,
} from "./components/AppUpdateCard";
import { CONNECT_GATE_DISMISSED_STORAGE_KEY } from "./connectGate";
import { LocaleProvider } from "./i18n/runtime";
import {
  ONBOARDING_STORAGE_KEY,
  WELCOME_SEEN_STORAGE_KEY,
  onboardingReplayEnabled,
} from "./onboarding";
import "./styles.css";

if (onboardingReplayEnabled()) {
  try {
    window.localStorage.removeItem(ONBOARDING_STORAGE_KEY);
    window.localStorage.removeItem(WELCOME_SEEN_STORAGE_KEY);
    window.localStorage.removeItem(CONNECT_GATE_DISMISSED_STORAGE_KEY);
  } catch {
    // Development replay still works in memory when storage is unavailable.
  }
}

// No StrictMode: its dev-only double mount re-ran the terminal
// auto-resume from a ghost instance whose results never rendered, and the
// real mount then reconciled those live sessions as duplicate tabs.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <LocaleProvider>
    <AppErrorBoundary>
      <App />
    </AppErrorBoundary>
    {/* Beside App, so the card also shows over the Connect gate. */}
    <AppUpdateCardBoundary>
      <AppUpdateCard />
    </AppUpdateCardBoundary>
  </LocaleProvider>,
);

import { clearHalvLocalStorage } from "./freshState";

// main.tsx must import this module first.
// The sweep must run before module stores initialize.
if (import.meta.env.VITE_HALV_FRESH_STATE === "1") {
  clearHalvLocalStorage();
}

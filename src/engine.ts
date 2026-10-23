import type { SidecarStatus } from "./types";

/**
 * Tells if a send can start now. Halv Engine "off" is not an error. Chat mode
 * starts the engine on demand at the first send.
 */
export function engineReadyForSend(sidecar: SidecarStatus | null): boolean {
  return sidecar?.state === "running" || sidecar?.state === "off";
}

/** Tells if the user turned Halv Engine off. */
export function engineOff(sidecar: SidecarStatus | null): boolean {
  return sidecar?.state === "off";
}

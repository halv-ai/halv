// Converts a Tauri drag-drop position to CSS pixels in the webview.
// Tauri types the position as physical, but the unit depends on the platform.
// On macOS, wry reports the NSView point (already logical). On Linux, GTK
// widget coordinates are also logical. Only WebView2 on Windows reports
// physical pixels, so only Windows divides by the device pixel ratio.

export type NativeDropPoint = { x: number; y: number };

export function nativeDropPositionIsPhysical(platform: string): boolean {
  return /^win/iu.test(platform);
}

export function nativeDropPointToCss(
  position: NativeDropPoint,
  platform: string,
  devicePixelRatio: number,
): NativeDropPoint {
  if (!nativeDropPositionIsPhysical(platform)) {
    return { x: position.x, y: position.y };
  }
  const ratio = Number.isFinite(devicePixelRatio) && devicePixelRatio > 0 ? devicePixelRatio : 1;
  return { x: position.x / ratio, y: position.y / ratio };
}

/** Reads the platform and the pixel ratio at event time, so a display change applies. */
export function nativeDropPoint(position: NativeDropPoint): NativeDropPoint {
  const platform = typeof navigator === "undefined" ? "" : navigator.platform;
  const ratio = typeof window === "undefined" ? 1 : window.devicePixelRatio;
  return nativeDropPointToCss(position, platform, ratio);
}

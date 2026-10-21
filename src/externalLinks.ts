import type { ILinkHandler } from "@xterm/xterm";
import { ipc } from "./ipc.ts";
import { isMacPlatform } from "./shortcuts.ts";

// Returns the parsed URL when the system browser can open the link, else null.
// The URL parser makes the scheme lower case, so "HTTPS://host" and
// "https:host" both give a normal https URL. Relative links give null.
export function externalLinkUrl(href: string | null | undefined): string | null {
  if (!href) {
    return null;
  }
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return null;
  }
  return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
}

export async function openExternalLink(href: string | null | undefined): Promise<void> {
  const url = externalLinkUrl(href);
  if (!url) {
    throw new Error("Only http:// and https:// links can be opened");
  }
  await ipc.app.openExternalUrl({ url });
}

// Sends an untrusted link to the system browser when it is http or https.
// Other links do nothing.
function openUntrustedLink(href: string | null | undefined) {
  if (!externalLinkUrl(href)) {
    return;
  }
  void openExternalLink(href).catch((error) => {
    console.error("External link open failed", error);
  });
}

// Links in markdown come from agents, so they are untrusted. Only a same-page
// "#" anchor keeps the default action. The click never navigates the Halv
// window: http and https links open in the system browser, other links do nothing.
export function handleMarkdownLinkClick(
  event: { preventDefault(): void },
  href: string | undefined,
) {
  if (href?.startsWith("#")) {
    return;
  }
  event.preventDefault();
  openUntrustedLink(href);
}

// The mouse event fields that tell a link click from other gestures.
export type TerminalLinkClick = Pick<MouseEvent, "button" | "detail" | "metaKey" | "ctrlKey">;

// Returns true for one primary-button click with Cmd on macOS or Ctrl on other
// platforms. iTerm2, Terminal.app, Ghostty and VS Code use the same rule.
// xterm activates a link on each mouseup that ends on the link where the
// mousedown started. It does not check the button, the click count or the
// selection. Without this check, a double-click to select a word opens 2 tabs,
// a drag to select the URL opens it, and a right-click or a middle-click in
// WebView2 opens it.
export function isTerminalLinkClick(event: TerminalLinkClick, platform?: string): boolean {
  if (event.button !== 0 || event.detail !== 1) {
    return false;
  }
  return isMacPlatform(platform) ? event.metaKey : event.ctrlKey;
}

// Makes the xterm linkHandler for OSC 8 hyperlinks. Programs in the terminal
// print these links, so they are untrusted. Without this handler, xterm calls
// confirm() and window.open(), and the main webview denies the new window.
// A Cmd-click on macOS or a Ctrl-click on other platforms opens http and https
// links in the system browser. Other gestures and other links do nothing.
// xterm also drops links that are not http or https, because
// allowNonHttpProtocols is false. When platform is not given, the handler reads
// navigator.platform at each click.
export function createTerminalLinkHandler(platform?: string): ILinkHandler {
  return {
    allowNonHttpProtocols: false,
    activate(event, uri) {
      activateTerminalWebLink(event, uri, platform);
    },
  };
}

// Opens a URL that the terminal shows as text. The OSC 8 handler above and
// the web links addon both use this function, so the two have the same
// rule: the click must pass isTerminalLinkClick, and only http and https
// links open in the system browser.
export function activateTerminalWebLink(
  event: TerminalLinkClick,
  uri: string,
  platform?: string,
): void {
  if (!isTerminalLinkClick(event, platform)) {
    return;
  }
  openUntrustedLink(uri);
}

export const terminalLinkHandler: ILinkHandler = createTerminalLinkHandler();

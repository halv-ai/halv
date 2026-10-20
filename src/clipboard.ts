// Clipboard helper that works in Tauri's WKWebView, where the packaged app
// loads from the tauri:// custom scheme and navigator.clipboard may be
// entirely absent (it is only exposed in secure contexts like the dev
// server's http://localhost). Falls back to the hidden-textarea
// execCommand("copy") path, which WKWebView still supports.
export async function copyText(text: string): Promise<boolean> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // NotAllowedError or scheme restriction — fall through to fallback.
    }
  }

  try {
    const textarea = document.createElement("textarea");
    textarea.value = text;
    textarea.setAttribute("readonly", "");
    textarea.style.position = "fixed";
    textarea.style.top = "-1000px";
    textarea.style.opacity = "0";
    document.body.appendChild(textarea);
    textarea.select();
    const copied = document.execCommand("copy");
    document.body.removeChild(textarea);
    return copied;
  } catch {
    return false;
  }
}

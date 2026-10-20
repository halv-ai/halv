import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import type { ILinkHandler } from "@xterm/xterm";
import React from "react";
import ReactDOMServer from "react-dom/server";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

// @ts-expect-error Node's test runner needs the source extension.
import { setIpcTransport } from "../src/ipc.ts";
// @ts-expect-error Node's test runner needs the source extension.
import {
  createTerminalLinkHandler,
  externalLinkUrl,
  handleMarkdownLinkClick,
  openExternalLink,
  terminalLinkHandler,
} from "../src/externalLinks.ts";

let opened: string[] = [];

setIpcTransport({
  invoke: async (name: string, args?: { url?: string }) => {
    if (name !== "app_open_external_url") {
      throw new Error(`unexpected command ${name}`);
    }
    opened.push(String(args?.url));
  },
  listen: async () => () => undefined,
} as never);

// Clicks a markdown link. Returns whether the click kept the webview on the
// page and which URLs went to the system browser.
function click(href: string | undefined) {
  opened = [];
  let prevented = false;
  handleMarkdownLinkClick(
    {
      preventDefault: () => {
        prevented = true;
      },
    },
    href,
  );
  return { prevented, opened };
}

// Renders markdown with the plugins of MarkdownMessage and returns the href
// that react-markdown gives to each link.
function markdownHrefs(markdown: string) {
  const hrefs: (string | undefined)[] = [];
  ReactDOMServer.renderToStaticMarkup(
    React.createElement(
      Markdown,
      {
        components: {
          a: ({ href }: { href?: string }) => {
            hrefs.push(href);
            return null;
          },
        },
        remarkPlugins: [remarkGfm],
      },
      markdown,
    ),
  );
  return hrefs;
}

test("http and https links open in the system browser in any scheme case", () => {
  for (const [href, url] of [
    ["https://example.com/docs", "https://example.com/docs"],
    ["http://example.com", "http://example.com/"],
    ["HTTPS://attacker.example/x", "https://attacker.example/x"],
    ["Https://attacker.example/x", "https://attacker.example/x"],
    ["https:attacker.example/x", "https://attacker.example/x"],
    ["https:/attacker.example/x", "https://attacker.example/x"],
    ["HTTP://127.0.0.1:47823/x", "http://127.0.0.1:47823/x"],
    ["http:127.0.0.1:47823/x", "http://127.0.0.1:47823/x"],
  ]) {
    assert.deepEqual(click(href), { prevented: true, opened: [url] }, href);
    assert.equal(externalLinkUrl(href), url, href);
  }
});

test("other links never navigate the Halv window", () => {
  for (const href of [
    undefined,
    "",
    "/relative",
    "relative/page",
    "?tab=logs",
    "//attacker.example/x",
    "https://",
    "mailto:me@example.com",
    "javascript:alert(1)",
    "data:text/html,hi",
    "file:///etc/passwd",
    "tauri://localhost/",
  ]) {
    assert.deepEqual(click(href), { prevented: true, opened: [] }, String(href));
    assert.equal(externalLinkUrl(href), null, String(href));
  }
});

test("same-page anchors keep the default scroll", () => {
  assert.deepEqual(click("#user-content-fn-1"), { prevented: false, opened: [] });
});

test("markdown link forms open in the system browser or do nothing", () => {
  for (const [markdown, urls] of [
    ["[Open the CI log](HTTPS://attacker.example/x)", ["https://attacker.example/x"]],
    ["see HTTPS://attacker.example/x now", ["https://attacker.example/x"]],
    ["see www.attacker.example now", ["http://www.attacker.example/"]],
    ["<Https://attacker.example/x>", ["https://attacker.example/x"]],
    ["[CI log][1]\n\n[1]: Https://attacker.example/x", ["https://attacker.example/x"]],
    ["[x](https:attacker.example/x)", ["https://attacker.example/x"]],
    ["[x](https:/attacker.example/x)", ["https://attacker.example/x"]],
    ["[docs](https://example.com/docs)", ["https://example.com/docs"]],
    ["[x](javascript:alert(1))", []],
    ["[x](data:text/html,hi)", []],
    ["[x](/relative)", []],
    ["[mail](mailto:me@example.com)", []],
  ] as const) {
    const hrefs = markdownHrefs(markdown);
    assert.equal(hrefs.length, 1, markdown);
    assert.deepEqual(click(hrefs[0]), { prevented: true, opened: urls }, markdown);
  }
});

test("footnote links stay on the page", () => {
  const hrefs = markdownHrefs("Claim[^1].\n\n[^1]: Source.");
  assert.equal(hrefs.length, 2);
  for (const href of hrefs) {
    assert.match(String(href), /^#user-content-fn/);
    assert.deepEqual(click(href), { prevented: false, opened: [] }, href);
  }
});

test("openExternalLink sends only http and https URLs to the system browser", async () => {
  opened = [];
  await openExternalLink("HTTPS://example.com/oauth?state=1");
  assert.deepEqual(opened, ["https://example.com/oauth?state=1"]);

  opened = [];
  await assert.rejects(openExternalLink("file:///etc/passwd"), /Only http/);
  await assert.rejects(openExternalLink("x-halv:connect"), /Only http/);
  await assert.rejects(openExternalLink(undefined), /Only http/);
  assert.deepEqual(opened, []);
});

type MouseUpFields = {
  button?: number;
  detail?: number;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
};

// Makes the mouseup event that xterm gives to activate(). The defaults are one
// primary-button click with no modifier. Node has no MouseEvent class, so the
// event is a plain object with the same fields.
function mouseUp(fields: MouseUpFields = {}) {
  return {
    type: "mouseup",
    button: 0,
    buttons: 0,
    detail: 1,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...fields,
  } as unknown as MouseEvent;
}

const macLinks = createTerminalLinkHandler("MacIntel");
const windowsLinks = createTerminalLinkHandler("Win32");
const linuxLinks = createTerminalLinkHandler("Linux x86_64");
const cmdClick = { metaKey: true };
const ctrlClick = { ctrlKey: true };

// Sends one mouseup on an OSC 8 hyperlink for each event, as xterm does for
// each mouseup that ends on the link. Returns which URLs went to the system
// browser. Node has no window, so a call to confirm() or window.open() throws
// here.
function terminalGesture(
  handler: ILinkHandler,
  events: MouseUpFields[],
  uri = "https://example.com/docs",
) {
  opened = [];
  for (const event of events) {
    handler.activate(mouseUp(event), uri, {
      start: { x: 1, y: 1 },
      end: { x: uri.length, y: 1 },
    });
  }
  return opened;
}

test("terminal OSC 8 links open in the system browser only for http and https", () => {
  for (const [uri, url] of [
    ["https://example.com/docs", "https://example.com/docs"],
    ["http://example.com", "http://example.com/"],
    ["HTTPS://example.com/x", "https://example.com/x"],
    ["https:example.com/x", "https://example.com/x"],
  ]) {
    assert.deepEqual(terminalGesture(macLinks, [cmdClick], uri), [url], uri);
  }

  for (const uri of [
    "",
    "relative/page",
    "javascript:alert(1)",
    "data:text/html,hi",
    "file:///etc/passwd",
    "mailto:me@example.com",
    "vscode://file/etc/passwd",
    "x-halv:connect",
    "tauri://localhost/",
  ]) {
    assert.deepEqual(terminalGesture(macLinks, [cmdClick], uri), [], uri);
  }
});

test("on macOS only a Cmd-click with the primary button opens a terminal link", () => {
  assert.deepEqual(terminalGesture(macLinks, [cmdClick]), ["https://example.com/docs"]);
  assert.deepEqual(
    terminalGesture(macLinks, [{ metaKey: true, shiftKey: true }]),
    ["https://example.com/docs"],
  );

  for (const event of [
    {},
    { ctrlKey: true },
    { altKey: true },
    { shiftKey: true },
    { metaKey: true, button: 1 },
    { metaKey: true, button: 2 },
    { metaKey: true, detail: 0 },
    { metaKey: true, detail: 2 },
    { metaKey: true, detail: 3 },
  ]) {
    assert.deepEqual(terminalGesture(macLinks, [event]), [], JSON.stringify(event));
  }
});

test("on Windows and Linux only a Ctrl-click with the primary button opens a terminal link", () => {
  for (const handler of [windowsLinks, linuxLinks]) {
    assert.deepEqual(terminalGesture(handler, [ctrlClick]), ["https://example.com/docs"]);

    for (const event of [
      {},
      { metaKey: true },
      { altKey: true },
      { shiftKey: true },
      { ctrlKey: true, button: 1 },
      { ctrlKey: true, button: 2 },
      { ctrlKey: true, detail: 0 },
      { ctrlKey: true, detail: 2 },
      { ctrlKey: true, detail: 3 },
    ]) {
      assert.deepEqual(terminalGesture(handler, [event]), [], JSON.stringify(event));
    }
  }
});

test("selection and context gestures on a terminal link do not open it", () => {
  for (const [handler, modifier] of [
    [macLinks, cmdClick],
    [windowsLinks, ctrlClick],
  ] as const) {
    // A double-click to select a word sends a mouseup with detail 1, then one
    // with detail 2.
    assert.deepEqual(terminalGesture(handler, [{ detail: 1 }, { detail: 2 }]), []);
    // A triple-click to select the line adds a mouseup with detail 3.
    assert.deepEqual(
      terminalGesture(handler, [{ detail: 1 }, { detail: 2 }, { detail: 3 }]),
      [],
    );
    // A drag to select part of the link sends one mouseup with detail 1.
    assert.deepEqual(terminalGesture(handler, [{ detail: 1 }]), []);
    // Chromium (WebView2) activates the link on a right-click and on a
    // middle-click.
    assert.deepEqual(terminalGesture(handler, [{ button: 2 }]), []);
    assert.deepEqual(terminalGesture(handler, [{ button: 1 }]), []);
    assert.deepEqual(terminalGesture(handler, [{ ...modifier, button: 2 }]), []);
    assert.deepEqual(terminalGesture(handler, [{ ...modifier, button: 1 }]), []);
    // A double-click with the modifier opens one tab, not 2.
    assert.deepEqual(
      terminalGesture(handler, [
        { ...modifier, detail: 1 },
        { ...modifier, detail: 2 },
      ]),
      ["https://example.com/docs"],
    );
  }
});

test("the default terminal link handler needs the platform modifier", () => {
  assert.deepEqual(terminalGesture(terminalLinkHandler, [{}]), []);
  assert.deepEqual(terminalGesture(terminalLinkHandler, [{ button: 2 }]), []);
  assert.deepEqual(terminalGesture(terminalLinkHandler, [{ detail: 2 }]), []);
  // With Cmd and Ctrl both down, the click opens the link on each platform.
  assert.deepEqual(
    terminalGesture(terminalLinkHandler, [{ metaKey: true, ctrlKey: true }]),
    ["https://example.com/docs"],
  );
});

test("xterm drops OSC 8 links that are not http or https", () => {
  assert.equal(terminalLinkHandler.allowNonHttpProtocols, false);
});

test("the terminal pane gives OSC 8 links to the terminal link handler", () => {
  const pane = readFileSync(
    new URL("../src/components/TerminalPane.tsx", import.meta.url),
    "utf8",
  );
  const start = pane.indexOf("new Terminal({");
  assert.notEqual(start, -1);
  const options = pane.slice(start, pane.indexOf("\n    });", start));
  assert.match(options, /\n\s+linkHandler: terminalLinkHandler,\n/);
});

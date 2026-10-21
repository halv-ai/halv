import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer, type ViteDevServer } from "vite";

// @ts-expect-error Node's test runner needs the source extension.
import { ANALYTICS_EVENTS } from "../src/analytics.ts";

// Node strips types but does not compile JSX. Vite loads the component the
// way the app does, and React stays the copy that this test imports.
type ConsentModule = typeof import("../src/components/AnonymousUsagePreference.tsx");

let server: ViteDevServer;
let consent: ConsentModule;

before(async () => {
  server = await createServer({
    appType: "custom",
    clearScreen: false,
    logLevel: "silent",
    optimizeDeps: { noDiscovery: true },
    server: { hmr: false, middlewareMode: true, watch: null },
  });
  consent = (await server.ssrLoadModule(
    "/src/components/AnonymousUsagePreference.tsx",
  )) as ConsentModule;
});

after(async () => {
  await server?.close();
});

const NEVER_SENT = [
  "Your prompts",
  "Your code",
  "File names",
  "Terminal output",
  "Account emails",
  "API keys",
];

const SENT_WITH_EVENTS = [
  "A random install ID",
  "The app version",
  "Your operating system",
  "The time of the event",
  "The download ID, if you came through a link on the Halv site",
];

function render(element: React.ReactElement) {
  return ReactDOMServer.renderToStaticMarkup(element);
}

// The HTML of each consent surface before the saved answer loads.
function surfaces() {
  return {
    ask: render(
      React.createElement(consent.ConsentCard, { variant: "ask" }),
    ),
    settings: render(
      React.createElement(consent.ConsentCard, { variant: "settings" }),
    ),
  };
}

test("the journey has 13 events", () => {
  assert.equal(ANALYTICS_EVENTS.length, 13);
});

test("both variants list the 13 journey events", () => {
  for (const [variant, html] of Object.entries(surfaces())) {
    for (const event of ANALYTICS_EVENTS) {
      assert.ok(
        html.includes(`>${event}</code>`),
        `${variant} lists ${event}`,
      );
    }
    assert.equal(
      (html.match(/<code /g) ?? []).length,
      ANALYTICS_EVENTS.length,
      `${variant} lists each event once`,
    );
  }
});

test("both variants list what Halv never sends", () => {
  for (const [variant, html] of Object.entries(surfaces())) {
    assert.ok(html.includes("Never sent"), `${variant} has the heading`);
    for (const item of NEVER_SENT) {
      assert.ok(html.includes(`<li>${item}</li>`), `${variant} lists ${item}`);
    }
  }
});

test("both variants list the facts that travel with each event", () => {
  for (const [variant, html] of Object.entries(surfaces())) {
    assert.ok(html.includes("Sent with every event"), variant);
    for (const item of SENT_WITH_EVENTS) {
      assert.ok(html.includes(`<li>${item}</li>`), `${variant} lists ${item}`);
    }
  }
});

test("the events appear in the four groups of the disclosure", () => {
  const { ask } = surfaces();
  for (const group of ["Setup", "First task", "Halv Cloud", "Activity"]) {
    assert.ok(ask.includes(`>${group}</p>`), group);
  }
  const setup = ask.indexOf(">Setup</p>");
  const firstTask = ask.indexOf(">First task</p>");
  for (const event of [
    "first_open",
    "account_connected",
    "project_selected",
    "setup_failed",
  ]) {
    const at = ask.indexOf(`>${event}</code>`);
    assert.ok(at > setup && at < firstTask, `${event} sits in Setup`);
  }
});

test("the ask variant has a title, a disclosure, and two equal buttons", () => {
  const { ask } = surfaces();
  assert.ok(ask.includes("Help improve Halv"));
  assert.ok(ask.includes("<details"));
  assert.ok(ask.includes(">What is sent</summary>"));
  assert.ok(ask.includes(">Share usage events</button>"));
  assert.ok(ask.includes(">No thanks</button>"));
  assert.equal((ask.match(/class="consent-choice"/g) ?? []).length, 2);
});

test("the settings variant is a checkbox that starts unchecked", () => {
  const { settings } = surfaces();
  assert.ok(settings.includes('type="checkbox"'));
  assert.ok(!settings.includes("checked"));
  assert.ok(settings.includes("Share anonymous usage events"));
});

test("SettingsPanel can hide the disclosure to render its own", () => {
  const html = render(
    React.createElement(consent.ConsentCard, {
      description: false,
      variant: "settings",
    }),
  );
  assert.ok(!html.includes("<details"));
  assert.ok(html.includes('type="checkbox"'));
});

function view(
  variant: "ask" | "settings" | "banner",
  answer: "unset" | "on" | "off" | null,
) {
  return render(
    React.createElement(consent.ConsentCardView, {
      onAnswer: () => undefined,
      onRetry: () => undefined,
      preference: {
        consent: answer,
        enabled: answer === null ? null : answer === "on",
        error: null,
        pending: false,
      },
      variant,
    }),
  );
}

test("the ask card shows a status line after the user answers", () => {
  const on = view("ask", "on");
  const off = view("ask", "off");
  assert.ok(on.includes("Usage events are on."));
  assert.ok(off.includes("Usage events are off."));
  for (const html of [on, off]) {
    assert.ok(!html.includes("Help improve Halv"));
    assert.ok(!html.includes("<button"));
  }
});

test("the ask card keeps its buttons enabled while the answer is unset", () => {
  const html = view("ask", "unset");
  assert.ok(html.includes("Help improve Halv"));
  assert.ok(!html.includes("disabled"));
  // The saved answer has not loaded yet, so the buttons wait.
  assert.ok(view("ask", null).includes("disabled"));
});

test("the banner shows one line and two buttons only while unset", () => {
  const html = view("banner", "unset");
  assert.ok(html.includes("Halv now asks before it sends usage events."));
  assert.ok(html.includes(">Share usage events</button>"));
  assert.ok(html.includes(">No thanks</button>"));
  assert.ok(html.includes("<details"));
  assert.ok(html.includes("Never sent"));
  for (const event of ANALYTICS_EVENTS) {
    assert.ok(html.includes(`>${event}</code>`), event);
  }
  for (const answer of ["on", "off", null] as const) {
    assert.equal(view("banner", answer), "", `no banner for ${answer}`);
  }
});

test("the connected banner renders nothing before the answer loads", () => {
  assert.equal(render(React.createElement(consent.ConsentAskBanner)), "");
});

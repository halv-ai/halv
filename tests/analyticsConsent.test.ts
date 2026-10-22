import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer, type ViteDevServer } from "vite";

// @ts-expect-error Node's test runner needs the source extension.
import { ANALYTICS_EVENTS, ANALYTICS_V2_EVENTS } from "../src/analytics.ts";

// Node strips types but does not compile JSX. Vite loads the component the
// way the app does, and React stays the copy that this test imports.
type ConsentModule = typeof import("../src/components/AnonymousUsagePreference.tsx");
type IpcModule = typeof import("../src/ipc.ts");

let server: ViteDevServer;
let consent: ConsentModule;
// The copy of src/ipc.ts that the consent module uses.
let consentIpc: IpcModule;

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
  consentIpc = (await server.ssrLoadModule("/src/ipc.ts")) as IpcModule;
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
  "Exact savings in usage events",
  "Crash stack traces",
  "Your IP address (the Halv server does not pass it on)",
];

const SENT_WITH_EVENTS = [
  "A random install ID",
  "The app version",
  "Your operating system",
  "The time of the event",
  "The download ID, if you came through a link on the Halv site",
  "The app language",
];

// List version 2 shows the 13 journey events and the 5 new events (ADR 0022).
const LISTED_EVENTS = [...ANALYTICS_EVENTS, ...ANALYTICS_V2_EVENTS];

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

test("both variants list the 18 events of list version 2", () => {
  assert.equal(LISTED_EVENTS.length, 18);
  for (const [variant, html] of Object.entries(surfaces())) {
    for (const event of LISTED_EVENTS) {
      assert.ok(
        html.includes(`>${event}</code>`),
        `${variant} lists ${event}`,
      );
    }
    assert.equal(
      (html.match(/<code /g) ?? []).length,
      LISTED_EVENTS.length,
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

test("the events appear in the six groups of the disclosure", () => {
  const { ask } = surfaces();
  const groups = [
    "Setup",
    "First task",
    "Halv Cloud",
    "Activity",
    "Daily usage summary",
    "App health",
  ];
  for (const group of groups) {
    assert.ok(ask.includes(`>${group}</p>`), group);
  }
  const summary = ask.indexOf(">Daily usage summary</p>");
  const health = ask.indexOf(">App health</p>");
  const at = (event: string) => ask.indexOf(`>${event}</code>`);
  assert.ok(at("usage_summarized") > summary && at("usage_summarized") < health);
  for (const event of [
    "crash_detected",
    "proxy_failed",
    "app_updated",
    "update_failed",
  ]) {
    assert.ok(at(event) > health, `${event} sits in App health`);
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

test("each version 2 event and fact has plain words", () => {
  const { ask } = surfaces();
  const facts = [
    // account_connected and trial_prompted
    "The agent and how you signed in (app, API key, or CLI)",
    "Where it showed",
    // usage_summarized
    "The agents you used",
    "The number of agent sessions, as a range",
    "Tokens and dollars saved, rounded to 2 significant digits",
    "Whether crux and task routing were used",
    "Your Halv Cloud plan",
    // crash_detected
    "The version of that run",
    "How long it ran and its memory use, as ranges",
    // proxy_failed
    "What failed (start, stop, or too many restarts)",
    "Whether an automatic restart fixed it",
    // app_updated and update_failed
    "The version before the update",
    "The step that failed (download, signature check, or install)",
  ];
  for (const fact of facts) {
    assert.ok(ask.includes(`<li>${fact}</li>`), fact);
  }
});

function noticeView(
  answer: "unset" | "on" | "off" | null,
  asked: 0 | 1 | 2 | null,
) {
  return render(
    React.createElement(consent.ConsentCardView, {
      onAnswer: () => undefined,
      onListVersion: () => undefined,
      onRetry: () => undefined,
      preference: {
        consent: answer,
        enabled: answer === null ? null : answer === "on",
        version: answer === "on" ? 1 : null,
        asked,
        error: null,
        pending: false,
      },
      variant: "notice",
    }),
  );
}

test("the version 2 notice asks a version 1 yes with two equal buttons", () => {
  const html = noticeView("on", 1);
  assert.ok(html.includes("Halv now also sends a daily usage summary"));
  assert.ok(html.includes(">Share the summary too</button>"));
  assert.ok(html.includes(">Keep the current list</button>"));
  assert.equal((html.match(/class="consent-choice"/g) ?? []).length, 2);
  // It is a banner in the page, not a dialog.
  assert.ok(html.startsWith("<aside"));
  assert.ok(!html.includes('role="dialog"'));
  assert.ok(!html.includes("aria-modal"));
  assert.ok(html.includes(">usage_summarized</code>"));
  for (const [answer, asked] of [
    ["on", 2],
    ["off", 1],
    ["unset", 0],
    [null, null],
    ["on", null],
  ] as const) {
    assert.equal(noticeView(answer, asked), "", `no notice for ${answer}/${asked}`);
  }
});

test("the notice answers keep yes and save the list version", async () => {
  const calls: unknown[] = [];
  consentIpc.setIpcTransport({
    invoke: async (name: string, args: unknown) => {
      calls.push([name, args]);
      const version =
        (args as { consentVersion?: number } | undefined)?.consentVersion ?? 2;
      return {
        analyticsConsent: (args as { consent?: string } | undefined)?.consent ?? "on",
        analyticsConsentVersion: version,
        analyticsConsentAsked: 2,
      } as never;
    },
    listen: async () => () => undefined,
  });
  try {
    await consent.answerConsentNotice(2);
    await consent.answerConsentNotice(1);
    await consent.answerConsent("on");
    await consent.answerConsent("off");
    assert.deepEqual(calls, [
      ["preferences_set_analytics_consent", { consent: "on", consentVersion: 2 }],
      ["preferences_set_analytics_consent", { consent: "on", consentVersion: 1 }],
      ["preferences_set_analytics_consent", { consent: "on" }],
      ["preferences_set_analytics_consent", { consent: "off" }],
    ]);
  } finally {
    consentIpc.setIpcTransport(consentIpc.tauriTransport);
  }
});

test("the connected notice renders nothing without a version 1 yes", () => {
  // The answers above left "off" in the shared store.
  assert.equal(render(React.createElement(consent.ConsentNoticeBanner)), "");
});

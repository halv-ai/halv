import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

// @ts-expect-error Node's test runner needs the source extension.
import {
  INITIAL_NAVIGATION,
  legacyNavigation,
  looseNavigation,
  NAVIGATION_STORAGE_KEY,
  navigationConversationId,
  navigationTabKey,
  readStoredNavigation,
  sameNavigation,
  workspaceNavigation,
  writeStoredNavigation,
} from "../src/appNavigation.ts";
// @ts-expect-error Node's test runner needs the source extension.
import { EMPTY_TABS_STATE, openTab, tabFacts } from "../src/workspaceTabs.ts";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "src");
const app = readFileSync(path.join(root, "App.tsx"), "utf8");

/** The text of the first call `name(...)` after `from`, with its arguments. */
function callText(source: string, name: string, from: number) {
  const start = source.indexOf(`${name}(`, from);
  if (start < 0) return null;
  let depth = 0;
  for (let index = start + name.length; index < source.length; index += 1) {
    if (source[index] === "(") depth += 1;
    if (source[index] === ")") {
      depth -= 1;
      if (depth === 0) return { start, text: source.slice(start, index + 1) };
    }
  }
  return null;
}

/** The body of `function name(` in App.tsx, up to its closing brace. */
function functionBody(name: string) {
  const start = app.search(new RegExp(`\\n  (async )?function ${name}\\(`));
  assert.ok(start >= 0, `App.tsx defines ${name}`);
  const end = app.indexOf("\n  }\n", start);
  return app.slice(start, end);
}

test("legacy navigation keeps today's single current conversation", () => {
  assert.deepEqual(INITIAL_NAVIGATION, { kind: "legacy", conversationId: null });
  assert.equal(navigationConversationId(INITIAL_NAVIGATION), null, "the app starts on the new-task composer");
  const opened = legacyNavigation("c1");
  assert.deepEqual(opened, { kind: "legacy", conversationId: "c1" });
  assert.equal(navigationConversationId(opened), "c1");
  assert.ok(sameNavigation(opened, legacyNavigation("c1")));
  assert.ok(!sameNavigation(opened, legacyNavigation("c2")));
  assert.ok(!sameNavigation(opened, INITIAL_NAVIGATION));
});

test("one writer: no setCurrentConversationId anywhere, and navigate() is the only navigation setter", () => {
  const offenders: string[] = [];
  const walk = (directory: string) => {
    for (const name of readdirSync(directory)) {
      const file = path.join(directory, name);
      if (statSync(file).isDirectory()) walk(file);
      else if (/\.tsx?$/.test(name) && readFileSync(file, "utf8").includes("setCurrentConversationId")) {
        offenders.push(path.relative(root, file));
      }
    }
  };
  walk(root);
  assert.deepEqual(offenders, []);
  assert.equal(app.match(/setNavigation\(/g)?.length, 1);
  assert.match(functionBody("navigate"), /setNavigation\(/);
  // The ref follows navigate() at once and the state on every render. No
  // other code assigns it.
  const refWrites = [...app.matchAll(/currentConversationIdRef\.current = /g)].map((match) => match.index ?? 0);
  assert.equal(refWrites.length, 2);
  assert.match(
    functionBody("navigate"),
    /currentConversationIdRef\.current = navigationConversationId\(\s*next,\s*workspaceTabsStore\.getSnapshot\(\),?\s*\)/,
  );
  assert.match(app, /useEffect\(\(\) => \{\n    currentConversationIdRef\.current = currentConversationId;\n  \}, \[currentConversationId\]\);/);
});

test("loadMessages only loads: openConversation alone starts a navigation and shows the conversation", () => {
  const definition = app.indexOf("async function loadMessages(");
  assert.ok(definition >= 0);
  const opener = functionBody("openConversation");
  let cursor = 0;
  let calls = 0;
  for (;;) {
    const call = callText(app, "loadMessages", cursor);
    if (!call) break;
    cursor = call.start + 1;
    if (call.start === definition + "async function ".length) continue;
    calls += 1;
    if (opener.includes(call.text)) {
      assert.match(call.text, /onApplied: show/);
      continue;
    }
    assert.match(call.text, /navigationGeneration/, `a refresh keeps its navigation: ${call.text.slice(0, 80)}`);
    assert.doesNotMatch(call.text, /onApplied/, "only openConversation shows a conversation");
  }
  assert.ok(calls >= 7, `found ${calls} loadMessages calls`);
  assert.doesNotMatch(functionBody("loadMessages"), /currentConversationIdRef\.current =|navigate\(/);
});

test("every user entry point opens a conversation through openConversation", () => {
  const anchors = [
    "focusNotifiedTerminalRef.current = (sessionId) =>",
    "function cyclePinned(event: KeyboardEvent)",
    "function navigateRelativeTask(",
    "async function handleArchiveCurrentConversation(",
    "async function forkConversationAt(",
    "async function handleOpenAutomationConversation(",
    "onLoadConversation={(conversationId) =>",
    "onFocusTerminal={(sessionId) =>",
    "const conversationIdToLoad = conversationIdToRestore(",
  ];
  for (const anchor of anchors) {
    const start = app.indexOf(anchor);
    assert.ok(start >= 0, `anchor ${anchor}`);
    const slice = app.slice(start, start + 1800);
    assert.match(slice, /openConversation\(/, `${anchor} uses openConversation`);
    const firstLoad = slice.indexOf("loadMessages(");
    const firstOpen = slice.indexOf("openConversation(");
    assert.ok(firstLoad < 0 || firstLoad > firstOpen, `${anchor} does not call loadMessages first`);
  }
  // Creating a conversation shows it through the same seam.
  for (const name of ["createConversationDraft", "handleSetConversationGoal"]) {
    assert.match(functionBody(name), /openConversation\([^)]*load: false/);
  }
  assert.match(functionBody("removeDraftConversation"), /startComposing\(\)/);
  assert.match(functionBody("startNewConversationForProject"), /startComposing\(projectId\)/);
});

function factsFor() {
  const summary = (id: string, workspaceId: string | null, projectId: number | null = 1) => ({
    id,
    projectId,
    workspaceId,
    mode: "agent",
    archivedAt: null,
    createdAt: "2026-10-01T00:00:00Z",
    updatedAt: "2026-10-01T00:00:00Z",
  });
  return tabFacts({
    conversations: [summary("a", null), summary("b", "wt-1"), summary("loose", null, null)] as never,
    workspaces: [
      { id: "primary-1", projectId: 1, kind: "primary" },
      { id: "wt-1", projectId: 1, kind: "worktree" },
    ],
  });
}

test("a workspace shows the active tab of its strip; composing shows the draft", () => {
  const facts = factsFor();
  const opened = openTab(EMPTY_TABS_STATE, facts, { key: "conversation:a", now: 10 }).state;
  const view = { tabs: opened, facts };
  const nav = workspaceNavigation("primary-1");
  assert.equal(navigationConversationId(nav, view), "a");
  assert.equal(navigationTabKey(nav, view), "conversation:a");
  // Without the tab view, a workspace shows no conversation yet.
  assert.equal(navigationConversationId(nav), null);
  const composing = workspaceNavigation("primary-1", true);
  assert.equal(navigationConversationId(composing, view), null);
  assert.equal(navigationTabKey(composing, view), null);
  // The first typed character names the new row before it exists.
  const drafting = workspaceNavigation("primary-1", true, "new-row");
  assert.equal(navigationConversationId(drafting, view), "new-row");
  assert.ok(!sameNavigation(composing, drafting));
  assert.ok(sameNavigation(drafting, workspaceNavigation("primary-1", true, "new-row")));
  // Another strip has its own active tab.
  assert.equal(navigationConversationId(workspaceNavigation("wt-1"), view), null);
  // The loose view has no strip.
  const loose = looseNavigation("loose");
  assert.equal(navigationConversationId(loose, view), "loose");
  assert.equal(navigationTabKey(loose, view), null);
  assert.ok(!sameNavigation(loose, legacyNavigation("loose")));
});

test("only the workspace kind is stored, and a broken value reads as nothing", () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  };
  assert.equal(readStoredNavigation(storage), null);
  assert.equal(writeStoredNavigation(looseNavigation("x"), storage), false);
  assert.equal(writeStoredNavigation(legacyNavigation("x"), storage), false);
  assert.equal(writeStoredNavigation(workspaceNavigation("wt-1", true, "draft"), storage), true);
  // A restart shows the strip of the workspace, not the draft.
  assert.deepEqual(readStoredNavigation(storage), workspaceNavigation("wt-1"));
  values.set(NAVIGATION_STORAGE_KEY, "{broken");
  assert.equal(readStoredNavigation(storage), null);
  values.set(NAVIGATION_STORAGE_KEY, JSON.stringify({ version: 2, workspaceId: "wt-1" }));
  assert.equal(readStoredNavigation(storage), null);
  const throwing = {
    getItem: () => {
      throw new Error("blocked");
    },
    setItem: () => {
      throw new Error("blocked");
    },
  };
  assert.equal(readStoredNavigation(throwing), null);
  assert.equal(writeStoredNavigation(workspaceNavigation("wt-1"), throwing), false);
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const appSource = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const sidebarSource = readFileSync(
  new URL("../src/components/ThreadSidebar.tsx", import.meta.url),
  "utf8",
);
const workspaceSource = readFileSync(
  new URL("../src/components/WorkspaceSwitcher.tsx", import.meta.url),
  "utf8",
);
const paletteSource = readFileSync(
  new URL("../src/components/CommandPaletteDialog.tsx", import.meta.url),
  "utf8",
);

test("narrow shells collapse into a labelled, dismissible history drawer", () => {
  assert.match(appSource, /matchMedia\("\(max-width: 767px\)"\)/);
  assert.match(appSource, /"Close conversation history"/);
  assert.match(appSource, /"Open conversation history"/);
  assert.match(
    sidebarSource,
    /aria-label=\{t\("Conversation navigation"\)\}/,
  );
  assert.match(sidebarSource, /md:relative md:w-80/);
});

test("conversation actions stay available through one accessible menu", () => {
  assert.match(sidebarSource, /aria-haspopup="menu"/);
  assert.match(sidebarSource, /role="menu"/);
  assert.match(sidebarSource, /Export Markdown/);
  assert.match(sidebarSource, /Export JSON/);
  assert.match(sidebarSource, /Restore conversation/);
  assert.match(sidebarSource, /Confirm delete/);
});

test("sidebar search and empty history expose useful status", () => {
  assert.match(sidebarSource, /aria-label=\{t\("Conversations"\)\}/);
  assert.match(sidebarSource, /role="status"/);
  assert.match(sidebarSource, /role="alert"/);
  assert.match(sidebarSource, /Nothing here yet\./);
  assert.match(sidebarSource, /Nothing active — restore one from Archived\./);
  assert.doesNotMatch(sidebarSource, /Start a chat or type \/goal/);
});

test("workspace switching is responsive and keeps advanced creation optional", () => {
  assert.match(workspaceSource, /aria-haspopup="dialog"/);
  assert.match(workspaceSource, /role="dialog"/);
  assert.match(workspaceSource, /calc\(100vw-1rem\)/);
  assert.match(workspaceSource, /Creation options/);
  assert.match(workspaceSource, /aria-controls="halv-worktree-creation-options"/);
  assert.match(workspaceSource, /Close workspace switcher/);
});

test("command palette exposes and contains keyboard focus", () => {
  assert.match(paletteSource, /aria-activedescendant=\{activeOptionId\}/);
  assert.match(paletteSource, /selectableIndexes/);
  assert.match(paletteSource, /aria-labelledby="halv-command-palette-title"/);
  assert.match(paletteSource, /Close command palette/);
  assert.match(paletteSource, /event\.key !== "Tab"/);
  assert.match(paletteSource, /role="status"/);
});

test("proxy failures lead with recovery copy and preserve the exact detail", () => {
  assert.match(
    appSource,
    /Halv couldn't start its local proxy\. Restart it to try again\./,
  );
  assert.match(appSource, /Technical details/);
  assert.match(appSource, /\{status\.sidecar\.message\}/);
});

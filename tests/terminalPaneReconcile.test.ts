import assert from "node:assert/strict";
import test from "node:test";

import {
  panelListsSession,
  type PanelSessionFact,
  type TerminalPanelBinding,
} from "../src/terminalPaneReconcile.ts";

// The panel filter of shell tabs (ADR 0010, P1 to P3). Port of the six
// checks of the delivery-first prototype `sessionOwnership.prototype.ts`.
// One rule differs from the prototype: when corrupt data sets both owners,
// the conversation wins (spec P1), not the shell tab.

const shell = (
  id: string,
  shellTabId: string | null = null,
  conversationId: string | null = null,
): PanelSessionFact => ({ id, conversationId, shellTabId, agentKind: null });

/** Today's rule before shell tabs, for comparison. */
function shippedListsSession(panel: TerminalPanelBinding, session: PanelSessionFact, inLayout = false) {
  if (panel.kind === "conversation") return session.conversationId === panel.conversationId;
  if (panel.kind === "agent") return session.id === panel.agentSessionId || inLayout;
  return !session.conversationId && (!session.agentKind || inLayout);
}

test("the shipped rule: two shell tab panels built as classic panels both take every plain shell", () => {
  const panels: TerminalPanelBinding[] = [{ kind: "classic" }, { kind: "classic" }];
  assert.equal(panels.filter((panel) => shippedListsSession(panel, shell("s1"))).length, 2);
});

test("every session shows in exactly one panel kind", () => {
  const panels: TerminalPanelBinding[] = [
    { kind: "shellTab", shellTabId: "tab-1" },
    { kind: "shellTab", shellTabId: "tab-2" },
    { kind: "conversation", conversationId: "c1" },
    { kind: "classic" },
  ];
  const sessions: PanelSessionFact[] = [
    shell("in-tab-1", "tab-1"),
    shell("in-tab-2", "tab-2"),
    shell("split-in-c1", null, "c1"),
    { id: "agent-c1", conversationId: "c1", shellTabId: null, agentKind: "claude" },
    shell("bottom-panel"),
  ];
  for (const session of sessions) {
    const owners = panels.filter((panel) => panelListsSession(panel, session));
    assert.equal(owners.length, 1, `${session.id} shows in one panel`);
  }
});

test("an owned shell never shows in a panel without an owner or in an agent panel", () => {
  const owned = shell("s", "tab-9");
  assert.equal(panelListsSession({ kind: "classic" }, owned), false);
  assert.equal(
    panelListsSession({ kind: "classic" }, owned, { inLayout: true }),
    false,
    "also not through a stale saved pane",
  );
  assert.equal(
    panelListsSession({ kind: "agent", agentSessionId: "agent-1" }, owned, { inLayout: true }),
    false,
  );
  // Corrupt data with both owners: the conversation wins (P1).
  const both = shell("s2", "tab-9", "c1");
  assert.equal(panelListsSession({ kind: "conversation", conversationId: "c1" }, both), true);
  assert.equal(panelListsSession({ kind: "shellTab", shellTabId: "tab-9" }, both), false);
});

test("a split in a shell tab copies the owner, so it stays in that tab", () => {
  const source = shell("s1", "tab-1");
  const split = { ...shell("s2"), shellTabId: source.shellTabId };
  assert.equal(panelListsSession({ kind: "shellTab", shellTabId: "tab-1" }, split), true);
  assert.equal(panelListsSession({ kind: "shellTab", shellTabId: "tab-2" }, split), false);
});

test("the replaced-source filter of pane-lifecycle still runs first", () => {
  const replaced = new Set(["old"]);
  assert.equal(
    panelListsSession({ kind: "shellTab", shellTabId: "tab-1" }, shell("old", "tab-1"), { replaced }),
    false,
  );
  assert.equal(
    panelListsSession({ kind: "conversation", conversationId: "c1" }, shell("old", null, "c1"), {
      replaced,
    }),
    false,
  );
});

test("a 0.7.1 build drops the owner field, and the shell shows in its classic panel", () => {
  const owned = shell("s", "tab-1");
  const asOldBuildSeesIt = { ...owned, shellTabId: null };
  assert.equal(panelListsSession({ kind: "classic" }, asOldBuildSeesIt), true, "visible, not lost");
  // The classic and agent rules stay as shipped for sessions without an owner.
  const agent: PanelSessionFact = { id: "a", conversationId: null, agentKind: "codex" };
  assert.equal(panelListsSession({ kind: "classic" }, agent), false);
  assert.equal(panelListsSession({ kind: "classic" }, agent, { inLayout: true }), true);
  assert.equal(panelListsSession({ kind: "agent", agentSessionId: "a" }, agent), true);
});

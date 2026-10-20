import assert from "node:assert/strict";
import test from "node:test";
import {
  agentGlyphTitle,
  agentPaneGlyph,
  agentPaneLabel,
  agentPaneStateGlyph,
  agentPaneNotificationText,
  nextHeuristicPane,
  resolveAgentPaneView,
  startHeuristicPane,
  strongestAgentPaneView,
  unreportedAgentPaneView,
  viewHeuristicPane,
  type AgentPaneGlyph,
  type AgentPaneView,
  type AgentStatusEntry,
} from "../src/agentPaneStatus.ts";

const identity = (source: string, values?: Record<string, string | number>) =>
  source.replace(/\{(\w+)\}/g, (_, name: string) => String(values?.[name] ?? `{${name}}`));

function view(overrides: Partial<AgentPaneView> = {}): AgentPaneView {
  return {
    status: "idle",
    reason: null,
    unread: false,
    waitSeq: 0,
    turnSeq: 0,
    prompt: null,
    source: "presence",
    updatedAt: 0,
    ...overrides,
  };
}

test("the heuristic counts only a spell with sustained output as a turn", () => {
  let pane = startHeuristicPane(0);
  pane = nextHeuristicPane(pane, "running", 1, false);
  assert.equal(pane.status, "working");
  pane = nextHeuristicPane(pane, "waiting_input", 2, false);
  assert.equal(pane.status, "idle");
  assert.equal(pane.turnSeq, 0);

  pane = nextHeuristicPane(pane, "running", 3, false);
  pane = nextHeuristicPane(pane, "responding", 4, false);
  pane = nextHeuristicPane(pane, "running", 5, false);
  pane = nextHeuristicPane(pane, "waiting_input", 6, false);
  assert.equal(pane.status, "done");
  assert.equal(pane.turnSeq, 1);
  assert.equal(resolveAgentPaneView(undefined, pane)?.unread, true);

  // A redraw after a finished turn returns to the finished state.
  pane = nextHeuristicPane(pane, "running", 7, false);
  pane = nextHeuristicPane(pane, null, 8, false);
  assert.equal(pane.status, "done");
  pane = viewHeuristicPane(pane);
  assert.equal(resolveAgentPaneView(undefined, pane)?.unread, false);
  assert.equal(viewHeuristicPane(pane), pane);
});

test("a heuristic turn that settles in the watched pane is read", () => {
  let pane = startHeuristicPane(0);
  pane = nextHeuristicPane(pane, "responding", 1, true);
  pane = nextHeuristicPane(pane, "waiting_input", 2, true);
  assert.equal(pane.status, "done");
  assert.equal(resolveAgentPaneView(undefined, pane)?.unread, false);
});

test("an agent entry with a source wins over the heuristic", () => {
  const heuristic = nextHeuristicPane(startHeuristicPane(0), "running", 1, false);
  const entry: AgentStatusEntry = {
    sessionId: "pane",
    status: "needsApproval",
    reason: "permission",
    updatedAt: 9,
    unread: false,
    waitSeq: 2,
    turnSeq: 3,
    source: "presence",
    prompt: { kind: "approval", toolName: "Bash", command: "npm test" },
  };
  const resolved = resolveAgentPaneView(entry, heuristic);
  assert.equal(resolved?.status, "needsApproval");
  assert.equal(resolved?.reason, "permission");
  assert.deepEqual(resolved?.prompt, entry.prompt);
  assert.equal(resolveAgentPaneView({ ...entry, source: "none" }, heuristic)?.status, "working");
  assert.equal(resolveAgentPaneView({ ...entry, status: "exited" }, heuristic)?.source, "heuristic");
  assert.equal(resolveAgentPaneView(undefined, undefined), null);
});

test("a title entry from Rust drives an old pane instead of the dashed ring", () => {
  // An old Claude pane without hooks: Rust reads its terminal title.
  const settled = nextHeuristicPane(
    nextHeuristicPane(startHeuristicPane(0), "running", 1, false),
    "waiting_input",
    2,
    false,
  );
  const entry = (overrides: Partial<AgentStatusEntry>): AgentStatusEntry => ({
    sessionId: "old",
    status: "idle",
    updatedAt: 9,
    unread: false,
    waitSeq: 0,
    turnSeq: 0,
    source: "title",
    ...overrides,
  });

  const working = resolveAgentPaneView(entry({ status: "working", turnSeq: 1 }), settled);
  assert.equal(working?.source, "title");
  assert.equal(agentPaneStateGlyph(working), "working");

  const asking = resolveAgentPaneView(
    entry({ status: "needsApproval", reason: "permission", turnSeq: 1, waitSeq: 1 }),
    settled,
  );
  assert.equal(agentPaneStateGlyph(asking), "attention");

  const finished = resolveAgentPaneView(entry({ status: "done", turnSeq: 1, unread: true }), settled);
  assert.equal(agentPaneStateGlyph(finished), "done");
  assert.equal(agentPaneGlyph(finished), "done");

  // A resting title is a signal: a quiet idle dot, not "not reporting".
  const resting = resolveAgentPaneView(entry({ status: "idle" }), settled);
  assert.equal(resting?.source, "title");
  assert.equal(agentPaneStateGlyph(resting), "idle");

  // Only a live pane without any signal keeps the dashed ring.
  assert.equal(agentPaneStateGlyph(resolveAgentPaneView(entry({ source: "none" }), settled)), "unverifiable");
  assert.equal(agentPaneStateGlyph(resolveAgentPaneView(undefined, settled)), "unverifiable");
  assert.equal(agentPaneStateGlyph(unreportedAgentPaneView(3)), "unverifiable");
});

test("only news earns a glyph", () => {
  assert.equal(agentPaneGlyph(view({ status: "needsInput" })), "attention");
  assert.equal(agentPaneGlyph(view({ status: "needsApproval" })), "attention");
  assert.equal(agentPaneGlyph(view({ status: "working" })), "working");
  assert.equal(agentPaneGlyph(view({ status: "done", unread: true })), "done");
  assert.equal(agentPaneGlyph(view({ status: "failed", unread: true })), "failed");
  for (const status of ["done", "failed", "idle", "interrupted", "exited"] as const) {
    assert.equal(agentPaneGlyph(view({ status })), null, status);
  }
});

test("every live pane has a state glyph, with Orca's vocabulary", () => {
  assert.equal(agentPaneStateGlyph(view({ status: "needsApproval" })), "attention");
  assert.equal(agentPaneStateGlyph(view({ status: "working" })), "working");
  assert.equal(agentPaneStateGlyph(view({ status: "done" })), "done", "a read turn keeps its check");
  assert.equal(agentPaneStateGlyph(view({ status: "failed" })), "failed");
  assert.equal(agentPaneStateGlyph(view({ status: "interrupted" })), "interrupted");
  assert.equal(agentPaneStateGlyph(view({ status: "idle", source: "presence" })), "idle");
  assert.equal(agentPaneStateGlyph(view({ status: "idle", source: "heuristic" })), "unverifiable");
  assert.equal(agentPaneStateGlyph(unreportedAgentPaneView(7)), "unverifiable");
  assert.equal(unreportedAgentPaneView(7).updatedAt, 7);
  assert.equal(agentPaneStateGlyph(view({ status: "exited" })), null);
  assert.equal(agentPaneStateGlyph(null), null);

  // The tree ranks by the state glyph: a read finished pane still counts.
  const read = view({ status: "done", updatedAt: 4 });
  const idle = view({ status: "idle", source: "presence", updatedAt: 9 });
  assert.equal(strongestAgentPaneView([idle, read], agentPaneStateGlyph), read);
  assert.equal(strongestAgentPaneView([idle, read]), null, "the news ranking ignores both");

  const glyphs: AgentPaneGlyph[] = ["attention", "failed", "working", "interrupted", "done", "unverifiable", "idle"];
  assert.deepEqual(
    glyphs.map((glyph) => agentGlyphTitle(glyph, identity)),
    ["Needs input", "Failed", "Working", "Stopped", "Done", "Not reporting status", "Idle"],
  );
});

test("a waiting pane outranks work, failure, and unread results", () => {
  const working = view({ status: "working", updatedAt: 5 });
  const done = view({ status: "done", unread: true, updatedAt: 9 });
  const failed = view({ status: "failed", unread: true, updatedAt: 1 });
  const question = view({ status: "needsInput", updatedAt: 2 });
  const approval = view({ status: "needsApproval", updatedAt: 3 });
  assert.equal(strongestAgentPaneView([working, done, question]), question);
  assert.equal(strongestAgentPaneView([question, approval]), approval);
  assert.equal(strongestAgentPaneView([working, failed, done]), failed);
  assert.equal(strongestAgentPaneView([done, working]), working);
  assert.equal(strongestAgentPaneView([view(), null, undefined]), null);
});

test("header labels name every state", () => {
  const labels = (["working", "needsApproval", "needsInput", "done", "failed", "interrupted", "exited", "idle"] as const)
    .map((status) => agentPaneLabel({ status }, identity));
  assert.deepEqual(labels, [
    "working",
    "needs approval",
    "needs input",
    "done",
    "failed",
    "stopped",
    "exited",
    "idle",
  ]);
});

test("notification text uses the prompt detail when the agent gave one", () => {
  const context = { agent: "Claude", title: "Auth refactor" };
  assert.deepEqual(
    agentPaneNotificationText(
      view({
        status: "needsApproval",
        prompt: { kind: "approval", toolName: "Bash", command: "pnpm test" },
      }),
      context,
      identity,
    ),
    {
      title: "Claude needs your approval",
      body: "Auth refactor · Claude wants to run: pnpm test",
    },
  );
  assert.deepEqual(
    agentPaneNotificationText(
      view({
        status: "needsInput",
        reason: "question",
        prompt: { kind: "question", text: "Which store?", options: ["Redis"] },
      }),
      context,
      identity,
    ),
    { title: "Claude has a question", body: "Auth refactor · Which store?" },
  );
  assert.deepEqual(
    agentPaneNotificationText(view({ status: "needsInput", reason: "dialog" }), context, identity),
    { title: "Claude needs your input", body: "Auth refactor" },
  );
  assert.deepEqual(
    agentPaneNotificationText(
      view({
        status: "needsApproval",
        prompt: { kind: "approval", toolName: "Edit", filePath: "src/a.ts" },
      }),
      context,
      identity,
    )?.body,
    "Auth refactor · Claude wants to edit src/a.ts",
  );
  assert.equal(
    agentPaneNotificationText(view({ status: "done", unread: true }), context, identity)?.title,
    "Claude finished",
  );
  assert.equal(
    agentPaneNotificationText(view({ status: "failed", unread: true }), context, identity)?.title,
    "Claude failed",
  );
  assert.equal(agentPaneNotificationText(view({ status: "working" }), context, identity), null);
});

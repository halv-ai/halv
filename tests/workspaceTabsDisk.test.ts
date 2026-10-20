import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// The disk-model fidelity check. It runs the migration file and the exact
// SQL statements of `storage::workspace_tabs` in real SQLite, and compares
// them with the TS disk model on random batches: the same final state and
// the same failures.

// @ts-expect-error Node's test runner needs the source extension.
import {
  applyBatchToDisk,
  cascadeConversationDelete,
  cascadeWorkspaceDelete,
  EMPTY_TABS_STATE,
  garbageTabKeys,
  type ConversationFact,
  type TabFacts,
  type TabKey,
  type TabRow,
  type TabsOp,
  type TabsState,
  type WorkspaceFact,
} from "../src/workspaceTabs.ts";

// Node 22.13 and later ship node:sqlite without a flag. An older Node skips
// these checks; the Rust tests still cover the SQL side.
const sqlite = await import("node:sqlite").catch(() => null);
const skip = sqlite ? false : "node:sqlite is not available";
type DatabaseSync = import("node:sqlite").DatabaseSync;

const SCHEMA = readFileSync(new URL("../src-tauri/src/storage/workspace_tabs.sql", import.meta.url), "utf8");
const RUST = readFileSync(new URL("../src-tauri/src/storage/workspace_tabs.rs", import.meta.url), "utf8");

/** The SQL constants of the Rust module, read from its source. */
const SQL = Object.fromEntries(
  [...RUST.matchAll(/const (\w+_SQL): &str =\s*"([^"]*)";/g)].map((match) => [match[1], match[2]]),
);

// A subset of Halv's schema: the columns and foreign keys that the tab
// tables and the GC read (storage/mod.rs).
const HALV_SUBSET = `
PRAGMA foreign_keys = ON;
CREATE TABLE projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL, root_path TEXT NOT NULL UNIQUE);
CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('primary', 'worktree'))
);
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    archived_at INTEGER,
    project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL,
    workspace_id TEXT REFERENCES workspaces(id) ON DELETE SET NULL,
    mode TEXT NOT NULL DEFAULT 'chat'
);
CREATE TABLE migrations (version TEXT PRIMARY KEY, applied_at TEXT NOT NULL);
`;

/** `validate_tab_key` of the Rust module. */
function rustTabKeyIsValid(key: string) {
  if (key.startsWith("conversation:")) return true;
  const id = key.startsWith("shell:") ? key.slice("shell:".length) : null;
  if (id === null) return false;
  const groups = id.split("-");
  return (
    groups.length === 5 &&
    groups.every((group, index) => group.length === [8, 4, 4, 4, 12][index] && /^[0-9a-fA-F]+$/.test(group))
  );
}

/** `Storage::workspace_tabs_apply`: one transaction per batch. */
function sqlApplyBatch(db: DatabaseSync, ops: readonly TabsOp[]): { error?: string } {
  db.exec("BEGIN IMMEDIATE");
  try {
    for (const op of ops) {
      switch (op.type) {
        case "insertTab": {
          const row = op.row;
          if (!rustTabKeyIsValid(row.key)) throw new Error(`\`${row.key}\` is not a tab key`);
          db.prepare(SQL.INSERT_TAB_SQL).run(
            row.key,
            row.conversationId,
            row.shellWorkspaceId,
            row.customTitle,
            row.defaultTitle,
            row.position,
            row.createdAt,
            row.activatedAt,
          );
          break;
        }
        case "deleteTab":
          db.prepare(SQL.DELETE_TAB_SQL).run(op.key);
          break;
        case "touchTab":
          db.prepare(SQL.TOUCH_TAB_SQL).run(op.activatedAt, op.key);
          break;
        case "setPositions":
          for (const [key, position] of op.positions) db.prepare(SQL.SET_POSITION_SQL).run(position, key);
          break;
        case "setTitle":
          db.prepare(SQL.SET_TITLE_SQL).run(op.title, op.key);
          break;
        case "setActive":
          db.prepare(SQL.SET_ACTIVE_SQL).run(op.workspaceId, op.key, 1);
          break;
      }
    }
    db.exec("COMMIT");
    return {};
  } catch (error) {
    db.exec("ROLLBACK");
    return { error: (error as Error).message };
  }
}

function sqlState(db: DatabaseSync): TabsState {
  const rows = db.prepare("SELECT * FROM workspace_tabs").all() as Array<Record<string, unknown>>;
  const states = db.prepare("SELECT workspace_id, active_tab_key FROM workspace_tab_state").all() as Array<{
    workspace_id: string;
    active_tab_key: string | null;
  }>;
  return {
    rows: Object.fromEntries(
      rows.map((row) => [
        row.tab_key as string,
        {
          key: row.tab_key as TabKey,
          conversationId: (row.conversation_id as string | null) ?? null,
          shellWorkspaceId: (row.shell_workspace_id as string | null) ?? null,
          customTitle: (row.custom_title as string | null) ?? null,
          defaultTitle: (row.default_title as string | null) ?? null,
          position: Number(row.position),
          createdAt: Number(row.created_at),
          activatedAt: row.activated_at === null ? null : Number(row.activated_at),
        } satisfies TabRow,
      ]),
    ),
    activeByWorkspace: Object.fromEntries(states.map((state) => [state.workspace_id, state.active_tab_key as TabKey | null])),
  };
}

/** What a reader sees: the rows, and the active pointers. */
function normalized(state: TabsState) {
  const rows = Object.values(state.rows).sort((a, b) => (a.key < b.key ? -1 : 1));
  const active = Object.entries(state.activeByWorkspace).sort(([a], [b]) => (a < b ? -1 : 1));
  return JSON.stringify({ rows, active });
}

function prng(seed: number) {
  let x = seed >>> 0 || 1;
  return () => {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    return (x >>> 0) / 4294967296;
  };
}

/** Random facts, written to SQLite and kept as TS facts. */
function randomWorld(seed: number) {
  const random = prng(seed);
  const pick = <T,>(items: readonly T[]): T => items[Math.floor(random() * items.length)];
  const db = new sqlite!.DatabaseSync(":memory:");
  db.exec(HALV_SUBSET);
  db.exec(SCHEMA);
  const workspaces: WorkspaceFact[] = [];
  for (const projectId of [1, 2, 3]) {
    db.prepare("INSERT INTO projects (id, name, root_path) VALUES (?, ?, ?)").run(projectId, `p${projectId}`, `/p${projectId}`);
    db.prepare("INSERT INTO workspaces (id, project_id, kind) VALUES (?, ?, 'primary')").run(`primary-${projectId}`, projectId);
    workspaces.push({ id: `primary-${projectId}`, projectId, kind: "primary" });
    const worktrees = Math.floor(random() * 3);
    for (let index = 0; index < worktrees; index += 1) {
      const id = `wt-${projectId}-${index}`;
      db.prepare("INSERT INTO workspaces (id, project_id, kind) VALUES (?, ?, 'worktree')").run(id, projectId);
      workspaces.push({ id, projectId, kind: "worktree" });
    }
  }
  const conversations: ConversationFact[] = [];
  for (let index = 0; index < 10; index += 1) {
    const projectId = random() < 0.15 ? null : pick([1, 2, 3]);
    const own = workspaces.filter((w) => w.projectId === projectId);
    const workspaceId = projectId === null ? null : random() < 0.3 ? null : pick(own).id;
    const fact: ConversationFact = {
      id: `c${index}`,
      projectId,
      workspaceId,
      mode: random() < 0.5 ? "agent" : "chat",
      archived: random() < 0.15,
      createdAt: index,
      updatedAt: index,
    };
    db.prepare("INSERT INTO conversations (id, project_id, workspace_id, mode, archived_at) VALUES (?, ?, ?, ?, ?)").run(
      fact.id,
      fact.projectId,
      fact.workspaceId,
      fact.mode,
      fact.archived ? 1 : null,
    );
    conversations.push(fact);
  }
  const facts: TabFacts = {
    complete: true,
    conversations: new Map(conversations.map((c) => [c.id, c])),
    workspaces: new Map(workspaces.map((w) => [w.id, w])),
  };
  return { db, facts, random, pick, workspaces, conversations };
}

const SHELL_KEYS = [
  "shell:0f8fad5b-d9cb-469f-a165-70867728950e",
  "shell:7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "shell:00000000-0000-4000-8000-000000000003",
] as TabKey[];

function randomRow(key: TabKey, random: () => number, workspaceId: string, step: number): TabRow {
  const conversationId = key.startsWith("conversation:") ? key.slice("conversation:".length) : null;
  const row: TabRow = {
    key,
    conversationId,
    shellWorkspaceId: conversationId ? null : workspaceId,
    customTitle: null,
    defaultTitle: conversationId ? null : "Terminal 1",
    position: step,
    createdAt: step,
    activatedAt: null,
  };
  // A few malformed rows exercise the CHECK constraint.
  const roll = random();
  if (roll < 0.04) return { ...row, customTitle: "x" };
  if (roll < 0.08) return { ...row, defaultTitle: conversationId ? "Terminal 1" : null };
  if (roll < 0.1) return { ...row, shellWorkspaceId: conversationId ? workspaceId : null };
  return row;
}

test("the TS model reads the Rust statements and the shared schema", { skip }, () => {
  for (const name of ["INSERT_TAB_SQL", "DELETE_TAB_SQL", "TOUCH_TAB_SQL", "SET_POSITION_SQL", "SET_TITLE_SQL", "SET_ACTIVE_SQL", "GC_SQL"]) {
    assert.ok(SQL[name], `${name} is missing in storage/workspace_tabs.rs`);
  }
  assert.match(SQL.INSERT_TAB_SQL, /ON CONFLICT DO NOTHING$/, "a targetless upsert");
  const production = RUST.slice(0, RUST.indexOf("#[cfg(test)]"));
  assert.doesNotMatch(production, /INSERT OR IGNORE INTO workspace_tabs/, "never INSERT OR IGNORE outside the tests");
  assert.match(RUST, /include_str!\("workspace_tabs\.sql"\)/, "Rust embeds the shared schema file");
});

test("disk model fidelity: random batches give the same state and the same failures in SQLite and in TS (300 worlds)", { skip }, () => {
  let failures = 0;
  let batches = 0;
  const failureKinds = new Set<string>();
  for (let seed = 1; seed <= 300; seed += 1) {
    const { db, facts, random, pick, workspaces, conversations } = randomWorld(seed + 1000);
    let state: TabsState = EMPTY_TABS_STATE;
    const keys = [
      ...conversations.map((c) => `conversation:${c.id}`),
      "conversation:ghost",
      "shell:not-a-uuid",
      ...SHELL_KEYS,
    ] as TabKey[];
    for (let step = 0; step < 25; step += 1) {
      const ops: TabsOp[] = [];
      const count = 1 + Math.floor(random() * 3);
      for (let index = 0; index < count; index += 1) {
        const key = pick(keys);
        const choice = random();
        const workspaceId = random() < 0.9 ? pick(workspaces).id : "gone";
        if (choice < 0.3) ops.push({ type: "insertTab", row: randomRow(key, random, workspaceId, step) });
        else if (choice < 0.45) ops.push({ type: "deleteTab", key });
        else if (choice < 0.6) ops.push({ type: "touchTab", key, activatedAt: step });
        else if (choice < 0.7) ops.push({ type: "setPositions", positions: [[key, Math.floor(random() * 5)]] });
        else if (choice < 0.78) ops.push({ type: "setTitle", key, title: random() < 0.5 ? "build" : null });
        else ops.push({ type: "setActive", workspaceId, key: random() < 0.2 ? null : key });
      }
      const sql = sqlApplyBatch(db, ops);
      const ts = applyBatchToDisk(state, ops, facts);
      batches += 1;
      assert.equal(Boolean(sql.error), Boolean(ts.error), `seed ${seed} step ${step}: SQL error ${sql.error} vs TS error ${ts.error}`);
      if (sql.error) {
        failures += 1;
        failureKinds.add(/CHECK|FOREIGN KEY|not a tab key/.exec(sql.error)?.[0] ?? sql.error);
      }
      state = ts.disk;
      assert.equal(normalized(sqlState(db)), normalized(state), `seed ${seed} step ${step}: state diverged`);
    }
    const gc = (db.prepare(`SELECT tab_key FROM workspace_tabs WHERE conversation_id IN (SELECT id FROM conversations WHERE archived_at IS NOT NULL OR project_id IS NULL) ORDER BY tab_key`).all() as Array<{ tab_key: string }>).map((row) => row.tab_key);
    assert.deepEqual(gc, garbageTabKeys(state, facts).sort(), `seed ${seed}: GC set`);
    db.exec(SQL.GC_SQL);
    assert.equal(
      (db.prepare("SELECT COUNT(*) AS n FROM workspace_tabs").get() as { n: number }).n,
      Object.keys(state.rows).length - gc.length,
    );
  }
  assert.ok(failures > 100, `coverage: ${failures} failing batches of ${batches}`);
  assert.deepEqual([...failureKinds].sort(), ["CHECK", "FOREIGN KEY", "not a tab key"]);
});

test("cascades of the TS model match the foreign keys", { skip }, () => {
  const { db, facts, conversations } = randomWorld(7);
  const target = conversations.find((c) => c.projectId !== null && !c.archived)!;
  const ops: TabsOp[] = [
    { type: "insertTab", row: { key: `conversation:${target.id}` as TabKey, conversationId: target.id, shellWorkspaceId: null, customTitle: null, defaultTitle: null, position: 0, createdAt: 1, activatedAt: null } },
    { type: "insertTab", row: { key: SHELL_KEYS[0], conversationId: null, shellWorkspaceId: "primary-1", customTitle: null, defaultTitle: "Terminal 1", position: 1, createdAt: 1, activatedAt: null } },
    { type: "setActive", workspaceId: "primary-1", key: SHELL_KEYS[0] },
  ];
  assert.equal(sqlApplyBatch(db, ops).error, undefined);
  const state = applyBatchToDisk(EMPTY_TABS_STATE, ops, facts).disk;
  db.prepare("DELETE FROM conversations WHERE id = ?").run(target.id);
  db.prepare("DELETE FROM workspaces WHERE id = 'primary-1'").run();
  const expected = cascadeWorkspaceDelete(cascadeConversationDelete(state, target.id), "primary-1");
  assert.equal(normalized(sqlState(db)), normalized(expected));
});

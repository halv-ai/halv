-- Migration 0054_workspace_tabs (ADR 0010). Keep one copy of this schema:
-- Rust embeds this file, and the frontend disk-model tests read it.
--
-- One row per open tab. tab_key is the identity, the owner key and the
-- TerminalPanel scope: 'conversation:<id>' or 'shell:<uuid>'.
-- A conversation tab stores no workspace. Its strip is the workspace of its
-- conversation (conversations.workspace_id, else the primary workspace).
-- The conversation mode is not in this table, so a Pro toggle never moves a tab.
-- Every foreign key is on the child side with CASCADE or SET NULL, so no
-- statement of an older build can fail because of these tables.
CREATE TABLE IF NOT EXISTS workspace_tabs (
    tab_key TEXT PRIMARY KEY,
    conversation_id TEXT UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    shell_workspace_id TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
    custom_title TEXT,
    default_title TEXT,
    position INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    activated_at INTEGER,
    CHECK (
        (conversation_id IS NOT NULL AND shell_workspace_id IS NULL
            AND custom_title IS NULL AND default_title IS NULL
            AND tab_key = 'conversation:' || conversation_id)
        OR
        (conversation_id IS NULL AND shell_workspace_id IS NOT NULL
            AND default_title IS NOT NULL
            AND tab_key LIKE 'shell:_%')
    )
);

CREATE INDEX IF NOT EXISTS idx_workspace_tabs_shell_workspace
    ON workspace_tabs(shell_workspace_id) WHERE shell_workspace_id IS NOT NULL;

-- The active tab of each workspace. A deleted tab clears the pointer. A
-- deleted workspace deletes the row.
CREATE TABLE IF NOT EXISTS workspace_tab_state (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    active_tab_key TEXT REFERENCES workspace_tabs(tab_key) ON DELETE SET NULL,
    updated_at INTEGER NOT NULL
);

-- The one-time seed marker. The seed runs at most once per install.
CREATE TABLE IF NOT EXISTS workspace_tab_seed (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    seeded_at INTEGER NOT NULL
);

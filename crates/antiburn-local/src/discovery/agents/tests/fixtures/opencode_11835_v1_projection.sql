-- Recorded from a fresh OpenCode 1.18.35 native database.
CREATE TABLE project (
    id text PRIMARY KEY
);
CREATE TABLE session (
    id text PRIMARY KEY,
    project_id text NOT NULL,
    workspace_id text,
    parent_id text,
    slug text NOT NULL,
    directory text NOT NULL,
    path text,
    title text NOT NULL,
    version text NOT NULL,
    share_url text,
    summary_additions integer,
    summary_deletions integer,
    summary_files integer,
    summary_diffs text,
    metadata text,
    cost real DEFAULT 0 NOT NULL,
    tokens_input integer DEFAULT 0 NOT NULL,
    tokens_output integer DEFAULT 0 NOT NULL,
    tokens_reasoning integer DEFAULT 0 NOT NULL,
    tokens_cache_read integer DEFAULT 0 NOT NULL,
    tokens_cache_write integer DEFAULT 0 NOT NULL,
    revert text,
    permission text,
    agent text,
    model text,
    time_created integer NOT NULL,
    time_updated integer NOT NULL,
    time_compacting integer,
    time_archived integer,
    CONSTRAINT fk_session_project_id_project_id_fk
        FOREIGN KEY (project_id) REFERENCES project(id) ON DELETE CASCADE
);
CREATE TABLE session_message (
    id text PRIMARY KEY,
    session_id text NOT NULL,
    type text NOT NULL,
    seq integer NOT NULL,
    time_created integer NOT NULL,
    time_updated integer NOT NULL,
    data text NOT NULL,
    CONSTRAINT fk_session_message_session_id_session_id_fk
        FOREIGN KEY (session_id) REFERENCES session(id) ON DELETE CASCADE
);

CREATE TABLE native_file_delivery (
    environment_key TEXT NOT NULL CHECK (environment_key = 'native'),
    agent TEXT NOT NULL CHECK (agent IN ('claude-code', 'codex', 'pi')),
    session_id TEXT NOT NULL,
    incarnation INTEGER NOT NULL,
    source_label TEXT NOT NULL,
    cwd TEXT NOT NULL,
    parser_revision INTEGER NOT NULL,
    cursor_json TEXT CHECK (cursor_json IS NULL OR json_valid(cursor_json)),
    observed_generation INTEGER NOT NULL DEFAULT -1,
    pending INTEGER NOT NULL DEFAULT 1 CHECK (pending IN (0, 1)),
    blocked INTEGER NOT NULL DEFAULT 0 CHECK (blocked IN (0, 1)),
    claim_fence INTEGER,
    lease_expires_at_epoch INTEGER,
    last_attempt_at_epoch INTEGER,
    last_attempt_order INTEGER NOT NULL DEFAULT 0,
    next_attempt_at_epoch INTEGER,
    active_until_epoch INTEGER NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    PRIMARY KEY (environment_key, agent, session_id),
    FOREIGN KEY (environment_key, agent, session_id)
        REFERENCES session (environment_key, agent, session_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX native_file_delivery_due
    ON native_file_delivery (pending, next_attempt_at_epoch, lease_expires_at_epoch);

CREATE INDEX native_tool_result_publication_identity
    ON native_tool_result_publication (
        environment_key, agent, session_id, source_format, native_identity,
        session_incarnation, project_cwd, publication_sequence)
    WHERE outcome = 'update';

CREATE TABLE burn_check_session_attachment (
    check_id TEXT NOT NULL,
    activation_scope_key TEXT NOT NULL,
    generation INTEGER NOT NULL,
    branch_key TEXT NOT NULL,
    incarnation INTEGER NOT NULL,
    PRIMARY KEY (check_id, activation_scope_key, generation, branch_key),
    FOREIGN KEY (check_id, activation_scope_key)
        REFERENCES burn_check_activation (check_id, activation_scope_key) ON DELETE CASCADE
) STRICT;

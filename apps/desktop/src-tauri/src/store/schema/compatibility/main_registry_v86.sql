CREATE TABLE IF NOT EXISTS burn_check_runtime_definition (
    check_id TEXT NOT NULL,
    activation_scope_key TEXT NOT NULL,
    generation INTEGER CHECK (generation IS NULL OR generation > 0),
    definition_revision TEXT NOT NULL,
    name TEXT NOT NULL,
    availability TEXT NOT NULL CHECK (availability IN ('available', 'paused', 'invalid', 'unsupported')),
    PRIMARY KEY (check_id, activation_scope_key)
) STRICT;
CREATE TABLE IF NOT EXISTS burn_check_runtime_definition_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    protocol_version INTEGER NOT NULL CHECK (protocol_version = 1),
    initialized INTEGER NOT NULL CHECK (initialized IN (0, 1))
) STRICT;
INSERT OR IGNORE INTO burn_check_runtime_definition_state VALUES (1, 1, 0);
CREATE INDEX IF NOT EXISTS turn_quota_published
    ON turn(environment_key, agent, session_id, claim_fence, ts_ms)
    WHERE ts_ms IS NOT NULL;

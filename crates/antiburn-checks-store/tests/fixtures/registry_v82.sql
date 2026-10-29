-- Read-only query subset pinned to bc56352c, desktop schema 82.
-- Unused columns and unrelated tables are omitted. The status/history join
-- columns exist, but capture tables and typed source metadata do not exist.
PRAGMA user_version = 82;
CREATE TABLE session (
    environment_key TEXT, agent TEXT, session_id TEXT, cwd TEXT, source_kind TEXT,
    source_generation INTEGER, incarnation INTEGER, source_label TEXT, wsl_distro TEXT);
CREATE TABLE session_relation (
    environment_key TEXT, agent TEXT, session_id TEXT, related_id TEXT, kind TEXT);
CREATE TABLE session_evidence (
    environment_key TEXT, agent TEXT, session_id TEXT, status TEXT,
    analyzed_generation INTEGER, processed_fingerprint TEXT, parser_revision INTEGER,
    analyzer_revision INTEGER, evidence_schema_revision INTEGER, evidence_json TEXT,
    diagnostics_json TEXT, published_fence INTEGER);
CREATE TABLE native_file_delivery (
    environment_key TEXT, agent TEXT, session_id TEXT, incarnation INTEGER,
    source_label TEXT, cwd TEXT, parser_revision INTEGER, cursor_json TEXT,
    blocked INTEGER, last_error TEXT);
CREATE TABLE opencode_journal_delivery (
    environment_key TEXT, agent TEXT, session_id TEXT, incarnation INTEGER,
    source_label TEXT, cwd TEXT, parser_revision INTEGER, cursor_json TEXT,
    blocked INTEGER, main_session INTEGER, last_error TEXT);
CREATE TABLE native_tool_result_publication (
    publication_sequence INTEGER PRIMARY KEY, environment_key TEXT, agent TEXT,
    session_id TEXT, source_format TEXT, session_incarnation INTEGER,
    project_cwd TEXT, outcome TEXT, gap_reason TEXT);
CREATE TABLE burn_check_activation (
    check_id TEXT, activation_scope_key TEXT, generation INTEGER, enabled INTEGER,
    definition_revision TEXT);
CREATE TABLE burn_check_runtime_definition (
    check_id TEXT NOT NULL, activation_scope_key TEXT NOT NULL,
    generation INTEGER CHECK (generation IS NULL OR generation > 0),
    definition_revision TEXT NOT NULL, name TEXT NOT NULL,
    availability TEXT NOT NULL CHECK (availability IN ('available', 'paused', 'invalid', 'unsupported')),
    PRIMARY KEY (check_id, activation_scope_key)
) STRICT;
CREATE TABLE burn_check_runtime_definition_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    protocol_version INTEGER NOT NULL CHECK (protocol_version = 1),
    initialized INTEGER NOT NULL CHECK (initialized IN (0, 1))
) STRICT;
CREATE TABLE burn_check_candidate (
    check_id TEXT, activation_scope_key TEXT, generation INTEGER,
    session_key_json TEXT, state TEXT);
CREATE TABLE burn_check_current_status (
    check_id TEXT, activation_scope_key TEXT, generation INTEGER,
    branch_key TEXT, status TEXT, assessment_id TEXT, evidence_valid INTEGER,
    position INTEGER);
CREATE TABLE burn_check_assessment_history (
    assessment_id TEXT, generation INTEGER, outcome TEXT, evidence_valid INTEGER,
    actions_json TEXT, accepted_at_epoch INTEGER, check_id TEXT,
    activation_scope_key TEXT, branch_key TEXT, trigger_position INTEGER);
CREATE TABLE burn_check_assessment (
    environment_key TEXT, agent TEXT, session_id TEXT, check_id TEXT, status TEXT,
    input_revision TEXT, evaluator_revision TEXT, result_revision TEXT, incarnation INTEGER,
    boundary_generation INTEGER, boundary_activity_cursor TEXT, boundary_at_epoch INTEGER,
    source_generation INTEGER, source_fingerprint TEXT, published_fence INTEGER,
    request_count INTEGER, created_at_epoch INTEGER, updated_at_epoch INTEGER,
    last_error_category TEXT, progress_json TEXT, result_json TEXT);

INSERT INTO session VALUES
    ('native', 'codex', 's1', '/repo', 'file', 1, 7, 'source', NULL);
INSERT INTO native_file_delivery VALUES
    ('native', 'codex', 's1', 7, 'source', '/repo', 62,
     '{"revision":4,"agent":"codex","session_id":"s1","source_format":"codex_rollout_jsonl"}',
     0, NULL);
INSERT INTO burn_check_runtime_definition_state VALUES (1, 1, 1);
INSERT INTO burn_check_activation VALUES
    ('retry-loop', '{"environment":"native","session_cwd":"/repo","run":"Live"}',
     2, 1, 'revision');
INSERT INTO burn_check_runtime_definition VALUES
    ('retry-loop', '{"environment":"native","session_cwd":"/repo","run":"Live"}',
     2, 'revision', 'Saved check', 'available');
INSERT INTO burn_check_assessment_history VALUES
    ('a1', 2, 'assessed', 1,
     '{"current_status":"Failure","outcomes":[{"index":0,"outcome":{"Applied":{"result":"Failure","message":"Saved YAML finding."}}}]}',
     123, 'retry-loop', '{"environment":"native","session_cwd":"/repo","run":"Live"}',
     '["native","codex","s1"]', 4);
INSERT INTO burn_check_current_status VALUES
    ('retry-loop', '{"environment":"native","session_cwd":"/repo","run":"Live"}',
     2, '["native","codex","s1"]', 'failure', 'a1', 1, 4);

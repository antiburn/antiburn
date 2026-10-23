CREATE TABLE session (
    environment_key TEXT, agent TEXT, session_id TEXT, cwd TEXT, source_kind TEXT,
    source_generation INTEGER, status TEXT, analyzed_generation INTEGER,
    parser_revision INTEGER, analyzer_revision INTEGER, evidence_schema_revision INTEGER,
    incarnation INTEGER, source_label TEXT, wsl_distro TEXT, updated_at_epoch INTEGER,
    started_at_epoch INTEGER);
CREATE TABLE session_relation (
    environment_key TEXT, agent TEXT, session_id TEXT, related_id TEXT, kind TEXT);
CREATE TABLE session_evidence (
    environment_key TEXT, agent TEXT, session_id TEXT, status TEXT,
    analyzed_generation INTEGER, parser_revision INTEGER, analyzer_revision INTEGER,
    evidence_schema_revision INTEGER);
CREATE TABLE native_file_delivery (
    environment_key TEXT, agent TEXT, session_id TEXT, incarnation INTEGER,
    source_label TEXT, cwd TEXT, parser_revision INTEGER, cursor_json TEXT,
    blocked INTEGER, last_error TEXT);
CREATE TABLE opencode_journal_delivery (
    environment_key TEXT, agent TEXT, session_id TEXT, incarnation INTEGER,
    source_label TEXT, cwd TEXT, parser_revision INTEGER, cursor_json TEXT,
    blocked INTEGER, main_session INTEGER, last_error TEXT);
CREATE TABLE burn_check_activation (
    check_id TEXT, activation_scope_key TEXT, generation INTEGER, enabled INTEGER,
    definition_revision TEXT);
CREATE TABLE burn_check_runtime_definition (
    check_id TEXT, activation_scope_key TEXT, generation INTEGER,
    definition_revision TEXT, name TEXT, availability TEXT);
CREATE TABLE burn_check_runtime_definition_state (
    singleton INTEGER PRIMARY KEY, protocol_version INTEGER, initialized INTEGER);
INSERT INTO burn_check_runtime_definition_state VALUES (1, 1, 1);
CREATE TABLE burn_check_candidate (
    check_id TEXT, activation_scope_key TEXT, generation INTEGER,
    session_key_json TEXT, state TEXT);
CREATE TABLE burn_check_current_status (
    check_id TEXT, activation_scope_key TEXT, generation INTEGER,
    branch_key TEXT, status TEXT, assessment_id TEXT, evidence_valid INTEGER);
CREATE TABLE burn_check_assessment_history (
    assessment_id TEXT, generation INTEGER, outcome TEXT, evidence_valid INTEGER,
    actions_json TEXT, accepted_at_epoch INTEGER);

INSERT INTO session VALUES
    ('native', 'claude-code', 's1', '/repo', 'file',
     1, NULL, NULL, NULL, NULL, NULL, 7, 'source', NULL, 20, 10);
INSERT INTO native_file_delivery VALUES
    ('native', 'claude-code', 's1', 7, 'source', '/repo', 59, '{}', 0, NULL);

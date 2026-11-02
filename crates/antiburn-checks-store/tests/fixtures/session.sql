CREATE TABLE session (
    environment_key TEXT, agent TEXT, session_id TEXT, cwd TEXT, source_kind TEXT,
    source_generation INTEGER, status TEXT, analyzed_generation INTEGER,
    parser_revision INTEGER, analyzer_revision INTEGER, evidence_schema_revision INTEGER,
    incarnation INTEGER, source_label TEXT, wsl_distro TEXT, updated_at_epoch INTEGER,
    started_at_epoch INTEGER, source_format TEXT);
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
    branch_key TEXT, status TEXT, assessment_id TEXT, evidence_valid INTEGER,
    position INTEGER NOT NULL DEFAULT 0);
CREATE TABLE burn_check_assessment_history (
    assessment_id TEXT, generation INTEGER, outcome TEXT, evidence_valid INTEGER,
    actions_json TEXT, accepted_at_epoch INTEGER, check_id TEXT,
    activation_scope_key TEXT, branch_key TEXT, trigger_position INTEGER NOT NULL DEFAULT 0);
CREATE TABLE opencode_v2_capture (
    environment_key TEXT, agent TEXT, session_id TEXT, incarnation INTEGER,
    source_label TEXT, cwd TEXT, capture_epoch TEXT, claim_fence INTEGER,
    lease_expires_at_epoch INTEGER);
CREATE TABLE setting (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE native_tool_result_publication (
    publication_sequence INTEGER PRIMARY KEY, environment_key TEXT, agent TEXT,
    session_id TEXT, source_format TEXT, session_incarnation INTEGER,
    project_cwd TEXT, outcome TEXT, gap_kind TEXT, gap_reason TEXT);
INSERT INTO setting VALUES
    ('internal:claudeRequestOriginParser65:["native","claude-code","s1"]',
     '{"publication_sequence":1,"source_label":"source","incarnation":7,"cwd":"/repo","source_format":"claude_jsonl"}');
INSERT INTO native_tool_result_publication VALUES
    (1, 'native', 'claude-code', 's1', '"claude_jsonl"', 7, '/repo',
     'coverage_gap', 'segment_reset', 'claude-request-origin-parser-65');

INSERT INTO session VALUES
    ('native', 'claude-code', 's1', '/repo', 'file',
     1, NULL, NULL, NULL, NULL, NULL, 7, 'source', NULL, 20, 10, '"claude_jsonl"');
INSERT INTO native_file_delivery VALUES
    ('native', 'claude-code', 's1', 7, 'source', '/repo', 65,
     '{"revision":5,"agent":"claude","session_id":"s1","source_format":"claude_jsonl"}', 0, NULL);

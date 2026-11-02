CREATE TEMP TABLE publication_high_water AS
SELECT seq FROM sqlite_sequence WHERE name = 'native_tool_result_publication';
CREATE TEMP TABLE current_evidence_copy AS SELECT * FROM native_tool_result_current;
DROP TABLE native_tool_result_current;
CREATE TABLE native_tool_result_publication_v86 (
    publication_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    outcome TEXT NOT NULL CHECK (outcome IN ('update', 'removal', 'coverage_gap', 'coverage_complete')),
    environment_key TEXT NOT NULL, agent TEXT NOT NULL, session_id TEXT NOT NULL,
    source_format TEXT,
    native_identity TEXT,
    native_position_json TEXT CHECK (native_position_json IS NULL OR json_valid(native_position_json)),
    native_revision INTEGER,
    payload_json TEXT CHECK (payload_json IS NULL OR json_valid(payload_json)),
    complete INTEGER NOT NULL CHECK (complete IN (0, 1)),
    source_generation INTEGER NOT NULL, claim_fence INTEGER NOT NULL,
    live_eligible INTEGER CHECK (live_eligible IS NULL OR live_eligible IN (0, 1)),
    project_cwd TEXT, session_incarnation INTEGER, gap_reason TEXT,
    event_kind TEXT NOT NULL DEFAULT 'tool_result' CHECK (event_kind IN
        ('tool_result', 'user_message', 'assistant_message', 'tool_call', 'response_cycle', 'session_start', 'session_idle')),
    first_eligible_publication INTEGER,
    owner_message_id TEXT,
    target_check_id TEXT, target_scope_key TEXT, target_generation INTEGER,
    target_branch_key TEXT, target_incarnation INTEGER, target_attachment_epoch TEXT,
    dependency_binding_json TEXT CHECK (dependency_binding_json IS NULL OR json_valid(dependency_binding_json)),
    gap_kind TEXT NOT NULL DEFAULT 'invalidated' CHECK (gap_kind IN ('invalidated', 'partial', 'segment_reset')),
    CHECK (
        (outcome IN ('coverage_gap', 'coverage_complete') AND source_format IS NOT NULL
            AND native_identity IS NULL AND native_position_json IS NULL AND native_revision IS NULL
            AND payload_json IS NULL AND complete = (outcome = 'coverage_complete') AND target_check_id IS NULL)
        OR (outcome IN ('update', 'removal') AND source_format IS NOT NULL AND payload_json IS NOT NULL AND
            ((event_kind NOT IN ('session_start', 'session_idle') AND native_identity IS NOT NULL
                AND native_position_json IS NOT NULL AND native_revision IS NOT NULL AND target_check_id IS NULL)
             OR (event_kind IN ('session_start', 'session_idle') AND outcome = 'update'
                AND native_identity IS NULL AND native_position_json IS NULL AND native_revision IS NULL
                AND target_check_id IS NOT NULL AND target_scope_key IS NOT NULL AND target_generation IS NOT NULL
                AND target_branch_key IS NOT NULL AND target_incarnation IS NOT NULL AND target_attachment_epoch IS NOT NULL)))
    )
) STRICT;
INSERT INTO native_tool_result_publication_v86
    (publication_sequence, outcome, environment_key, agent, session_id, source_format,
     native_identity, native_position_json, native_revision, payload_json, complete,
     source_generation, claim_fence, live_eligible, project_cwd, session_incarnation,
     gap_reason, gap_kind, owner_message_id)
SELECT publication_sequence, outcome, environment_key, agent, session_id, source_format,
    native_identity, native_position_json, native_revision, payload_json, complete,
    source_generation, claim_fence, live_eligible, project_cwd, session_incarnation,
    gap_reason, gap_kind, json_extract(payload_json, '$.record_id') FROM native_tool_result_publication;
DROP TRIGGER native_tool_result_publication_no_delete;
DROP TABLE native_tool_result_publication;
ALTER TABLE native_tool_result_publication_v86 RENAME TO native_tool_result_publication;
UPDATE sqlite_sequence SET seq = MAX(seq, COALESCE((SELECT seq FROM publication_high_water), 0))
WHERE name = 'native_tool_result_publication';
INSERT INTO sqlite_sequence(name, seq)
SELECT 'native_tool_result_publication', seq FROM publication_high_water
WHERE NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name = 'native_tool_result_publication');
DROP TABLE publication_high_water;
CREATE INDEX native_tool_result_publication_session ON native_tool_result_publication
    (environment_key, agent, session_id, publication_sequence);
CREATE INDEX native_tool_result_publication_identity ON native_tool_result_publication
    (environment_key, agent, session_id, source_format, native_identity,
     session_incarnation, project_cwd, publication_sequence) WHERE outcome = 'update';
CREATE INDEX native_evidence_publication_target ON native_tool_result_publication
    (target_check_id, target_scope_key, target_generation, publication_sequence);
CREATE INDEX native_evidence_publication_unsafe ON native_tool_result_publication
    (environment_key, agent, session_id, source_format, session_incarnation, project_cwd, publication_sequence)
    WHERE outcome = 'coverage_gap' AND gap_reason IN
        ('native-unsafe-identity', 'native-unsafe-order', 'native-unsafe-ownership');
CREATE TRIGGER native_tool_result_publication_no_update BEFORE UPDATE ON native_tool_result_publication
BEGIN SELECT RAISE(ABORT, 'native tool-result publications are append-only'); END;
CREATE TRIGGER native_tool_result_publication_no_delete BEFORE DELETE ON native_tool_result_publication
WHEN EXISTS (SELECT 1 FROM session WHERE environment_key = OLD.environment_key
    AND agent = OLD.agent AND session_id = OLD.session_id)
BEGIN SELECT RAISE(ABORT, 'native tool-result publications are append-only'); END;
CREATE TABLE native_tool_result_current (
    environment_key TEXT NOT NULL, agent TEXT NOT NULL, session_id TEXT NOT NULL,
    source_format TEXT NOT NULL, native_identity TEXT NOT NULL,
    native_position_json TEXT NOT NULL CHECK (json_valid(native_position_json)),
    native_revision INTEGER NOT NULL, payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    complete INTEGER NOT NULL CHECK (complete IN (0, 1)),
    is_removed INTEGER NOT NULL CHECK (is_removed IN (0, 1)),
    publication_sequence INTEGER NOT NULL REFERENCES native_tool_result_publication(publication_sequence),
    source_generation INTEGER NOT NULL, claim_fence INTEGER NOT NULL,
    event_kind TEXT NOT NULL DEFAULT 'tool_result', first_eligible_publication INTEGER,
    owner_message_id TEXT,
    PRIMARY KEY (environment_key, agent, session_id, source_format, native_identity)
) STRICT;
INSERT INTO native_tool_result_current
SELECT *, 'tool_result', NULL, json_extract(payload_json, '$.record_id') FROM current_evidence_copy;
DROP TABLE current_evidence_copy;
CREATE INDEX native_tool_result_current_session ON native_tool_result_current
    (environment_key, agent, session_id, is_removed);
CREATE INDEX native_evidence_current_owner ON native_tool_result_current
    (environment_key, agent, session_id, source_format, owner_message_id, native_identity);
ALTER TABLE burn_check_session_attachment ADD COLUMN source_label TEXT;
ALTER TABLE burn_check_session_attachment ADD COLUMN source_format TEXT;
ALTER TABLE burn_check_session_attachment ADD COLUMN source_receipt TEXT;
ALTER TABLE burn_check_session_attachment ADD COLUMN observer_epoch TEXT;
ALTER TABLE burn_check_session_attachment ADD COLUMN start_publication INTEGER;
ALTER TABLE burn_check_session_attachment ADD COLUMN observation_started_at_ms INTEGER;
ALTER TABLE burn_check_session_attachment ADD COLUMN last_activity_token TEXT;
ALTER TABLE burn_check_session_attachment ADD COLUMN last_activity_order_json TEXT;
ALTER TABLE burn_check_session_attachment ADD COLUMN last_activity_at_ms INTEGER;
ALTER TABLE burn_check_session_attachment ADD COLUMN activity_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE burn_check_session_attachment ADD COLUMN idle_deadline_ms INTEGER;
ALTER TABLE burn_check_session_attachment ADD COLUMN idle_emitted_revision INTEGER;
ALTER TABLE burn_check_session_attachment ADD COLUMN idle_publication INTEGER;
ALTER TABLE burn_check_session_attachment ADD COLUMN continuity_eligible INTEGER NOT NULL DEFAULT 0;
ALTER TABLE burn_check_session_attachment ADD COLUMN session_selection_json TEXT;
CREATE INDEX burn_check_attachment_idle_due ON burn_check_session_attachment(idle_deadline_ms)
WHERE observer_epoch IS NOT NULL AND continuity_eligible = 1 AND idle_emitted_revision IS NULL;
ALTER TABLE native_file_delivery ADD COLUMN source_selection_json TEXT;
ALTER TABLE opencode_journal_delivery ADD COLUMN source_selection_json TEXT;
ALTER TABLE opencode_v2_capture ADD COLUMN source_selection_json TEXT;
CREATE TABLE burn_check_observation_v86 (
    check_id TEXT NOT NULL, activation_scope_key TEXT NOT NULL, observation_id TEXT NOT NULL,
    session_key_json TEXT NOT NULL CHECK (json_valid(session_key_json)),
    publication_sequence INTEGER NOT NULL CHECK (publication_sequence > 0), source_format TEXT NOT NULL,
    native_identity TEXT, native_revision INTEGER, evidence_version TEXT NOT NULL,
    removed INTEGER NOT NULL CHECK (removed IN (0, 1)), payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    event_kind TEXT NOT NULL DEFAULT 'tool_result', observer_binding_json TEXT,
    PRIMARY KEY (check_id, activation_scope_key, observation_id)
) STRICT;
INSERT INTO burn_check_observation_v86 SELECT *, 'tool_result', NULL FROM burn_check_observation;
DROP TABLE burn_check_observation;
ALTER TABLE burn_check_observation_v86 RENAME TO burn_check_observation;
CREATE INDEX burn_check_observation_publication ON burn_check_observation
    (check_id, activation_scope_key, publication_sequence);
CREATE TABLE burn_check_candidate_dependency_v86 (
    candidate_id TEXT NOT NULL REFERENCES burn_check_candidate(candidate_id) ON DELETE CASCADE,
    observation_id TEXT NOT NULL, session_key_json TEXT NOT NULL CHECK (json_valid(session_key_json)),
    source_format TEXT NOT NULL, native_revision INTEGER, evidence_version TEXT NOT NULL,
    removed INTEGER NOT NULL CHECK (removed IN (0, 1)), payload_sha256 TEXT NOT NULL,
    event_kind TEXT NOT NULL DEFAULT 'tool_result', dependency_binding_json TEXT,
    PRIMARY KEY (candidate_id, observation_id)
) STRICT;
INSERT INTO burn_check_candidate_dependency_v86 SELECT *, 'tool_result', NULL FROM burn_check_candidate_dependency;
DROP TABLE burn_check_candidate_dependency;
ALTER TABLE burn_check_candidate_dependency_v86 RENAME TO burn_check_candidate_dependency;
CREATE INDEX burn_check_candidate_dependency_observation ON burn_check_candidate_dependency
    (observation_id, session_key_json, candidate_id);

-- Pinned to e479d52491b9d3c770b22ae3abf9b876fd8fe9cc.
CREATE INDEX session_relation_related
    ON session_relation(environment_key, agent, related_id, kind);
ALTER TABLE session ADD COLUMN source_format TEXT
    CHECK (source_format IS NULL OR (json_valid(source_format) AND json_type(source_format) = 'text'));
UPDATE session SET source_format = CASE
    WHEN source_kind = 'providerDb' AND source_label = 'opencode:' || session_id
        THEN '"open_code_sqlite_v2"'
    WHEN source_kind = 'providerDb'
        AND source_label = 'opencode:' || session_id || ':open_code_v2_sqlite'
        THEN '"open_code_v2_sqlite"'
    WHEN source_kind = 'inline' AND source_label = 'opencode:' || session_id
        THEN '"open_code_jsonl"'
    END
WHERE environment_key = 'native' AND agent = 'opencode';
ALTER TABLE native_tool_result_publication ADD COLUMN gap_kind TEXT NOT NULL
    DEFAULT 'invalidated' CHECK (gap_kind IN ('invalidated', 'partial', 'segment_reset'));
DROP TRIGGER native_tool_result_publication_no_update;
UPDATE native_tool_result_publication SET gap_kind = CASE
    WHEN gap_reason = 'v2-history-incomplete' THEN 'partial'
    ELSE 'segment_reset' END
WHERE outcome = 'coverage_gap' AND source_format = '"open_code_v2_sqlite"';
CREATE TRIGGER native_tool_result_publication_no_update
BEFORE UPDATE ON native_tool_result_publication
BEGIN
    SELECT RAISE(ABORT, 'native tool-result publications are append-only');
END;

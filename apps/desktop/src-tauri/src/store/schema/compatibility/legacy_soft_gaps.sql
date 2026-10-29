DROP TRIGGER native_tool_result_publication_no_update;
UPDATE native_tool_result_publication SET gap_kind = 'partial'
WHERE outcome = 'coverage_gap' AND gap_kind = 'invalidated'
  AND environment_key = 'native'
  AND publication_sequence <= (
      SELECT CAST(value AS INTEGER) FROM setting
      WHERE key = 'internal:untypedNativeGapPublicationHeadV1')
  AND (
      (gap_reason = 'file-history-incomplete' AND (
          (agent = 'claude-code' AND source_format = '"claude_jsonl"')
          OR (agent = 'codex' AND source_format = '"codex_rollout_jsonl"')
          OR (agent = 'pi' AND source_format = '"pi_v3_jsonl"')))
      OR (gap_reason = 'journal-history-incomplete'
          AND agent = 'opencode' AND source_format = '"open_code_sqlite_v2"'));
DELETE FROM setting WHERE key = 'internal:untypedNativeGapPublicationHeadV1';
CREATE TRIGGER native_tool_result_publication_no_update
BEFORE UPDATE ON native_tool_result_publication
BEGIN
    SELECT RAISE(ABORT, 'native tool-result publications are append-only');
END;

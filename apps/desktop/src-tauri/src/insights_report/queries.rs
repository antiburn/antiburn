pub(super) const CURRENT_EVIDENCE_PREDICATE: &str = "
    e.status = 'ready'
    AND NOT (e.analyzed_generation IS NOT s.source_generation)
    AND NOT (e.parser_revision IS NOT ?4)
    AND NOT (e.analyzer_revision IS NOT ?5)
    AND NOT (e.evidence_schema_revision IS NOT ?6)";

pub(super) const DENOMINATOR_SQL: &str = "
SELECT bucket, COUNT(*), SUM(awaiting_provider_support), SUM(evidence_pending),
       SUM(evidence_deferred)
  FROM (
    SELECT CASE
             WHEN s.started_at_epoch IS NULL THEN 'unknown_start'
             WHEN e.status IS NULL OR e.status = 'pending' THEN 'pending'
             WHEN e.status = 'processing' THEN 'processing'
             WHEN e.status = 'failed' THEN 'failed'
             WHEN e.status = 'unsupported' THEN 'unsupported'
             WHEN NOT ({current}) THEN 'stale'
             ELSE 'ready'
           END AS bucket,
           CASE WHEN s.started_at_epoch IS NOT NULL AND e.status IS NULL
                 THEN 1 ELSE 0 END AS awaiting_provider_support,
            CASE WHEN e.status = 'pending' OR e.status = 'processing'
                 THEN 1 ELSE 0 END AS evidence_pending,
            CASE WHEN e.status = 'pending' AND e.next_attempt_at_epoch > ?7
                 THEN 1 ELSE 0 END AS evidence_deferred
      FROM session s
      LEFT JOIN session_evidence e
        ON e.environment_key = s.environment_key
       AND e.agent = s.agent
       AND e.session_id = s.session_id
     WHERE s.environment_key = ?1
       AND COALESCE(s.updated_at_epoch, s.started_at_epoch) >= ?2
       AND COALESCE(s.updated_at_epoch, s.started_at_epoch) < ?3
  )
 GROUP BY bucket
 ORDER BY bucket";

pub(super) const COHORT_SQL: &str = "
SELECT e.evidence_json, s.agent, s.session_id, e.published_fence, a.initial_context_json, s.cwd,
       s.incarnation, s.source_generation, s.source_fingerprint
  FROM session s
  JOIN session_evidence e
    ON e.environment_key = s.environment_key
   AND e.agent = s.agent
   AND e.session_id = s.session_id
  LEFT JOIN session_analysis a
    ON a.environment_key = s.environment_key
   AND a.agent = s.agent
   AND a.session_id = s.session_id
   AND NOT (a.analyzed_generation IS NOT s.source_generation)
   AND NOT (a.parser_revision IS NOT ?4)
   AND NOT (a.analyzer_revision IS NOT ?5)
   AND NOT (a.metrics_schema_revision IS NOT ?7)
 WHERE s.environment_key = ?1
   AND COALESCE(s.updated_at_epoch, s.started_at_epoch) >= ?2
   AND COALESCE(s.updated_at_epoch, s.started_at_epoch) < ?3
   AND {current}
   ORDER BY COALESCE(s.updated_at_epoch, s.started_at_epoch) DESC, s.session_id DESC";

pub(super) const RESOURCE_USE_SQL: &str = "
SELECT e.evidence_json, s.agent, s.session_id, a.initial_context_json, s.cwd
  FROM session s
  JOIN session_evidence e
    ON e.environment_key = s.environment_key
   AND e.agent = s.agent
   AND e.session_id = s.session_id
  LEFT JOIN session_analysis a
    ON a.environment_key = s.environment_key
   AND a.agent = s.agent
   AND a.session_id = s.session_id
   AND NOT (a.analyzed_generation IS NOT s.source_generation)
   AND NOT (a.parser_revision IS NOT ?4)
   AND NOT (a.analyzer_revision IS NOT ?5)
   AND NOT (a.metrics_schema_revision IS NOT ?7)
 WHERE s.environment_key = ?1
   AND COALESCE(s.updated_at_epoch, s.started_at_epoch) >= ?2
   AND COALESCE(s.updated_at_epoch, s.started_at_epoch) < ?3
   AND e.status = 'unsupported'
   AND NOT (e.analyzed_generation IS NOT s.source_generation)
   AND NOT (e.parser_revision IS NOT ?4)
   AND NOT (e.analyzer_revision IS NOT ?5)
   AND NOT (e.evidence_schema_revision IS NOT ?6)
 ORDER BY COALESCE(s.updated_at_epoch, s.started_at_epoch) DESC, s.session_id DESC";

pub(super) const TOKEN_BURN_TURNS_SQL: &str = "
SELECT scope, model, effort, speed, ts_ms, input_tokens, output_tokens,
       cache_read_tokens, cache_write_tokens, cache_write_1h_tokens
  FROM turn
 WHERE environment_key = ?1
   AND agent = ?2
   AND session_id = ?3
   AND claim_fence = ?4
   AND role = 'assistant'";

pub(super) const CURRENT_FINDINGS_SQL: &str = "
SELECT e.evidence_json, s.environment_key, s.agent, s.session_id,
        s.source_generation, e.published_fence, s.source_fingerprint,
       e.processed_fingerprint, e.parser_revision, e.analyzer_revision,
       e.evidence_schema_revision, a.metrics_schema_revision,
       s.started_at_epoch, s.cwd, a.initial_context_json,
        e.effective_model_target_hash, e.effective_model_scope, e.effective_model,
        e.effective_reasoning_target_hash, e.effective_reasoning_scope, e.effective_reasoning,
        s.incarnation
  FROM session s
  JOIN session_evidence e
    ON e.environment_key = s.environment_key
   AND e.agent = s.agent
   AND e.session_id = s.session_id
  JOIN session_analysis a
    ON a.environment_key = s.environment_key
   AND a.agent = s.agent
   AND a.session_id = s.session_id
   AND NOT (a.analyzed_generation IS NOT s.source_generation)
   AND NOT (a.parser_revision IS NOT ?4)
   AND NOT (a.analyzer_revision IS NOT ?5)
   AND NOT (a.metrics_schema_revision IS NOT ?7)
 WHERE s.environment_key = ?1
   AND COALESCE(s.updated_at_epoch, s.started_at_epoch) >= ?2
   AND COALESCE(s.updated_at_epoch, s.started_at_epoch) < ?3
    AND {current}
  ORDER BY COALESCE(s.updated_at_epoch, s.started_at_epoch) DESC, s.agent DESC, s.session_id DESC
  LIMIT ?8";

pub(super) const CURRENT_FINDING_BY_KEY_SQL: &str = "
SELECT e.evidence_json, s.environment_key, s.agent, s.session_id,
       s.source_generation, e.published_fence, s.source_fingerprint,
       e.processed_fingerprint, e.parser_revision, e.analyzer_revision,
       e.evidence_schema_revision, a.metrics_schema_revision,
       s.started_at_epoch, s.cwd, a.initial_context_json,
        e.effective_model_target_hash, e.effective_model_scope, e.effective_model,
        e.effective_reasoning_target_hash, e.effective_reasoning_scope, e.effective_reasoning,
        s.incarnation
  FROM session s
  JOIN session_evidence e
    ON e.environment_key = s.environment_key
   AND e.agent = s.agent
   AND e.session_id = s.session_id
  JOIN session_analysis a
    ON a.environment_key = s.environment_key
   AND a.agent = s.agent
   AND a.session_id = s.session_id
   AND NOT (a.analyzed_generation IS NOT s.source_generation)
   AND NOT (a.parser_revision IS NOT ?4)
   AND NOT (a.analyzer_revision IS NOT ?5)
   AND NOT (a.metrics_schema_revision IS NOT ?7)
 WHERE s.environment_key = ?1
   AND s.agent = ?2
   AND s.session_id = ?3
   AND {current}";

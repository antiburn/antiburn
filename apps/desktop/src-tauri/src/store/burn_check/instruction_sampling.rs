//! Compact instruction sampling reads and publication-fenced replacement.

use super::*;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

impl Store {
    /// Read one coordinate row per comparison. Resolve dependency variants by
    /// exact digest after the worker reconstructs current evidence.
    pub fn instruction_sampled_pairs(
        &self,
        key: &SessionKey,
        incarnation: u64,
    ) -> anyhow::Result<(u32, Vec<BurnCheckSampledPair>)> {
        let connection = self.lock();
        let round = connection.query_row(
            "SELECT COALESCE(MAX(round), 0) FROM burn_check_sampled_pair WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = 'ignored_instructions' AND incarnation = ?4",
            rusqlite::params![key.environment_key, key.agent, key.session_id, incarnation], |row| row.get(0),
        )?;
        let mut statement = connection.prepare(
            "SELECT comparison_id, dependency_digest, incarnation, action_id, action_digest, instruction_digest, selector_revision, MIN(round), MAX(assessed)
             FROM burn_check_sampled_pair WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = 'ignored_instructions' AND incarnation = ?4
             GROUP BY comparison_id ORDER BY comparison_id",
        )?;
        let pairs = statement
            .query_map(
                rusqlite::params![key.environment_key, key.agent, key.session_id, incarnation],
                pair_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((round, pairs))
    }

    pub fn matching_instruction_sampled_pairs(
        &self,
        key: &SessionKey,
        incarnation: u64,
        round: u32,
        backlog: bool,
        dependencies: &[(String, String, String)],
    ) -> anyhow::Result<Vec<BurnCheckSampledPair>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT comparison_id, dependency_digest, incarnation, action_id, action_digest, instruction_digest, selector_revision, round, assessed
             FROM burn_check_sampled_pair WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = 'ignored_instructions' AND incarnation = ?4 AND comparison_id = ?5
             AND dependency_digest IN (?6, ?7) AND ((assessed = 1 AND round < ?8) OR (?9 AND round = ?8))
             ORDER BY assessed DESC, round DESC LIMIT 1",
        )?;
        let mut matched = Vec::new();
        for (id, digest, encoded) in dependencies {
            if let Some(pair) = statement
                .query_row(
                    rusqlite::params![
                        key.environment_key,
                        key.agent,
                        key.session_id,
                        incarnation,
                        id,
                        digest,
                        encoded,
                        round,
                        backlog
                    ],
                    pair_from_row,
                )
                .optional()?
            {
                matched.push(pair);
            }
        }
        Ok(matched)
    }

    /// Keep exact current dependencies when validation reuses an older answer.
    pub fn prune_verified_instruction_pairs(
        &self,
        input: &BurnCheckInput,
        pairs: &[BurnCheckSampledPair],
        dependencies: &[(String, String, String)],
    ) -> anyhow::Result<bool> {
        if pairs.is_empty() {
            return Ok(false);
        }
        anyhow::ensure!(
            input.check_id == "ignored_instructions",
            "instruction sampling requires its check identity"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if !sampling_source_is_current(&transaction, input)? {
            return Ok(false);
        }
        let dependencies: BTreeMap<_, _> = dependencies
            .iter()
            .map(|(id, digest, encoded)| (id.as_str(), (digest, encoded)))
            .collect();
        for pair in pairs {
            let Some((digest, encoded)) = dependencies.get(pair.comparison_id.as_str()) else {
                continue;
            };
            if pair.incarnation != input.incarnation
                || *digest != &pair.dependency_digest && *encoded != &pair.dependency_digest
            {
                continue;
            }
            transaction.execute(
                "DELETE FROM burn_check_sampled_pair WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = 'ignored_instructions'
                 AND incarnation = ?4 AND comparison_id = ?5 AND dependency_digest NOT IN (?6, ?7)
                 AND EXISTS (SELECT 1 FROM burn_check_sampled_pair witness WHERE witness.environment_key = ?1 AND witness.agent = ?2 AND witness.session_id = ?3
                    AND witness.check_id = 'ignored_instructions' AND witness.incarnation = ?4 AND witness.comparison_id = ?5 AND witness.dependency_digest IN (?6, ?7))",
                rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id, input.incarnation, pair.comparison_id, digest, encoded],
            )?;
        }
        transaction.execute("DELETE FROM burn_check_sampled_pair WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = 'ignored_instructions' AND incarnation != ?4", rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id, input.incarnation])?;
        transaction.commit()?;
        Ok(true)
    }

    /// Replace variants only after the exact current publication commits.
    pub fn save_instruction_sampled_pairs(
        &self,
        input: &BurnCheckInput,
        pairs: &[BurnCheckSampledPair],
    ) -> anyhow::Result<bool> {
        anyhow::ensure!(
            input.check_id == "ignored_instructions",
            "instruction sampling requires its check identity"
        );
        anyhow::ensure!(
            pairs
                .iter()
                .all(|pair| pair.incarnation == input.incarnation),
            "sample incarnation does not match publication"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let current: bool = transaction.query_row(
            "SELECT EXISTS (SELECT 1 FROM burn_check_assessment WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4
             AND input_revision = ?5 AND incarnation = ?6 AND source_generation = ?7 AND source_fingerprint IS ?8 AND published_fence = ?9
             AND status IN ('completed', 'failed') AND result_revision = ?5)",
            rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id, input.check_id, input.input_revision, input.incarnation, input.source_generation, input.source_fingerprint, input.published_fence], |row| row.get(0),
        )?;
        if !current {
            return Ok(false);
        }
        let can_prune = sampling_source_is_current(&transaction, input)?;
        if can_prune {
            transaction.execute("DELETE FROM burn_check_sampled_pair WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4 AND incarnation != ?5", rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id, input.check_id, input.incarnation])?;
        }
        for pair in pairs {
            let digest = if pair.dependency_digest.starts_with('{') {
                serde_json::from_str::<DependencyDigest>(&pair.dependency_digest)?.digest
            } else {
                pair.dependency_digest.clone()
            };
            let prior_round: Option<u32> = transaction.query_row(
                "SELECT MIN(round) FROM burn_check_sampled_pair WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4
                 AND incarnation = ?5 AND comparison_id = ?6 AND dependency_digest IN (?7, ?8) AND assessed = 1",
                rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id, input.check_id, input.incarnation, pair.comparison_id, digest, pair.dependency_digest], |row| row.get(0),
            )?;
            let assessed = pair.assessed || prior_round.is_some();
            let round = prior_round.map_or(pair.round, |prior| prior.min(pair.round));
            if can_prune {
                transaction.execute(
                "DELETE FROM burn_check_sampled_pair WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4 AND comparison_id = ?5 AND dependency_digest != ?6",
                rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id, input.check_id, pair.comparison_id, pair.dependency_digest],
            )?;
            }
            transaction.execute(
                "INSERT INTO burn_check_sampled_pair (environment_key, agent, session_id, check_id, comparison_id, dependency_digest, incarnation, action_id, action_digest, instruction_digest, selector_revision, round, assessed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(environment_key, agent, session_id, check_id, comparison_id, dependency_digest) DO UPDATE SET selector_revision = excluded.selector_revision, round = excluded.round, assessed = max(assessed, excluded.assessed)",
                rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id, input.check_id, pair.comparison_id, pair.dependency_digest, pair.incarnation, pair.action_id, pair.action_digest, pair.instruction_digest, pair.selector_revision, round, assessed],
            )?;
        }
        transaction.commit()?;
        Ok(true)
    }
}

#[derive(serde::Deserialize)]
struct DependencyDigest {
    digest: String,
}

fn sampling_source_is_current(
    connection: &rusqlite::Connection,
    input: &BurnCheckInput,
) -> anyhow::Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM session s JOIN session_evidence e USING (environment_key, agent, session_id)
         WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3 AND s.incarnation = ?4 AND s.source_generation = ?5
         AND s.source_fingerprint IS ?6 AND s.activity_cursor = ?7 AND e.status = 'ready' AND e.published_fence = ?8 AND e.analyzed_generation = ?5
         AND e.processed_fingerprint IS s.source_fingerprint AND e.parser_revision = ?9 AND e.analyzer_revision = ?10 AND e.evidence_schema_revision = ?11)",
        rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id, input.incarnation, input.source_generation, input.source_fingerprint, input.activity_cursor, input.published_fence,
            antiburn_local::analysis::PARSER_REVISION, antiburn_local::analysis::ANALYZER_REVISION, antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION], |row| row.get(0),
    )?)
}

fn pair_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BurnCheckSampledPair> {
    Ok(BurnCheckSampledPair {
        comparison_id: row.get(0)?,
        dependency_digest: row.get(1)?,
        incarnation: row.get(2)?,
        action_id: row.get(3)?,
        action_digest: row.get(4)?,
        instruction_digest: row.get(5)?,
        selector_revision: row.get(6)?,
        round: row.get(7)?,
        assessed: row.get(8)?,
    })
}

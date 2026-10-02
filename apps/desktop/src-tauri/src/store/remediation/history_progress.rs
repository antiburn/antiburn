//! Counts behind the dedicated historical scan's progress indicator.

use anyhow::Result;

use super::super::Store;

impl Store {
    /// How many indexed sessions last active before `cutoff_epoch`, and how
    /// many of those have settled evidence (ready, failed, or unsupported).
    ///
    /// A routine pass's own filter keeps a session this old out of its
    /// candidate list (see `crate::scan::current_window_candidates`), so any
    /// indexed session past the cutoff was admitted by the dedicated
    /// historical pass. This counts them directly, with no separate marker
    /// to maintain.
    pub fn history_progress_counts(
        &self,
        agents: &[&str],
        cutoff_epoch: i64,
    ) -> Result<(usize, usize)> {
        if agents.is_empty() {
            return Ok((0, 0));
        }
        let connection = self.lock();
        let agent_placeholders = vec!["?"; agents.len()].join(", ");
        let mut values: Vec<rusqlite::types::Value> = agents
            .iter()
            .map(|agent| rusqlite::types::Value::Text((*agent).to_string()))
            .collect();
        values.push(rusqlite::types::Value::Integer(cutoff_epoch));
        let cutoff_parameter = values.len();
        let (total, completed): (i64, Option<i64>) = connection.query_row(
            &format!(
                "SELECT COUNT(*),
                        SUM(CASE WHEN evidence.status IN ('ready', 'failed', 'unsupported')
                                 THEN 1 ELSE 0 END)
                   FROM session
                   LEFT JOIN session_evidence AS evidence
                     ON evidence.environment_key = session.environment_key
                    AND evidence.agent = session.agent
                    AND evidence.session_id = session.session_id
                  WHERE session.agent IN ({agent_placeholders})
                    AND COALESCE(session.updated_at_epoch, session.started_at_epoch)
                        < ?{cutoff_parameter}"
            ),
            rusqlite::params_from_iter(values.iter()),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((
            usize::try_from(total).unwrap_or(0),
            usize::try_from(completed.unwrap_or(0)).unwrap_or(0),
        ))
    }
}

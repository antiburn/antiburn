use super::*;

const MAX_USAGE_AGGREGATES: usize = 8;
const MAX_USAGE_RESERVATIONS: usize = 4096;
const PINNED_PRICE_VERSION: &str = "typesafe-model-catalog-2026-09-25";
const PINNED_INPUT_NANODOLLARS_PER_TOKEN: u64 = 42;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckUsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub confirmed_calls: u64,
    pub cache_hits: u64,
    pub unknown_outcomes: u64,
    pub estimated_usd: Option<String>,
    pub last_used_at_epoch: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(super) struct UsageLedgerSummary {
    aggregates: Vec<ModelUsageAggregate>,
    unknown_outcomes: u64,
    cache_hits: u64,
    last_unknown_at_epoch: Option<i64>,
    unpriced_usage: bool,
    overflow_input_tokens: u64,
    overflow_output_tokens: u64,
    overflow_confirmed_calls: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelUsageAggregate {
    provider: String,
    check_id: String,
    model: String,
    price_version: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    confirmed_calls: u64,
    #[serde(default)]
    cache_hits: u64,
    estimated_cost_nanos: Option<u64>,
    last_used_at_epoch: i64,
}

impl Store {
    pub fn recover_abandoned_burn_check_usage(
        &self,
        now_epoch: i64,
    ) -> anyhow::Result<(usize, usize)> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut ledger = load_ledger(&transaction)?;
        let mut statement = transaction.prepare(
            "SELECT r.data FROM burn_check_usage_reservation AS r
             WHERE EXISTS (
                 SELECT 1 FROM burn_check_request_outcome AS o
                 WHERE o.reservation_id = r.id
             )",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut reservations = Vec::new();
        for row in rows {
            reservations.push(serde_json::from_str::<UsageReservation>(&row?)?);
        }
        drop(statement);

        let mut recovered = 0;
        for mut reservation in reservations {
            if reservation.settled || reservation.unknown_recorded {
                continue;
            }
            ledger.summary.unknown_outcomes = ledger.summary.unknown_outcomes.saturating_add(1);
            ledger.summary.last_unknown_at_epoch = Some(now_epoch);
            reservation.unknown_recorded = true;
            save_reservation(&transaction, &reservation)?;
            recovered += 1;
        }

        let unresolved_identities = transaction.query_row(
            "SELECT count(*) FROM burn_check_request_outcome",
            [],
            |row| row.get::<_, usize>(0),
        )?;
        transaction.execute(
            "DELETE FROM burn_check_usage_reservation
             WHERE expires_at_epoch <= ?1
               AND NOT EXISTS (SELECT 1 FROM burn_check_request_outcome AS o
                               WHERE o.reservation_id = burn_check_usage_reservation.id)",
            [now_epoch],
        )?;
        write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
        transaction.commit()?;
        Ok((recovered, unresolved_identities))
    }

    pub fn release_rejected_burn_check_usage(&self, reservation_id: &str) -> anyhow::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        load_ledger(&transaction)?;
        transaction.execute(
            "DELETE FROM burn_check_usage_reservation WHERE id = ?1",
            [reservation_id],
        )?;
        transaction.execute(
            "DELETE FROM burn_check_request_outcome WHERE reservation_id = ?1",
            [reservation_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Reserve the conservative request maximum before provider dispatch.
    pub fn reserve_burn_check_usage(
        &self,
        input: &BurnCheckInput,
        provider: &str,
        model: &str,
        reserved_input_tokens: u64,
        now_epoch: i64,
        idle_secs: i64,
    ) -> anyhow::Result<BurnCheckReservation> {
        if provider.is_empty() || provider.len() > 128 || model.is_empty() || model.len() > 128 {
            anyhow::bail!("Burn Check usage identity is invalid");
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let current_evidence = super::super::revision_sql::current_evidence("e", "s");
        if internal_value_in(&transaction, ENABLED_AT_KEY)?.is_none()
            || !check_id_enabled_in(&transaction, &input.check_id)?
        {
            transaction.commit()?;
            return Ok(BurnCheckReservation::Stale);
        }
        let row: Option<(i64, String, Option<i64>, bool)> = transaction
            .query_row(
                &format!("SELECT request_count, status, lease_expires_at_epoch,
                        EXISTS (
                            SELECT 1 FROM session AS s
                            JOIN session_evidence AS e
                              ON e.environment_key = s.environment_key AND e.agent = s.agent
                             AND e.session_id = s.session_id
                             AND e.status = 'ready'
                              AND e.processed_fingerprint IS s.source_fingerprint
                              AND e.published_fence = :published_fence
                              AND {current_evidence}
                             WHERE s.environment_key = :environment_key AND s.agent = :agent AND s.session_id = :session_id
                               AND s.incarnation = :incarnation AND s.source_generation = :source_generation
                               AND s.source_fingerprint IS :source_fingerprint AND s.activity_cursor = :activity_cursor
                               AND s.updated_at_epoch <= :now_epoch - :idle_secs
                        )
                   FROM burn_check_assessment
                  WHERE environment_key = :environment_key AND agent = :agent AND session_id = :session_id
                    AND check_id = :check_id AND input_revision = :input_revision"),
                rusqlite::named_params![
                    ":environment_key": input.key.environment_key,
                    ":agent": input.key.agent,
                    ":session_id": input.key.session_id,
                    ":check_id": input.check_id,
                    ":input_revision": input.input_revision,
                    ":published_fence": input.published_fence,
                    ":now_epoch": now_epoch,
                    ":incarnation": input.incarnation,
                    ":source_generation": input.source_generation,
                    ":source_fingerprint": input.source_fingerprint,
                    ":activity_cursor": input.activity_cursor,
                    ":idle_secs": idle_secs.max(0),
                    ":parser_revision": antiburn_local::analysis::PARSER_REVISION,
                    ":analyzer_revision": antiburn_local::analysis::ANALYZER_REVISION,
                    ":evidence_schema_revision": antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((request_count, status, lease_expires, input_is_current)) = row else {
            transaction.commit()?;
            return Ok(BurnCheckReservation::Stale);
        };
        if status != "running"
            || lease_expires.is_none_or(|expiry| expiry <= now_epoch)
            || !input_is_current
        {
            transaction.commit()?;
            return Ok(BurnCheckReservation::Stale);
        }
        let ledger = load_ledger(&transaction)?;
        transaction.execute(
            "DELETE FROM burn_check_usage_reservation
             WHERE expires_at_epoch <= ?1
               AND NOT EXISTS (SELECT 1 FROM burn_check_request_outcome AS o
                               WHERE o.reservation_id = burn_check_usage_reservation.id)",
            [now_epoch],
        )?;
        prune_settled_reservations(&transaction)?;
        let count: usize = transaction.query_row(
            "SELECT count(*) FROM burn_check_usage_reservation",
            [],
            |row| row.get(0),
        )?;
        if count >= MAX_USAGE_RESERVATIONS {
            anyhow::bail!("Burn Check rolling usage reservation limit exceeded");
        }
        let attempt = request_count.saturating_add(1).to_string();
        let reservation_id = digest_parts([
            input.key.environment_key.as_str(),
            input.key.agent.as_str(),
            input.key.session_id.as_str(),
            input.check_id.as_str(),
            input.input_revision.as_str(),
            attempt.as_str(),
        ]);
        let session_key = digest_parts([
            input.key.environment_key.as_str(),
            input.key.agent.as_str(),
            input.key.session_id.as_str(),
        ]);
        save_reservation(
            &transaction,
            &UsageReservation {
                id: reservation_id.clone(),
                session_key,
                provider: provider.to_owned(),
                check_id: input.check_id.clone(),
                model: model.to_owned(),
                input_tokens: reserved_input_tokens,
                expires_at_epoch: now_epoch.saturating_add(USAGE_WINDOW_SECS),
                settled: false,
                unknown_recorded: false,
            },
        )?;
        write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
        transaction.execute(
            "UPDATE burn_check_assessment SET request_count = request_count + 1,
                    updated_at_epoch = ?6
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5 AND status = 'running'",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                now_epoch,
            ],
        )?;
        transaction.commit()?;
        Ok(BurnCheckReservation::Reserved(reservation_id))
    }

    /// Settle a reservation once; `None` keeps its safety bound for an unknown outcome.
    pub fn settle_burn_check_usage(
        &self,
        reservation_id: &str,
        actual_input_tokens: Option<u64>,
        now_epoch: i64,
    ) -> anyhow::Result<()> {
        self.settle_burn_check_usage_with_output(
            reservation_id,
            actual_input_tokens,
            None,
            now_epoch,
        )
    }

    pub fn settle_burn_check_usage_with_output(
        &self,
        reservation_id: &str,
        actual_input_tokens: Option<u64>,
        actual_output_tokens: Option<u64>,
        now_epoch: i64,
    ) -> anyhow::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut ledger = load_ledger(&transaction)?;
        let mut reservation = load_reservation(&transaction, reservation_id, now_epoch)?;
        if let Some(actual) = actual_input_tokens {
            if !reservation.settled {
                record_confirmed_usage(
                    &mut ledger.summary,
                    &reservation,
                    actual,
                    actual_output_tokens.unwrap_or_default(),
                    now_epoch,
                );
                reservation.settled = true;
            }
            if reservation.unknown_recorded {
                ledger.summary.unknown_outcomes = ledger.summary.unknown_outcomes.saturating_sub(1);
                reservation.unknown_recorded = false;
            }
            reservation.input_tokens = actual;
        } else if !reservation.settled && !reservation.unknown_recorded {
            ledger.summary.unknown_outcomes = ledger.summary.unknown_outcomes.saturating_add(1);
            ledger.summary.last_unknown_at_epoch = Some(now_epoch);
            reservation.unknown_recorded = true;
        }
        save_reservation(&transaction, &reservation)?;
        write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn record_burn_check_cache_hit(
        &self,
        input: &BurnCheckInput,
        provider: &str,
        model: &str,
        check_id: &str,
        attempt_id: &str,
    ) -> anyhow::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let last_hit: Option<String> = transaction
            .query_row(
                "SELECT COALESCE(last_usage_cache_hit_id, '') FROM burn_check_assessment
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND check_id = ?4 AND input_revision = ?5 AND status = 'running'",
                rusqlite::params![
                    input.key.environment_key,
                    input.key.agent,
                    input.key.session_id,
                    input.check_id,
                    input.input_revision,
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if last_hit.as_deref() == Some(attempt_id) {
            transaction.commit()?;
            return Ok(());
        }
        let updated = transaction.execute(
            "UPDATE burn_check_assessment SET last_usage_cache_hit_id = ?6
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5 AND status = 'running'",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                attempt_id,
            ],
        )?;
        if updated != 1 {
            anyhow::bail!("Burn Check cache hit is stale");
        }
        let mut ledger = load_ledger(&transaction)?;
        if !provider.is_empty()
            && !model.is_empty()
            && !check_id.is_empty()
            && !attempt_id.is_empty()
        {
            ledger.summary.cache_hits = ledger.summary.cache_hits.saturating_add(1);
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            let local = provider == "ollama-systemone";
            let price_version = (provider == "typesafe-systemone"
                && model == antiburn_local::analysis::jev::PINNED_MODEL)
                .then(|| PINNED_PRICE_VERSION.to_owned());
            let index = ledger
                .summary
                .aggregates
                .iter()
                .position(|aggregate| {
                    aggregate.provider == provider
                        && aggregate.check_id == check_id
                        && aggregate.model == model
                        && aggregate.price_version == price_version
                })
                .unwrap_or_else(|| {
                    if ledger.summary.aggregates.len() >= MAX_USAGE_AGGREGATES {
                        return usize::MAX;
                    }
                    ledger.summary.aggregates.push(ModelUsageAggregate {
                        provider: provider.to_owned(),
                        check_id: check_id.to_owned(),
                        model: model.to_owned(),
                        price_version: price_version.clone(),
                        input_tokens: 0,
                        output_tokens: 0,
                        confirmed_calls: 0,
                        cache_hits: 0,
                        estimated_cost_nanos: if local {
                            Some(0)
                        } else {
                            price_version.as_ref().map(|_| 0)
                        },
                        last_used_at_epoch: now,
                    });
                    ledger.summary.aggregates.len() - 1
                });
            if !local && price_version.is_none() {
                ledger.summary.unpriced_usage = true;
            }
            if let Some(aggregate) = ledger.summary.aggregates.get_mut(index) {
                aggregate.cache_hits = aggregate.cache_hits.saturating_add(1);
                aggregate.last_used_at_epoch = now;
            }
        }
        write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn burn_check_usage_summary(&self) -> anyhow::Result<BurnCheckUsageSummary> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if internal_value_in(&transaction, USAGE_LEDGER_KEY)?.is_none() {
            return Ok(BurnCheckUsageSummary::default());
        }
        let ledger = load_ledger(&transaction)?;
        transaction.commit()?;
        let mut summary = BurnCheckUsageSummary {
            input_tokens: ledger.summary.overflow_input_tokens,
            output_tokens: ledger.summary.overflow_output_tokens,
            confirmed_calls: ledger.summary.overflow_confirmed_calls,
            cache_hits: ledger.summary.cache_hits,
            unknown_outcomes: ledger.summary.unknown_outcomes,
            ..BurnCheckUsageSummary::default()
        };
        summary.last_used_at_epoch = ledger.summary.last_unknown_at_epoch;
        let mut total_cost_nanos = 0_u64;
        for aggregate in &ledger.summary.aggregates {
            summary.last_used_at_epoch = Some(
                summary
                    .last_used_at_epoch
                    .unwrap_or(i64::MIN)
                    .max(aggregate.last_used_at_epoch),
            );
            summary.input_tokens = summary.input_tokens.saturating_add(aggregate.input_tokens);
            summary.output_tokens = summary
                .output_tokens
                .saturating_add(aggregate.output_tokens);
            summary.confirmed_calls = summary
                .confirmed_calls
                .saturating_add(aggregate.confirmed_calls);
            summary.cache_hits = summary.cache_hits.max(aggregate.cache_hits);
            if let Some(cost) = aggregate.estimated_cost_nanos {
                total_cost_nanos = total_cost_nanos.saturating_add(cost);
            }
        }
        if !ledger.summary.unpriced_usage {
            summary.estimated_usd = Some(format_usd_nanos(total_cost_nanos));
        }
        Ok(summary)
    }
}

pub(super) fn load_ledger(connection: &rusqlite::Connection) -> anyhow::Result<UsageLedger> {
    let raw = internal_value_in(connection, USAGE_LEDGER_KEY)?;
    let mut ledger = raw
        .as_deref()
        .map(serde_json::from_str::<UsageLedger>)
        .transpose()?
        .unwrap_or_default();
    if !ledger.reservations.is_empty() {
        for reservation in &ledger.reservations {
            save_reservation(connection, reservation)?;
        }
        ledger.reservations.clear();
        write_json_setting(connection, USAGE_LEDGER_KEY, &ledger)?;
    }
    Ok(ledger)
}

pub(super) fn load_reservation(
    connection: &rusqlite::Connection,
    id: &str,
    now: i64,
) -> anyhow::Result<UsageReservation> {
    let row: Option<(String, i64, bool)> = connection
        .query_row(
            "SELECT r.data, r.expires_at_epoch,
                    EXISTS (SELECT 1 FROM burn_check_request_outcome AS o
                            WHERE o.reservation_id = r.id)
               FROM burn_check_usage_reservation AS r
              WHERE r.id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (raw, expires_at_epoch, has_unresolved_identity) =
        row.ok_or_else(|| anyhow::anyhow!("Burn Check usage reservation is missing"))?;
    if expires_at_epoch <= now && !has_unresolved_identity {
        anyhow::bail!("Burn Check usage reservation has expired");
    }
    Ok(serde_json::from_str(&raw)?)
}

pub(super) fn save_reservation(
    connection: &rusqlite::Connection,
    reservation: &UsageReservation,
) -> anyhow::Result<()> {
    connection.execute(
        "INSERT INTO burn_check_usage_reservation (id, expires_at_epoch, session_key, data)
        VALUES (?1, ?2, ?3, ?4) ON CONFLICT(id) DO UPDATE SET data = excluded.data,
        expires_at_epoch = excluded.expires_at_epoch, session_key = excluded.session_key",
        rusqlite::params![
            reservation.id,
            reservation.expires_at_epoch,
            reservation.session_key,
            serde_json::to_string(reservation)?
        ],
    )?;
    Ok(())
}

fn prune_settled_reservations(connection: &rusqlite::Connection) -> anyhow::Result<()> {
    connection.execute(
        "DELETE FROM burn_check_usage_reservation
          WHERE json_extract(data, '$.settled') = 1",
        [],
    )?;
    Ok(())
}

pub(super) fn record_confirmed_usage(
    summary: &mut UsageLedgerSummary,
    reservation: &UsageReservation,
    input_tokens: u64,
    output_tokens: u64,
    at_epoch: i64,
) {
    let price_version = (reservation.provider == "typesafe-systemone"
        && reservation.model == antiburn_local::analysis::jev::PINNED_MODEL)
        .then(|| PINNED_PRICE_VERSION.to_owned());
    let local = reservation.provider == "ollama-systemone";
    let cost = if local {
        Some(0)
    } else {
        price_version
            .as_ref()
            .map(|_| input_tokens.saturating_mul(PINNED_INPUT_NANODOLLARS_PER_TOKEN))
    };
    let existing = summary.aggregates.iter().position(|aggregate| {
        aggregate.provider == reservation.provider
            && aggregate.check_id == reservation.check_id
            && aggregate.model == reservation.model
            && aggregate.price_version == price_version
    });
    let index = existing.or_else(|| {
        if summary.aggregates.len() >= MAX_USAGE_AGGREGATES {
            return None;
        }
        summary.aggregates.push(ModelUsageAggregate {
            provider: reservation.provider.clone(),
            check_id: reservation.check_id.clone(),
            model: reservation.model.clone(),
            price_version: price_version.clone(),
            input_tokens: 0,
            output_tokens: 0,
            confirmed_calls: 0,
            cache_hits: 0,
            estimated_cost_nanos: cost.map(|_| 0),
            last_used_at_epoch: at_epoch,
        });
        Some(summary.aggregates.len() - 1)
    });
    if let Some(index) = index {
        let aggregate = &mut summary.aggregates[index];
        aggregate.input_tokens = aggregate.input_tokens.saturating_add(input_tokens);
        aggregate.output_tokens = aggregate.output_tokens.saturating_add(output_tokens);
        aggregate.confirmed_calls = aggregate.confirmed_calls.saturating_add(1);
        aggregate.last_used_at_epoch = at_epoch;
        if let (Some(total), Some(cost)) = (&mut aggregate.estimated_cost_nanos, cost) {
            *total = total.saturating_add(cost);
        } else {
            summary.unpriced_usage = true;
        }
    } else {
        summary.overflow_input_tokens = summary.overflow_input_tokens.saturating_add(input_tokens);
        summary.overflow_output_tokens =
            summary.overflow_output_tokens.saturating_add(output_tokens);
        summary.overflow_confirmed_calls = summary.overflow_confirmed_calls.saturating_add(1);
        summary.unpriced_usage = true;
    }
}

fn format_usd_nanos(nanos: u64) -> String {
    let whole = nanos / 1_000_000_000;
    let mut fraction = format!("{:09}", nanos % 1_000_000_000);
    while fraction.len() > 2 && fraction.ends_with('0') {
        fraction.pop();
    }
    format!("${whole}.{fraction}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_reservations_are_pruned_but_unknown_reservations_remain() {
        let store = Store::open_in_memory(std::path::Path::new("usage-pruning")).unwrap();
        let connection = store.lock();
        for (id, settled, unknown_recorded) in [("settled", true, false), ("unknown", false, true)]
        {
            save_reservation(
                &connection,
                &UsageReservation {
                    id: id.to_owned(),
                    session_key: "session".to_owned(),
                    provider: "typesafe-systemone".to_owned(),
                    check_id: "ignored_instructions".to_owned(),
                    model: "jev-1.13.0".to_owned(),
                    input_tokens: 65_536,
                    expires_at_epoch: 86_400,
                    settled,
                    unknown_recorded,
                },
            )
            .unwrap();
        }

        prune_settled_reservations(&connection).unwrap();

        assert!(load_reservation(&connection, "settled", 1).is_err());
        assert!(load_reservation(&connection, "unknown", 1).is_ok());
    }

    #[test]
    fn indexed_usage_migration_keeps_unknown_bounds_and_summary() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        for migration in &super::super::super::schema::MIGRATIONS[..65] {
            connection.execute_batch(migration).unwrap();
        }
        let mut saved = reservation("jev-1.13.0");
        saved.input_tokens = u64::MAX;
        saved.unknown_recorded = true;
        let ledger = UsageLedger {
            reservations: vec![saved.clone()],
            summary: UsageLedgerSummary {
                unknown_outcomes: 1,
                ..UsageLedgerSummary::default()
            },
        };
        write_json_setting(&connection, USAGE_LEDGER_KEY, &ledger).unwrap();
        connection
            .execute_batch(super::super::super::schema::MIGRATIONS[65])
            .unwrap();
        let restored = load_reservation(&connection, "attempt", 50).unwrap();
        assert_eq!(restored.input_tokens, u64::MAX);
        assert!(restored.unknown_recorded);
        assert!(load_reservation(&connection, "attempt", 100).is_err());
        let ledger = load_ledger(&connection).unwrap();
        assert!(ledger.reservations.is_empty());
        assert_eq!(ledger.summary.unknown_outcomes, 1);
        let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT data FROM burn_check_usage_reservation WHERE id = 'attempt'", [], |row| row.get(3)).unwrap();
        assert!(plan.contains("SEARCH") && plan.contains("INDEX"), "{plan}");
    }

    #[test]
    fn profile_bounded_usage_ledger_reads_and_settlement() {
        let store = Store::open_in_memory(std::path::Path::new("synthetic-state")).unwrap();
        let ledger = UsageLedger {
            reservations: (0..1024)
                .map(|index| UsageReservation {
                    id: format!("{index:064x}"),
                    session_key: "s".repeat(64),
                    provider: "typesafe-systemone".to_owned(),
                    check_id: "synthetic-check".to_owned(),
                    model: "jev-1.13.0".to_owned(),
                    input_tokens: 65536,
                    expires_at_epoch: 86400,
                    settled: false,
                    unknown_recorded: false,
                })
                .collect(),
            summary: UsageLedgerSummary::default(),
        };
        let raw = serde_json::to_string(&ledger).unwrap();
        store.set_internal_value(USAGE_LEDGER_KEY, &raw);
        store.burn_check_usage_summary().unwrap();
        let start = std::time::Instant::now();
        for _ in 0..10 {
            std::hint::black_box(store.burn_check_usage_summary().unwrap());
        }
        let read_us = start.elapsed().as_micros();
        let start = std::time::Instant::now();
        for index in 0..10 {
            store
                .settle_burn_check_usage(&format!("{index:064x}"), None, 1000)
                .unwrap();
        }
        let settle_us = start.elapsed().as_micros();
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            10
        );
        println!(
            "phase6 usage profile reservations=1024 ledger_bytes={} reads_10_us={read_us} settlement_10_us={settle_us}",
            raw.len()
        );
    }

    fn reservation(model: &str) -> UsageReservation {
        UsageReservation {
            id: "attempt".to_owned(),
            session_key: "session-digest".to_owned(),
            provider: "typesafe-systemone".to_owned(),
            check_id: "ignored_instructions".to_owned(),
            model: model.to_owned(),
            input_tokens: 0,
            expires_at_epoch: 100,
            settled: false,
            unknown_recorded: false,
        }
    }

    #[test]
    fn pinned_model_cost_uses_the_price_version_at_settlement() {
        let mut summary = UsageLedgerSummary::default();
        record_confirmed_usage(
            &mut summary,
            &reservation(antiburn_local::analysis::jev::PINNED_MODEL),
            1_000_000,
            24,
            50,
        );
        assert_eq!(format_usd_nanos(42_000_000), "$0.042");
        assert_eq!(summary.aggregates.len(), 1);
        assert_eq!(
            summary.aggregates[0].price_version.as_deref(),
            Some(PINNED_PRICE_VERSION)
        );
        assert_eq!(summary.aggregates[0].estimated_cost_nanos, Some(42_000_000));
        assert_eq!(summary.aggregates[0].input_tokens, 1_000_000);
    }

    #[test]
    fn unknown_model_usage_has_no_estimated_dollar_value() {
        let mut summary = UsageLedgerSummary::default();
        record_confirmed_usage(&mut summary, &reservation("jev-new-model"), 100, 0, 50);
        assert!(summary.unpriced_usage);
        assert_eq!(summary.aggregates[0].estimated_cost_nanos, None);
    }

    #[test]
    fn local_ollama_usage_is_confirmed_without_an_api_charge() {
        let mut usage = reservation("same-model");
        usage.provider = "ollama-systemone".to_owned();
        let mut summary = UsageLedgerSummary::default();
        record_confirmed_usage(&mut summary, &usage, 100, 4, 50);
        assert_eq!(summary.aggregates[0].input_tokens, 100);
        assert_eq!(summary.aggregates[0].output_tokens, 4);
        assert_eq!(summary.aggregates[0].estimated_cost_nanos, Some(0));
        assert!(!summary.unpriced_usage);
    }
}

//! Bounded incurred usage totals and provider pricing for model-based checks.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::Store;

pub(super) const USAGE_LEDGER_KEY: &str = "internal:burnCheckUsageLedgerV1";
pub(super) const MAX_USAGE_AGGREGATES: usize = 8;
pub(super) const PINNED_PRICE_VERSION: &str = "typesafe-model-catalog-2026-09-25";
const JEV_1_14_PRICE_VERSION: &str = "typesafe-model-catalog-2026-10-26";
const INPUT_NANODOLLARS_PER_TOKEN: u64 = 42;
const MAX_USAGE_CHECK_ID_BYTES: usize = 256;

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
pub(super) struct UsageLedger {
    #[serde(default)]
    pub(super) summary: UsageLedgerSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(super) struct UsageLedgerSummary {
    pub(super) aggregates: Vec<ModelUsageAggregate>,
    pub(super) unknown_outcomes: u64,
    pub(super) cache_hits: u64,
    pub(super) last_unknown_at_epoch: Option<i64>,
    pub(super) unpriced_usage: bool,
    pub(super) overflow_input_tokens: u64,
    pub(super) overflow_output_tokens: u64,
    pub(super) overflow_confirmed_calls: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ModelUsageAggregate {
    pub(super) provider: String,
    pub(super) check_id: String,
    pub(super) model: String,
    pub(super) price_version: Option<String>,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) confirmed_calls: u64,
    #[serde(default)]
    pub(super) cache_hits: u64,
    pub(super) estimated_cost_nanos: Option<u64>,
    pub(super) last_used_at_epoch: i64,
}

pub(super) struct UsageIdentity<'a> {
    pub(super) provider: &'a str,
    pub(super) check_id: &'a str,
    pub(super) model: &'a str,
}

impl Store {
    pub fn burn_check_usage_summary(&self) -> anyhow::Result<BurnCheckUsageSummary> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if ledger_value_in(&transaction)?.is_none() {
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

fn ledger_value_in(connection: &rusqlite::Connection) -> anyhow::Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT value FROM setting WHERE key = ?1",
            [USAGE_LEDGER_KEY],
            |row| row.get(0),
        )
        .optional()?)
}

pub(super) fn load_ledger(connection: &rusqlite::Connection) -> anyhow::Result<UsageLedger> {
    let raw = ledger_value_in(connection)?;
    let ledger = raw
        .as_deref()
        .map(serde_json::from_str::<UsageLedger>)
        .transpose()?
        .unwrap_or_default();
    Ok(ledger)
}

pub(super) fn write_json_setting(
    connection: &rusqlite::Connection,
    key: &str,
    value: &impl Serialize,
) -> anyhow::Result<()> {
    let serialized = serde_json::to_string(value)?;
    connection.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, serialized],
    )?;
    Ok(())
}

pub(super) fn record_confirmed_usage(
    summary: &mut UsageLedgerSummary,
    identity: &UsageIdentity<'_>,
    input_tokens: u64,
    output_tokens: u64,
    at_epoch: i64,
) {
    // https://docs.typesafe.ai/models.md, read 2026-10-26. Output is free.
    let price_version = if identity.provider == "typesafe-systemone" {
        match identity.model {
            "jev-1.13.0" => Some(PINNED_PRICE_VERSION.to_owned()),
            "jev-1.14.0" => Some(JEV_1_14_PRICE_VERSION.to_owned()),
            _ => None,
        }
    } else {
        None
    };
    let local = identity.provider == "ollama-systemone";
    let cost = if local {
        Some(0)
    } else {
        price_version
            .as_ref()
            .map(|_| input_tokens.saturating_mul(INPUT_NANODOLLARS_PER_TOKEN))
    };
    let existing = summary.aggregates.iter().position(|aggregate| {
        aggregate.provider == identity.provider
            && aggregate.check_id == identity.check_id
            && aggregate.model == identity.model
            && aggregate.price_version == price_version
    });
    let index = existing.or_else(|| {
        if summary.aggregates.len() >= MAX_USAGE_AGGREGATES
            || identity.check_id.len() > MAX_USAGE_CHECK_ID_BYTES
        {
            return None;
        }
        summary.aggregates.push(ModelUsageAggregate {
            provider: identity.provider.to_owned(),
            check_id: identity.check_id.to_owned(),
            model: identity.model.to_owned(),
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

    fn identity(model: &str) -> UsageIdentity<'_> {
        UsageIdentity {
            provider: "typesafe-systemone",
            check_id: "custom-check",
            model,
        }
    }

    #[test]
    fn pinned_model_cost_uses_the_price_version_at_settlement() {
        let mut summary = UsageLedgerSummary::default();
        record_confirmed_usage(
            &mut summary,
            &identity(antiburn_local::analysis::jev::PINNED_MODEL),
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
    fn concrete_versions_keep_distinct_sourced_prices_and_count_free_output() {
        let mut summary = UsageLedgerSummary::default();
        for model in ["jev-1.13.0", "jev-1.14.0"] {
            record_confirmed_usage(&mut summary, &identity(model), 1_000_000, u64::MAX, 50);
        }
        assert_eq!(summary.aggregates.len(), 2);
        for (aggregate, price) in summary
            .aggregates
            .iter()
            .zip([PINNED_PRICE_VERSION, JEV_1_14_PRICE_VERSION])
        {
            assert_eq!(aggregate.price_version.as_deref(), Some(price));
            assert_eq!(aggregate.estimated_cost_nanos, Some(42_000_000));
            assert_eq!(aggregate.output_tokens, u64::MAX);
        }
        assert!(!summary.unpriced_usage);
    }

    #[test]
    fn unknown_model_usage_has_no_estimated_dollar_value() {
        let mut summary = UsageLedgerSummary::default();
        for model in ["jev-new-model", "jev-latest", "unknown"] {
            record_confirmed_usage(&mut summary, &identity(model), 100, 0, 50);
        }
        assert!(summary.unpriced_usage);
        assert_eq!(summary.unknown_outcomes, 0);
        assert_eq!(summary.aggregates.len(), 3);
        for aggregate in summary.aggregates {
            assert_eq!(aggregate.estimated_cost_nanos, None);
            assert_eq!(aggregate.confirmed_calls, 1);
        }
    }

    #[test]
    fn other_providers_do_not_inherit_jev_pricing_from_the_model_name() {
        for provider in ["custom-systemone", "cloudflare-systemone"] {
            let mut usage = identity(antiburn_local::analysis::jev::PINNED_MODEL);
            usage.provider = provider;
            let mut summary = UsageLedgerSummary::default();
            record_confirmed_usage(&mut summary, &usage, 100, 4, 50);
            assert!(summary.unpriced_usage, "{provider}");
            assert_eq!(summary.aggregates[0].price_version, None, "{provider}");
            assert_eq!(
                summary.aggregates[0].estimated_cost_nanos, None,
                "{provider}"
            );
        }
    }

    #[test]
    fn local_ollama_usage_is_confirmed_without_an_api_charge() {
        let mut usage = identity("same-model");
        usage.provider = "ollama-systemone";
        let mut summary = UsageLedgerSummary::default();
        record_confirmed_usage(&mut summary, &usage, 100, 4, 50);
        assert_eq!(summary.aggregates[0].input_tokens, 100);
        assert_eq!(summary.aggregates[0].output_tokens, 4);
        assert_eq!(summary.aggregates[0].estimated_cost_nanos, Some(0));
        assert!(!summary.unpriced_usage);
    }

    #[test]
    fn version_and_identifier_overflow_keeps_saturating_token_and_call_totals() {
        let mut summary = UsageLedgerSummary::default();
        for version in 0..MAX_USAGE_AGGREGATES + 3 {
            record_confirmed_usage(
                &mut summary,
                &identity(&format!("jev-future-{version}")),
                u64::MAX,
                u64::MAX,
                50,
            );
        }
        assert_eq!(summary.aggregates.len(), MAX_USAGE_AGGREGATES);
        assert_eq!(summary.overflow_confirmed_calls, 3);
        assert_eq!(summary.overflow_input_tokens, u64::MAX);
        assert_eq!(summary.overflow_output_tokens, u64::MAX);

        let mut summary = UsageLedgerSummary::default();
        let mut usage = identity("jev-1.14.0");
        let check_id = "x".repeat(MAX_USAGE_CHECK_ID_BYTES + 1);
        usage.check_id = &check_id;
        record_confirmed_usage(&mut summary, &usage, 123, 4, 50);
        assert!(summary.aggregates.is_empty());
        assert_eq!(summary.overflow_confirmed_calls, 1);
        assert_eq!(summary.overflow_input_tokens, 123);
        assert_eq!(summary.overflow_output_tokens, 4);
        assert!(summary.unpriced_usage);
    }
}

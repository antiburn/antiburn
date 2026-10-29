use antiburn_checks::bounded_json::{JsonLimits, decode_strict_json};
use antiburn_checks::fit::{FitReport, LocalPreparation, PreparationFailure, PreparationRecord};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

/// Historical records duplicate the fitted input and omission report in the
/// request. Two 8 MiB inputs plus two reports with 4,096 targets fit in 64 MiB:
/// 2 * 8 MiB + 2 * 4,096 * (2 * (2,048 + 256) + 128) leaves room for bounded
/// request metadata and the profile. The extra 256 bytes cover concrete indexes
/// in 64 path segments; the factor two covers JSON escaping of path text.
/// Two 16,384-node inputs and two five-value-per-target reports need fewer than
/// 131,072 values. The old depth starts at zero; request wrappers add four levels.
/// These bounds apply only to report reads and cannot authorize execution.
const LEGACY_LIMITS: JsonLimits = JsonLimits {
    max_bytes: 64 * 1024 * 1024,
    max_nodes: 131_072,
    max_depth: 72,
};

pub(super) enum ReportPreparation {
    Model(ModelPreparation),
    Local(LocalPreparation),
    Rejected(PreparationFailure),
}

pub(super) struct ModelPreparation {
    pub profile: String,
    pub blocks: Vec<ReportBlock>,
}

pub(super) struct ReportBlock {
    pub decision: Option<String>,
    pub original_input: Option<Value>,
    pub input: Value,
    pub report: FitReport,
}

pub(super) fn decode(bytes: &[u8], accepted: bool) -> Result<ReportPreparation> {
    match antiburn_checks::preparation::decode_preparation_record(bytes) {
        Ok(PreparationRecord::Ready(prepared)) => Ok(ReportPreparation::Model(ModelPreparation {
            profile: prepared.profile,
            blocks: prepared
                .decisions
                .into_iter()
                .map(|block| ReportBlock {
                    decision: Some(block.decision),
                    original_input: Some(block.original_input),
                    input: block.input,
                    report: block.report,
                })
                .collect(),
        })),
        Ok(PreparationRecord::Local(prepared)) => Ok(ReportPreparation::Local(prepared)),
        Ok(PreparationRecord::Rejected(failure)) => Ok(ReportPreparation::Rejected(failure)),
        Err(error) if !accepted => Err(anyhow::anyhow!(error.detail)),
        Err(_) => decode_legacy(bytes),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyReady {
    status: LegacyReadyStatus,
    profile: String,
    input: Value,
    request: Option<Value>,
    report: LegacyReport,
    #[serde(default)]
    evaluation_budget: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyReadyStatus {
    Ready,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyReport {
    omissions: Vec<LegacyOmission>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyOmission {
    path: String,
    strategy: antiburn_checks::fit::FitStrategy,
    original_count: usize,
    retained_count: usize,
}

fn decode_legacy(bytes: &[u8]) -> Result<ReportPreparation> {
    let value =
        decode_strict_json(bytes, LEGACY_LIMITS).context("decode historical preparation")?;
    let legacy: LegacyReady =
        serde_json::from_value(value).context("decode historical ready preparation")?;
    let LegacyReady {
        status: LegacyReadyStatus::Ready,
        profile,
        input,
        request,
        report,
        evaluation_budget,
    } = legacy;
    // Reports never use saved requests or budgets to resume historical work.
    drop((request, evaluation_budget));
    Ok(ReportPreparation::Model(ModelPreparation {
        profile,
        blocks: vec![ReportBlock {
            decision: None,
            original_input: None,
            input,
            report: FitReport {
                omissions: report
                    .omissions
                    .into_iter()
                    .map(|omission| antiburn_checks::fit::FitOmission {
                        path: omission.path,
                        strategy: omission.strategy,
                        original_count: omission.original_count,
                        retained_count: omission.retained_count,
                    })
                    .collect(),
            },
        }],
    }))
}

use std::io::{self, Write};

use super::*;

pub(super) fn resource_limit(budget: PreparationBudget) -> PreparationFailure {
    PreparationFailure {
        code: PreparationFailureCode::PreparationResourceLimit,
        detail: "Input preparation reached a local resource limit. Select less input or fewer fitting targets. The model request was not sent.".into(),
        report: None,
        context: PreparationFailureContext {
            field: match budget.kind {
                PreparationBudgetKind::InputBytes | PreparationBudgetKind::InputNodes | PreparationBudgetKind::InputDepth => Some("input".into()),
                PreparationBudgetKind::FitRules | PreparationBudgetKind::PathDepth | PreparationBudgetKind::PathBytes |
                PreparationBudgetKind::ExpandedTargets | PreparationBudgetKind::TraversalNodes | PreparationBudgetKind::Turns => Some("fit".into()),
                PreparationBudgetKind::WorkBytes | PreparationBudgetKind::RequestBytes | PreparationBudgetKind::EstimatedTokens |
                PreparationBudgetKind::ResidentBytes | PreparationBudgetKind::Criteria => None,
            },
            budget: Some(budget),
            ..Default::default()
        }.into(),
    }
}

pub(super) fn check_count(
    kind: PreparationBudgetKind,
    observed: usize,
    maximum: usize,
) -> Result<(), PreparationFailure> {
    if observed > maximum {
        return Err(resource_limit(PreparationBudget {
            kind,
            observed: u64::try_from(observed).unwrap_or(u64::MAX),
            maximum: u64::try_from(maximum).unwrap_or(u64::MAX),
        }));
    }
    Ok(())
}

pub(super) fn admit_input(input: &Value) -> Result<(), PreparationFailure> {
    let mut nodes = 0;
    let mut text_bytes = 0;
    admit_value(input, 0, &mut nodes, &mut text_bytes)?;
    measure_preparation_bytes(
        input,
        MAX_FIT_INPUT_BYTES,
        PreparationBudgetKind::InputBytes,
    )?;
    Ok(())
}

fn admit_value(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
    text_bytes: &mut u64,
) -> Result<(), PreparationFailure> {
    check_count(
        PreparationBudgetKind::InputDepth,
        depth,
        MAX_FIT_INPUT_DEPTH,
    )?;
    *nodes += 1;
    check_count(
        PreparationBudgetKind::InputNodes,
        *nodes,
        MAX_FIT_INPUT_NODES,
    )?;
    match value {
        Value::String(text) => add_text_bytes(text.len(), text_bytes)?,
        Value::Object(fields) => {
            for (key, value) in fields {
                add_text_bytes(key.len(), text_bytes)?;
                admit_value(value, depth + 1, nodes, text_bytes)?;
            }
        }
        Value::Array(items) => {
            for value in items {
                admit_value(value, depth + 1, nodes, text_bytes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn add_text_bytes(count: usize, total: &mut u64) -> Result<(), PreparationFailure> {
    *total = total.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    if *total > MAX_FIT_INPUT_BYTES {
        return Err(resource_limit(PreparationBudget {
            kind: PreparationBudgetKind::InputBytes,
            observed: *total,
            maximum: MAX_FIT_INPUT_BYTES,
        }));
    }
    Ok(())
}

struct ByteCounter {
    observed: u64,
    maximum: u64,
}

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.observed = self
            .observed
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if self.observed > self.maximum {
            return Err(io::Error::other("preparation byte limit reached"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Counts serialized bytes without allocating a request body or exceeding the limit.
pub fn measure_preparation_bytes(
    value: &impl Serialize,
    maximum: u64,
    kind: PreparationBudgetKind,
) -> Result<u64, PreparationFailure> {
    let mut counter = ByteCounter {
        observed: 0,
        maximum,
    };
    if serde_json::to_writer(&mut counter, value).is_err() {
        if counter.observed > maximum {
            return Err(resource_limit(PreparationBudget {
                kind,
                observed: counter.observed,
                maximum,
            }));
        }
        return Err(PreparationFailure {
            code: PreparationFailureCode::InvalidInput,
            detail: "The decision input cannot be serialized.".into(),
            report: None,
            context: Default::default(),
        });
    }
    Ok(counter.observed)
}

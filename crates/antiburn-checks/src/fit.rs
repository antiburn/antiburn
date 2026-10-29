//! Ordered, explicitly authorized reductions of decision input.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[path = "fit_limits.rs"]
mod limits;
pub use limits::{FittingWorkBudget, measure_preparation_bytes};

pub const MAX_FIT_INPUT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_FIT_INPUT_NODES: usize = 16_384;
pub const MAX_FIT_INPUT_DEPTH: usize = 64;
pub const MAX_FIT_RULES: usize = 128;
pub const MAX_FIT_PATH_SEGMENTS: usize = 64;
pub const MAX_FIT_PATH_BYTES: usize = 2048;
pub const MAX_FIT_TARGETS: usize = 4096;
pub const MAX_FIT_TRAVERSAL_NODES: usize = 32_768;
pub const MAX_FIT_TURNS: usize = 4096;

/// Original bytes removed from a string in one turn, before Unicode alignment.
pub const TRUNCATE_CHUNK_BYTES: usize = 1024;
/// Minimum retained original text, excluding the omission marker.
pub const MINIMUM_TEXT_BYTES: usize = 256;
/// One marker replaces all omitted middle text.
pub const OMISSION_MARKER: &str = "\n[... omitted ...]\n";

/// An author-approved input reduction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitStrategy {
    /// Remove the oldest whole array record, retaining at least one.
    Recent,
    /// Remove middle text, retaining its head and tail.
    Truncate,
}

/// One parsed field or array traversal in an authored input path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum FitPathSegment {
    Field(String),
    Each,
}

/// One fit rule, stored in YAML declaration order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FitRule {
    pub path: String,
    pub segments: Vec<FitPathSegment>,
    pub strategy: FitStrategy,
}

/// Counts use array records for recent and original UTF-8 bytes for truncate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FitOmission {
    pub path: String,
    pub strategy: FitStrategy,
    pub original_count: usize,
    pub retained_count: usize,
}

/// Only changed concrete targets appear, in expansion order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FitReport {
    pub omissions: Vec<FitOmission>,
}

/// Durable local start and the shared budget used by its status actions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPreparation {
    pub version: u8,
    pub started_at_unix_ms: u64,
    pub evaluation_budget: crate::expression_eval::EvaluationBudget,
}

impl LocalPreparation {
    /// Starts local evaluation without provider preparation.
    #[must_use]
    pub fn new(started_at_unix_ms: u64) -> Self {
        Self {
            version: 1,
            started_at_unix_ms,
            evaluation_budget: crate::expression_eval::EvaluationBudget::default(),
        }
    }

    /// Validates saved local state without replacing its budget.
    ///
    /// # Errors
    /// Returns an incompatible-profile failure for unsupported or invalid state.
    pub fn validate_for_resume(&self) -> Result<(), PreparationFailure> {
        let invalid = |detail| PreparationFailure {
            code: PreparationFailureCode::IncompatibleProfile,
            detail,
            report: None,
            context: Default::default(),
        };
        if self.version != 1 {
            return Err(invalid(
                "The saved local preparation version is unsupported.".into(),
            ));
        }
        self.evaluation_budget
            .validate_for_resume()
            .map_err(|error| invalid(error.message))
    }
}

/// Closed classifications for local failures before provider dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationFailureCode {
    ConditionFalse,
    InputUnavailable,
    InvalidInput,
    InvalidRequest,
    NoFit,
    PreparationResourceLimit,
    InvalidFit,
    ProviderUnavailable,
    IncompatibleProfile,
}

/// A local rejection with safe detail and any attempted reductions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationFailure {
    pub code: PreparationFailureCode,
    pub detail: String,
    pub report: Option<FitReport>,
    #[serde(default)]
    pub context: Box<PreparationFailureContext>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoFitReason {
    NoRules,
    NoMatchingTargets,
    TargetsAtMinimum,
    ReductionsExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationBudgetKind {
    InputBytes,
    InputNodes,
    InputDepth,
    FitRules,
    PathDepth,
    PathBytes,
    ExpandedTargets,
    TraversalNodes,
    Turns,
    WorkBytes,
    RequestBytes,
    EstimatedTokens,
    ResidentBytes,
    Criteria,
}

/// Local observations and estimates do not represent provider billing usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationBudget {
    pub kind: PreparationBudgetKind,
    pub observed: u64,
    pub maximum: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationFailureContext {
    pub decision: Option<String>,
    pub field: Option<String>,
    pub input_path: Option<String>,
    pub no_fit_reason: Option<NoFitReason>,
    pub budget: Option<PreparationBudget>,
}

/// Saved on the candidate before dispatch intent or a local terminal outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PreparationRecord {
    Ready(crate::prepared_assessment::PreparedAssessment),
    Local(LocalPreparation),
    Rejected(PreparationFailure),
}

/// Complete-request validation distinguishes size from other local failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FitValidationError {
    OverBudget(PreparationBudget),
    ResourceLimit(PreparationBudget),
    Unavailable(String),
    InvalidRequest {
        detail: String,
        context: Box<PreparationFailureContext>,
    },
}

/// Parses dot fields and `[*]` traversals without accepting indexes or expressions.
pub fn parse_fit_path(path: &str) -> Option<Vec<FitPathSegment>> {
    if path.len() > MAX_FIT_PATH_BYTES {
        return None;
    }
    let bytes = path.as_bytes();
    let mut cursor = 0;
    let mut segments = Vec::new();
    loop {
        let start = cursor;
        if !bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            return None;
        }
        cursor += 1;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            cursor += 1;
        }
        if segments.is_empty()
            && (!bytes[start].is_ascii_lowercase()
                || bytes[start..cursor].iter().any(u8::is_ascii_uppercase))
        {
            return None;
        }
        segments.push(FitPathSegment::Field(path[start..cursor].to_owned()));
        while path[cursor..].starts_with("[*]") {
            segments.push(FitPathSegment::Each);
            cursor += 3;
        }
        if cursor == bytes.len() {
            return Some(segments);
        }
        if bytes[cursor] != b'.' {
            return None;
        }
        cursor += 1;
    }
}

/// Prefix targets overlap because reducing a parent changes its descendants.
#[must_use]
pub fn paths_overlap(left: &[FitPathSegment], right: &[FitPathSegment]) -> bool {
    left.iter().zip(right).all(|(left, right)| left == right)
}

struct Target {
    pointer: String,
    path: String,
    strategy: FitStrategy,
    original_count: usize,
    retained_count: usize,
    original_text: Option<String>,
    exhausted: bool,
}

#[derive(Default)]
struct Expansion {
    targets: Vec<Target>,
    visited: usize,
    matched: usize,
}

/// Expands once, then gives each concrete target one turn per round.
/// The validator must include provider framing, questions, and omission metadata.
pub fn fit_input(
    input: Value,
    rules: &[FitRule],
    mut validate: impl FnMut(&Value, &FitReport) -> Result<(), FitValidationError>,
) -> Result<(Value, FitReport), PreparationFailure> {
    fit_input_with_work_budget(
        input,
        rules,
        &mut FittingWorkBudget::default(),
        |input, report, _| validate(input, report),
    )
}

/// Fits one block against the shared complete-preparation work allowance.
pub fn fit_input_with_work_budget(
    mut input: Value,
    rules: &[FitRule],
    work: &mut FittingWorkBudget,
    mut validate: impl FnMut(
        &Value,
        &FitReport,
        &mut FittingWorkBudget,
    ) -> Result<(), FitValidationError>,
) -> Result<(Value, FitReport), PreparationFailure> {
    limits::admit_input(&input)?;
    work.measure(&input)?;
    limits::check_count(PreparationBudgetKind::FitRules, rules.len(), MAX_FIT_RULES)?;
    let mut expansion = Expansion::default();
    for (index, rule) in rules.iter().enumerate() {
        limits::check_count(
            PreparationBudgetKind::PathDepth,
            rule.segments.len(),
            MAX_FIT_PATH_SEGMENTS,
        )?;
        let path_bytes = rule
            .segments
            .iter()
            .map(|segment| match segment {
                FitPathSegment::Field(field) => field.len().saturating_add(1),
                FitPathSegment::Each => 3,
            })
            .fold(0_usize, usize::saturating_add)
            .saturating_sub(1);
        limits::check_count(
            PreparationBudgetKind::PathBytes,
            path_bytes,
            MAX_FIT_PATH_BYTES,
        )?;
        if rules[..index]
            .iter()
            .any(|previous| paths_overlap(&previous.segments, &rule.segments))
        {
            return Err(invalid_fit("Fit targets must not overlap.", &rule.path));
        }
        expand(
            &input,
            &rule.segments,
            "",
            "",
            rule.strategy,
            &mut expansion,
        )
        .map_err(|mut error| {
            error
                .context
                .input_path
                .get_or_insert_with(|| rule.path.clone());
            error
        })?;
    }
    let mut report = FitReport::default();
    let mut budget = None;
    if validate_request(&input, &report, &mut budget, work, &mut validate)? {
        return Ok((input, report));
    }
    let mut turns = 0;
    loop {
        let mut changed = false;
        for target in &mut expansion.targets {
            if target.exhausted {
                continue;
            }
            turns += 1;
            limits::check_count(PreparationBudgetKind::Turns, turns, MAX_FIT_TURNS).map_err(
                |mut error| {
                    error.context.input_path = Some(target.path.clone());
                    error.report = Some(report.clone());
                    error
                },
            )?;
            if !reduce(&mut input, target) {
                continue;
            }
            work.measure(&input)?;
            changed = true;
            let omission = FitOmission {
                path: target.path.clone(),
                strategy: target.strategy,
                original_count: target.original_count,
                retained_count: target.retained_count,
            };
            if let Some(saved) = report
                .omissions
                .iter_mut()
                .find(|saved| saved.path == target.path)
            {
                *saved = omission;
            } else {
                report.omissions.push(omission);
            }
            if validate_request(&input, &report, &mut budget, work, &mut validate)? {
                return Ok((input, report));
            }
        }
        if !changed {
            let (reason, detail) = if rules.is_empty() {
                (
                    NoFitReason::NoRules,
                    "The complete request exceeds the local model budget. No input fitting rules permit reductions.",
                )
            } else if expansion.matched == 0 {
                (
                    NoFitReason::NoMatchingTargets,
                    "The complete request exceeds the local model budget. The selected fit paths do not match present values.",
                )
            } else if report.omissions.is_empty() {
                (
                    NoFitReason::TargetsAtMinimum,
                    "The complete request exceeds the local model budget. Selected targets are at their retention limits or cannot become smaller after marking omissions.",
                )
            } else {
                (
                    NoFitReason::ReductionsExhausted,
                    "The complete request still exceeds the local model budget after all permitted reductions.",
                )
            };
            return Err(PreparationFailure {
                code: PreparationFailureCode::NoFit,
                detail: detail.into(),
                report: Some(report),
                context: PreparationFailureContext {
                    field: Some(if rules.is_empty() { "input" } else { "fit" }.into()),
                    input_path: (rules.len() == 1).then(|| rules[0].path.clone()),
                    no_fit_reason: Some(reason),
                    budget,
                    ..Default::default()
                }
                .into(),
            });
        }
    }
}

fn validate_request(
    input: &Value,
    report: &FitReport,
    budget: &mut Option<PreparationBudget>,
    work: &mut FittingWorkBudget,
    validate: &mut impl FnMut(
        &Value,
        &FitReport,
        &mut FittingWorkBudget,
    ) -> Result<(), FitValidationError>,
) -> Result<bool, PreparationFailure> {
    match validate(input, report, work) {
        Ok(()) => Ok(true),
        Err(FitValidationError::OverBudget(estimate)) => {
            *budget = Some(estimate);
            Ok(false)
        }
        Err(FitValidationError::ResourceLimit(budget)) => {
            let mut error = limits::resource_limit(budget);
            error.report = Some(report.clone());
            Err(error)
        }
        Err(FitValidationError::Unavailable(detail)) => Err(PreparationFailure {
            code: PreparationFailureCode::ProviderUnavailable,
            detail,
            report: Some(report.clone()),
            context: Default::default(),
        }),
        Err(FitValidationError::InvalidRequest { detail, context }) => Err(PreparationFailure {
            code: PreparationFailureCode::InvalidRequest,
            detail,
            context,
            report: Some(report.clone()),
        }),
    }
}

fn invalid_fit(detail: &str, path: &str) -> PreparationFailure {
    PreparationFailure {
        code: PreparationFailureCode::InvalidFit,
        detail: detail.into(),
        report: None,
        context: PreparationFailureContext {
            field: Some("fit".into()),
            input_path: Some(path.into()),
            ..Default::default()
        }
        .into(),
    }
}

fn expand(
    value: &Value,
    segments: &[FitPathSegment],
    pointer: &str,
    path: &str,
    strategy: FitStrategy,
    expansion: &mut Expansion,
) -> Result<(), PreparationFailure> {
    expansion.visited += 1;
    limits::check_count(
        PreparationBudgetKind::TraversalNodes,
        expansion.visited,
        MAX_FIT_TRAVERSAL_NODES,
    )?;
    let Some((segment, rest)) = segments.split_first() else {
        let count = match (strategy, value) {
            (FitStrategy::Recent, Value::Array(items)) => items.len(),
            (FitStrategy::Truncate, Value::String(text)) => text.len(),
            _ => {
                return Err(invalid_fit(
                    &format!("Fit target `{path}` has an incompatible value type."),
                    path,
                ));
            }
        };
        expansion.matched += 1;
        limits::check_count(
            PreparationBudgetKind::ExpandedTargets,
            expansion.matched,
            MAX_FIT_TARGETS,
        )?;
        if count
            <= match strategy {
                FitStrategy::Recent => 1,
                FitStrategy::Truncate => MINIMUM_TEXT_BYTES,
            }
        {
            return Ok(());
        }
        expansion.targets.push(Target {
            pointer: pointer.into(),
            path: path.into(),
            strategy,
            original_count: count,
            retained_count: count,
            original_text: None,
            exhausted: false,
        });
        return Ok(());
    };
    match (segment, value) {
        (FitPathSegment::Field(field), Value::Object(object)) => {
            if let Some(value) = object.get(field) {
                let escaped = field.replace('~', "~0").replace('/', "~1");
                let path = if path.is_empty() {
                    field.clone()
                } else {
                    format!("{path}.{field}")
                };
                expand(
                    value,
                    rest,
                    &format!("{pointer}/{escaped}"),
                    &path,
                    strategy,
                    expansion,
                )?;
            }
        }
        (FitPathSegment::Each, Value::Array(items)) => {
            for (index, value) in items.iter().enumerate() {
                expand(
                    value,
                    rest,
                    &format!("{pointer}/{index}"),
                    &format!("{path}[{index}]"),
                    strategy,
                    expansion,
                )?;
            }
        }
        _ => {
            return Err(invalid_fit(
                &format!("Fit path `{path}` traverses an incompatible value type."),
                path,
            ));
        }
    }
    Ok(())
}

fn reduce(input: &mut Value, target: &mut Target) -> bool {
    let value = input
        .pointer_mut(&target.pointer)
        .expect("expanded nonoverlapping target remains present");
    match target.strategy {
        FitStrategy::Recent => {
            let items = value
                .as_array_mut()
                .expect("expanded recent target is an array");
            if items.len() <= 1 {
                target.exhausted = true;
                return false;
            }
            items.remove(0);
            target.retained_count -= 1;
            target.exhausted = items.len() == 1;
        }
        FitStrategy::Truncate => {
            if target.retained_count <= MINIMUM_TEXT_BYTES {
                target.exhausted = true;
                return false;
            }
            let original = target.original_text.get_or_insert_with(|| {
                value
                    .as_str()
                    .expect("expanded truncate target has text")
                    .to_owned()
            });
            let keep = target
                .retained_count
                .saturating_sub(TRUNCATE_CHUNK_BYTES)
                .max(MINIMUM_TEXT_BYTES);
            let mut head = keep.div_ceil(2);
            while !original.is_char_boundary(head) {
                head += 1;
            }
            let mut tail = original.len() - keep / 2;
            while !original.is_char_boundary(tail) {
                tail -= 1;
            }
            let retained = head + original.len() - tail;
            if retained >= target.retained_count {
                target.exhausted = true;
                return false;
            }
            if target.retained_count == target.original_count
                && serde_json::to_string(&original[head..tail])
                    .expect("UTF-8 text serializes")
                    .len()
                    <= serde_json::to_string(OMISSION_MARKER)
                        .expect("omission marker serializes")
                        .len()
            {
                target.exhausted = true;
                return false;
            }
            *value = Value::String(format!(
                "{}{OMISSION_MARKER}{}",
                &original[..head],
                &original[tail..]
            ));
            target.retained_count = retained;
            target.exhausted = keep == MINIMUM_TEXT_BYTES;
        }
    }
    true
}

#[cfg(test)]
#[path = "fit_tests.rs"]
mod tests;

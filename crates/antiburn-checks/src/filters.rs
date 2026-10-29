//! Pure filters for facts captured at the selected event's own scope.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum aggregate byte-product work for one event's include and veto patterns.
pub const MAX_PATH_FILTER_WORK: usize = 1_000_000;

/// The recorded scope used for model and mode attribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttributionScope {
    Message,
    Turn,
    Session,
}

/// One event filter field that prevented selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterField {
    Model,
    FastMode,
    Paths,
    PathsIgnore,
}

/// Why captured evidence did not satisfy a filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterReason {
    Unavailable,
    Incomplete,
    Invalid,
    KnownEmpty,
    NoMatch,
    IgnoredPath,
    LimitExceeded,
}

/// Bounded selector diagnostic retained with branch progress.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterDiagnostic {
    pub field: FilterField,
    pub reason: FilterReason,
    pub scope: AttributionScope,
    /// Source-specific limits captured beside the selected facts.
    pub limits: Vec<String>,
}

/// Checks the exact provider/model syntax without changing case or namespaces.
#[must_use]
pub fn is_model_identifier(value: &str) -> bool {
    value.contains('/')
        && value.split('/').all(|segment| !segment.is_empty())
        && !value.chars().any(char::is_whitespace)
}

/// Applies common filters to exactly the supplied scope. It does not read other roots.
pub fn evaluate_attribution_filters(
    models: Option<&[String]>,
    fast_mode: Option<bool>,
    scope: AttributionScope,
    value: &Value,
) -> Result<(), FilterDiagnostic> {
    if let Some(models) = models {
        let model = value.get("model").and_then(Value::as_str);
        let reason = match model {
            None => Some(FilterReason::Unavailable),
            Some(model) if !is_model_identifier(model) => Some(FilterReason::Invalid),
            Some(model) if !models.iter().any(|expected| expected == model) => {
                Some(FilterReason::NoMatch)
            }
            Some(_) => None,
        };
        if let Some(reason) = reason {
            return Err(FilterDiagnostic {
                field: FilterField::Model,
                reason,
                scope,
                limits: recorded_limit(value, "model_limit"),
            });
        }
    }
    if let Some(expected) = fast_mode {
        let reason = match value.get("fast_mode").and_then(Value::as_bool) {
            None => Some(FilterReason::Unavailable),
            Some(mode) if mode != expected => Some(FilterReason::NoMatch),
            Some(_) => None,
        };
        if let Some(reason) = reason {
            return Err(FilterDiagnostic {
                field: FilterField::FastMode,
                reason,
                scope,
                limits: recorded_limit(value, "fast_mode_limit"),
            });
        }
    }
    Ok(())
}

fn recorded_limit(value: &Value, field: &str) -> Vec<String> {
    value
        .get("quality")
        .and_then(|quality| quality.get(field))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .into_iter()
        .collect()
}

/// A validated repository-relative glob with only segment wildcards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryGlob(String);

impl RepositoryGlob {
    /// Rejects unsupported syntax before a definition can activate.
    pub fn parse(pattern: &str) -> Result<Self, &'static str> {
        if !is_repository_path(pattern)
            || pattern.contains(['[', ']', '{', '}'])
            || ["@(", "!(", "?(", "*(", "+("]
                .iter()
                .any(|syntax| pattern.contains(syntax))
            || pattern
                .split('/')
                .any(|segment| segment.contains("**") && segment != "**")
        {
            return Err(
                "Use a repository-relative pattern with / separators, *, ?, and whole ** segments.",
            );
        }
        Ok(Self(pattern.to_owned()))
    }

    /// Returns the authored pattern without normalization.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Matches an entire recorded path. It never reads the filesystem or command text.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        if !is_repository_path(path) {
            return false;
        }
        let path: Vec<_> = path.split('/').collect();
        let mut reachable = vec![false; path.len() + 1];
        reachable[0] = true;
        for pattern in self.0.split('/') {
            if pattern == "**" {
                for index in 1..reachable.len() {
                    reachable[index] |= reachable[index - 1];
                }
            } else {
                for index in (1..reachable.len()).rev() {
                    reachable[index] =
                        reachable[index - 1] && segment_matches(pattern, path[index - 1]);
                }
                reachable[0] = false;
            }
        }
        reachable[path.len()]
    }
}

/// Tests the normalized repository-relative path contract, not wildcard syntax.
#[must_use]
pub fn is_repository_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', '\0'])
        && !value.as_bytes().get(1).is_some_and(|byte| *byte == b':')
        && value
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
}

fn segment_matches(pattern: &str, value: &str) -> bool {
    let value: Vec<_> = value.chars().collect();
    let mut reachable = vec![false; value.len() + 1];
    reachable[0] = true;
    for token in pattern.chars() {
        if token == '*' {
            for index in 1..reachable.len() {
                reachable[index] |= reachable[index - 1];
            }
        } else {
            for index in (1..reachable.len()).rev() {
                reachable[index] =
                    reachable[index - 1] && (token == '?' || token == value[index - 1]);
            }
            reachable[0] = false;
        }
    }
    reachable[value.len()]
}

/// Applies include and veto filters to known recorded paths.
/// A partial set can prove an include or veto, but cannot prove a veto absent.
pub fn evaluate_path_filters(
    includes: Option<&[RepositoryGlob]>,
    ignores: Option<&[RepositoryGlob]>,
    message: &Value,
) -> Result<(), FilterDiagnostic> {
    if includes.is_none() && ignores.is_none() {
        return Ok(());
    }
    let fail = |field, reason| {
        Err(FilterDiagnostic {
            field,
            reason,
            scope: AttributionScope::Message,
            limits: message
                .pointer("/quality/paths_limits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        })
    };
    let field = if ignores.is_some() {
        FilterField::PathsIgnore
    } else {
        FilterField::Paths
    };
    let Some(paths) = message.pointer("/tool/paths").and_then(Value::as_array) else {
        return fail(field, FilterReason::Unavailable);
    };
    let Some(paths) = paths
        .iter()
        .map(|path| path.as_str().filter(|path| is_repository_path(path)))
        .collect::<Option<Vec<_>>>()
    else {
        return fail(field, FilterReason::Invalid);
    };
    let complete = message
        .pointer("/quality/paths_complete")
        .and_then(Value::as_bool)
        == Some(true);
    let mut work_left = MAX_PATH_FILTER_WORK;
    if let Some(ignores) = ignores {
        let Some(matches) = any_path_matches(ignores, &paths, &mut work_left) else {
            return fail(FilterField::PathsIgnore, FilterReason::LimitExceeded);
        };
        if matches {
            return fail(FilterField::PathsIgnore, FilterReason::IgnoredPath);
        }
        if !complete {
            return fail(FilterField::PathsIgnore, FilterReason::Incomplete);
        }
    }
    if let Some(includes) = includes {
        let Some(matches) = any_path_matches(includes, &paths, &mut work_left) else {
            return fail(FilterField::Paths, FilterReason::LimitExceeded);
        };
        if !matches {
            return fail(
                FilterField::Paths,
                if !complete {
                    FilterReason::Incomplete
                } else if paths.is_empty() {
                    FilterReason::KnownEmpty
                } else {
                    FilterReason::NoMatch
                },
            );
        }
    }
    Ok(())
}

fn any_path_matches(
    globs: &[RepositoryGlob],
    paths: &[&str],
    work_left: &mut usize,
) -> Option<bool> {
    for path in paths {
        for glob in globs {
            let cost = glob.as_str().len().checked_mul(path.len())?;
            *work_left = work_left.checked_sub(cost)?;
            if glob.matches(path) {
                return Some(true);
            }
        }
    }
    Some(false)
}

//! Pure resolution of immediate global and fixed-project check files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::compiler::{CompiledCheck, compile};
use crate::diagnostics::{Diagnostic, DiagnosticCode};

/// Directories used to classify already enumerated check files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryConfig {
    /// The global `~/.antiburn/checks` directory, when known.
    pub global_checks_directory: Option<PathBuf>,
    /// The fixed project root. Discovery uses only its `.antiburn/checks` child.
    pub project_root: Option<PathBuf>,
}

/// One file supplied by an outer filesystem boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckFile {
    /// The file path as enumerated by the caller.
    pub path: PathBuf,
    /// Complete UTF-8 content or a safe read failure from the filesystem boundary.
    pub content: Result<String, Diagnostic>,
    /// Whether the definition is enabled. A disabled project override remains selected.
    pub enabled: bool,
}

/// The installation scope of a selected definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DefinitionScope {
    /// A file under the global check directory.
    Global,
    /// A file under the fixed project check directory.
    Project,
}

/// A selected definition state after override and collision resolution.
#[derive(Clone, Debug, PartialEq)]
pub enum DefinitionState {
    /// The file compiled to a supported definition.
    Ready(Box<CompiledCheck>),
    /// The file or same-scope ID group is invalid.
    Invalid(Vec<Diagnostic>),
    /// The selected definition is paused. Any configuration diagnostics remain visible.
    Paused(Vec<Diagnostic>),
}

/// One effective check definition after project override resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectiveDefinition {
    /// The lowercase filename-stem identity.
    pub id: String,
    /// The scope that supplied the selected definition.
    pub scope: DefinitionScope,
    /// The selected path. Collisions list every matching path here.
    pub source_paths: Vec<PathBuf>,
    /// SHA-256 of the selected file content, or of sorted collision contents.
    pub content_revision: [u8; 32],
    /// The selected definition state.
    pub state: DefinitionState,
}

/// Results from pure directory-entry filtering and definition resolution.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiscoveryResult {
    /// Effective definitions in lowercase ID order.
    pub definitions: Vec<EffectiveDefinition>,
    /// File, collision, and shadowing diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Filters immediate files, compiles definitions, and resolves project overrides.
///
/// This function performs no directory reads, file watching, path canonicalization,
/// or desktop integration. `files` is an outer boundary's file listing; entries
/// qualify only when their immediate parent exactly matches one configured check
/// directory. A project file replaces a global definition with the same ID even
/// when the project file is invalid or disabled. Same-scope duplicate stems
/// invalidate that ID.
#[must_use]
pub fn discover_definitions(config: &DiscoveryConfig, files: &[CheckFile]) -> DiscoveryResult {
    let global_dir = config.global_checks_directory.as_deref();
    let project_dir = config
        .project_root
        .as_deref()
        .map(|root| root.join(".antiburn").join("checks"));
    let mut grouped: BTreeMap<(DefinitionScope, String), Vec<ParsedCandidate>> = BTreeMap::new();
    let mut diagnostics = Vec::new();

    for file in files {
        let scope =
            if global_dir.is_some_and(|directory| is_immediate_yaml_child(directory, &file.path)) {
                Some(DefinitionScope::Global)
            } else if project_dir
                .as_deref()
                .is_some_and(|directory| is_immediate_yaml_child(directory, &file.path))
            {
                Some(DefinitionScope::Project)
            } else {
                None
            };
        let Some(scope) = scope else {
            continue;
        };
        let Some(id) = check_id(&file.path) else {
            diagnostics.push(
                Diagnostic::new(
                    DiagnosticCode::DiscoveryInvalidId,
                    "Check filename stem must match [a-z][a-z0-9-]*.",
                )
                .with_source_path(file.path.clone()),
            );
            continue;
        };
        let result = match &file.content {
            Ok(content) => compile(content).map_err(|errors| {
                errors
                    .into_iter()
                    .map(|error| error.with_source_path(file.path.clone()))
                    .collect::<Vec<_>>()
            }),
            Err(error) => Err(vec![error.clone().with_source_path(file.path.clone())]),
        };
        grouped
            .entry((scope, id))
            .or_default()
            .push(ParsedCandidate {
                file: file.clone(),
                result,
            });
    }

    let mut global = BTreeMap::new();
    let mut project = BTreeMap::new();
    for ((scope, id), candidates) in grouped {
        let definition = resolve_scope_group(&id, scope, candidates, &mut diagnostics);
        match scope {
            DefinitionScope::Global => {
                global.insert(id, definition);
            }
            DefinitionScope::Project => {
                project.insert(id, definition);
            }
        }
    }

    for id in project.keys() {
        if let Some(shadowed) = global.get(id) {
            for path in &shadowed.source_paths {
                diagnostics.push(
                    Diagnostic::new(
                        DiagnosticCode::DiscoveryShadowed,
                        format!("Global check `{id}` is shadowed by a project definition."),
                    )
                    .with_source_path(path.clone()),
                );
            }
        }
    }

    let mut definitions = global;
    definitions.extend(project);
    DiscoveryResult {
        definitions: definitions.into_values().collect(),
        diagnostics,
    }
}

struct ParsedCandidate {
    file: CheckFile,
    result: Result<CompiledCheck, Vec<Diagnostic>>,
}

fn resolve_scope_group(
    id: &str,
    scope: DefinitionScope,
    mut candidates: Vec<ParsedCandidate>,
    diagnostics: &mut Vec<Diagnostic>,
) -> EffectiveDefinition {
    candidates.sort_by(|left, right| left.file.path.cmp(&right.file.path));
    let source_paths = candidates
        .iter()
        .map(|candidate| candidate.file.path.clone())
        .collect::<Vec<_>>();
    let revision = content_revision(&candidates);
    if candidates.len() > 1 {
        let duplicate_diagnostic = Diagnostic::new(
            DiagnosticCode::DiscoveryDuplicateId,
            format!(
                "Multiple {} files define check ID `{id}`.",
                scope_label(scope)
            ),
        );
        for path in &source_paths {
            diagnostics.push(duplicate_diagnostic.clone().with_source_path(path.clone()));
        }
        return EffectiveDefinition {
            id: id.to_owned(),
            scope,
            source_paths,
            content_revision: revision,
            state: DefinitionState::Invalid(vec![duplicate_diagnostic]),
        };
    }

    let candidate = candidates.pop().expect("a scope group has a file");
    let file_path = candidate.file.path.clone();
    let (compiled, mut errors) = match candidate.result {
        Ok(compiled) => (Some(compiled), Vec::new()),
        Err(errors) => (None, errors),
    };
    for error in &mut errors {
        error.source_path = Some(file_path.clone());
    }
    let state = if !candidate.file.enabled {
        diagnostics.extend(errors.iter().cloned());
        diagnostics.push(
            Diagnostic::new(
                DiagnosticCode::DiscoveryPaused,
                format!("Selected check `{id}` is paused."),
            )
            .with_source_path(file_path),
        );
        DefinitionState::Paused(errors)
    } else if let Some(compiled) = compiled {
        DefinitionState::Ready(Box::new(compiled))
    } else {
        diagnostics.extend(errors.iter().cloned());
        DefinitionState::Invalid(errors)
    };
    EffectiveDefinition {
        id: id.to_owned(),
        scope,
        source_paths,
        content_revision: revision,
        state,
    }
}

fn is_immediate_yaml_child(directory: &Path, path: &Path) -> bool {
    path.parent() == Some(directory)
        && matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("yml" | "yaml")
        )
}

fn check_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let mut bytes = stem.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        .then_some(())?;
    if !bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-') {
        return None;
    }
    Some(stem.to_owned())
}

fn content_revision(candidates: &[ParsedCandidate]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for candidate in candidates {
        match &candidate.file.content {
            Ok(content) => hasher.update(content.as_bytes()),
            Err(error) => {
                hasher.update([0xff]);
                hasher.update(error.code.as_str().as_bytes());
                hasher.update(error.message.as_bytes());
            }
        }
        hasher.update([0]);
    }
    hasher.finalize().into()
}

const fn scope_label(scope: DefinitionScope) -> &'static str {
    match scope {
        DefinitionScope::Global => "global",
        DefinitionScope::Project => "project",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_check(name: &str) -> String {
        format!(
            "name: {name}\non:\n  tool_result:\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input:\n      activity: ${{{{ message.tool.result.status }}}}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria:\n          yes: Useful.\n          no: Not useful.\nactions:\n  - type: status\n    input:\n      result: failure\n"
        )
    }

    fn file(path: &str, content: &str, enabled: bool) -> CheckFile {
        CheckFile {
            path: PathBuf::from(path),
            content: Ok(content.to_owned()),
            enabled,
        }
    }

    #[test]
    fn project_definition_replaces_global_even_when_invalid() {
        let config = DiscoveryConfig {
            global_checks_directory: Some(PathBuf::from("/home/user/.antiburn/checks")),
            project_root: Some(PathBuf::from("/work/project")),
        };
        let result = discover_definitions(
            &config,
            &[
                file(
                    "/home/user/.antiburn/checks/review.yml",
                    &valid_check("Global"),
                    true,
                ),
                file(
                    "/work/project/.antiburn/checks/review.yaml",
                    "name: Invalid\non: {turn_end: null}\nactions: []\n",
                    true,
                ),
            ],
        );
        assert_eq!(result.definitions.len(), 1);
        assert_eq!(result.definitions[0].scope, DefinitionScope::Project);
        assert!(matches!(
            result.definitions[0].state,
            DefinitionState::Invalid(_)
        ));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::DiscoveryShadowed)
        );
    }

    #[test]
    fn duplicate_extensions_in_one_scope_invalidate_one_id() {
        let config = DiscoveryConfig {
            global_checks_directory: Some(PathBuf::from("/checks")),
            project_root: None,
        };
        let result = discover_definitions(
            &config,
            &[
                file("/checks/review.yml", &valid_check("First"), true),
                file("/checks/review.yaml", &valid_check("Second"), true),
            ],
        );
        assert_eq!(result.definitions.len(), 1);
        assert_eq!(result.definitions[0].source_paths.len(), 2);
        assert!(matches!(
            result.definitions[0].state,
            DefinitionState::Invalid(_)
        ));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::DiscoveryDuplicateId)
        );
    }

    #[test]
    fn resolver_ignores_nested_and_unconfigured_files() {
        let config = DiscoveryConfig {
            global_checks_directory: Some(PathBuf::from("/checks")),
            project_root: None,
        };
        let result = discover_definitions(
            &config,
            &[
                file("/checks/nested/review.yml", &valid_check("Nested"), true),
                file("/elsewhere/review.yml", &valid_check("Elsewhere"), true),
                file("/checks/Bad_Name.yml", &valid_check("Bad"), true),
            ],
        );
        assert!(result.definitions.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].code,
            DiagnosticCode::DiscoveryInvalidId
        );
    }

    #[test]
    fn unreadable_project_override_remains_selected_and_invalid() {
        let config = DiscoveryConfig {
            global_checks_directory: Some(PathBuf::from("/checks")),
            project_root: Some(PathBuf::from("/project")),
        };
        let mut unreadable = file("/project/.antiburn/checks/review.yml", "", true);
        unreadable.content = Err(Diagnostic::new(
            DiagnosticCode::DiscoveryReadFailed,
            "Check file is not valid UTF-8.",
        ));
        let result = discover_definitions(
            &config,
            &[
                file("/checks/review.yml", &valid_check("Global"), true),
                unreadable,
            ],
        );
        assert_eq!(result.definitions.len(), 1);
        assert_eq!(result.definitions[0].scope, DefinitionScope::Project);
        assert!(
            matches!(&result.definitions[0].state, DefinitionState::Invalid(errors)
            if errors[0].code == DiagnosticCode::DiscoveryReadFailed)
        );
    }

    #[test]
    fn paused_files_still_report_configuration_errors_without_activation() {
        let config = DiscoveryConfig {
            global_checks_directory: Some(PathBuf::from("/checks")),
            project_root: None,
        };
        let result =
            discover_definitions(&config, &[file("/checks/review.yml", "name: [\n", false)]);
        assert!(
            matches!(&result.definitions[0].state, DefinitionState::Paused(errors) if !errors.is_empty())
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|error| error.code == DiagnosticCode::YamlSyntax)
        );
    }
}

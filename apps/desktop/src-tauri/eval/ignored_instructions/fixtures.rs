use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::jev_evidence::JevOperationState;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Expected {
    pub(crate) verdict: String,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) actions: Vec<usize>,
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Shape {
    #[default]
    Plain,
    LongHistory,
    LongText,
    Truncated,
    MissingHistory,
    SiblingHistory,
    CurrentFile,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Scenario {
    pub(crate) id: String,
    pub(crate) suite: String,
    pub(crate) family: String,
    pub(crate) instruction: String,
    pub(crate) events: Vec<Event>,
    pub(crate) expected: Expected,
    #[serde(default)]
    pub(crate) shape: Shape,
    #[serde(default)]
    pub(crate) authority_control: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Assistant,
    User,
    Bash,
    BashOutput,
    Read,
    ReadOutput,
    Search,
    SearchOutput,
    Other,
    OtherOutput,
    EditPath,
    EditContent,
    Patch,
    Rename,
    Unknown,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum Event {
    Text((Kind, String)),
    Result((Kind, String, usize, JevOperationState)),
}

impl Event {
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::Text((kind, _)) | Self::Result((kind, _, _, _)) => *kind,
        }
    }
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Text((_, text)) | Self::Result((_, text, _, _)) => text,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Case {
    pub(crate) id: String,
    pub(crate) agent: &'static str,
    pub(crate) source_format: SourceFormat,
    pub(crate) scenario: Scenario,
}

const AGENTS: [(&str, SourceFormat); 4] = [
    ("opencode", SourceFormat::OpenCodeSqliteV2),
    ("codex", SourceFormat::CodexRolloutJsonl),
    ("claude", SourceFormat::ClaudeJsonl),
    ("pi", SourceFormat::PiV3Jsonl),
];

pub(crate) fn select(suite: &str, selected: Option<&str>) -> Result<Vec<Case>, String> {
    let suite = match suite {
        "development" => "core",
        "controls" => "limits",
        other => other,
    };
    let scenarios: Vec<Scenario> =
        serde_json::from_str(include_str!("data/cases.json")).map_err(|error| error.to_string())?;
    if suite != "all" && !scenarios.iter().any(|case| case.suite == suite) {
        return Err(format!("unknown suite: {suite}"));
    }
    let cases = scenarios
        .into_iter()
        .filter(|case| suite == "all" || case.suite == suite)
        .flat_map(|scenario| {
            AGENTS.into_iter().map(move |(agent, source_format)| Case {
                id: format!("{}:{agent}", scenario.id),
                agent,
                source_format,
                scenario: scenario.clone(),
            })
        })
        .filter(|case| selected.is_none_or(|id| case.id == id || case.scenario.id == id))
        .collect::<Vec<_>>();
    if cases.is_empty() {
        return Err(format!("no cases match suite={suite} case={selected:?}"));
    }
    Ok(cases)
}

pub(crate) fn cases(suite: &str) -> Vec<Case> {
    select(suite, None).expect("valid instruction diagnostic suite")
}

pub(crate) fn inventory(case: &Case) -> Value {
    json!({"id":case.id,"agent":case.agent,"suite":case.scenario.suite,"family":case.scenario.family,"source_format":format!("{:?}",case.source_format),
        "expected":case.scenario.expected.verdict,"expected_reason":case.scenario.expected.reason,
        "expected_references":crate::evidence::expected_references(case),"authority_control":case.scenario.authority_control})
}

#[test]
fn inventory_has_unique_labels_references_and_four_agents() {
    let cases = select("all", None).expect("case inventory");
    let mut ids = std::collections::BTreeSet::new();
    for case in &cases {
        assert!(ids.insert(&case.id));
        assert!(matches!(
            case.scenario.expected.verdict.as_str(),
            "finding" | "no_finding" | "pending" | "unassessed"
        ));
        assert_eq!(
            case.scenario.expected.verdict == "finding",
            !case.scenario.expected.actions.is_empty()
        );
        assert_eq!(
            case.scenario.expected.verdict == "finding",
            case.scenario.expected.reason.as_deref() == Some("ignored_instruction_violation")
        );
        for index in &case.scenario.expected.actions {
            assert!(*index < case.scenario.events.len());
        }
        if case.scenario.authority_control {
            assert_eq!(case.scenario.expected.verdict, "unassessed");
        }
        for (index, event) in case.scenario.events.iter().enumerate() {
            if let Event::Result((kind, _, request, _)) = event {
                assert_eq!(*kind, Kind::BashOutput);
                assert!(*request < index);
                assert_eq!(case.scenario.events[*request].kind(), Kind::Bash);
            }
        }
    }
    for suite in ["core", "context", "limits"] {
        let selected = select(suite, None).expect("suite");
        assert_eq!(
            selected
                .iter()
                .map(|case| case.agent)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            4
        );
    }
}

#[test]
fn selection_is_repeatable_and_rejects_mistakes() {
    assert_eq!(
        select("context", Some("failure-marker-missing"))
            .expect("scenario")
            .len(),
        4
    );
    assert_eq!(
        select("core", Some("command-ban:pi"))
            .expect("agent case")
            .len(),
        1
    );
    assert!(select("unknown", None).is_err());
    assert!(select("core", Some("failure-marker-missing")).is_err());
    assert!(select("all", Some("typo")).is_err());
}

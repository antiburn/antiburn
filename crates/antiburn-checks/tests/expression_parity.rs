use serde::Deserialize;

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    compile_valid: bool,
    source: String,
}

#[test]
fn shared_expression_and_local_corpus_matches_compiler_admission() {
    let corpus: Corpus = serde_json::from_str(include_str!(
        "../../../docs/burn-check/tests/expression-parity.json"
    ))
    .unwrap();
    let mut differences = Vec::new();
    for case in corpus.cases {
        let result = antiburn_checks::compile(&case.source);
        if result.is_ok() != case.compile_valid {
            differences.push(format!(
                "{}: expected {}, got {}: {result:?}",
                case.id,
                case.compile_valid,
                result.is_ok()
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

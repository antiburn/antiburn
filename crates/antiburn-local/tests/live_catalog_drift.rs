//! The antiburn-live mod bundles a hand-maintained catalog in `hooks/catalog.ts`.
//! A mod module cannot import a JSON file, so the module holds the JSON between
//! two marker comments. This test fails when its D, T, O or S policy values
//! differ from `ReportCatalogs::default()`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use antiburn_local::checks::{ModelFamily, REGISTRY_REVISION, ReportCatalogs};
use serde_json::Value;

/// Reads the JSON that the mod module holds between its two marker comments.
fn catalog() -> Value {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = if manifest_dir.join("docs").is_dir() {
        manifest_dir.to_path_buf()
    } else {
        manifest_dir.join("../..")
    };
    let path = root.join("plugins/antiburn-live/hooks/catalog.ts");
    let module = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let (_, rest) = module
        .split_once("/* catalog:begin */")
        .expect("catalog.ts must have the begin marker");
    let (json, _) = rest
        .split_once("/* catalog:end */")
        .expect("catalog.ts must have the end marker");
    serde_json::from_str(json).expect("the catalog between the markers must be valid JSON")
}

fn strings(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .expect("expected an array")
        .iter()
        .map(|item| item.as_str().expect("expected a string").to_owned())
        .collect()
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .expect("expected an object")
        .keys()
        .map(String::as_str)
        .collect()
}

/// The tests below compare a fixed set of policy fields. A new field in the
/// bundled policy would pass them unchecked, so this test lists every field.
#[test]
fn every_bundled_policy_field_has_a_test() {
    let catalog = catalog();
    let policy = &catalog["policy"];
    assert_eq!(
        keys(policy),
        BTreeSet::from(["claude", "depthCapTokens", "modelReplacements"])
    );
    assert_eq!(
        keys(&policy["claude"]),
        BTreeSet::from(["effortAboveCap", "premiumSubstrings"])
    );
}

#[test]
fn revisions_match_the_report_catalog() {
    let catalog = catalog();
    let defaults = ReportCatalogs::default();
    assert_eq!(catalog["reportCatalogRevision"], defaults.revision);
    assert_eq!(catalog["modelRegistryRevision"], REGISTRY_REVISION);
}

#[test]
fn depth_cap_matches_d() {
    let catalog = catalog();
    assert_eq!(
        catalog["policy"]["depthCapTokens"],
        ReportCatalogs::default().depth_cap_tokens
    );
}

#[test]
fn effort_cap_matches_t() {
    let catalog = catalog();
    let defaults = ReportCatalogs::default();
    let claude = &defaults.families[&ModelFamily::Claude];
    assert_eq!(
        strings(&catalog["policy"]["claude"]["effortAboveCap"]),
        claude.effort.above_cap
    );
}

#[test]
fn premium_tier_matches_s() {
    let catalog = catalog();
    let defaults = ReportCatalogs::default();
    let premium = &defaults.families[&ModelFamily::Claude].premium;
    assert!(premium.prefixes.is_empty() && premium.exceptions.is_empty());
    assert_eq!(
        strings(&catalog["policy"]["claude"]["premiumSubstrings"]),
        premium.substrings.iter().cloned().collect::<BTreeSet<_>>()
    );
}

#[test]
fn model_replacements_match_o() {
    let catalog = catalog();
    let bundled: BTreeMap<String, String> = catalog["policy"]["modelReplacements"]
        .as_object()
        .expect("modelReplacements must be an object")
        .iter()
        .map(|(model, target)| {
            let target = target.as_str().expect("a replacement must be a string");
            (model.clone(), target.to_owned())
        })
        .collect();
    let expected: BTreeMap<String, String> = ReportCatalogs::default()
        .model_replacements
        .entries
        .into_iter()
        .map(|(model, entry)| (model, entry.replacement))
        .collect();
    assert_eq!(bundled, expected);
}

# Ignored Instructions evaluations

This directory owns the desktop crate's synthetic evaluation runner, support code,
and active development and regression JSON data. The independent confirmation
wrapper and its native projection tests are part of the same Cargo target. Its
sealed fixture stays at its existing path and is not part of the development or
regression inputs.

Every case has one semantic outcome label. `bindings.json` keeps exact production
rule/action IDs separate from semantic source references. A finding with
`resolution: "pending"` is not ready for live execution. Empty bindings are
explicit and valid only for a reviewed non-finding, pending, or unassessed label.

The development, regression, and sealed confirmation cohorts have separate
purposes. Focused-labelled cases are part of the development cohort and its
scheduled denominator. Keep their labels and exact binding sidecars with each
cohort; changes to production binding revisions require an offline review of the
bindings before a live run. The obsolete direct-provider focused diagnostics are
removed. Focused checks use the shared runner and scorer with the authored
development labels. The paid `OnePass` and `Hybrid` comparison is also removed;
development, regression, and confirmation runs use the production check path.

The live evaluation policy in `data/gates.json` requires at least
80% joint outcome-and-exact-binding accuracy, 80% observable binding recall,
80% published binding precision, and complete scheduled results with exact
binding labels. The joint thresholds are 128/159 development cases and 39/48
independent confirmation cases. Counts use scheduled cases, observable expected
bindings, and published bindings as their respective denominators; a missing
result is a non-pass, and a zero-denominator metric is unavailable. Publication
confidence thresholds remain 0.85 for possible and 0.90 for likely findings.
Development evidence alone does not establish independent acceptance. Offline
synthetic tests cannot validate live model quality, sampling recall, or elapsed
time with TypeSafe. Evaluate the default sample of 256 high-priority
rule/action pairs per review on mixed rules, low-overlap conflicts, long text,
and appended activity. Check that the next review considers new activity first,
then older pairs not yet sampled, and that the remaining gap falls without new
work. Compare exact source bindings and reuse after completion, append, and
restart. Report missed findings, sampled coverage, paid requests, tokens,
checkpoint cost, and worker-start-to-result time separately. The
ordinary-session goal of about 60 seconds after worker start is not a test
cutoff, a spending cap, or a guarantee. Provider and evidence failures must
remain distinct from a completed sample with no findings.

Run offline harness checks with:

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --test ignored_instructions
```

Live tests remain ignored by default. They require explicit authorization and
write unique captures under `.agent-artifacts/reviews/`.

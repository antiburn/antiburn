# API support notes

The desktop application is not the complete usage boundary for this crate.
Release archives support external embeddings that pin an engine release tag.
Repository-wide caller searches therefore do not prove that a public item is
dead.

## Retained compatibility APIs

- The batch analysis path (`analyze_sources`, `analyze_sources_with`,
  `normalize_source`, `NormalizedSession`, `SessionCollector`, and
  `analyze_session`) remains supported. The changelog introduced the streaming
  interface as a way to rebuild the same batch result, and engine integration
  tests and benchmarks still use both paths for characterization and parity.
- The repository orchestration exports remain supported. The 0.5.0 changelog
  explicitly retains `resolve_granted_repos` and `scan_roots_for_repos` as
  embedding building blocks. The repository module documents the matching,
  progress, root-derivation, and session-working-directory contracts used to
  assemble an external discovery pipeline.
- `SessionMetrics.context_available` remains part of the serialized metrics
  contract. Aggregate analysis uses it to distinguish an empty cohort from a
  cohort with context metrics, and parity tests compare the field across the
  live and row-replay paths.
- `SessionMetrics.context_window_source` remains part of the serialized
  metrics contract. Version 0.6.0 added it as diagnostic evidence and records
  the addition as a breaking schema change.

Removing or narrowing these items requires a versioned compatibility decision,
not only evidence that the desktop has no current caller.

## Deleted internal work

- Git command execution no longer constructs an unused display-only argument
  vector. Structured tracing already records the sanitized repository path,
  argument slice, and environment variable names without recording values.

# Antiburn CLI project instructions

These instructions apply to `apps/cli`.

## Purpose and boundary

- Keep this standalone Rust app as the user-facing, agent-oriented Antiburn CLI.
- Keep the public invocation `antiburn checks <session_id>` and machine-readable JSON on stdout.
- Keep the reusable Antiburn skill harness-neutral in `skills/antiburn/SKILL.md`. It asks for the active session ID; it does not name a harness environment variable or guess the agent or environment. Treat this as the canonical skill source; package helpers may copy it but must not author a separate version.
- Read the Antiburn product database in read-only mode. Do not run migrations or write to the product database.
- Read `session_evidence.evidence_json` and legacy `burn_check_assessment` rows separately. An absent legacy table is normal after retirement; a present malformed table or row remains an error. Keep raw evidence separate from persisted assessment rows and preserve their source labels. Omit raw evidence from stdout.
- `session_evidence.evidence_json` stores facts, not calculated findings. Use `antiburn-local`'s public `insights::session_badges_with_selection` API to derive its six session badges; do not duplicate evaluator rules or label raw evidence facts as results.
- Keep the CLI independent from the Tauri desktop crate. It may depend on `antiburn-local` as a Rust library. The engine does not expose a product-database reader, so keep the CLI's product-table queries modular and read-only unless a shared reader is added by an explicit task.
- The `session_id` argument is the CLI's source of truth. Do not read a harness-specific session environment variable. Resolve agent and environment from the product database and fail closed when the ID is missing or ambiguous.
- The CLI does not evaluate the desktop's full currentness rules. Keep `freshness_evaluated` false; do not call a completed or empty result current or clean.
- Track session state by `(environment_key, agent, session_id)` and check state by `(source, check_id)`. Return only first-seen or changed problematic findings; use clean and not-assessed outcomes internally to detect resolution, but never return them.
- The tracing event records the explicit CLI session ID and summary counts only. Never log evidence JSON, check results, prompts, transcripts, credentials, or session content.
- Do not add desktop/Tauri dependencies or duplicate evidence-detector rules here. Keep evidence-check decisions in the desktop and engine. The CLI has one scoped derived exception: the priced-session outlier presentation rule reads cached analysis and applies the desktop threshold with a bounded CLI cohort. Do not expand this exception to other checks.

## CLI and data rules

- Use release data-path defaults from the JSON config module. Keep behavior-affecting defaults in the config file schema and make them user-overridable; do not hard-code alternate debug paths in discovery. The default config path is `dirs::config_dir()/antiburn/cli.json`, and `--config PATH` selects another file. The default state file is `dirs::config_dir()/antiburn/cli-state.json`; allow a config override.
- Keep stdout for JSON results only. Send usage and runtime errors to stderr and return a non-zero exit code. Initialize file-based JSON Lines tracing during startup; if the diagnostic file is unavailable, warn on stderr and continue.
- Keep JSON field names, new-finding filtering, resolution/retrigger behavior, session isolation, and command arguments covered by tests. The `session_price_outlier` source is synthesized from `session_analysis`; it is not a persisted check row.
- Treat harness IDs as untrusted input. Do not infer that an environment variable, hook task ID, thread ID, conversation ID, or database session ID has the same identity without evidence.
- Hook payloads differ by harness. A wrapper must parse the documented event schema and translate Antiburn results into that harness's hook output contract. Do not treat Antiburn's JSON as a universal hook response.
- Keep the state file separate from the product database. Store fingerprints and last-triggered metadata, not raw evidence or full results. Lock and atomically replace the file so concurrent CLI calls do not lose updates.

## Harness research

- Keep `README.md` current when the supported harness inventory or its session-ID transport changes.
- Separate shell environment variables, command-hook stdin JSON, CLI argument payloads, and in-process plugin/extension APIs.
- Link every harness claim to its official documentation or source. Label missing documentation as “not documented in the sources checked,” not as proof the harness never exposes an ID.
- Include the date of the research snapshot and note when schemas or availability may vary by harness version.

## Rust and validation

- This crate is a standalone Cargo workspace with its own `Cargo.lock`; do not add it to the desktop Tauri workspace.
- Keep modules small: argument parsing, database discovery, read-only database access, response models, logging, and process output have separate boundaries.
- Use the Tokio current-thread runtime for async command execution. Keep stdout/stderr async and run synchronous rusqlite work in `spawn_blocking`. The low-volume tracing subscriber is the deliberate synchronous file-writer exception. Keep small CPU-only work such as parsing, validation, JSON serialization, and process metadata synchronous.
- Use `tracing` events for diagnostics; do not append manually formatted records to the trace file.
- Use the strict repository-root `.aislop/config.yml`. Run Aislop against `apps/cli` to check the CLI source. Do not weaken the rules to make this app pass.
- Run these checks from this directory:

  ```sh
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo nextest run
  cargo build
  ```

- From the repository root, also run `pnpm exec aislop ci apps/cli` when validating this app's strict config.
- Use synthetic IDs, typed engine evidence, and temporary SQLite fixtures in tests. Do not read or mutate a real Antiburn database in tests. Do not commit call logs or real session data.
- Write comments in active, present-tense Simplified Technical English. Add comments only for important information the code cannot show.

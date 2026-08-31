# `tests/fixtures`

`tool_catalog.json` regenerates with:

```
node scripts/build-tool-catalog.mjs <systemprompts-checkout> \
  --out crates/antiburn-local/tests/fixtures/tool_catalog.json \
  --versions "claude=2.1.220,2.1.232,2.1.233,2.1.246;codex=0.146.1,0.147.0,0.149.1"
```

It is a small, committed stand-in for the file `scripts/build-tool-catalog.mjs`
compacts from a full `antiburn/systemprompts` checkout (see that script's own
header comment). The real catalogue is not committed: the release workflow
builds it and names it in `ANTIBURN_TOOL_CATALOG`, and `build.rs` embeds
this fixture whenever that variable is not set. Dev builds and tests never
need the checkout.

The four Claude versions span the `task_*` tool family's removal at 2.1.233,
so the fixture exercises a real tool-surface change and not just a
token-count update. `source.commit` in the file is the real
`antiburn/systemprompts` commit the fixture was cut from, not a placeholder.

Regenerate by re-running the command above against a current
`antiburn/systemprompts` checkout, and commit the result. Do not hand-edit
the JSON: it would drift from what the script actually produces.

Each subdirectory below holds its own fixtures with its own README:

| Directory | Fixtures for |
| --- | --- |
| `claude_characterization/` | The Claude JSONL normalization and analysis integration test. |
| `codex_characterization/` | The Codex rollout normalization and analysis integration test. |
| `initial_context/` | The initial-context attribution pass in `src/analysis/initial_context.rs`. |
| `pi_characterization/` | The Pi record normalization and analysis integration test. |

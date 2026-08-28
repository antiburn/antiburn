# Local Insights followups

This document is append-only and not digest-bound. Any seam can append an entry. Each entry records what the seam found and how later work must handle it.

## Cached account limits in quota pressure

- **What was found:** A report-time join could add cached account limits to the quota-pressure section.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** The join needs a separate consent review and is not part of transcript evidence.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` after the consent boundary is defined. **No issue yet:** the consent-boundary question was posted for product discussion; an issue follows only if it gains momentum.

## Session-level hygiene badges

- **What was found:** Existing evidence could support session badges for reasoning overkill, excessive cache rehydration, and bloated initial context.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** A second session-level reducer needs product scope, but it needs no new parsing, evidence, or schema.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` after the report reducer ships. **Filed as antiburn#221.**

## Additional Claude JSONL row types

- **What was found:** Later work can model more row types from the collected unknown-type diagnostic.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** The first release must collect evidence before the parser adds unused record types. A new parser revision can reparse them.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` after release data supports specific row types. **Filed as antiburn#222 and shipped in PR antiburn#231.** The structural best-effort policy is implemented for antiburn#229 but has no PR yet. The three missing Claude capabilities are tracked by antiburn#226.

## Runtime unknown-type discriminators in analytics

- **What was found:** The catalog event for unrecognized records cannot name a type because analytics properties use a closed `&'static str` vocabulary.
- **Found by seam:** antiburn#229.
- **Why deferred:** Sending runtime strings changes the payload type and needs a separate privacy review. Local diagnostics and the Insights note expose the names meanwhile.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` after the first `antiburn.unrecognized_records_observed` data arrives. **No issue yet:** the event must ship before data can justify the privacy review.

## Per-group degradation on record loss

- **What was found:** Record loss degrades every supported top-level group, while only some groups can be affected by a specific lost record.
- **Found by seam:** antiburn#229.
- **Why deferred:** Narrowing the loss changes verdicts for malformed, oversized, and pinned-prefix records and needs a separate test matrix.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` with its own blast-radius analysis. **No issue yet:** the separate verdict change needs product scope first.

## Zero-work sessions in detector denominators

- **What was found:** A session without assistant turns can enter and be assessed by six detectors. Two absence detectors exclude it, and one detector lacks Claude capability.
- **Found by seam:** antiburn#229.
- **Why deferred:** Extending the exclusion changes existing housekeeping-only sessions and needs detector-specific product rules.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` with a detector-by-detector denominator rule. **No issue yet:** the product rule needs detector-specific decisions first.

## End-to-end analytics consent test

- **What was found:** The desktop shell has no Tauri mock harness, so the new event's consent gate is proven at the shared analytics choke point.
- **Found by seam:** antiburn#229.
- **Why deferred:** Enabling Tauri's `test` feature is a separate build and dependency change.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` for a queue-level opt-out test. **No issue yet:** the test requires a separate Tauri feature and dependency review.

## Migration ladder squash

- **What was found:** The migration ladder can be squashed before version 1.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** An appended migration updates `user_version` during branch switches. An edited migration does not update an existing developer database. Release-candidate tags exist through `antiburn-v0.1.0-rc.6`.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` as antiburn#76 after release-candidate distribution is checked. **antiburn#76 remains open** after the repository history squash.

## Slow discovery while the popover is hidden

- **What was found:** Scheduled scanning stops while the popover is hidden, so discovery waits for another trigger.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** A 15-to-30-minute tick would refresh metrics that no reader sees. It would reverse the current pause and require Locked Decision 15 to change. CH-008 drains only work that discovery already queued.
- **Kind:** `enhancement`
- **Disposition:** `fold-into-later-seam` for CH-013 measurement and review. This work is a natural prerequisite for session-level badges. **Folded into antiburn#224**, where the measurement and review now live.

## Second provider

- **What was found:** A second provider can follow the first Claude slice.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** Source Phase 12 is outside GH-70.
- **Kind:** `deferred`
- **Disposition:** `file-issue` after the first provider ships. **Filed as antiburn#227** (Codex).

## Deleted transcript reconciliation

- **What was found:** The session index does not remove a session row when its transcript disappears from disk.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** Evidence cascades only when the session row is deleted. Current deletion paths cover gate rejection, ignored paths, and explicit user deletion, but not a missing file.
- **Kind:** `deferred`
- **Disposition:** `file-issue` with a privacy review before CH-013 completes. **Filed as antiburn#223.**

## Phase 13 optimizations

- **What was found:** Report caching, relational evidence projections, and more read pooling can reduce later costs.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** Each optimization needs measurements from the first provider path.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` only when CH-013 measurements justify a specific optimization. **Folded into antiburn#224**; the measurement ticket resolves it per the numbers.

## Evidence for the Claude append-only guarantee

- **What was found:** `append_only_guarantee()` returns `AppendOnlyGuarantee::Absent` for every agent (`crates/antiburn-local/src/analysis/source_validity.rs:46-48`), and seam 0007 pinned that result for Claude by test (`claude_carries_no_append_only_guarantee` in `crates/antiburn-local/tests/source_validity_timing.rs`). Locked Decision 8 lets no pinned prefix publish without an evidence-backed guarantee, so every Claude source takes the full-reprocess path. A transcript that is written during the read is rejected as `SourceChanged` and publishes neither projection. Thus a session that is actively written shows no numbers until the next trigger.
- **Found by seam:** GH-70 seam 0007, confirmed at the seam 0008 Tier 3 human review.
- **Why deferred:** The guarantee is a property of how the Claude CLI writes its transcript. Evidence must come from repeatable checks against pinned CLI versions. `CONTRIBUTING.md` requires synthetic fixtures, and a repository fixture cannot prove the behavior of a third-party writer. No captured session file may enter this repository.
- **Kind:** `deferred`
- **Disposition:** `file-issue`, gated on CH-013's measurement of impact while a representative Claude session is actively written. If the observed rejection rate is near zero, the work may never be justified. To flip Claude to `Evidenced` is a one-function change and needs no change to the streaming code. **Folded into antiburn#224**; the measurement ticket resolves it per the numbers.

## Fork-job transcript materialization during discovery

- **What was found:** Claude fork-job discovery reads the complete transcript into `SessionSource::Inline` before analysis streams that content (`crates/antiburn-local/src/discovery/agents/claude.rs:603-624`).
- **Found by seam:** GH-70 seam 0008.
- **Why deferred:** The read occurs upstream of the CH-005 analysis boundary. Removing it requires a discovery source contract change.
- **Kind:** `deferred`
- **Disposition:** `file-issue` after CH-013 measures the affected source volume. **Folded into antiburn#224**; the measurement ticket resolves it per the numbers.

## Unsupported evidence publication trigger

- **What was found:** The first production writer of `PublishedEvidence::Unsupported` needs the detector prerequisite sets before it can classify a provider.
- **Found by seam:** GH-70 seam 0016.
- **Why deferred:** CH-010 reads unsupported rows but does not own evidence publication policy. CH-011 shipped the detector prerequisite sets the writer needs but stayed engine-only; the writer lives in the desktop worker.
- **Kind:** `deferred`
- **Disposition:** `fold-into-later-seam` for CH-011b. **Resolved:** CH-011b shipped the writer; this entry is closed.

## Report scope for hosts with WSL sessions

- **What was found:** The report entry point accepts one environment key, so CH-012 must decide whether a host report combines native and WSL scopes.
- **Found by seam:** GH-70 seam 0016.
- **Why deferred:** CH-012 owns the IPC request and the reader-facing scope.
- **Kind:** `enhancement`
- **Disposition:** `fold-into-later-seam` for CH-012. **Decided in CH-012:** one environment key per report; the host report covers the native scope only and never combines native and WSL scopes. The reduction queries are pinned to single-scope semantics and detector statuses cannot be recombined from two finished reports. The decision is documented at the request construction site (`apps/desktop/src-tauri/src/commands.rs`, `insights_report_request`), and the pane's scope wording names the native environment explicitly. Per-environment reports for Windows hosts with WSL sessions are the entry below, filed as antiburn#225.

## Per-environment insights reports on Windows hosts with WSL

- **What was found:** CH-012 scoped the insights report to the native environment. A Windows host with WSL sessions has `wsl:<distro>` scopes the pane does not report on.
- **Found by seam:** GH-70 seam 0017 (CH-012).
- **Why deferred:** A scope selector or per-environment report list is new product surface. Combining scopes inside one reduction would reopen CH-010's population queries and their tests.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` after CH-013, when the first Windows+WSL usage is confirmed. **Filed as antiburn#225.**

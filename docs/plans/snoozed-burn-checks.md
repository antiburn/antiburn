# Snoozed Burn Checks

## Goal

Let a reader defer one burn check for one week, one month, or forever. A
check-level snooze applies to all projects and sessions. The storage shape keeps
the scope field so a later change can add a target-specific snooze without
changing existing records.

## Design

1. Persist a bounded list of snooze records in local desktop storage. Each
   record has a check detector, a `check` scope, and either an expiry epoch or
   a forever value.
2. Share the record store with all renderer surfaces through a shell event after
   each successful SQLite write.
3. Remove active snoozed categories from Failed and Passed. Render them in the
   collapsed Snoozed group, with their original verdict counts and an expiry
   label.
4. Filter matching session hygiene checks before session badges, session detail
   rows, and Sessions failing/passing filters calculate their result. Snoozed
   checks do not become passes.
5. Compute processing text only from actual pending or processing evidence
   rows. Missing evidence remains incomplete coverage, but is not active work.

## Interaction

1. The Snooze control opens an accessible menu with one week, one
   month, and forever choices.
2. A successful choice opens Snoozed and selects the moved check. Snoozed rows
   offer Change reminder and Unsnooze.
3. The row uses a short entrance animation. The existing reduced-motion rule
   disables it.
4. Expired records are removed when the store reads or writes. A timer updates
   the store at the next finite expiry while the surface is mounted.

## Validation

1. Test storage parsing, expiry, calendar-month calculation, and detector to
   session-check mapping.
2. Test reminder choices, Snoozed grouping, unsnoozing, and Sessions filter
   behavior.
3. Test that incomplete evidence without queued work does not say processing.
4. Run desktop lint, type check, tests, build, `aislop scan --changes`, and
   the relevant Rust checks.

# Local Insights followups

This document is append-only and not digest-bound. Any seam can append an entry. Each entry records what the seam found and how later work must handle it.

## Cached account limits in quota pressure

- **What was found:** A report-time join could add cached account limits to the quota-pressure section.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** The join needs a separate consent review and is not part of transcript evidence.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` after the consent boundary is defined.

## Session-level hygiene badges

- **What was found:** Existing evidence could support session badges for reasoning overkill, excessive cache rehydration, and bloated initial context.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** A second session-level reducer needs product scope, but it needs no new parsing, evidence, or schema.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` after the report reducer ships.

## Additional Claude JSONL row types

- **What was found:** Later work can model more row types from the collected unknown-type diagnostic.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** The first release must collect evidence before the parser adds unused record types. A new parser revision can reparse them.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` after release data supports specific row types.

## Migration ladder squash

- **What was found:** The migration ladder can be squashed before version 1.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** An appended migration updates `user_version` during branch switches. An edited migration does not update an existing developer database. Release-candidate tags exist through `antiburn-v0.1.0-rc.6`.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` as antiburn#76 after release-candidate distribution is checked.

## Slow discovery while the popover is hidden

- **What was found:** Scheduled scanning stops while the popover is hidden, so discovery waits for another trigger.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** A 15-to-30-minute tick would refresh metrics that no reader sees. It would reverse the current pause and require Locked Decision 15 to change. CH-008 drains only work that discovery already queued.
- **Kind:** `enhancement`
- **Disposition:** `fold-into-later-seam` for CH-013 measurement and review. This work is a natural prerequisite for session-level badges.

## Second provider

- **What was found:** A second provider can follow the first Claude slice.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** Source Phase 12 is outside GH-70.
- **Kind:** `deferred`
- **Disposition:** `file-issue` after the first provider ships.

## Deleted transcript reconciliation

- **What was found:** The session index does not remove a session row when its transcript disappears from disk.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** Evidence cascades only when the session row is deleted. Current deletion paths cover gate rejection, ignored paths, and explicit user deletion, but not a missing file.
- **Kind:** `deferred`
- **Disposition:** `file-issue` with a privacy review before CH-013 completes.

## Phase 13 optimizations

- **What was found:** Report caching, relational evidence projections, and more read pooling can reduce later costs.
- **Found by seam:** GH-70 seam 0001.
- **Why deferred:** Each optimization needs measurements from the first provider path.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` only when CH-013 measurements justify a specific optimization.

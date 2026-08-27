<!-- SPDX-License-Identifier: MIT -->

# Design review: {window} — {surface}

- **Window:** {popover | settings | onboarding | notification} ({width} × {height})
- **Surface:** {activity | session | usage | pane name | step name | resting | expanded}
- **Build:** {dev instance, branch, or commit}
- **Date:** {YYYY-MM-DD}
- **Reviewer:** antiburn design-review skill

## Captures

Surface pass:

- {surface}: {path}
- {surface}: {path}

Theme pass:

- Light: {path}
- Dark: {path}
- Reduced transparency: {path}
- Reduced motion: {path, or "checked live, no capture"}

## Summary

{2 to 3 sentences. The overall read, the biggest win, and the biggest risk.}

## Top fixes

1. {highest-impact fix}
2. {next}
3. {next}

## Findings

Ranked most severe first.

| # | Severity | Dimension | Finding | Rule / token | Suggested fix | Evidence |
|---|----------|-----------|---------|--------------|---------------|----------|
| 1 | High | Colour | {what is wrong} | 3.1 no raw colour, `tokens.css` | {fix} | dark capture, card X |
| 2 | High | State | {what is wrong} | 7.1 empty state | {fix} | `LocalActivityList.tsx:265` |
| 3 | Medium | Motion | {what is wrong} | 10.1 token durations | {fix} | `duration-[120ms]` |

## Checked and clean

{The dimensions that passed. This keeps the next review from re-testing them
blind, and it stops a short findings table from reading as a shallow pass.}

## Notes

{Anything you could not confirm, or that needs the user. A recorded deviation
that explains a difference. A state you could not reach. A capture taken from
the browser path, where no shell is present.}

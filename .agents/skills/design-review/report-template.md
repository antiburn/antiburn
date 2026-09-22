<!-- SPDX-License-Identifier: MIT -->

# Design review: {window} — {surface}

- **Window:** {main | popover | HUD | settings | onboarding | notification} ({width} × {height})
- **Surface:** {activity | session | usage | pane name | step name | resting | expanded}
- **Interface size:** {preference and tested viewport sizes}
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
- System: {path}
- Reduced transparency: {path}
- Reduced motion: {path, or "checked live, no capture"}
- Untested surface/theme pairs: {pair and reason, or "None"}

## Summary

{2 to 3 sentences. The overall read, the biggest win, and the biggest risk.}

## Top fixes

1. {highest-impact fix}
2. {next}
3. {next}

## Findings

Ranked most severe first.

| #   | Severity | Dimension | Finding         | Rule / token                                    | Suggested fix | Evidence                    |
| --- | -------- | --------- | --------------- | ----------------------------------------------- | ------------- | --------------------------- |
| 1   | High     | Colour    | {what is wrong} | Design guide: Colour and surfaces; `tokens.css` | {fix}         | dark capture, card X        |
| 2   | High     | State     | {what is wrong} | Design guide: Review a change                   | {fix}         | `LocalActivityList.tsx:265` |
| 3   | Medium   | Motion    | {what is wrong} | Design guide: Type, controls, and interaction   | {fix}         | `duration-[120ms]`          |

## Design risks

{Observed problems that no current rule covers, or source-backed risks that
could not be reproduced live. Do not rank these as confirmed violations.}

| #   | User impact     | Risk               | Evidence            | How to confirm | Suggested direction |
| --- | --------------- | ------------------ | ------------------- | -------------- | ------------------- |
| 1   | {affected task} | {observed problem} | {source or capture} | {steps}        | {option}            |

## Checked scope and limits

{List each captured surface/theme pair and state that passed. List each
untested surface/theme pair and its reason. Do not claim untested areas are
clean.}

## Notes

{Anything you could not confirm, or that needs the user. A recorded deviation
that explains a difference. A state you could not reach. A capture taken from
the browser path, where no shell is present.}

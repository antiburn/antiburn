# Burn Checks cosmetic polish

## Scope

Polish Zack’s Burn Checks UI from PR #488 without changing check ordering,
disclosures, target loading, sample navigation, remediation, or analytics.
Keith approved the final design after Notate and Discuss review.

PR #488 is merged into main. This branch contains the cosmetic follow-up.

## Approved design

- Use the full workspace width with 32px horizontal padding.
- Use the approved 02D label-first hero: an 88px proportional ring, a grey
  12px flame beside Estimated burn, and neutral large-title percentage text.
- Show Less than 1% for positive estimates below 1%. Draw the supplied burn
  proportion without rounded endpoints exaggerating small values.
- Reuse Marty’s SegmentedRadialDial from commit 9cb51e4f, with an optional
  flat-cap treatment for this hero. Preserve its default rounded treatment.
- Keep failed and passed groups. Use bare grey category icons and semantic
  colors for findings and verified savings.
- Present named MCP and skill targets in responsive cards. Keep actions and
  sample disclosures inside their original finding.
- Match the shared Mac-style push buttons: 22px height, 12px regular text and
  glyphs, and a 4px gap. Keep the approved orange hover treatment.
- Match the loading skeleton and desktop design contract to the approved UI.

## Status

| Step | Status |
| --- | --- |
| Restore Zack’s functional baseline | Complete |
| Review eight hero variations and refinements | Complete — 02D approved |
| Apply native button and grey-icon feedback | Complete |
| Desktop checks and native review build | Complete |
| Keith’s review | Approved to move ahead |
| Pre-push checks | Complete |

## Validation

The final desktop suite passed all 1,401 tests across 111 files. Type checking,
lint, formatting, design drift, changed-file aislop, and the secrets scan passed.
The full aislop check passed with one existing duplicate-block warning in the
unchanged popoverPeekIpc.ts file. The proportional ring
also has coverage for zero, tiny, partial, complete, and unavailable estimates.
The native debug bundle built successfully and was launched for review.

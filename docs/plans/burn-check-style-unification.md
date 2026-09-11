# Burn Checks cosmetic polish

## Scope

Polish the Burn Checks UI from merged PR #488. Preserve check ordering,
disclosures, target loading, sample navigation, remediation, and analytics.
Keith approved opening the PR and fixing any CI failures.

## Final design

- Fill the workspace with the report and responsive finding cards.
- Use an 88px hero dial with an 8px stroke at full opacity: darker cyan
  measure for the remainder and brand-tint orange for avoidable usage.
- Use a 4px minimum positive arc at the call site. This can overstate shares
  below about 1.59%; visible text and accessible labels retain the supplied
  value or its existing display formatting. Zero has no issue arc.
- Reuse the SegmentedRadialDial from commit 9cb51e4f. Its default behaviour
  remains unchanged; the hero opts into flat endpoints.
- Align the text and ring with explicit 88px geometry, a 24px gap, and 32px
  vertical padding. Use a semibold large-title percentage.
- Use grey category icons and values, with red failed-session counts.
  Preserve provider logo colours and cyan action-success glyphs.
- Use subtle parent outlines and borderless surface-card/75 finding cards.
- Use compact buttons with solid orange hover. Copy buttons fill named
  target cards up to 24rem; wide check-level panels use left-aligned actions.
- Use semibold count-first sample disclosures without chevrons and with
  neutral hover fills. Preserve disclosure semantics and keyboard behaviour.
- Match the loading skeleton and design contract to the final presentation.

## Status

| Step | Status |
| --- | --- |
| Restore functional baseline and iterate on design | Complete |
| Apply approved final UI | Complete |
| Focused checks and native review build | Complete |
| Final pre-push checks | Complete |
| Open PR and resolve CI failures | PR #493 open; CI in progress |

## Validation

The latest review build passed 52 BurnChecksView tests, lint, type checking,
formatting, design drift, and changed-file aislop. Native debug bundling passed
and the app was launched for review. The shared dial has eight additional tests.

After integrating main, all 1,451 desktop tests passed, together with formatting,
lint, type checking, unused-code checks, and design-contract drift.
PR: https://github.com/antiburn/antiburn/pull/493

<!-- SPDX-License-Identifier: MIT -->

# How to assess a desktop design

Read [the desktop design guide](../../../apps/desktop/design.md) for design intent
and shared rules. Follow its source map for the CSS values and component behavior
that apply to the surface. This file defines the review method and severity;
it does not maintain another copy of the design system.

## Evidence and authority

- Judge the rendered result against the user task. An implementation can be
  consistent with its CSS and still have poor hierarchy or obscure an action.
- Cite a guide section for a shared rule, and the owning stylesheet or component
  for an exact value or behavior. Check the current source before reporting it.
- Existing code establishes current behavior, not proof that the design is good.
  If no shared rule covers a problem, report a design risk with its user impact
  and a way to confirm it. Do not turn personal preference into a violation.
- Enforce the guide's explicit prohibitions, including raw feature colours,
  stock colour and type utilities, ad-hoc radii, and copied transition timings.
  Existing violations do not authorize new ones. Apply only the guide's stated
  exceptions; a local preference or rationale alone cannot create an exception.
- `scripts/check-design-drift.mjs` checks static theme completeness, System/explicit
  palette agreement, inherited type leading, and native/CSS window corners.
  It does not prove visual quality, accessibility, or all runtime theme behavior.
- A browser capture cannot establish native material, focus, window placement,
  or WebView scaling behavior. State that limit beside the finding.

## Review questions

Use the guide's review checklist, then inspect the source for any suspected
violation. Focus on the following questions rather than a frozen inventory of
window sizes, pane names, token values, or component recipes.

| Dimension             | What to establish                                                                                                                                                         |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Purpose and hierarchy | Can the user find the reading, its scope, and the next action? Does decoration compete with evidence?                                                                     |
| Navigation            | Do destinations match their labels, preserve context, and provide a way back? Does search focus a control without changing it?                                            |
| Layout                | Does the surface work at its supported widths and interface sizes, including long content? Do scroll regions and native chrome remain usable?                             |
| Colour and type       | Do semantic meanings remain consistent? Are figures comparable, text readable, and hover, selection, and focus distinct?                                                  |
| Controls              | Does the surface reuse shared primitives and their interaction states? Are actions reachable by keyboard and pointer?                                                     |
| Data and states       | Are unknown, partial, loading, empty, failure, and permission-blocked states honest and actionable? Can the user discover a figure's method and freshness where relevant? |
| Themes and motion     | Do Light, Dark, and System work? Does reduced transparency remain legible, and does reduced motion preserve meaning?                                                      |
| Accessibility         | Are focus, names, roles, contrast, and non-colour cues sufficient? Can essential information and actions be reached without hover?                                        |
| Copy                  | Is wording calm, specific, and consistent with the guide? Does it name the action and explain limits without exposing implementation details?                             |
| Robustness            | Do errors, large values, slow data, and repeated interaction preserve a usable surface?                                                                                   |

A screenshot cannot prove keyboard or screen-reader behavior. Exercise it or
record it as untested. The keyboard focus ring is deliberately gated by
`html[data-keyboard]`; absence on pointer click is not a finding.

## Findings and risks

For a confirmed violation, identify the observed problem, affected task,
evidence, applicable guide section or source rule, and smallest useful fix.
Rank by impact, not by how easy the issue is to detect in code.

For a design risk, state what remains uncertain and how to test it. Keep risks
separate from confirmed violations. Do not claim an untested state is clean.
A documented exception is context to assess, not automatic proof of a defect.

## Severity

- **Blocker:** the task is unusable, essential content is unreadable or clipped,
  or an essential control cannot be reached. An accessibility AA miss in new UI
  is a blocker.
- **High:** a clear shared-rule violation or substantial user difficulty;
  missing recovery, misleading evidence, or an accessibility AA miss in existing UI.
- **Medium:** a contained consistency or interaction problem that adds effort
  without blocking the task.
- **Nitpick:** minor polish with little effect on task completion.

Measure the guide's contrast requirements on the actual foreground/background
combination; a token name alone does not prove contrast on a translucent surface.

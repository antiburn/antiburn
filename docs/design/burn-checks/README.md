# Burn Check design bundle

This bundle records the approved desktop Burn Check design and the alternatives considered.
The editable fragments are in [`source/`](source/); browser-ready exports are in
[`standalone/`](standalone/). The source set defaults to the selected cyan pass palette, and the
session-card information architecture marks **C · Result first** as implemented.

## Design sets

- [`session-card-ia-variants`](standalone/session-card-ia-variants.html) — five information
  architectures, including the implemented C variant.
- [`session-card-hierarchy-comparison`](standalone/session-card-hierarchy-comparison.html) — agent
  and Burn Check hierarchy alternatives.
- [`session-list-check-line`](standalone/session-list-check-line.html) — card-level palette and
  result-line alternatives.
- [`session-check-line`](standalone/session-check-line.html) — four complete indicator systems.
- [`checks-summary-row`](standalone/checks-summary-row.html) — the compact 380px popover summary.
- [`burn-check-specimens`](standalone/burn-check-specimens.html) — the generic dial at 16, 24, 32,
  and 48px plus the approved state-language matrix.

See [`DECISIONS.md`](DECISIONS.md) for the implementation record and [`VALIDATION.md`](VALIDATION.md)
for the evidence checklist.

The screenshot set also includes
[`production-components-light.png`](screenshots/production-components-light.png) and
[`production-components-dark.png`](screenshots/production-components-dark.png), captured from the
production-component browser harness described in the validation record.

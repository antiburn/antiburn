# Burn Check validation evidence

The implementation test suite covers:

- empty, invalid, single-segment, highly uneven, multi-segment, and excessive-gap dial geometry;
- labelled standalone and decorative dial rendering;
- adapter precedence, pluralization, lifecycle states, incomplete evidence, retained results, and
  refresh failure;
- report-category classification without double counting;
- matching indicator segments and structured count phrases;
- neutral passed wording and orange failure wording;
- status tooltip detail, cost and allowance thresholds, summary hover and focus coordination,
  selection, keyboard navigation, and virtualization;
- import boundaries between the generic dial, presentation adapters, Burn Check components, IPC,
  and window controllers;
- exact light and dark token values and design-contract drift.

The standalone design sets provide fixed-width light and dark specimens for the 380px popover and
session cards, including long content, rest, hover, active, complete, incomplete, loading,
unsupported, and unavailable states. These files are design comparisons, not production UI.

`apps/desktop/tests/visual/burn-check-review.html` is the browser review harness. It imports the
production `SessionStatusBar`, `ChecksSummary`, `TruncatedText`, presentation adapters, agent icon,
and complete desktop stylesheet. The light and dark captures use the 1100px default main-window
width, its 340px session collection, and the 380px popover width. The selected summary uses the
production `active` prop. The session-card wrapper reproduces the production layout classes around
`SessionStatusBar`; deterministic rest, hover, and selected surfaces replace native pointer input.
The harness does not run the Tauri shell, IPC subscriptions, virtualization, native titlebar, or
companion preview window. Automated tests cover those preserved interaction boundaries.

Screenshots in [`screenshots/`](screenshots/) include both design comparisons and the two
production-component browser captures.

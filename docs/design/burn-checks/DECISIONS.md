# Burn Check design decisions

- Use the cyan palette for pass arcs and the terminal pass mark: `#108EAC` in light mode and
  `#2EA5C6` in dark mode.
- Use orange for failure arcs and marks (`#FF6A2C`) with separate accessible failure text
  (`#BA3800` light, `#FF8D5C` dark).
- Use neutral grey for unassessed evidence (`#B8BCC0` light, `#7D8185` dark).
- Implement session-card variant **C · Result first**. The result and existing price or allowance
  lead the card; the title aligns with the verdict; the neutral vendor mark moves beside model data.
- Keep pass wording neutral. Color identifies failures, not success prose.
- Reserve terminal tick and exclamation treatments for complete, settled, nonempty evidence.
  Partial evidence uses proportional cyan, orange, and grey dial segments.
- Keep report categories and session checks as separate count scopes. Each report category counts
  once even when its underlying session evidence is mixed.
- Preserve existing results through refresh and refresh failure. Disclose refresh, growth,
  incomplete evidence, and refresh failure as separate context instead of rewriting outcomes.
- Keep the companion Checks preview, cost and allowance calculations, tooltips, selection,
  navigation, virtualization, and motion behavior outside the reusable presentation components.

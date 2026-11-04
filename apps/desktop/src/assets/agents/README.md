# Vendor artwork

Cadence supplies the base light/dark SVGs. `provenance.json` pins its revision
and records each asset hash. `officialOverrides` identifies exact official
Kiro color app icon, current Devin symbols, and Pi color logo that replace those Cadence files. Source colors
are preserved; monochrome vendors do not receive invented colors.

Amp uses its unboxed monochrome wordmark.

Every neutral icon is a CSS mask derived from its default SVG. Kiro
omits its app tile background; Kiro's eyes and OpenCode's interior retain their
negative space through luminance masks. Other vendors use alpha masks.
There is no separate neutral path registry. Oh My Pi uses the recorded official `src/lib/fixtures/omp-mark.svg` for both
appearances. Its pink/purple/cyan gradient remains in the default image. The
background tile is removed and the viewBox expands to `10 14 44 44`, reducing
the glyph by about 9% relative to the previous `12 16 40 40` glyph crop,
while keeping its center and requested icon box fixed.

Vendor marks remain their owners' trademarks. This use identifies coding
agents and does not imply affiliation or endorsement.

Devin uses the official `devin.ai/brand` symbol in the legacy `windsurf` icon
slot. Its light/dark SVGs omit only the source background rectangle. The
manifest records source URLs, original source hashes, and normalized asset hashes.
Session and provider identifiers retain their existing values.

Pi uses the official `pi.dev/logo-auto.svg` in both themes, preserving its coral,
blue, and yellow fills. Its neutral alpha mask comes from the same SVG.

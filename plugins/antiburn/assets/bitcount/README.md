# Usage caption glyphs

`hooks/usage-font-data.ts` contains deterministic glyph geometry extracted from
[Bitcount Prop Single](https://github.com/google/fonts/tree/main/ofl/bitcountpropsingle),
weight 400, at the default variation axes (ELXP 0, ELSH 0, slnt 0, CRSV 0.5).
The upstream source revision is `af0818eaeb3b0839806ea19134fc18f317cdcf5a` in
[TYPETR-Bitcount](https://github.com/petrvanblokland/TYPETR-Bitcount).

The data retains the font's pixel outline, component positions, advances and
`tnum` substitutions. Captions use proportional advances; percentages use
tabular substitutions in fixed 700-unit digit cells. The upstream `tnum` feature
still has mixed 600/700-unit advances (confirmed with HarfBuzz 0.56.3); the fixed
cells keep unchanged percentage columns stationary during rolls. Units are 1000 per em. Runtime rendering reads bundled
TypeScript data and does not fetch or load a font file.

Copyright 2024 The Bitcount Project Authors. Distributed under the SIL Open
Font License 1.1; see [OFL.txt](OFL.txt). This is a glyph-geometry derivative,
not an independently named installable font.

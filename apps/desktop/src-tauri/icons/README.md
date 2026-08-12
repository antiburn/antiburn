# antiburn icons

Every PNG in this directory is **generated**, not hand-drawn or imported.

- Generator: [`apps/desktop/scripts/generate-icons.mjs`](../../scripts/generate-icons.mjs)
- Regenerate with `pnpm --filter @antiburn/desktop icons`
- Dependencies: none — the mark is drawn analytically and the PNGs are encoded
  with Node's built-in `zlib`, so the output is byte-for-byte reproducible.

| File             | Purpose                                                              |
| ---------------- | -------------------------------------------------------------------- |
| `icon.png`       | 1024px master app icon (installer, dock, about)                      |
| `512x512.png`    | Linux/`hicolor` app icon                                             |
| `128x128@2x.png` | 256px app icon                                                       |
| `128x128.png`    | app icon                                                             |
| `32x32.png`      | small app icon                                                       |
| `tray.png`       | menu-bar / tray glyph, black-on-transparent macOS **template** image |

The mark is three descending rounded bars: a burn-down of agent spend. The tray
glyph carries its shape entirely in the alpha channel so macOS can tint it for
light, dark, and pressed menu-bar states.

Platform bundle formats (`icon.icns`, `icon.ico`) are produced from `icon.png`
by `pnpm --filter @antiburn/desktop tauri icon` during release packaging; they
are not checked in because nothing in the repository's build or test path needs
them yet.

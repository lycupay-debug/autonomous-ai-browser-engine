# Headless text shaping, raster and image decode PoC

A pure-Rust prototype uses `rustybuzz 0.20.1` to shape Unicode text with OpenType tables, `fontdue 0.9.5` to rasterize the resulting glyph IDs, and `png 0.18.1` to encode an RGBA surface. The PNG is then decoded with `image 0.25.10`.

Unit tests exercise mixed Latin/Simplified Chinese shaping, missing-glyph detection, invalid font sizes, and PNG/JPEG/GIF/WebP encode/decode roundtrips. The tested Noto Sans SC variable font and its OFL notice remain under `/tmp/aibe-fonts` and are not committed.

This does not yet implement full bidi paragraph layout, line breaking, script itemization, font fallback, variable-font axis selection, color fonts, CSS layout, paint invalidation, SVG, canvas, GPU rendering, or browser screenshots. A shaping call for one text run is not a complete multilingual text-layout system.

Run:

```sh
cargo test -p aibe-headless-raster-poc --locked
cargo run -p aibe-headless-raster-poc --locked -- /path/to/font.ttf /tmp/output.png
```

# Headless CPU raster PoC

A pure-Rust prototype parses a temporary Noto Sans SC variable TTF, rasterizes Latin and Simplified Chinese glyphs using `fontdue`, composites coverage into an RGBA buffer, and encodes a PNG using `png`.

The font and its OFL notice are downloaded to `/tmp/aibe-fonts` for the experiment only; neither is committed to this repository. The tested font file SHA-256 is recorded in the Phase 0 Notion log.

The PoC does not implement OpenType shaping, ligatures, complex-script shaping, bidi reordering, line breaking, fallback font selection, CSS layout, paint invalidation, image decoding, GPU rendering, or browser screenshots. A future renderer needs a shaping layer (for example Rustybuzz/HarfBuzz) and a font fallback model before it can claim multilingual text layout.

Run:

```sh
cargo run -p aibe-headless-raster-poc --locked -- /path/to/font.ttf /tmp/output.png
```

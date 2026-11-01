//! Headless text shaping, glyph rasterization, and image decoding feasibility.
//!
//! This is not a browser layout or paint engine. It validates a Rust-native
//! shaping/raster/image pipeline on the target FreeBSD host. Swash owns font
//! parsing, Unicode-script shaping, outline scaling, and glyph rasterization;
//! paragraph layout, bidi reordering, and font fallback remain out of scope.

use swash::{
    FontRef,
    scale::{Render, ScaleContext, Source},
    shape::{Direction, ShapeContext},
    text::{Codepoint, Script},
};

#[derive(Debug)]
pub struct RasterizedText {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub input_codepoints: usize,
    pub shaped_glyphs: usize,
    pub missing_glyphs: usize,
    pub nonzero_coverage_pixels: usize,
}

pub fn shape_and_rasterize(
    font_bytes: &[u8],
    text: &str,
    pixel_size: f32,
) -> Result<RasterizedText, String> {
    if !pixel_size.is_finite() || pixel_size <= 0.0 {
        return Err("pixel size must be finite and positive".into());
    }

    let font = FontRef::from_index(font_bytes, 0)
        .ok_or_else(|| "OpenType font face could not be parsed".to_string())?;
    let missing_glyphs = text
        .chars()
        .filter(|character| font.charmap().map(*character) == 0)
        .count();

    // Itemize this small PoC by Unicode script. Common and inherited
    // characters stay with the preceding run; a full browser itemizer must
    // also account for language, direction, style, bidi, and font fallback.
    let mut runs: Vec<(Script, String)> = Vec::new();
    for character in text.chars() {
        let script = match character.properties().script() {
            Script::Common | Script::Inherited => runs
                .last()
                .map(|(script, _)| *script)
                .unwrap_or(Script::Latin),
            script => script,
        };
        if let Some((last_script, run_text)) = runs.last_mut()
            && *last_script == script
        {
            run_text.push(character);
            continue;
        }
        runs.push((script, character.to_string()));
    }

    let mut shape_context = ShapeContext::new();
    let mut shaped_glyphs = Vec::new();
    for (script, run_text) in &runs {
        let mut shaper = shape_context
            .builder(font)
            .script(*script)
            .direction(Direction::LeftToRight)
            .size(pixel_size)
            .build();
        shaper.add_str(run_text);
        shaper.shape_with(|cluster| {
            shaped_glyphs.extend_from_slice(cluster.glyphs);
        });
    }

    let baseline = 56.0_f32;
    let margin = 16.0_f32;
    let mut pen_x = 0.0_f32;
    let mut rightmost = margin;
    let mut scale_context = ScaleContext::new();
    let mut scaler = scale_context
        .builder(font)
        .size(pixel_size)
        .hint(false)
        .build();
    let renderer = Render::new(&[Source::Outline]);
    let mut rendered_glyphs = Vec::with_capacity(shaped_glyphs.len());

    for glyph in &shaped_glyphs {
        let x = pen_x + glyph.x;
        let y = baseline - glyph.y;
        if let Some(image) = renderer.render(&mut scaler, glyph.id) {
            let left = (margin + x + image.placement.left as f32).floor() as i32;
            let top = (y - image.placement.top as f32).floor() as i32;
            rightmost = rightmost.max((left + image.placement.width as i32) as f32);
            rendered_glyphs.push((left, top, image));
        }
        pen_x += glyph.advance;
    }

    let width = (rightmost.ceil() as u32 + margin as u32).max(1);
    let height = 88_u32;
    let mut rgba = vec![255_u8; (width * height * 4) as usize];
    let mut nonzero_coverage_pixels = 0;

    for (left, top, image) in rendered_glyphs {
        let glyph_width = image.placement.width as usize;
        let glyph_height = image.placement.height as usize;
        if image.data.len() < glyph_width.saturating_mul(glyph_height) {
            return Err("Swash returned a truncated alpha glyph bitmap".into());
        }
        for glyph_y in 0..glyph_height {
            for glyph_x in 0..glyph_width {
                let coverage = image.data[glyph_y * glyph_width + glyph_x];
                if coverage == 0 {
                    continue;
                }
                nonzero_coverage_pixels += 1;
                let x = left + glyph_x as i32;
                let y = top + glyph_y as i32;
                if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                    continue;
                }
                let offset = ((y as u32 * width + x as u32) * 4) as usize;
                let alpha = u16::from(coverage);
                for channel in 0..3 {
                    rgba[offset + channel] = ((255 * (255 - alpha) + 32 * alpha) / 255) as u8;
                }
                rgba[offset + 3] = 255;
            }
        }
    }

    Ok(RasterizedText {
        width,
        height,
        rgba,
        input_codepoints: text.chars().count(),
        shaped_glyphs: shaped_glyphs.len(),
        missing_glyphs,
        nonzero_coverage_pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::shape_and_rasterize;
    use image::{DynamicImage, GenericImageView, ImageBuffer, ImageFormat, Rgb};
    use std::io::Cursor;

    #[cfg(target_os = "freebsd")]
    const FONT_PATH: &str = "/tmp/aibe-fonts/NotoSansSC.ttf";

    #[cfg(target_os = "freebsd")]
    #[test]
    fn shapes_and_rasterizes_mixed_latin_and_cjk_text() {
        let font = std::fs::read(FONT_PATH).expect("temporary Noto Sans SC font exists");
        let result = shape_and_rasterize(&font, "Browser OS / 自主 AI 浏览器引擎", 32.0)
            .expect("text shaping and rasterization succeed");
        assert!(result.shaped_glyphs > 0);
        assert_eq!(result.missing_glyphs, 0);
        assert!(result.nonzero_coverage_pixels > 0);
        assert!(
            result
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[0] < 255)
        );
        assert!(result.width > 100);
    }

    #[test]
    fn rejects_invalid_font_size_before_parsing_font_data() {
        assert!(shape_and_rasterize(&[], "AIBE", 0.0).is_err());
        assert!(shape_and_rasterize(&[], "AIBE", f32::NAN).is_err());
    }

    #[test]
    fn rejects_malformed_font_data() {
        assert!(shape_and_rasterize(b"not a font", "AIBE", 16.0).is_err());
    }

    #[test]
    fn decodes_png_jpeg_gif_and_webp_images() {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(3, 2, Rgb([24, 64, 128])));
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
        ] {
            let mut encoded = Cursor::new(Vec::new());
            image
                .write_to(&mut encoded, format)
                .unwrap_or_else(|error| panic!("{format:?} encode: {error}"));
            let decoded = image::load_from_memory_with_format(encoded.get_ref(), format)
                .unwrap_or_else(|error| panic!("{format:?} decode: {error}"));
            assert_eq!(decoded.dimensions(), (3, 2));
        }
    }
}

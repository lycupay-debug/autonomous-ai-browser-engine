//! Headless text shaping, glyph rasterization, and image decoding feasibility.
//!
//! This is not a browser layout or paint engine. It validates that a Rust-native
//! shaping/raster/image pipeline can run on the target FreeBSD host.

use fontdue::{Font, FontSettings};
use rustybuzz::{UnicodeBuffer, shape};

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
    let face = rustybuzz::Face::from_slice(font_bytes, 0)
        .ok_or_else(|| "OpenType font face could not be parsed".to_string())?;
    let font = Font::from_bytes(font_bytes, FontSettings::default())
        .map_err(|error| format!("font rasterizer parse failed: {error}"))?;

    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let shaped = shape(&face, &[], buffer);
    let infos = shaped.glyph_infos();
    let positions = shaped.glyph_positions();
    let scale = pixel_size / face.units_per_em() as f32;
    let baseline = 56.0_f32;
    let margin = 16.0_f32;
    let mut pen_x = margin;
    let mut pen_y = 0.0_f32;
    let mut glyphs = Vec::with_capacity(infos.len());
    let mut missing_glyphs = 0;
    let mut rightmost = margin;

    for (info, position) in infos.iter().zip(positions.iter()) {
        let glyph_id = info.glyph_id as u16;
        if glyph_id == 0 {
            missing_glyphs += 1;
        }
        let (metrics, bitmap) = font.rasterize_indexed(glyph_id, pixel_size);
        let offset_x = position.x_offset as f32 * scale;
        let offset_y = position.y_offset as f32 * scale;
        let origin_x = pen_x + offset_x;
        let glyph_baseline = baseline - pen_y - offset_y;
        let left = origin_x.floor() as i32 + metrics.xmin;
        let top = (glyph_baseline - metrics.ymin as f32 - metrics.height as f32).floor() as i32;
        rightmost = rightmost.max((left + metrics.width as i32) as f32);
        glyphs.push((left, top, metrics, bitmap));
        pen_x += position.x_advance as f32 * scale;
        pen_y += position.y_advance as f32 * scale;
    }

    let width = (rightmost.ceil() as u32 + margin as u32).max(1);
    let height = 88_u32;
    let mut rgba = vec![255_u8; (width * height * 4) as usize];
    let mut nonzero_coverage_pixels = 0;

    for (left, top, metrics, bitmap) in glyphs {
        for glyph_y in 0..metrics.height {
            for glyph_x in 0..metrics.width {
                let coverage = bitmap[glyph_y * metrics.width + glyph_x];
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
        shaped_glyphs: infos.len(),
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
        assert!(result.width > 100);
    }

    #[test]
    fn rejects_invalid_font_size_before_parsing_font_data() {
        assert!(shape_and_rasterize(&[], "AIBE", 0.0).is_err());
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

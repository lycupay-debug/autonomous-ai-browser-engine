//! CPU-only glyph rasterization feasibility test using a temporary external font.
//!
//! This PoC does not perform OpenType shaping, bidi reordering, line breaking,
//! CSS layout, paint invalidation, or a browser rendering pipeline.

use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::BufWriter;

use fontdue::{Font, FontSettings};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let font_path = args
        .next()
        .unwrap_or_else(|| "/tmp/aibe-fonts/NotoSansSC.ttf".into());
    let output_path = args
        .next()
        .unwrap_or_else(|| "/tmp/aibe-headless-raster.png".into());
    let text = "Browser OS / 自主 AI 浏览器引擎";
    let font_bytes = fs::read(&font_path)?;
    let font = Font::from_bytes(font_bytes, FontSettings::default())
        .map_err(|error| format!("font parse failed: {error}"))?;

    let pixel_size = 32.0_f32;
    let baseline = 54_i32;
    let margin = 16_i32;
    let mut pen_x = margin as f32;
    let mut glyphs = Vec::new();
    let mut previous = None;
    let mut missing = 0_usize;

    for character in text.chars() {
        if let Some(previous_character) = previous {
            pen_x += font
                .horizontal_kern(previous_character, character, pixel_size)
                .unwrap_or(0.0);
        }
        let glyph_index = font.lookup_glyph_index(character);
        if glyph_index == 0 && character != '\0' {
            missing += 1;
        }
        let (metrics, bitmap) = font.rasterize(character, pixel_size);
        glyphs.push((pen_x, metrics, bitmap));
        pen_x += glyphs
            .last()
            .expect("glyph was just pushed")
            .1
            .advance_width;
        previous = Some(character);
    }

    let width = (pen_x.ceil() as u32 + margin as u32).max(1);
    let height = 80_u32;
    let mut rgba = vec![255_u8; (width * height * 4) as usize];
    let mut nonzero_coverage = 0_usize;

    for (origin_x, metrics, bitmap) in glyphs {
        let left = origin_x.floor() as i32 + metrics.xmin;
        let top = baseline - metrics.ymin - metrics.height as i32;
        for glyph_y in 0..metrics.height {
            for glyph_x in 0..metrics.width {
                let coverage = bitmap[glyph_y * metrics.width + glyph_x];
                if coverage == 0 {
                    continue;
                }
                nonzero_coverage += 1;
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

    let output = BufWriter::new(File::create(&output_path)?);
    let mut encoder = png::Encoder::new(output, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;

    println!("font={font_path}");
    println!("output={output_path}");
    println!("text={text}");
    println!("glyph_count={}", text.chars().count());
    println!("missing_glyphs={missing}");
    println!("nonzero_coverage_pixels={nonzero_coverage}");
    println!("canvas={width}x{height}");
    Ok(())
}

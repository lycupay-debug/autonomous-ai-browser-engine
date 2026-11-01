//! CPU-only shaping, glyph rasterization, and image decode feasibility probe.

use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::BufWriter;

use aibe_headless_raster_poc::shape_and_rasterize;

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
    let raster = shape_and_rasterize(&font_bytes, text, 32.0)?;

    let output = BufWriter::new(File::create(&output_path)?);
    let mut encoder = png::Encoder::new(output, raster.width, raster.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&raster.rgba)?;

    let decoded = image::open(&output_path)?;
    println!("font={font_path}");
    println!("output={output_path}");
    println!("text={text}");
    println!("input_codepoints={}", raster.input_codepoints);
    println!("shaped_glyphs={}", raster.shaped_glyphs);
    println!("missing_glyphs={}", raster.missing_glyphs);
    println!("nonzero_coverage_pixels={}", raster.nonzero_coverage_pixels);
    println!("canvas={}x{}", raster.width, raster.height);
    println!("decoded_png={}x{}", decoded.width(), decoded.height());
    Ok(())
}

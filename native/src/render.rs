//! Page rasterization with hayro (pure Rust).

use std::io::Cursor;
use std::sync::Arc;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::AlphaColor;
use hayro::vello_cpu::color::palette::css::{TRANSPARENT, WHITE};
use hayro::vello_cpu::{Pixmap, RenderContext};
use hayro::{RenderCache, RenderSettings};
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};

use crate::doc::{Res, err};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    /// Raw RGBA8, unpremultiplied (what PDFNet's PDFDraw.getBitmap returns as BGRA; callers convert).
    Rgba,
}

impl Format {
    pub fn parse(s: &str) -> Res<Self> {
        match s.to_ascii_uppercase().as_str() {
            "PNG" => Ok(Format::Png),
            "JPG" | "JPEG" => Ok(Format::Jpeg),
            "RAW" | "RGBA" => Ok(Format::Rgba),
            other => Err(format!(
                "unsupported image format {other} (PNG, JPEG, RGBA)"
            )),
        }
    }
}

pub struct RenderOpts {
    pub dpi: f64,
    /// When set, the page is scaled to fit this pixel box (overrides dpi).
    pub size: Option<(u32, u32, bool)>,
    pub transparent: bool,
    pub annotations: bool,
    pub format: Format,
    pub jpeg_quality: u8,
}

pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub fn render_page(bytes: Arc<Vec<u8>>, page_index: usize, opts: &RenderOpts) -> Res<Rendered> {
    let pdf = Pdf::new(bytes).map_err(|e| format!("cannot parse PDF for rendering: {e:?}"))?;
    let pages = pdf.pages();
    let page = pages
        .get(page_index)
        .ok_or_else(|| format!("page index {page_index} out of range"))?;

    let (w, h) = page.render_dimensions();
    let (sx, sy) = match opts.size {
        Some((tw, th, preserve)) => {
            let (fx, fy) = (tw as f32 / w, th as f32 / h);
            if preserve {
                let f = fx.min(fy);
                (f, f)
            } else {
                (fx, fy)
            }
        }
        None => {
            let f = (opts.dpi / 72.0) as f32;
            (f, f)
        }
    };
    let (pw, ph) = ((w * sx).round(), (h * sy).round());
    if pw < 1.0 || ph < 1.0 || pw > u16::MAX as f32 || ph > u16::MAX as f32 {
        return Err(format!(
            "rendered size {pw}x{ph} is out of range (1..=65535 px per side)"
        ));
    }

    let interpreter = InterpreterSettings {
        render_annotations: opts.annotations,
        ..Default::default()
    };
    let bg: AlphaColor<_> = if opts.transparent { TRANSPARENT } else { WHITE };
    // Same as hayro::render, but with a rounded (not truncated) pixel size and the scale
    // adjusted to fill it exactly.
    let (pw, ph) = (pw as u16, ph as u16);
    let (sx, sy) = (pw as f64 / w as f64, ph as f64 / h as f64);
    let mut ctx = RenderContext::new(pw, ph);
    let transform = Affine::scale_non_uniform(sx, sy) * page.initial_transform(true).to_kurbo();
    hayro::render_into(
        page,
        &RenderCache::new(),
        &interpreter,
        &RenderSettings::default(),
        &mut ctx,
        transform,
    );
    ctx.flush();
    let mut pixmap = Pixmap::new(pw, ph);
    ctx.render_with(
        &mut pixmap,
        &mut hayro::vello_cpu::Resources::default(),
        hayro::vello_cpu::RasterizerSettings {
            target_init: hayro::vello_cpu::TargetInit::Clear(bg),
            ..Default::default()
        },
    );
    let (width, height) = (pw as u32, ph as u32);

    let data = match opts.format {
        Format::Png => pixmap.into_png().map_err(err("PNG encoding failed"))?,
        Format::Rgba => pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha),
        Format::Jpeg => {
            let rgba = pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha);
            // JPEG has no alpha: composite over white.
            let rgb: Vec<u8> = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    let a = p[3] as u32;
                    let blend = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
                    [blend(p[0]), blend(p[1]), blend(p[2])]
                })
                .collect();
            let mut out = Cursor::new(Vec::new());
            JpegEncoder::new_with_quality(&mut out, opts.jpeg_quality.clamp(1, 100))
                .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
                .map_err(err("JPEG encoding failed"))?;
            out.into_inner()
        }
    };
    Ok(Rendered {
        width,
        height,
        data,
    })
}

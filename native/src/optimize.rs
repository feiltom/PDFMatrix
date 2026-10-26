//! Image optimization (PDFNet.Optimizer): downsampling to a target resolution and
//! recompression (JPEG / Flate) of 8-bit gray and RGB images.

use std::collections::HashMap;
use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ColorType, DynamicImage, GrayImage, ImageDecoder, ImageEncoder, RgbImage};
use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::content::concat;
use crate::doc::{Doc, Res, deref, inherited_in, number};

/// PDFNet Optimizer.ImageSettings.CompressionMode
pub const RETAIN: u32 = 0;
pub const FLATE: u32 = 1;
pub const JPEG: u32 = 2;
pub const JPEG2000: u32 = 3;
pub const NONE: u32 = 4;

#[derive(Clone, Copy, Debug)]
pub struct ImageSettings {
    pub mode: u32,
    /// 1 (smallest) – 10 (best)
    pub quality: u32,
    pub max_dpi: f64,
    pub resample_dpi: f64,
    pub downsample: bool,
    pub force_recompression: bool,
    pub force_changes: bool,
}

impl Default for ImageSettings {
    /// PDFNet defaults: retain compression, downsample above 225 dpi to 150 dpi, quality 5.
    fn default() -> Self {
        ImageSettings {
            mode: RETAIN,
            quality: 5,
            max_dpi: 225.0,
            resample_dpi: 150.0,
            downsample: true,
            force_recompression: false,
            force_changes: false,
        }
    }
}

#[derive(Default, Debug)]
pub struct Stats {
    pub images: usize,
    pub optimized: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

type M = [f64; 6];
const IDENTITY: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn num(o: &Object) -> f64 {
    match o {
        Object::Integer(i) => *i as f64,
        Object::Real(r) => *r as f64,
        _ => 0.0,
    }
}

/// Records, for every image XObject drawn on the page (recursively through forms), the largest
/// size in points it is drawn at (to derive its effective resolution).
fn collect_placements(
    doc: &Document,
    content: &[u8],
    resources: &Dictionary,
    base: M,
    depth: u32,
    out: &mut HashMap<ObjectId, (f64, f64)>,
) {
    if depth > 10 {
        return;
    }
    let Ok(parsed) = Content::decode(content) else {
        return;
    };
    let mut ctm = base;
    let mut stack = Vec::new();
    for op in parsed.operations {
        let o = &op.operands;
        match op.operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => ctm = stack.pop().unwrap_or(base),
            "cm" if o.len() == 6 => {
                ctm = concat(
                    [
                        num(&o[0]),
                        num(&o[1]),
                        num(&o[2]),
                        num(&o[3]),
                        num(&o[4]),
                        num(&o[5]),
                    ],
                    ctm,
                )
            }
            "Do" if o.len() == 1 => {
                let Ok(name) = o[0].as_name() else { continue };
                let Some(id) = resources
                    .get(b"XObject")
                    .ok()
                    .and_then(|x| deref(doc, x).as_dict().ok())
                    .and_then(|x| x.get(name).ok())
                    .and_then(|r| r.as_reference().ok())
                else {
                    continue;
                };
                let Ok(Object::Stream(s)) = doc.get_object(id) else {
                    continue;
                };
                match s.dict.get(b"Subtype").ok().and_then(|x| x.as_name().ok()) {
                    Some(b"Image") => {
                        // Unit square scaled by the CTM: drawn width/height in points.
                        let w = (ctm[0] * ctm[0] + ctm[1] * ctm[1]).sqrt();
                        let h = (ctm[2] * ctm[2] + ctm[3] * ctm[3]).sqrt();
                        let e = out.entry(id).or_insert((0.0, 0.0));
                        // Smallest drawn size gives the highest effective resolution.
                        if e.0 == 0.0 || w < e.0 {
                            e.0 = w;
                        }
                        if e.1 == 0.0 || h < e.1 {
                            e.1 = h;
                        }
                    }
                    Some(b"Form") => {
                        let m: M = s
                            .dict
                            .get(b"Matrix")
                            .ok()
                            .and_then(|x| deref(doc, x).as_array().ok())
                            .and_then(|a| {
                                a.iter()
                                    .map(|x| number(doc, x))
                                    .collect::<Option<Vec<f64>>>()
                            })
                            .and_then(|v| v.try_into().ok())
                            .unwrap_or(IDENTITY);
                        let res = s
                            .dict
                            .get(b"Resources")
                            .ok()
                            .and_then(|x| deref(doc, x).as_dict().ok())
                            .cloned()
                            .unwrap_or_else(|| resources.clone());
                        let data = s
                            .decompressed_content()
                            .unwrap_or_else(|_| s.content.clone());
                        collect_placements(doc, &data, &res, concat(m, ctm), depth + 1, out);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Only plain gray/RGB colorspaces are re-encoded (indexed, separation, Lab… are left alone).
fn colorspace_ok(doc: &Document, s: &Stream) -> bool {
    let simple = |n: &[u8]| matches!(n, b"DeviceGray" | b"DeviceRGB" | b"CalGray" | b"CalRGB");
    match s.dict.get(b"ColorSpace").map(|o| deref(doc, o)) {
        Ok(Object::Name(n)) => simple(n),
        Ok(Object::Array(a)) => match a.first().and_then(|o| o.as_name().ok()) {
            Some(b"ICCBased") => a
                .get(1)
                .and_then(|o| deref(doc, o).as_stream().ok())
                .and_then(|st| st.dict.get(b"N").ok())
                .and_then(|n| n.as_i64().ok())
                .is_some_and(|n| n == 1 || n == 3),
            Some(n) => simple(n),
            None => false,
        },
        _ => false,
    }
}

enum Decoded {
    Gray(GrayImage),
    Rgb(RgbImage),
}

/// Decodes 8-bit Gray/RGB images stored raw, Flate-compressed or as JPEG. Returns the image and
/// whether it was a JPEG.
fn decode(s: &Stream) -> Option<(Decoded, bool)> {
    let d = &s.dict;
    if d.get(b"ImageMask").ok().and_then(|o| o.as_bool().ok()) == Some(true) || d.has(b"Decode") {
        return None;
    }
    let w = d.get(b"Width").ok()?.as_i64().ok()? as u32;
    let h = d.get(b"Height").ok()?.as_i64().ok()? as u32;
    let filters: Vec<Vec<u8>> = match d.get(b"Filter") {
        Ok(Object::Name(n)) => vec![n.clone()],
        Ok(Object::Array(a)) => a
            .iter()
            .filter_map(|o| o.as_name().ok().map(|n| n.to_vec()))
            .collect(),
        _ => vec![],
    };
    if filters.len() == 1 && filters[0] == b"DCTDecode" {
        let dec = image::codecs::jpeg::JpegDecoder::new(Cursor::new(&s.content)).ok()?;
        let ct = dec.color_type();
        let img = DynamicImage::from_decoder(dec).ok()?;
        return match ct {
            ColorType::L8 => Some((Decoded::Gray(img.to_luma8()), true)),
            ColorType::Rgb8 => Some((Decoded::Rgb(img.to_rgb8()), true)),
            _ => None, // CMYK JPEGs are left untouched
        };
    }
    if !filters.iter().all(|f| f == b"FlateDecode")
        || d.get(b"BitsPerComponent")
            .ok()
            .and_then(|o| o.as_i64().ok())
            != Some(8)
    {
        return None;
    }
    let data = if filters.is_empty() {
        s.content.clone()
    } else {
        s.decompressed_content().ok()?
    };
    match data.len() / (w as usize * h as usize).max(1) {
        1 if data.len() == (w * h) as usize => {
            Some((Decoded::Gray(GrayImage::from_raw(w, h, data)?), false))
        }
        3 if data.len() == (w * h * 3) as usize => {
            Some((Decoded::Rgb(RgbImage::from_raw(w, h, data)?), false))
        }
        _ => None,
    }
}

fn encode_jpeg(img: &Decoded, quality: u32) -> Option<Vec<u8>> {
    let q = (quality.clamp(1, 10) * 10).clamp(10, 95) as u8;
    let mut out = Cursor::new(Vec::new());
    let enc = JpegEncoder::new_with_quality(&mut out, q);
    match img {
        Decoded::Gray(g) => enc
            .write_image(
                g.as_raw(),
                g.width(),
                g.height(),
                image::ExtendedColorType::L8,
            )
            .ok()?,
        Decoded::Rgb(c) => enc
            .write_image(
                c.as_raw(),
                c.width(),
                c.height(),
                image::ExtendedColorType::Rgb8,
            )
            .ok()?,
    }
    Some(out.into_inner())
}

fn raw(img: &Decoded) -> (Vec<u8>, u32, u32) {
    match img {
        Decoded::Gray(g) => (g.as_raw().clone(), g.width(), g.height()),
        Decoded::Rgb(c) => (c.as_raw().clone(), c.width(), c.height()),
    }
}

fn resize(img: Decoded, w: u32, h: u32) -> Decoded {
    match img {
        Decoded::Gray(g) => Decoded::Gray(image::imageops::resize(&g, w, h, FilterType::Triangle)),
        Decoded::Rgb(c) => Decoded::Rgb(image::imageops::resize(&c, w, h, FilterType::Triangle)),
    }
}

pub fn optimize(doc: &mut Doc, color: ImageSettings, gray: ImageSettings) -> Res<Stats> {
    let mut placements: HashMap<ObjectId, (f64, f64)> = HashMap::new();
    for page in doc.page_ids() {
        let content = doc.inner.get_page_content(page);
        let res = inherited_in(&doc.inner, page, b"Resources")
            .and_then(|o| deref(&doc.inner, &o).as_dict().ok().cloned())
            .unwrap_or_default();
        collect_placements(&doc.inner, &content, &res, IDENTITY, 0, &mut placements);
    }

    let mut stats = Stats::default();
    for (id, (drawn_w, drawn_h)) in placements {
        let Ok(Object::Stream(s)) = doc.inner.get_object(id).cloned() else {
            continue;
        };
        if !colorspace_ok(&doc.inner, &s) {
            continue;
        }
        let Some((img, was_jpeg)) = decode(&s) else {
            continue;
        };
        stats.images += 1;
        let (_, w, h) = raw(&img);
        let settings = match img {
            Decoded::Gray(_) => gray,
            Decoded::Rgb(_) => color,
        };
        if settings.mode == NONE && !settings.force_changes {
            continue;
        }
        // Effective resolution: pixels per inch of the drawn size.
        let dpi =
            (w as f64 / (drawn_w / 72.0).max(1e-6)).max(h as f64 / (drawn_h / 72.0).max(1e-6));
        let mut img = img;
        let mut resized = false;
        if settings.downsample && dpi > settings.max_dpi && settings.resample_dpi > 0.0 {
            let f = settings.resample_dpi / dpi;
            let (nw, nh) = (
                ((w as f64) * f).round().max(1.0) as u32,
                ((h as f64) * f).round().max(1.0) as u32,
            );
            if nw < w && nh < h {
                img = resize(img, nw, nh);
                resized = true;
            }
        }
        let use_jpeg = match settings.mode {
            JPEG | JPEG2000 => true,
            FLATE | NONE => false,
            _ => was_jpeg, // retain
        };
        if !resized && !settings.force_recompression && use_jpeg == was_jpeg {
            continue; // nothing to gain
        }
        let mut dict = s.dict.clone();
        dict.remove(b"DecodeParms");
        let (new_stream, gray_cs) = if use_jpeg {
            let Some(bytes) = encode_jpeg(&img, settings.quality) else {
                continue;
            };
            dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            let mut st = Stream::new(dict, bytes);
            st.allows_compression = false;
            (st, matches!(img, Decoded::Gray(_)))
        } else {
            let (data, _, _) = raw(&img);
            dict.remove(b"Filter");
            let mut st = Stream::new(dict, data);
            let _ = st.compress();
            (st, matches!(img, Decoded::Gray(_)))
        };
        let (_, nw, nh) = raw(&img);
        let mut new_stream = new_stream;
        new_stream.dict.set("Width", Object::Integer(nw as i64));
        new_stream.dict.set("Height", Object::Integer(nh as i64));
        new_stream.dict.set("BitsPerComponent", Object::Integer(8));
        if !matches!(
            new_stream.dict.get(b"ColorSpace"),
            Ok(Object::Name(_)) | Ok(Object::Reference(_)) | Ok(Object::Array(_))
        ) {
            new_stream.dict.set(
                "ColorSpace",
                Object::Name(if gray_cs {
                    b"DeviceGray".to_vec()
                } else {
                    b"DeviceRGB".to_vec()
                }),
            );
        }
        let before = s.content.len();
        let after = new_stream.content.len();
        if !resized && after >= before && !settings.force_changes {
            continue;
        }
        stats.optimized += 1;
        stats.bytes_before += before;
        stats.bytes_after += after;
        doc.inner.objects.insert(id, Object::Stream(new_stream));
    }
    if stats.optimized > 0 {
        doc.touch();
    }
    Ok(stats)
}

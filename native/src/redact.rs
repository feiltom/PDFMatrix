//! True redaction: content under the redaction regions is removed from the file, not hidden.
//!
//! The page content stream is interpreted (graphics state, text state, font metrics) to locate
//! every glyph, image and path:
//! - glyphs overlapping a region are dropped from Tj/TJ/'/" operators and replaced by a TJ
//!   displacement, so the remaining text keeps its position;
//! - images overlapping a region have the covered pixels blanked (8-bit Flate/raw/DCT images)
//!   or are removed altogether (other encodings, inline images);
//! - paths entirely inside a region are not painted;
//! - Form XObjects are processed recursively (copied when modified);
//! - annotations overlapping a region are removed;
//! - an optional overlay (filled box, border, text) is drawn on top.

use std::collections::HashMap;
use std::io::Cursor;

use image::{ColorType, ImageDecoder};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::annots;
use crate::content::{Ops, Resources, concat, page_add_content, transform_rect};
use crate::doc::{Doc, Res, deref, err, inherited_in, number, rect_of};
use crate::fonts::{StdFont, wrap};

type M = [f64; 6];
const IDENTITY: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

pub struct Overlay {
    pub enabled: bool,
    pub color: Vec<f64>,
    pub border: bool,
    pub border_color: Vec<f64>,
    pub text: Option<String>,
    pub text_color: Vec<f64>,
    pub min_font_size: f64,
    pub max_font_size: f64,
    /// -1 left, 0 center, 1 right
    pub text_align: i32,
}

#[derive(Default, Debug)]
pub struct Stats {
    pub glyphs: usize,
    pub images: usize,
    pub paths: usize,
    pub annotations: usize,
}

fn overlaps(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let w = a[2].min(b[2]) - a[0].max(b[0]);
    let h = a[3].min(b[3]) - a[1].max(b[1]);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

/// A glyph is redacted when its center is inside a region, or when the region covers at least
/// 10% of its width and 30% of its height (glyph boxes use a generous ascent/descent, so the
/// height threshold keeps adjacent lines intact).
fn glyph_hit(g: &[f64; 4], rects: &[[f64; 4]]) -> bool {
    let (gw, gh) = ((g[2] - g[0]).max(1e-9), (g[3] - g[1]).max(1e-9));
    let (cx, cy) = ((g[0] + g[2]) / 2.0, (g[1] + g[3]) / 2.0);
    rects.iter().any(|r| {
        let w = r[2].min(g[2]) - r[0].max(g[0]);
        let h = r[3].min(g[3]) - r[1].max(g[1]);
        (cx > r[0] && cx < r[2] && cy > r[1] && cy < r[3]) || (w >= 0.1 * gw && h >= 0.3 * gh)
    })
}

fn inside(b: &[f64; 4], rects: &[[f64; 4]]) -> bool {
    rects.iter().any(|r| {
        b[0] >= r[0] - 0.01 && b[1] >= r[1] - 0.01 && b[2] <= r[2] + 0.01 && b[3] <= r[3] + 0.01
    })
}

fn any_overlap(b: &[f64; 4], rects: &[[f64; 4]]) -> bool {
    rects.iter().any(|r| overlaps(b, r) > 0.0)
}

// ------------------------------------------------------------------ fonts

struct FontMetrics {
    widths: HashMap<u32, f64>,
    default_width: f64,
    two_byte: bool,
    ascent: f64,
    descent: f64,
    /// Glyph space → text space scale (1/1000 for all but Type3).
    scale: f64,
}

impl FontMetrics {
    fn width(&self, code: u32) -> f64 {
        self.widths
            .get(&code)
            .copied()
            .unwrap_or(self.default_width)
    }
}

fn nums(doc: &Document, o: &Object) -> Vec<f64> {
    deref(doc, o)
        .as_array()
        .map(|a| a.iter().filter_map(|x| number(doc, x)).collect())
        .unwrap_or_default()
}

fn font_metrics(doc: &Document, font: &Dictionary) -> FontMetrics {
    let subtype = font
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .unwrap_or(b"");
    let base = font
        .get(b"BaseFont")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    let mut m = FontMetrics {
        widths: HashMap::new(),
        default_width: 500.0,
        two_byte: false,
        ascent: 800.0,
        descent: -200.0,
        scale: 0.001,
    };
    let mut descriptor = font
        .get(b"FontDescriptor")
        .ok()
        .and_then(|o| deref(doc, o).as_dict().ok());

    if subtype == b"Type0" {
        m.two_byte = true;
        let desc = font
            .get(b"DescendantFonts")
            .ok()
            .and_then(|o| deref(doc, o).as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| deref(doc, o).as_dict().ok());
        if let Some(d) = desc {
            m.default_width = d
                .get(b"DW")
                .ok()
                .and_then(|o| number(doc, o))
                .unwrap_or(1000.0);
            descriptor = d
                .get(b"FontDescriptor")
                .ok()
                .and_then(|o| deref(doc, o).as_dict().ok());
            // /W [c [w1 w2 …] | c_first c_last w …]
            if let Ok(w) = d.get(b"W").map(|o| deref(doc, o))
                && let Ok(items) = w.as_array()
            {
                let mut i = 0;
                while i < items.len() {
                    let Some(start) = number(doc, &items[i]) else {
                        break;
                    };
                    match items.get(i + 1).map(|o| deref(doc, o)) {
                        Some(Object::Array(ws)) => {
                            for (k, wv) in ws.iter().enumerate() {
                                if let Some(v) = number(doc, wv) {
                                    m.widths.insert(start as u32 + k as u32, v);
                                }
                            }
                            i += 2;
                        }
                        Some(end) => {
                            let (Some(end), Some(v)) = (
                                number(doc, end),
                                items.get(i + 2).and_then(|o| number(doc, o)),
                            ) else {
                                break;
                            };
                            for c in start as u32..=end as u32 {
                                m.widths.insert(c, v);
                            }
                            i += 3;
                        }
                        None => break,
                    }
                }
            }
        }
    } else {
        let first = font
            .get(b"FirstChar")
            .ok()
            .and_then(|o| number(doc, o))
            .unwrap_or(0.0) as u32;
        let widths = font
            .get(b"Widths")
            .map(|o| nums(doc, o))
            .unwrap_or_default();
        for (i, w) in widths.iter().enumerate() {
            m.widths.insert(first + i as u32, *w);
        }
        if widths.is_empty()
            && let Some(std) = StdFont::from_base_font(&base)
        {
            for code in 0u32..256 {
                let w = crate::fonts::std_width(std, code as u8);
                if w > 0.0 {
                    m.widths.insert(code, w);
                }
            }
        }
        if subtype == b"Type3" {
            let fm = font
                .get(b"FontMatrix")
                .map(|o| nums(doc, o))
                .unwrap_or_default();
            m.scale = fm.first().copied().unwrap_or(0.001);
            m.ascent = 1.0 / m.scale.abs().max(1e-9) * 0.8;
            m.descent = -1.0 / m.scale.abs().max(1e-9) * 0.2;
        }
    }
    if let Some(d) = descriptor {
        if let Some(missing) = d.get(b"MissingWidth").ok().and_then(|o| number(doc, o))
            && !m.two_byte
        {
            m.default_width = missing;
        }
        // Be generous vertically so that glyphs are never missed.
        if let Some(a) = d
            .get(b"Ascent")
            .ok()
            .and_then(|o| number(doc, o))
            .filter(|a| *a > 0.0)
        {
            m.ascent = m.ascent.max(a);
        }
        if let Some(dsc) = d
            .get(b"Descent")
            .ok()
            .and_then(|o| number(doc, o))
            .filter(|d| *d < 0.0)
        {
            m.descent = m.descent.min(dsc);
        }
    }
    m
}

// ------------------------------------------------------------------ interpreter

#[derive(Clone)]
struct GState {
    ctm: M,
    font: Option<Vec<u8>>,
    size: f64,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    rise: f64,
}

impl Default for GState {
    fn default() -> Self {
        GState {
            ctm: IDENTITY,
            font: None,
            size: 0.0,
            tc: 0.0,
            tw: 0.0,
            th: 1.0,
            tl: 0.0,
            rise: 0.0,
        }
    }
}

struct Ctx<'a> {
    regions: &'a [[f64; 4]],
    stats: &'a mut Stats,
    depth: u32,
}

/// Result of redacting one content stream: new operations and the (possibly updated) resources.
struct Redacted {
    ops: Vec<Operation>,
    resources: Dictionary,
    changed: bool,
}

fn n(o: &Object) -> f64 {
    match o {
        Object::Integer(i) => *i as f64,
        Object::Real(r) => *r as f64,
        _ => 0.0,
    }
}

fn redact_ops(
    doc: &mut Document,
    ops: Vec<Operation>,
    resources: Dictionary,
    base: M,
    cx: &mut Ctx,
) -> Res<Redacted> {
    let mut out = Vec::with_capacity(ops.len());
    let mut res = resources;
    let mut changed = false;
    let mut gs = GState {
        ctm: base,
        ..Default::default()
    };
    let mut stack: Vec<GState> = Vec::new();
    let mut tm = IDENTITY;
    let mut tlm = IDENTITY;
    let mut fonts: HashMap<Vec<u8>, FontMetrics> = HashMap::new();
    let mut path_box: Option<[f64; 4]> = None;

    let font_dict = |doc: &Document, res: &Dictionary, name: &[u8]| -> Option<Dictionary> {
        let fonts = deref(doc, res.get(b"Font").ok()?).as_dict().ok()?;
        deref(doc, fonts.get(name).ok()?).as_dict().ok().cloned()
    };

    for op in ops {
        let o = &op.operands;
        match op.operator.as_str() {
            "q" => stack.push(gs.clone()),
            "Q" => {
                if let Some(s) = stack.pop() {
                    gs = s;
                }
            }
            "cm" if o.len() == 6 => {
                let m = [n(&o[0]), n(&o[1]), n(&o[2]), n(&o[3]), n(&o[4]), n(&o[5])];
                gs.ctm = concat(m, gs.ctm);
            }
            "BT" => {
                tm = IDENTITY;
                tlm = IDENTITY;
            }
            "Tf" if o.len() == 2 => {
                gs.font = o[0].as_name().ok().map(|x| x.to_vec());
                gs.size = n(&o[1]);
                if let Some(name) = &gs.font
                    && !fonts.contains_key(name)
                {
                    let fm = font_dict(doc, &res, name)
                        .map(|d| font_metrics(doc, &d))
                        .unwrap_or(FontMetrics {
                            widths: HashMap::new(),
                            default_width: 500.0,
                            two_byte: false,
                            ascent: 800.0,
                            descent: -200.0,
                            scale: 0.001,
                        });
                    fonts.insert(name.clone(), fm);
                }
            }
            "Tc" if o.len() == 1 => gs.tc = n(&o[0]),
            "Tw" if o.len() == 1 => gs.tw = n(&o[0]),
            "Tz" if o.len() == 1 => gs.th = n(&o[0]) / 100.0,
            "TL" if o.len() == 1 => gs.tl = n(&o[0]),
            "Ts" if o.len() == 1 => gs.rise = n(&o[0]),
            "Td" if o.len() == 2 => {
                tlm = concat([1.0, 0.0, 0.0, 1.0, n(&o[0]), n(&o[1])], tlm);
                tm = tlm;
            }
            "TD" if o.len() == 2 => {
                gs.tl = -n(&o[1]);
                tlm = concat([1.0, 0.0, 0.0, 1.0, n(&o[0]), n(&o[1])], tlm);
                tm = tlm;
            }
            "Tm" if o.len() == 6 => {
                tlm = [n(&o[0]), n(&o[1]), n(&o[2]), n(&o[3]), n(&o[4]), n(&o[5])];
                tm = tlm;
            }
            "T*" => {
                tlm = concat([1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], tlm);
                tm = tlm;
            }
            "Tj" | "TJ" | "'" | "\"" => {
                let mut prefix = Vec::new();
                let elements: Vec<Object> = match op.operator.as_str() {
                    "Tj" | "'" if !o.is_empty() => vec![o[o.len() - 1].clone()],
                    "\"" if o.len() == 3 => {
                        gs.tw = n(&o[0]);
                        gs.tc = n(&o[1]);
                        prefix.push(Operation::new("Tw", vec![o[0].clone()]));
                        prefix.push(Operation::new("Tc", vec![o[1].clone()]));
                        vec![o[2].clone()]
                    }
                    "TJ" if !o.is_empty() => o[0].as_array().cloned().unwrap_or_default(),
                    _ => vec![],
                };
                if matches!(op.operator.as_str(), "'" | "\"") {
                    tlm = concat([1.0, 0.0, 0.0, 1.0, 0.0, -gs.tl], tlm);
                    tm = tlm;
                    prefix.push(Operation::new("T*", vec![]));
                }
                let fm = gs.font.as_ref().and_then(|f| fonts.get(f));
                let (new_elems, removed) = match fm {
                    Some(fm) => redact_text(&elements, fm, &gs, &mut tm, cx.regions),
                    None => (elements.clone(), 0),
                };
                if removed > 0 {
                    cx.stats.glyphs += removed;
                    changed = true;
                    out.extend(prefix);
                    out.push(Operation::new("TJ", vec![Object::Array(new_elems)]));
                } else {
                    out.push(op);
                }
                continue;
            }
            "m" if o.len() == 2 => {
                path_box = Some(extend(
                    path_box,
                    crate::content::apply(gs.ctm, n(&o[0]), n(&o[1])),
                ));
            }
            "l" if o.len() == 2 => {
                path_box = Some(extend(
                    path_box,
                    crate::content::apply(gs.ctm, n(&o[0]), n(&o[1])),
                ));
            }
            "c" | "v" | "y" if o.len() >= 4 => {
                for pair in o.as_chunks::<2>().0 {
                    path_box = Some(extend(
                        path_box,
                        crate::content::apply(gs.ctm, n(&pair[0]), n(&pair[1])),
                    ));
                }
            }
            "re" if o.len() == 4 => {
                let b = transform_rect(
                    gs.ctm,
                    [n(&o[0]), n(&o[1]), n(&o[0]) + n(&o[2]), n(&o[1]) + n(&o[3])],
                );
                path_box = Some(extend(extend_opt(path_box, (b[0], b[1])), (b[2], b[3])));
            }
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                let pb = path_box.take();
                if pb.is_some_and(|b| inside(&b, cx.regions)) {
                    cx.stats.paths += 1;
                    changed = true;
                    out.push(Operation::new("n", vec![]));
                    continue;
                }
            }
            "n" => path_box = None,
            "BI" => {
                let b = transform_rect(gs.ctm, [0.0, 0.0, 1.0, 1.0]);
                if any_overlap(&b, cx.regions) {
                    cx.stats.images += 1;
                    changed = true;
                    continue;
                }
            }
            "Do" if o.len() == 1 => {
                if let Ok(name) = o[0].as_name() {
                    let name = name.to_vec();
                    if let Some((xid, replacement)) = redact_xobject(doc, &res, &name, gs.ctm, cx)?
                    {
                        changed = true;
                        match replacement {
                            None => continue, // removed
                            Some(new_id) => {
                                let new_name = rename_xobject(doc, &mut res, &name, xid, new_id);
                                out.push(Operation::new("Do", vec![Object::Name(new_name)]));
                                continue;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        out.push(op);
    }
    Ok(Redacted {
        ops: out,
        resources: res,
        changed,
    })
}

fn extend(b: Option<[f64; 4]>, p: (f64, f64)) -> [f64; 4] {
    match b {
        None => [p.0, p.1, p.0, p.1],
        Some(b) => [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)],
    }
}

fn extend_opt(b: Option<[f64; 4]>, p: (f64, f64)) -> Option<[f64; 4]> {
    Some(extend(b, p))
}

/// Removes redacted glyphs from text-showing elements (strings and TJ numbers), advancing `tm`
/// exactly as the original operator would. Returns the new TJ elements and the removal count.
fn redact_text(
    elements: &[Object],
    fm: &FontMetrics,
    gs: &GState,
    tm: &mut M,
    regions: &[[f64; 4]],
) -> (Vec<Object>, usize) {
    let mut out: Vec<Object> = Vec::new();
    let mut removed = 0;
    let mut pending_shift = 0.0; // accumulated TJ units for removed glyphs
    let fs = gs.size;
    let flush_shift = |out: &mut Vec<Object>, shift: &mut f64| {
        if shift.abs() > 1e-9 {
            out.push(Object::Real(*shift as f32));
            *shift = 0.0;
        }
    };
    for el in elements {
        match el {
            Object::String(bytes, fmt) => {
                let step = if fm.two_byte { 2 } else { 1 };
                let mut kept: Vec<u8> = Vec::new();
                for chunk in bytes.chunks(step) {
                    let code = chunk.iter().fold(0u32, |a, b| (a << 8) | *b as u32);
                    let w0 = fm.width(code) * fm.scale; // text space units per 1 font size
                    let space = !fm.two_byte && code == 32;
                    let tx = (w0 * fs + gs.tc + if space { gs.tw } else { 0.0 }) * gs.th;
                    // Glyph box in text space, then to user space.
                    let trm = concat(
                        [fs * gs.th, 0.0, 0.0, fs, 0.0, gs.rise],
                        concat(*tm, gs.ctm),
                    );
                    let gb = transform_rect(
                        trm,
                        [
                            0.0,
                            fm.descent * fm.scale,
                            w0.max(0.0001),
                            fm.ascent * fm.scale,
                        ],
                    );
                    if glyph_hit(&gb, regions) && !space {
                        removed += 1;
                        if !kept.is_empty() {
                            flush_shift(&mut out, &mut pending_shift);
                            out.push(Object::String(std::mem::take(&mut kept), *fmt));
                        }
                        // TJ number n moves by -n/1000 * fs * th
                        if fs.abs() > 1e-9 && gs.th.abs() > 1e-9 {
                            pending_shift -= tx / gs.th * 1000.0 / fs;
                        }
                    } else {
                        if kept.is_empty() {
                            flush_shift(&mut out, &mut pending_shift);
                        }
                        kept.extend_from_slice(chunk);
                    }
                    *tm = concat([1.0, 0.0, 0.0, 1.0, tx, 0.0], *tm);
                }
                if !kept.is_empty() {
                    out.push(Object::String(kept, *fmt));
                }
            }
            other => {
                let adj = n(other);
                *tm = concat([1.0, 0.0, 0.0, 1.0, -adj / 1000.0 * fs * gs.th, 0.0], *tm);
                pending_shift += adj;
            }
        }
    }
    flush_shift(&mut out, &mut pending_shift);
    (out, removed)
}

/// Handles `Do name`: returns None when the XObject is untouched, Some((id, None)) when it must
/// be removed, Some((id, Some(new_id))) when it was replaced by a redacted copy.
fn redact_xobject(
    doc: &mut Document,
    res: &Dictionary,
    name: &[u8],
    ctm: M,
    cx: &mut Ctx,
) -> Res<Option<(ObjectId, Option<ObjectId>)>> {
    let Some(xid) = res
        .get(b"XObject")
        .ok()
        .and_then(|o| deref(doc, o).as_dict().ok())
        .and_then(|x| x.get(name).ok())
        .and_then(|o| o.as_reference().ok())
    else {
        return Ok(None);
    };
    let Ok(Object::Stream(stream)) = doc.get_object(xid).cloned() else {
        return Ok(None);
    };
    let subtype = stream
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .unwrap_or(b"")
        .to_vec();
    match subtype.as_slice() {
        b"Image" => {
            let b = transform_rect(ctm, [0.0, 0.0, 1.0, 1.0]);
            if !any_overlap(&b, cx.regions) {
                return Ok(None);
            }
            cx.stats.images += 1;
            if inside(&b, cx.regions) {
                return Ok(Some((xid, None)));
            }
            match blank_image(&stream, ctm, cx.regions) {
                Some(new_stream) => Ok(Some((xid, Some(doc.add_object(new_stream))))),
                None => Ok(Some((xid, None))),
            }
        }
        b"Form" => {
            if cx.depth > 8 {
                return Ok(None);
            }
            let matrix: M = stream
                .dict
                .get(b"Matrix")
                .map(|o| nums(doc, o))
                .ok()
                .and_then(|v| v.try_into().ok())
                .unwrap_or(IDENTITY);
            let bbox = stream
                .dict
                .get(b"BBox")
                .ok()
                .and_then(|o| rect_of(doc, o))
                .unwrap_or([0.0; 4]);
            let form_ctm = concat(matrix, ctm);
            if !any_overlap(&transform_rect(form_ctm, bbox), cx.regions) {
                return Ok(None);
            }
            let content = stream
                .decompressed_content()
                .unwrap_or_else(|_| stream.content.clone());
            let Ok(parsed) = Content::decode(&content) else {
                return Ok(None);
            };
            let form_res = stream
                .dict
                .get(b"Resources")
                .ok()
                .and_then(|o| deref(doc, o).as_dict().ok())
                .cloned()
                .unwrap_or_else(|| res.clone());
            cx.depth += 1;
            let r = redact_ops(doc, parsed.operations, form_res, form_ctm, cx)?;
            cx.depth -= 1;
            if !r.changed {
                return Ok(None);
            }
            let mut dict = stream.dict.clone();
            dict.remove(b"Filter");
            dict.remove(b"DecodeParms");
            dict.set("Resources", Object::Dictionary(r.resources));
            let bytes = Content { operations: r.ops }
                .encode()
                .map_err(err("cannot encode form content"))?;
            let mut s = Stream::new(dict, bytes);
            let _ = s.compress();
            Ok(Some((xid, Some(doc.add_object(s)))))
        }
        _ => Ok(None),
    }
}

/// Points `name` at `new_id` in a copy of the XObject resources (the original object may be
/// shared with other pages) and returns the name to use.
fn rename_xobject(
    doc: &Document,
    res: &mut Dictionary,
    name: &[u8],
    _old: ObjectId,
    new_id: ObjectId,
) -> Vec<u8> {
    let mut xobjects = res
        .get(b"XObject")
        .ok()
        .and_then(|o| deref(doc, o).as_dict().ok())
        .cloned()
        .unwrap_or_default();
    let new_name = format!("{}R", String::from_utf8_lossy(name)).into_bytes();
    xobjects.set(new_name.clone(), Object::Reference(new_id));
    res.set("XObject", Object::Dictionary(xobjects));
    new_name
}

/// Blanks (sets to black) the pixels of an image covered by the regions. Supports 8-bit
/// Gray/RGB/CMYK images stored raw, Flate-compressed or as JPEG (DCT).
fn blank_image(stream: &Stream, ctm: M, regions: &[[f64; 4]]) -> Option<Stream> {
    let w = stream.dict.get(b"Width").ok()?.as_i64().ok()? as usize;
    let h = stream.dict.get(b"Height").ok()?.as_i64().ok()? as usize;
    let filters: Vec<Vec<u8>> = match stream.dict.get(b"Filter") {
        Ok(Object::Name(n)) => vec![n.clone()],
        Ok(Object::Array(a)) => a
            .iter()
            .filter_map(|o| o.as_name().ok().map(|n| n.to_vec()))
            .collect(),
        _ => vec![],
    };
    let (mut pixels, comps) = if filters.iter().any(|f| f == b"DCTDecode") {
        if filters.len() != 1 {
            return None;
        }
        let dec = image::codecs::jpeg::JpegDecoder::new(Cursor::new(&stream.content)).ok()?;
        let comps = match dec.color_type() {
            ColorType::L8 => 1,
            ColorType::Rgb8 => 3,
            _ => return None,
        };
        let mut buf = vec![0u8; dec.total_bytes() as usize];
        dec.read_image(&mut buf).ok()?;
        (buf, comps)
    } else {
        if stream
            .dict
            .get(b"BitsPerComponent")
            .ok()
            .and_then(|o| o.as_i64().ok())
            != Some(8)
        {
            return None;
        }
        if !filters.iter().all(|f| f == b"FlateDecode") {
            return None;
        }
        let data = if filters.is_empty() {
            stream.content.clone()
        } else {
            stream.decompressed_content().ok()?
        };
        let comps = data.len() / (w * h).max(1);
        if !(1..=4).contains(&comps) || data.len() != w * h * comps {
            return None;
        }
        (data, comps)
    };
    // Pixel (col,row) covers image space [col/w, (col+1)/w] x [1-(row+1)/h, 1-row/h].
    for row in 0..h {
        for col in 0..w {
            let px = transform_rect(
                ctm,
                [
                    col as f64 / w as f64,
                    1.0 - (row + 1) as f64 / h as f64,
                    (col + 1) as f64 / w as f64,
                    1.0 - row as f64 / h as f64,
                ],
            );
            if any_overlap(&px, regions) {
                let o = (row * w + col) * comps;
                // Black in Gray/RGB is 0; in CMYK it is full K.
                for c in 0..comps {
                    pixels[o + c] = if comps == 4 {
                        if c == 3 { 255 } else { 0 }
                    } else {
                        0
                    };
                }
            }
        }
    }
    let mut dict = stream.dict.clone();
    dict.remove(b"Filter");
    dict.remove(b"DecodeParms");
    if comps != 4 || !dict.has(b"ColorSpace") {
        dict.set(
            "ColorSpace",
            Object::Name(if comps == 1 {
                b"DeviceGray".to_vec()
            } else if comps == 3 {
                b"DeviceRGB".to_vec()
            } else {
                b"DeviceCMYK".to_vec()
            }),
        );
    }
    dict.set("BitsPerComponent", Object::Integer(8));
    let mut s = Stream::new(dict, pixels);
    let _ = s.compress();
    Some(s)
}

// ------------------------------------------------------------------ page level

pub fn redact_page(
    doc: &mut Doc,
    page: ObjectId,
    regions: &[[f64; 4]],
    overlay: &Overlay,
) -> Res<Stats> {
    let mut stats = Stats::default();
    if regions.is_empty() {
        return Ok(stats);
    }
    let content = doc.inner.get_page_content(page);
    let parsed = Content::decode(&content).map_err(err("cannot parse page content"))?;
    let resources = inherited_in(&doc.inner, page, b"Resources")
        .and_then(|o| deref(&doc.inner, &o).as_dict().ok().cloned())
        .unwrap_or_default();
    let mut cx = Ctx {
        regions,
        stats: &mut stats,
        depth: 0,
    };
    let r = redact_ops(
        &mut doc.inner,
        parsed.operations,
        resources,
        IDENTITY,
        &mut cx,
    )?;
    if r.changed {
        let bytes = Content { operations: r.ops }
            .encode()
            .map_err(err("cannot encode page content"))?;
        let mut s = Stream::new(Dictionary::new(), bytes);
        let _ = s.compress();
        let id = doc.inner.add_object(s);
        let pd = doc
            .inner
            .get_dictionary_mut(page)
            .map_err(err("invalid page"))?;
        pd.set("Contents", Object::Reference(id));
        pd.set("Resources", Object::Dictionary(r.resources));
    }

    // Annotations under the regions go too.
    for a in annots::page_annots(&doc.inner, page) {
        let rect = doc
            .inner
            .get_dictionary(a)
            .ok()
            .and_then(|d| d.get(b"Rect").ok())
            .and_then(|o| rect_of(&doc.inner, o));
        if rect.is_some_and(|r| any_overlap(&r, regions)) {
            annots::remove_from_page(doc, page, a)?;
            stats.annotations += 1;
        }
    }

    if overlay.enabled {
        draw_overlay(doc, page, regions, overlay)?;
    }
    doc.touch();
    Ok(stats)
}

fn draw_overlay(doc: &mut Doc, page: ObjectId, regions: &[[f64; 4]], o: &Overlay) -> Res<()> {
    let mut res = Resources::default();
    let mut ops = Ops::new();
    ops.q();
    let font = doc.std_font(StdFont::Helvetica);
    for r in regions {
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        if !o.color.is_empty() {
            ops.fill(&o.color).rect(r[0], r[1], w, h).op("f");
        }
        if o.border {
            let bc = if o.border_color.is_empty() {
                vec![1.0, 0.0, 0.0]
            } else {
                o.border_color.clone()
            };
            ops.stroke(&bc)
                .line_width(1.0)
                .rect(r[0] + 0.5, r[1] + 0.5, w - 1.0, h - 1.0)
                .op("S");
        }
        if let Some(text) = o.text.as_deref().filter(|t| !t.is_empty()) {
            let fname = res.font(doc.fonts[font].obj);
            let unit = doc.fonts[font].width(text) / 1000.0;
            let asc = doc.fonts[font].ascender() / 1000.0;
            let desc = doc.fonts[font].descender() / 1000.0;
            let size = ((w - 2.0) / unit.max(0.01))
                .min((h - 1.0) / (asc - desc))
                .clamp(o.min_font_size, o.max_font_size);
            let lines = wrap(&doc.fonts[font], size, text, Some(w - 2.0));
            let line_h = (asc - desc) * size;
            let total = line_h * lines.len() as f64;
            ops.q()
                .rect(r[0], r[1], w, h)
                .raw("W n")
                .begin_text()
                .font(&fname, size)
                .fill(&o.text_color);
            for (i, line) in lines.iter().enumerate() {
                let lw = doc.fonts[font].width(line) * size / 1000.0;
                let x = match o.text_align {
                    -1 => r[0] + 1.0,
                    1 => r[2] - 1.0 - lw,
                    _ => r[0] + (w - lw) / 2.0,
                };
                let y = r[1] + (h + total) / 2.0 - asc * size - i as f64 * line_h;
                let enc = doc.fonts[font].encode(line);
                ops.text_matrix([1.0, 0.0, 0.0, 1.0, x, y]).show(&enc);
            }
            ops.end_text().Q();
        }
    }
    ops.Q();
    // Overlay resources must live in the page resources (they are shared by this content stream).
    let font_obj = doc.fonts[font].obj;
    let sub = crate::content::page_resource_sub(&mut doc.inner, page, "Font")?;
    for (k, v) in res
        .dict
        .get(b"Font")
        .ok()
        .and_then(|o| o.as_dict().ok())
        .cloned()
        .unwrap_or_default()
    {
        if !sub.has(&k) {
            sub.set(k, v);
        } else if sub.get(&k).ok().and_then(|x| x.as_reference().ok()) != Some(font_obj) {
            // Name clash: give our font a unique name and rewrite the operators accordingly.
            let unique = crate::content::unique_name(sub, "BWF");
            sub.set(unique.as_bytes(), Object::Reference(font_obj));
            ops.buf = ops.buf.replace(
                &format!("/{} ", String::from_utf8_lossy(&k)),
                &format!("/{unique} "),
            );
        }
    }
    page_add_content(&mut doc.inner, page, ops.into_bytes(), true, None)?;
    Ok(())
}

/// Applies (then removes) all /Redact annotations of a page, using their /QuadPoints or /Rect,
/// /IC as fill color and /OverlayText.
pub fn apply_redact_annotations(doc: &mut Doc, page: ObjectId) -> Res<Stats> {
    let mut total = Stats::default();
    for a in annots::page_annots(&doc.inner, page) {
        let Ok(d) = doc.inner.get_dictionary(a).cloned() else {
            continue;
        };
        if d.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) != Some(b"Redact") {
            continue;
        }
        let quads = d
            .get(b"QuadPoints")
            .map(|o| nums(&doc.inner, o))
            .unwrap_or_default();
        let mut regions: Vec<[f64; 4]> = quads
            .as_chunks::<8>()
            .0
            .iter()
            .map(|q| {
                let xs = [q[0], q[2], q[4], q[6]];
                let ys = [q[1], q[3], q[5], q[7]];
                [
                    xs.iter().cloned().fold(f64::INFINITY, f64::min),
                    ys.iter().cloned().fold(f64::INFINITY, f64::min),
                    xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                    ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                ]
            })
            .collect();
        if regions.is_empty()
            && let Some(r) = d.get(b"Rect").ok().and_then(|o| rect_of(&doc.inner, o))
        {
            regions.push(r);
        }
        annots::remove_from_page(doc, page, a)?;
        let color = d
            .get(b"IC")
            .map(|o| nums(&doc.inner, o))
            .unwrap_or_else(|_| vec![0.0]);
        let overlay = Overlay {
            enabled: true,
            color: if color.is_empty() { vec![0.0] } else { color },
            border: false,
            border_color: vec![],
            text: d
                .get(b"OverlayText")
                .ok()
                .and_then(crate::strings::object_to_text),
            text_color: vec![1.0],
            min_font_size: 2.0,
            max_font_size: 24.0,
            text_align: 0,
        };
        let s = redact_page(doc, page, &regions, &overlay)?;
        total.glyphs += s.glyphs;
        total.images += s.images;
        total.paths += s.paths;
        total.annotations += s.annotations + 1;
    }
    Ok(total)
}

//! Appearance streams (/AP /N) for markup annotations.
//!
//! The appearance BBox is the annotation /Rect in page space with an identity /Matrix, so all
//! drawing below uses page coordinates. Geometry-based annotations (lines, polygons, ink,
//! text markup) get their /Rect recomputed from their points.

use lopdf::{Dictionary, Object, ObjectId};

use crate::content::{Ops, Resources, form_xobject};
use crate::doc::{Doc, Res, deref, err, number, real, rect_of};
use crate::fonts::{StdFont, wrap};

pub fn nums(doc: &lopdf::Document, d: &Dictionary, key: &[u8]) -> Vec<f64> {
    d.get(key)
        .ok()
        .and_then(|o| deref(doc, o).as_array().ok())
        .map(|a| a.iter().filter_map(|o| number(doc, o)).collect())
        .unwrap_or_default()
}

fn name(d: &Dictionary, key: &[u8]) -> Option<String> {
    d.get(key)
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
}

pub struct Style {
    pub color: Vec<f64>,
    pub interior: Vec<f64>,
    pub width: f64,
    pub dash: Option<Vec<f64>>,
    pub opacity: f64,
}

pub fn style(doc: &lopdf::Document, d: &Dictionary) -> Style {
    let bs = d.get(b"BS").ok().and_then(|o| deref(doc, o).as_dict().ok());
    let width = bs
        .and_then(|b| b.get(b"W").ok())
        .and_then(|o| number(doc, o))
        .or_else(|| nums(doc, d, b"Border").get(2).copied())
        .unwrap_or(1.0);
    let dashed = bs.and_then(|b| name(b, b"S")).as_deref() == Some("D");
    let dash = dashed.then(|| {
        let p = bs.map(|b| nums(doc, b, b"D")).unwrap_or_default();
        if p.is_empty() { vec![3.0] } else { p }
    });
    Style {
        color: nums(doc, d, b"C"),
        interior: nums(doc, d, b"IC"),
        width,
        dash,
        opacity: d
            .get(b"CA")
            .ok()
            .and_then(|o| number(doc, o))
            .unwrap_or(1.0),
    }
}

fn union_points(points: &[(f64, f64)], margin: f64) -> [f64; 4] {
    let mut r = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for &(x, y) in points {
        r = [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)];
    }
    [r[0] - margin, r[1] - margin, r[2] + margin, r[3] + margin]
}

fn pairs(v: &[f64]) -> Vec<(f64, f64)> {
    v.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).collect()
}

/// Quads as axis-aligned boxes [x1 y1 x2 y2].
fn quad_boxes(q: &[f64]) -> Vec<[f64; 4]> {
    q.as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            let xs = [c[0], c[2], c[4], c[6]];
            let ys = [c[1], c[3], c[5], c[7]];
            [
                xs.iter().cloned().fold(f64::INFINITY, f64::min),
                ys.iter().cloned().fold(f64::INFINITY, f64::min),
                xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            ]
        })
        .collect()
}

fn apply_stroke_style(ops: &mut Ops, s: &Style) {
    ops.line_width(s.width);
    if let Some(d) = &s.dash {
        ops.dash(d, 0.0);
    }
}

/// Draws a line ending at `tip`, pointing in direction (from → tip).
fn line_ending(
    ops: &mut Ops,
    kind: &str,
    tip: (f64, f64),
    from: (f64, f64),
    size: f64,
    filled: bool,
) {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let back = (tip.0 - ux * size, tip.1 - uy * size);
    let half = size * 0.5;
    match kind {
        "OpenArrow" | "ClosedArrow" => {
            ops.move_to(back.0 + px * half, back.1 + py * half)
                .line_to(tip.0, tip.1)
                .line_to(back.0 - px * half, back.1 - py * half);
            if kind == "ClosedArrow" {
                ops.close().op(if filled { "B" } else { "S" });
            } else {
                ops.op("S");
            }
        }
        "Circle" => {
            ops.ellipse(tip.0 - half, tip.1 - half, tip.0 + half, tip.1 + half)
                .op(if filled { "B" } else { "S" });
        }
        "Square" => {
            ops.rect(tip.0 - half, tip.1 - half, size, size)
                .op(if filled { "B" } else { "S" });
        }
        "Diamond" => {
            ops.move_to(tip.0 + ux * half, tip.1 + uy * half)
                .line_to(tip.0 + px * half, tip.1 + py * half)
                .line_to(tip.0 - ux * half, tip.1 - uy * half)
                .line_to(tip.0 - px * half, tip.1 - py * half)
                .close()
                .op(if filled { "B" } else { "S" });
        }
        "Butt" => {
            ops.move_to(tip.0 + px * half, tip.1 + py * half)
                .line_to(tip.0 - px * half, tip.1 - py * half)
                .op("S");
        }
        "Slash" => {
            let (sx, sy) = (px * half + ux * half * 0.6, py * half + uy * half * 0.6);
            ops.move_to(tip.0 + sx, tip.1 + sy)
                .line_to(tip.0 - sx, tip.1 - sy)
                .op("S");
        }
        _ => {}
    }
}

/// Default appearance string parsing: font resource name, size and fill color.
pub fn parse_da(da: &str) -> (Option<String>, f64, Vec<f64>) {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let mut font = None;
    let mut size = 0.0;
    let mut color = vec![0.0];
    for (i, t) in tokens.iter().enumerate() {
        match *t {
            "Tf" if i >= 2 => {
                font = Some(tokens[i - 2].trim_start_matches('/').to_string());
                size = tokens[i - 1].parse().unwrap_or(0.0);
            }
            "g" if i >= 1 => color = vec![tokens[i - 1].parse().unwrap_or(0.0)],
            "rg" if i >= 3 => {
                color = tokens[i - 3..i]
                    .iter()
                    .map(|v| v.parse().unwrap_or(0.0))
                    .collect()
            }
            "k" if i >= 4 => {
                color = tokens[i - 4..i]
                    .iter()
                    .map(|v| v.parse().unwrap_or(0.0))
                    .collect()
            }
            _ => {}
        }
    }
    (font, size, color)
}

/// Writes text lines inside `area` ([x1 y1 x2 y2]), top-aligned, with quadding 0/1/2.
#[allow(clippy::too_many_arguments)]
pub fn text_block(
    doc: &mut Doc,
    ops: &mut Ops,
    res: &mut Resources,
    font_idx: usize,
    size: f64,
    color: &[f64],
    text: &str,
    area: [f64; 4],
    quadding: i64,
    wrap_lines: bool,
) {
    let font_obj = doc.fonts[font_idx].obj;
    let font_name = res.font(font_obj);
    let width = area[2] - area[0];
    let lines = wrap(
        &doc.fonts[font_idx],
        size,
        text,
        wrap_lines.then_some(width),
    );
    let asc = doc.fonts[font_idx].ascender() * size / 1000.0;
    let desc = doc.fonts[font_idx].descender() * size / 1000.0;
    let leading = (asc - desc) * 1.15;
    ops.begin_text().font(&font_name, size).fill(color);
    for (i, line) in lines.iter().enumerate() {
        let w = doc.fonts[font_idx].width(line) * size / 1000.0;
        let x = match quadding {
            1 => area[0] + (width - w) / 2.0,
            2 => area[2] - w,
            _ => area[0],
        };
        let y = area[3] - asc - i as f64 * leading;
        if y + asc < area[1] {
            break;
        }
        let encoded = doc.fonts[font_idx].encode(line);
        ops.text_matrix([1.0, 0.0, 0.0, 1.0, x, y]).show(&encoded);
    }
    ops.end_text();
}

fn stamp_label(icon: &str) -> (String, Vec<f64>) {
    let mut label = String::new();
    for (i, c) in icon.trim_start_matches('#').chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            label.push(' ');
        }
        label.push(c.to_ascii_uppercase());
    }
    let color = match icon {
        "Approved" | "Final" | "Completed" | "ForPublicRelease" => vec![0.13, 0.55, 0.13],
        "Draft" | "ForComment" | "Experimental" | "Departmental" | "AsIs" => vec![0.15, 0.3, 0.7],
        _ => vec![0.8, 0.1, 0.1],
    };
    (
        if label.is_empty() {
            "DRAFT".into()
        } else {
            label
        },
        color,
    )
}

/// Generates the normal appearance of an annotation. Returns false for unsupported types
/// (the annotation is left untouched).
pub fn generate(doc: &mut Doc, annot: ObjectId) -> Res<bool> {
    let d = doc
        .inner
        .get_dictionary(annot)
        .map_err(err("invalid annotation"))?
        .clone();
    let subtype = name(&d, b"Subtype").unwrap_or_default();
    let s = style(&doc.inner, &d);
    let mut rect = d
        .get(b"Rect")
        .ok()
        .and_then(|o| rect_of(&doc.inner, o))
        .unwrap_or([0.0; 4]);
    let mut ops = Ops::new();
    let mut res = Resources::default();
    let stroke_color = if s.color.is_empty() {
        vec![0.0]
    } else {
        s.color.clone()
    };

    ops.q();
    if s.opacity < 1.0 {
        let blend = (subtype == "Highlight").then_some("Multiply");
        let gs = res.opacity(s.opacity, blend);
        ops.gs(&gs);
    } else if subtype == "Highlight" {
        let gs = res.opacity(1.0, Some("Multiply"));
        ops.gs(&gs);
    }

    match subtype.as_str() {
        "Square" | "Circle" => {
            let h = s.width / 2.0;
            let r = [rect[0] + h, rect[1] + h, rect[2] - h, rect[3] - h];
            apply_stroke_style(&mut ops, &s);
            ops.stroke(&stroke_color).fill(&s.interior);
            if subtype == "Square" {
                ops.rect(r[0], r[1], r[2] - r[0], r[3] - r[1]);
            } else {
                ops.ellipse(r[0], r[1], r[2], r[3]);
            }
            let fill = !s.interior.is_empty();
            let stroke = s.width > 0.0 && !s.color.is_empty();
            ops.op(match (fill, stroke) {
                (true, true) => "B",
                (true, false) => "f",
                (false, true) => "S",
                _ => "n",
            });
        }
        "Line" => {
            let l = nums(&doc.inner, &d, b"L");
            if l.len() == 4 {
                let (a, b) = ((l[0], l[1]), (l[2], l[3]));
                let le: Vec<String> = d
                    .get(b"LE")
                    .ok()
                    .and_then(|o| o.as_array().ok())
                    .map(|a| {
                        a.iter()
                            .filter_map(|n| n.as_name().ok())
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                let size = (s.width * 4.0).max(6.0);
                apply_stroke_style(&mut ops, &s);
                ops.stroke(&stroke_color).fill(if s.interior.is_empty() {
                    &stroke_color
                } else {
                    &s.interior
                });
                ops.move_to(a.0, a.1).line_to(b.0, b.1).op("S");
                ops.raw("[] 0 d");
                let filled = !s.interior.is_empty();
                if let Some(start) = le.first() {
                    line_ending(&mut ops, start, a, b, size, filled);
                }
                if let Some(end) = le.get(1) {
                    line_ending(&mut ops, end, b, a, size, filled);
                }
                rect = union_points(&[a, b], size + s.width);
            }
        }
        "Polygon" | "PolyLine" => {
            let pts = pairs(&nums(&doc.inner, &d, b"Vertices"));
            if let Some((first, rest)) = pts.split_first() {
                apply_stroke_style(&mut ops, &s);
                ops.raw("1 j").stroke(&stroke_color).fill(&s.interior);
                ops.move_to(first.0, first.1);
                for p in rest {
                    ops.line_to(p.0, p.1);
                }
                if subtype == "Polygon" {
                    ops.close()
                        .op(if s.interior.is_empty() { "S" } else { "B" });
                } else {
                    ops.op("S");
                }
                rect = union_points(&pts, s.width * 2.0);
            }
        }
        "Ink" => {
            let strokes: Vec<Vec<(f64, f64)>> = d
                .get(b"InkList")
                .ok()
                .and_then(|o| deref(&doc.inner, o).as_array().ok())
                .map(|paths| {
                    paths
                        .iter()
                        .filter_map(|p| deref(&doc.inner, p).as_array().ok())
                        .map(|a| {
                            pairs(
                                &a.iter()
                                    .filter_map(|o| number(&doc.inner, o))
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            apply_stroke_style(&mut ops, &s);
            ops.raw("1 J 1 j").stroke(&stroke_color);
            for path in &strokes {
                if let Some((first, rest)) = path.split_first() {
                    ops.move_to(first.0, first.1);
                    if rest.is_empty() {
                        ops.line_to(first.0 + 0.01, first.1);
                    }
                    for p in rest {
                        ops.line_to(p.0, p.1);
                    }
                    ops.op("S");
                }
            }
            let all: Vec<(f64, f64)> = strokes.concat();
            if !all.is_empty() {
                rect = union_points(&all, s.width * 2.0);
            }
        }
        "Highlight" | "Underline" | "StrikeOut" | "Squiggly" => {
            let boxes = quad_boxes(&nums(&doc.inner, &d, b"QuadPoints"));
            let color = if s.color.is_empty() {
                vec![1.0, 1.0, 0.0]
            } else {
                s.color.clone()
            };
            for b in &boxes {
                let h = b[3] - b[1];
                match subtype.as_str() {
                    "Highlight" => {
                        ops.fill(&color).rect(b[0], b[1], b[2] - b[0], h).op("f");
                    }
                    "Underline" => {
                        let w = (h * 0.07).max(0.5);
                        ops.stroke(&color)
                            .line_width(w)
                            .move_to(b[0], b[1] + w)
                            .line_to(b[2], b[1] + w)
                            .op("S");
                    }
                    "StrikeOut" => {
                        let w = (h * 0.07).max(0.5);
                        let y = b[1] + h * 0.45;
                        ops.stroke(&color)
                            .line_width(w)
                            .move_to(b[0], y)
                            .line_to(b[2], y)
                            .op("S");
                    }
                    _ => {
                        let step = (h * 0.12).max(1.5);
                        let w = (h * 0.05).max(0.5);
                        ops.stroke(&color).line_width(w).move_to(b[0], b[1] + step);
                        let mut x = b[0];
                        let mut up = false;
                        while x < b[2] {
                            x = (x + step).min(b[2]);
                            ops.line_to(x, if up { b[1] + step } else { b[1] });
                            up = !up;
                        }
                        ops.op("S");
                    }
                }
            }
            if !boxes.is_empty() {
                let pts: Vec<(f64, f64)> = boxes
                    .iter()
                    .flat_map(|b| [(b[0], b[1]), (b[2], b[3])])
                    .collect();
                rect = union_points(&pts, 0.0);
            }
        }
        "FreeText" => {
            let da = d
                .get(b"DA")
                .ok()
                .and_then(crate::strings::object_to_text)
                .unwrap_or_default();
            let (font_name, size, text_color) = parse_da(&da);
            let size = if size > 0.0 { size } else { 12.0 };
            let std = font_name
                .as_deref()
                .and_then(StdFont::from_base_font)
                .unwrap_or(StdFont::Helvetica);
            let font = doc.std_font(std);
            let bw = s.width;
            if !s.interior.is_empty() {
                ops.fill(&s.interior)
                    .rect(rect[0], rect[1], rect[2] - rect[0], rect[3] - rect[1])
                    .op("f");
            }
            if bw > 0.0 {
                apply_stroke_style(&mut ops, &s);
                let border = if s.color.is_empty() {
                    text_color.clone()
                } else {
                    s.color.clone()
                };
                ops.stroke(&border)
                    .rect(
                        rect[0] + bw / 2.0,
                        rect[1] + bw / 2.0,
                        rect[2] - rect[0] - bw,
                        rect[3] - rect[1] - bw,
                    )
                    .op("S");
            }
            let pad = bw + 2.0;
            let area = [rect[0] + pad, rect[1] + pad, rect[2] - pad, rect[3] - pad];
            let contents = d
                .get(b"Contents")
                .ok()
                .and_then(crate::strings::object_to_text)
                .unwrap_or_default();
            let q = d.get(b"Q").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0);
            ops.rect(
                rect[0] + bw,
                rect[1] + bw,
                rect[2] - rect[0] - 2.0 * bw,
                rect[3] - rect[1] - 2.0 * bw,
            )
            .raw("W n");
            text_block(
                doc,
                &mut ops,
                &mut res,
                font,
                size,
                &text_color,
                &contents,
                area,
                q,
                true,
            );
        }
        "Text" => {
            // Sticky-note icon: a speech bubble with three lines.
            let color = if s.color.is_empty() {
                vec![1.0, 0.85, 0.2]
            } else {
                s.color.clone()
            };
            let (x, y) = (rect[0], rect[3] - 20.0);
            rect = [x, y, x + 20.0, y + 20.0];
            ops.line_width(0.8).stroke(&[0.0]).fill(&color);
            ops.move_to(x + 1.5, y + 18.5)
                .line_to(x + 18.5, y + 18.5)
                .line_to(x + 18.5, y + 6.5)
                .line_to(x + 9.0, y + 6.5)
                .line_to(x + 4.5, y + 1.5)
                .line_to(x + 5.5, y + 6.5)
                .line_to(x + 1.5, y + 6.5)
                .close()
                .op("B");
            for i in 0..3 {
                let ly = y + 15.0 - i as f64 * 3.0;
                ops.move_to(x + 4.0, ly).line_to(x + 16.0, ly);
            }
            ops.op("S");
        }
        "Stamp" => {
            let icon = name(&d, b"Name").unwrap_or_else(|| "Draft".into());
            let (label, default_color) = stamp_label(&icon);
            let color = if s.color.is_empty() {
                default_color
            } else {
                s.color.clone()
            };
            let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
            let bw = (h * 0.06).clamp(1.0, 4.0);
            let r = (h * 0.15).min(w / 4.0);
            let (x1, y1, x2, y2) = (rect[0] + bw, rect[1] + bw, rect[2] - bw, rect[3] - bw);
            const K: f64 = 0.552_284_75;
            ops.line_width(bw).stroke(&color);
            ops.move_to(x1 + r, y1)
                .line_to(x2 - r, y1)
                .curve_to(x2 - r + r * K, y1, x2, y1 + r - r * K, x2, y1 + r)
                .line_to(x2, y2 - r)
                .curve_to(x2, y2 - r + r * K, x2 - r + r * K, y2, x2 - r, y2)
                .line_to(x1 + r, y2)
                .curve_to(x1 + r - r * K, y2, x1, y2 - r + r * K, x1, y2 - r)
                .line_to(x1, y1 + r)
                .curve_to(x1, y1 + r - r * K, x1 + r - r * K, y1, x1 + r, y1)
                .close()
                .op("S");
            let font = doc.std_font(StdFont::HelveticaBold);
            let unit = doc.fonts[font].width(&label) / 1000.0;
            let size = ((w - 4.0 * bw - 2.0 * r) / unit.max(0.1))
                .min((h - 4.0 * bw) * 0.7)
                .max(1.0);
            let asc = doc.fonts[font].ascender() * size / 1000.0;
            let tw = unit * size;
            let fname = res.font(doc.fonts[font].obj);
            let encoded = doc.fonts[font].encode(&label);
            ops.begin_text()
                .font(&fname, size)
                .fill(&color)
                .text_matrix([
                    1.0,
                    0.0,
                    0.0,
                    1.0,
                    rect[0] + (w - tw) / 2.0,
                    rect[1] + (h - asc) / 2.0,
                ])
                .show(&encoded)
                .end_text();
        }
        "Redact" => {
            let color = if s.color.is_empty() {
                vec![1.0, 0.0, 0.0]
            } else {
                s.color.clone()
            };
            let boxes = {
                let q = quad_boxes(&nums(&doc.inner, &d, b"QuadPoints"));
                if q.is_empty() { vec![rect] } else { q }
            };
            ops.stroke(&color).line_width(1.0);
            for b in &boxes {
                ops.rect(b[0], b[1], b[2] - b[0], b[3] - b[1]).op("S");
            }
        }
        _ => return Ok(false),
    }
    ops.Q();

    set_appearance(doc, annot, rect, res, ops.into_bytes())?;
    Ok(true)
}

/// Stores `content` as the normal appearance of `annot` and updates its /Rect.
pub fn set_appearance(
    doc: &mut Doc,
    annot: ObjectId,
    rect: [f64; 4],
    res: Resources,
    content: Vec<u8>,
) -> Res<()> {
    let ap = form_xobject(&mut doc.inner, rect, None, res, content);
    let d = doc
        .inner
        .get_dictionary_mut(annot)
        .map_err(err("invalid annotation"))?;
    d.set(
        "Rect",
        Object::Array(rect.iter().map(|v| real(*v)).collect()),
    );
    d.set(
        "AP",
        Object::Dictionary(Dictionary::from_iter(vec![("N", Object::Reference(ap))])),
    );
    doc.touch();
    Ok(())
}

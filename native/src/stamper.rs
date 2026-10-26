//! Stamper: places text, images or other pages on a set of pages (PDFNet.Stamper semantics).
//!
//! Positions are computed in display space (page rotation applied, origin at the bottom-left
//! of the visible box) and mapped back to user space, so stamps look the same on rotated pages.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::annots;
use crate::appearance;
use crate::content::{
    Ops, Resources, STAMP_KEY, concat, display_to_user, form_xobject, page_add_content,
    page_add_xobject, page_has_marked, page_remove_marked, transform_rect,
};
use crate::doc::{BoxKind, Doc, Res, deref, err, inherited_in, rect_of};
use crate::fonts::StdFont;

pub struct StampParams {
    /// 1 relative scale, 2 absolute size, 3 font size (PDFNet Stamper.SizeType).
    pub size_type: u32,
    pub a: f64,
    pub b: f64,
    /// -1 left/bottom, 0 center, 1 right/top.
    pub h_align: i32,
    pub v_align: i32,
    pub x: f64,
    pub y: f64,
    pub percentage: bool,
    pub rotation: f64,
    pub opacity: f64,
    pub background: bool,
    pub as_annotation: bool,
    /// Alignment of multi-line text inside the stamp (-1, 0, 1).
    pub text_align: i32,
    pub font: Option<usize>,
    pub color: Vec<f64>,
    pub underline: bool,
}

pub enum Source {
    Text(String),
    Image {
        obj: ObjectId,
        width: u32,
        height: u32,
    },
    /// A page of this or another document, already converted to a Form XObject.
    Form {
        obj: ObjectId,
        bbox: [f64; 4],
    },
}

/// Nominal font size used to build text stamps that are scaled afterwards.
const NOMINAL_SIZE: f64 = 24.0;

/// Builds the stamp as a Form XObject at its natural size. Returns (xobject, width, height).
fn build_form(doc: &mut Doc, src: &Source, p: &StampParams) -> Res<(ObjectId, f64, f64)> {
    let mut res = Resources::default();
    let mut ops = Ops::new();
    ops.q();
    if p.opacity < 1.0 {
        let gs = res.opacity(p.opacity.max(0.0), None);
        ops.gs(&gs);
    }
    let (w, h) = match src {
        Source::Text(text) => {
            let font = p.font.unwrap_or_else(|| doc.std_font(StdFont::Helvetica));
            let size = if p.size_type == 3 { p.a } else { NOMINAL_SIZE };
            let asc = doc.fonts[font].ascender() * size / 1000.0;
            let desc = doc.fonts[font].descender() * size / 1000.0;
            let leading = asc - desc;
            let lines: Vec<&str> = text.split('\n').collect();
            let widths: Vec<f64> = lines
                .iter()
                .map(|l| doc.fonts[font].width(l) * size / 1000.0)
                .collect();
            let w = widths.iter().cloned().fold(0.0, f64::max).max(1.0);
            let h = leading * lines.len() as f64;
            let fname = res.font(doc.fonts[font].obj);
            let color = if p.color.is_empty() {
                vec![1.0, 0.0, 0.0]
            } else {
                p.color.clone()
            };
            ops.fill(&color)
                .stroke(&color)
                .begin_text()
                .font(&fname, size);
            let mut underlines = Vec::new();
            for (i, (line, lw)) in lines.iter().zip(&widths).enumerate() {
                let x = match p.text_align {
                    -1 => 0.0,
                    1 => w - lw,
                    _ => (w - lw) / 2.0,
                };
                let y = h - asc - i as f64 * leading;
                let enc = doc.fonts[font].encode(line);
                ops.text_matrix([1.0, 0.0, 0.0, 1.0, x, y]).show(&enc);
                underlines.push((x, y, *lw));
            }
            ops.end_text();
            if p.underline {
                ops.line_width(size * 0.05);
                for (x, y, lw) in underlines {
                    ops.move_to(x, y - size * 0.1)
                        .line_to(x + lw, y - size * 0.1)
                        .op("S");
                }
            }
            (w, h)
        }
        Source::Image { obj, width, height } => {
            let name = res.xobject(*obj);
            let (w, h) = (*width as f64, *height as f64);
            ops.cm([w, 0.0, 0.0, h, 0.0, 0.0]).do_xobject(&name);
            (w, h)
        }
        Source::Form { obj, bbox } => {
            let name = res.xobject(*obj);
            ops.cm([1.0, 0.0, 0.0, 1.0, -bbox[0], -bbox[1]])
                .do_xobject(&name);
            (bbox[2] - bbox[0], bbox[3] - bbox[1])
        }
    };
    ops.Q();
    let id = form_xobject(
        &mut doc.inner,
        [0.0, 0.0, w, h],
        None,
        res,
        ops.into_bytes(),
    );
    Ok((id, w, h))
}

/// Placement matrix (stamp space → user space) for one page.
fn placement(doc: &Doc, page: ObjectId, w0: f64, h0: f64, p: &StampParams) -> [f64; 6] {
    let crop = doc.page_box(page, BoxKind::Crop);
    let rot = doc.rotation(page);
    let (cw, ch) = (crop[2] - crop[0], crop[3] - crop[1]);
    let (pw, ph) = if rot % 180 == 90 { (ch, cw) } else { (cw, ch) };

    let s = match p.size_type {
        1 => {
            let sx = p.a * pw / w0;
            if p.b > 0.0 { sx.min(p.b * ph / h0) } else { sx }
        }
        2 => match (p.a > 0.0, p.b > 0.0) {
            (true, true) => (p.a / w0).min(p.b / h0),
            (true, false) => p.a / w0,
            (false, true) => p.b / h0,
            _ => 1.0,
        },
        _ => 1.0,
    };
    let theta = p.rotation.to_radians();
    let (sin, cos) = theta.sin_cos();
    let (sw, sh) = (w0 * s, h0 * s);
    let bw = (sw * cos).abs() + (sh * sin).abs();
    let bh = (sw * sin).abs() + (sh * cos).abs();
    let (xo, yo) = if p.percentage {
        (p.x * pw, p.y * ph)
    } else {
        (p.x, p.y)
    };
    let cx = match p.h_align {
        -1 => bw / 2.0 + xo,
        1 => pw - bw / 2.0 - xo,
        _ => pw / 2.0 + xo,
    };
    let cy = match p.v_align {
        -1 => bh / 2.0 + yo,
        1 => ph - bh / 2.0 - yo,
        _ => ph / 2.0 + yo,
    };
    let m = concat(
        concat(
            [1.0, 0.0, 0.0, 1.0, -w0 / 2.0, -h0 / 2.0],
            [s, 0.0, 0.0, s, 0.0, 0.0],
        ),
        concat(
            [cos, sin, -sin, cos, 0.0, 0.0],
            [1.0, 0.0, 0.0, 1.0, cx, cy],
        ),
    );
    concat(m, display_to_user(crop, rot))
}

pub fn stamp(doc: &mut Doc, src: Source, pages: &[usize], p: &StampParams) -> Res<()> {
    let (form, w0, h0) = build_form(doc, &src, p)?;
    for &index in pages {
        let page = doc.page_id(index)?;
        let m = placement(doc, page, w0, h0, p);
        if p.as_annotation {
            let rect = transform_rect(m, [0.0, 0.0, w0, h0]);
            let annot = annots::create(doc, "Stamp", rect);
            let mut res = Resources::default();
            let name = res.xobject(form);
            let mut ops = Ops::new();
            ops.q().cm(m).do_xobject(&name).Q();
            appearance::set_appearance(doc, annot, rect, res, ops.into_bytes())?;
            doc.inner
                .get_dictionary_mut(annot)
                .map_err(err("annotation"))?
                .set(STAMP_KEY, Object::Boolean(true));
            annots::add_to_page(doc, page, annot, None)?;
        } else {
            let name = page_add_xobject(&mut doc.inner, page, form)?;
            let mut ops = Ops::new();
            ops.q().cm(m).do_xobject(&name).Q();
            page_add_content(
                &mut doc.inner,
                page,
                ops.into_bytes(),
                !p.background,
                Some(STAMP_KEY),
            )?;
        }
    }
    doc.touch();
    Ok(())
}

fn stamp_annots(doc: &Document, page: ObjectId) -> Vec<ObjectId> {
    annots::page_annots(doc, page)
        .into_iter()
        .filter(|a| doc.get_dictionary(*a).is_ok_and(|d| d.has(STAMP_KEY)))
        .collect()
}

pub fn delete_stamps(doc: &mut Doc, pages: &[usize]) -> Res<usize> {
    let mut removed = 0;
    for &index in pages {
        let page = doc.page_id(index)?;
        removed += page_remove_marked(&mut doc.inner, page, STAMP_KEY)?;
        for a in stamp_annots(&doc.inner, page) {
            annots::remove_from_page(doc, page, a)?;
            removed += 1;
        }
    }
    doc.touch();
    Ok(removed)
}

pub fn has_stamps(doc: &Doc, pages: &[usize]) -> Res<bool> {
    for &index in pages {
        let page = doc.page_id(index)?;
        if page_has_marked(&doc.inner, page, STAMP_KEY)
            || !stamp_annots(&doc.inner, page).is_empty()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Converts a page (of `src`, which may be this document) into a Form XObject of `doc`.
pub fn page_to_form(
    doc: &mut Doc,
    src: Option<&Document>,
    page: ObjectId,
) -> Res<(ObjectId, [f64; 4])> {
    let source: &Document = match src {
        Some(s) => s,
        None => &doc.inner,
    };
    let content = source.get_page_content(page);
    let resources = inherited_in(source, page, b"Resources")
        .map(|o| deref(source, &o).clone())
        .unwrap_or(Object::Dictionary(Dictionary::new()));
    let read_box = |key: &[u8]| inherited_in(source, page, key).and_then(|o| rect_of(source, &o));
    let bbox = read_box(b"CropBox")
        .or_else(|| read_box(b"MediaBox"))
        .unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let resources = match src {
        Some(s) => doc.graft_from(s, resources),
        None => resources,
    };
    let dict = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"XObject".to_vec())),
        ("Subtype", Object::Name(b"Form".to_vec())),
        (
            "BBox",
            Object::Array(bbox.iter().map(|v| crate::doc::real(*v)).collect()),
        ),
        ("Resources", resources),
    ]);
    let mut s = Stream::new(dict, content);
    let _ = s.compress();
    let id = doc.inner.add_object(s);
    doc.touch();
    Ok((id, bbox))
}

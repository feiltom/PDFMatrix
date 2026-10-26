//! Interactive forms (AcroForm): field tree, values, widget appearances, field creation.

use std::collections::HashSet;

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::appearance::{nums, parse_da};
use crate::content::{Ops, Resources, form_xobject};
use crate::doc::{Doc, Res, deref, err, number, real, rect_of};
use crate::fonts::{StdFont, wrap};
use crate::strings::{encode_text, object_to_text};

// Field flags (ISO 32000-1, tables 226, 228, 230, 232), as bit masks.
pub const FF_READ_ONLY: i64 = 1 << 0;
pub const FF_MULTILINE: i64 = 1 << 12;
pub const FF_PASSWORD: i64 = 1 << 13;
pub const FF_RADIO: i64 = 1 << 15;
pub const FF_PUSHBUTTON: i64 = 1 << 16;
pub const FF_COMBO: i64 = 1 << 17;
pub const FF_COMB: i64 = 1 << 24;

#[derive(Clone, Debug)]
pub struct Field {
    pub id: ObjectId,
    pub name: String,
    pub ftype: String,
    pub flags: i64,
    pub widgets: Vec<ObjectId>,
}

impl Field {
    pub fn kind(&self) -> &'static str {
        match self.ftype.as_str() {
            "Tx" => "text",
            "Ch" if self.flags & FF_COMBO != 0 => "combo",
            "Ch" => "list",
            "Btn" if self.flags & FF_PUSHBUTTON != 0 => "button",
            "Btn" if self.flags & FF_RADIO != 0 => "radio",
            "Btn" => "check",
            "Sig" => "signature",
            _ => "unknown",
        }
    }
}

fn acroform_id(doc: &Document) -> Option<ObjectId> {
    match doc.catalog().ok()?.get(b"AcroForm").ok()? {
        Object::Reference(id) => Some(*id),
        _ => None,
    }
}

fn acroform(doc: &Document) -> Option<&Dictionary> {
    let o = doc.catalog().ok()?.get(b"AcroForm").ok()?;
    deref(doc, o).as_dict().ok()
}

/// Attribute of a field, inherited through /Parent (FT, Ff, V, DV, DA, Q, Opt, MaxLen).
pub fn field_attr(doc: &Document, id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = id;
    for _ in 0..32 {
        let d = doc.get_dictionary(current).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(deref(doc, v).clone());
        }
        current = d.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

fn is_widget(d: &Dictionary) -> bool {
    d.get(b"Subtype")
        .and_then(Object::as_name)
        .is_ok_and(|n| n == b"Widget")
}

/// All terminal fields, in document order.
pub fn fields(doc: &Document) -> Vec<Field> {
    let mut out = Vec::new();
    let Some(form) = acroform(doc) else {
        return out;
    };
    let roots: Vec<ObjectId> = form
        .get(b"Fields")
        .ok()
        .and_then(|o| deref(doc, o).as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    for id in roots {
        walk(doc, id, "", &mut out, &mut seen);
    }
    out
}

fn walk(
    doc: &Document,
    id: ObjectId,
    parent: &str,
    out: &mut Vec<Field>,
    seen: &mut HashSet<ObjectId>,
) {
    if !seen.insert(id) {
        return;
    }
    let Ok(d) = doc.get_dictionary(id) else {
        return;
    };
    let partial = d.get(b"T").ok().and_then(object_to_text);
    let name = match (&partial, parent.is_empty()) {
        (Some(t), true) => t.clone(),
        (Some(t), false) => format!("{parent}.{t}"),
        (None, _) => parent.to_string(),
    };
    let kids: Vec<ObjectId> = d
        .get(b"Kids")
        .ok()
        .and_then(|o| deref(doc, o).as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    let (field_kids, widget_kids): (Vec<ObjectId>, Vec<ObjectId>) =
        kids.into_iter().partition(|k| {
            doc.get_dictionary(*k)
                .is_ok_and(|kd| kd.has(b"T") || (!is_widget(kd) && kd.has(b"Kids")))
        });
    for k in &field_kids {
        walk(doc, *k, &name, out, seen);
    }
    if field_kids.is_empty() || !widget_kids.is_empty() {
        let mut widgets = widget_kids;
        if widgets.is_empty() && is_widget(d) {
            widgets.push(id);
        }
        let ftype = field_attr(doc, id, b"FT")
            .and_then(|o| {
                o.as_name()
                    .ok()
                    .map(|n| String::from_utf8_lossy(n).into_owned())
            })
            .unwrap_or_default();
        if ftype.is_empty() && widgets.is_empty() {
            return;
        }
        let flags = field_attr(doc, id, b"Ff")
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        out.push(Field {
            id,
            name,
            ftype,
            flags,
            widgets,
        });
    }
}

pub fn find(doc: &Document, name: &str) -> Option<Field> {
    fields(doc).into_iter().find(|f| f.name == name)
}

/// Field value as a string: text, selected option(s) (joined by '\n'), button state, or "".
pub fn value_string(doc: &Document, f: &Field) -> String {
    match field_attr(doc, f.id, b"V") {
        Some(Object::Array(items)) => items
            .iter()
            .filter_map(|o| object_to_text(deref(doc, o)))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(o) => object_to_text(&o).unwrap_or_default(),
        None if f.ftype == "Btn" && f.kind() != "button" => "Off".into(),
        None => String::new(),
    }
}

/// On-state names of a button widget (keys of /AP /N other than Off).
pub fn on_states(doc: &Document, widget: ObjectId) -> Vec<String> {
    let Ok(d) = doc.get_dictionary(widget) else {
        return vec![];
    };
    let n = d
        .get(b"AP")
        .ok()
        .and_then(|o| deref(doc, o).as_dict().ok())
        .and_then(|ap| ap.get(b"N").ok())
        .and_then(|o| deref(doc, o).as_dict().ok());
    match n {
        Some(n) => n
            .iter()
            .map(|(k, _)| String::from_utf8_lossy(k).into_owned())
            .filter(|k| k != "Off")
            .collect(),
        None => vec![],
    }
}

/// Options of a choice field: (export value, display text).
pub fn options(doc: &Document, f: &Field) -> Vec<(String, String)> {
    let Some(Object::Array(opts)) = field_attr(doc, f.id, b"Opt") else {
        return vec![];
    };
    opts.iter()
        .filter_map(|o| match deref(doc, o) {
            Object::Array(pair) if pair.len() >= 2 => {
                let e = object_to_text(deref(doc, &pair[0]))?;
                let d = object_to_text(deref(doc, &pair[1]))?;
                Some((e, d))
            }
            other => object_to_text(other).map(|t| (t.clone(), t)),
        })
        .collect()
}

pub fn set_value(doc: &mut Doc, f: &Field, value: &str) -> Res<()> {
    if f.flags & FF_READ_ONLY != 0 {
        // PDFNet lets programs fill read-only fields; only the UI is restricted.
    }
    match f.kind() {
        "text" | "combo" | "list" => {
            set_field_key(doc, f.id, "V", encode_text(value))?;
            regenerate(doc, f)?;
        }
        "check" | "radio" => {
            let off = value.is_empty() || value == "Off";
            let mut matched = off;
            for &w in &f.widgets {
                let states = on_states(&doc.inner, w);
                let state = if off {
                    "Off".to_string()
                } else if states.iter().any(|s| s == value) {
                    matched = true;
                    value.to_string()
                } else if f.kind() == "check"
                    && (value == "Yes" || value == "On" || value == "true")
                {
                    matched = true;
                    states.first().cloned().unwrap_or_else(|| "Yes".into())
                } else {
                    "Off".into()
                };
                doc.inner
                    .get_dictionary_mut(w)
                    .map_err(err("invalid widget"))?
                    .set("AS", Object::Name(state.into_bytes()));
            }
            if !matched {
                return Err(format!("field '{}' has no state '{value}'", f.name));
            }
            let v = if off {
                "Off".to_string()
            } else {
                f.widgets
                    .iter()
                    .find_map(|w| {
                        doc.inner
                            .get_dictionary(*w)
                            .ok()?
                            .get(b"AS")
                            .ok()?
                            .as_name()
                            .ok()
                            .map(|n| String::from_utf8_lossy(n).into_owned())
                    })
                    .filter(|s| s != "Off")
                    .unwrap_or_else(|| value.to_string())
            };
            set_field_key(doc, f.id, "V", Object::Name(v.into_bytes()))?;
            regenerate(doc, f)?;
        }
        other => return Err(format!("cannot set a value on a {other} field")),
    }
    Ok(())
}

fn set_field_key(doc: &mut Doc, id: ObjectId, key: &str, v: Object) -> Res<()> {
    doc.inner
        .get_dictionary_mut(id)
        .map_err(err("invalid field"))?
        .set(key, v);
    doc.touch();
    Ok(())
}

// ------------------------------------------------------------------ appearances

struct WidgetStyle {
    rect: [f64; 4],
    rotation: i64,
    bg: Vec<f64>,
    border: Vec<f64>,
    border_width: f64,
    border_style: String,
}

fn widget_style(doc: &Document, w: ObjectId) -> Option<WidgetStyle> {
    let d = doc.get_dictionary(w).ok()?;
    let rect = d.get(b"Rect").ok().and_then(|o| rect_of(doc, o))?;
    let mk = d.get(b"MK").ok().and_then(|o| deref(doc, o).as_dict().ok());
    let bs = d.get(b"BS").ok().and_then(|o| deref(doc, o).as_dict().ok());
    let border: Vec<f64> = mk.map(|m| nums(doc, m, b"BC")).unwrap_or_default();
    Some(WidgetStyle {
        rect,
        rotation: mk
            .and_then(|m| m.get(b"R").ok())
            .and_then(|o| number(doc, o))
            .unwrap_or(0.0) as i64,
        bg: mk.map(|m| nums(doc, m, b"BG")).unwrap_or_default(),
        border_width: if border.is_empty() {
            0.0
        } else {
            bs.and_then(|b| b.get(b"W").ok())
                .and_then(|o| number(doc, o))
                .unwrap_or(1.0)
        },
        border,
        border_style: bs
            .and_then(|b| b.get(b"S").ok())
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_else(|| "S".into()),
    })
}

/// (width, height, matrix) of the appearance form for a widget rect and /MK /R rotation.
fn form_geometry(s: &WidgetStyle) -> (f64, f64, Option<[f64; 6]>) {
    let (w, h) = (s.rect[2] - s.rect[0], s.rect[3] - s.rect[1]);
    match ((s.rotation % 360) + 360) % 360 {
        90 => (h, w, Some([0.0, 1.0, -1.0, 0.0, 0.0, 0.0])),
        180 => (w, h, Some([-1.0, 0.0, 0.0, -1.0, 0.0, 0.0])),
        270 => (h, w, Some([0.0, -1.0, 1.0, 0.0, 0.0, 0.0])),
        _ => (w, h, None),
    }
}

fn draw_frame(ops: &mut Ops, s: &WidgetStyle, w: f64, h: f64, round: bool) {
    if !s.bg.is_empty() {
        ops.fill(&s.bg);
        if round {
            ops.ellipse(0.0, 0.0, w, h).op("f");
        } else {
            ops.rect(0.0, 0.0, w, h).op("f");
        }
    }
    if s.border_width > 0.0 && !s.border.is_empty() {
        let bw = s.border_width;
        ops.stroke(&s.border).line_width(bw);
        if s.border_style == "D" {
            ops.dash(&[3.0], 0.0);
        }
        if s.border_style == "U" {
            ops.move_to(0.0, bw / 2.0).line_to(w, bw / 2.0).op("S");
        } else if round {
            ops.ellipse(bw / 2.0, bw / 2.0, w - bw / 2.0, h - bw / 2.0)
                .op("S");
        } else {
            ops.rect(bw / 2.0, bw / 2.0, w - bw, h - bw).op("S");
        }
        ops.raw("[] 0 d");
    }
}

/// Resolves the DA of a widget/field to (font index, size, color) and returns the resource name
/// the font must be registered under in the appearance.
fn resolve_font(doc: &mut Doc, f: &Field, widget: ObjectId) -> (usize, String, f64, Vec<f64>) {
    let da = doc
        .inner
        .get_dictionary(widget)
        .ok()
        .and_then(|d| d.get(b"DA").ok())
        .and_then(object_to_text)
        .or_else(|| {
            field_attr(&doc.inner, f.id, b"DA")
                .as_ref()
                .and_then(object_to_text)
        })
        .or_else(|| {
            acroform(&doc.inner)
                .and_then(|a| a.get(b"DA").ok())
                .and_then(object_to_text)
        })
        .unwrap_or_else(|| "/Helv 0 Tf 0 g".into());
    let (name, size, color) = parse_da(&da);
    let name = name.unwrap_or_else(|| "Helv".into());
    let dr_font = acroform(&doc.inner)
        .and_then(|a| a.get(b"DR").ok())
        .and_then(|o| deref(&doc.inner, o).as_dict().ok())
        .and_then(|dr| dr.get(b"Font").ok())
        .and_then(|o| deref(&doc.inner, o).as_dict().ok())
        .and_then(|fd| fd.get(name.as_bytes()).ok())
        .and_then(|o| o.as_reference().ok());
    let idx = match dr_font {
        Some(obj) => doc.existing_font(obj),
        None => {
            let std = StdFont::from_base_font(&name).unwrap_or(StdFont::Helvetica);
            doc.std_font(std)
        }
    };
    (idx, name, size, color)
}

/// Regenerates the appearance of every widget of a field.
pub fn regenerate(doc: &mut Doc, f: &Field) -> Res<()> {
    for &w in &f.widgets {
        widget_appearance(doc, f, w)?;
    }
    Ok(())
}

fn widget_appearance(doc: &mut Doc, f: &Field, widget: ObjectId) -> Res<()> {
    let Some(style) = widget_style(&doc.inner, widget) else {
        return Ok(());
    };
    let (w, h, matrix) = form_geometry(&style);
    let bbox = [0.0, 0.0, w, h];
    let pad = if style.border_width > 0.0 {
        style.border_width + 2.0
    } else {
        2.0
    };

    match f.kind() {
        "check" | "radio" => {
            let states = on_states(&doc.inner, widget);
            if !states.is_empty() {
                return Ok(()); // keep the author's appearances; /AS selects the state
            }
            let on = if f.kind() == "check" {
                "Yes".to_string()
            } else {
                format!("Choice{}", widget.0)
            };
            let round = f.kind() == "radio";
            let (_, _, _, color) = resolve_font(doc, f, widget);
            let mut off_ops = Ops::new();
            off_ops.q();
            draw_frame(&mut off_ops, &style, w, h, round);
            off_ops.Q();
            let mut on_ops = Ops::new();
            on_ops.q();
            draw_frame(&mut on_ops, &style, w, h, round);
            on_ops.fill(&color).stroke(&color);
            let m = w.min(h);
            let (cx, cy) = (w / 2.0, h / 2.0);
            if round {
                let r = m * 0.25;
                on_ops.ellipse(cx - r, cy - r, cx + r, cy + r).op("f");
            } else {
                // A check mark, stroked with round caps.
                on_ops
                    .line_width(m * 0.12)
                    .raw("1 J 1 j")
                    .move_to(cx - m * 0.28, cy + m * 0.02)
                    .line_to(cx - m * 0.08, cy - m * 0.2)
                    .line_to(cx + m * 0.3, cy + m * 0.25)
                    .op("S");
            }
            on_ops.Q();
            let off_id = form_xobject(
                &mut doc.inner,
                bbox,
                matrix,
                Resources::default(),
                off_ops.into_bytes(),
            );
            let on_id = form_xobject(
                &mut doc.inner,
                bbox,
                matrix,
                Resources::default(),
                on_ops.into_bytes(),
            );
            let value = value_string(&doc.inner, f);
            let d = doc
                .inner
                .get_dictionary_mut(widget)
                .map_err(err("invalid widget"))?;
            let mut n = Dictionary::new();
            n.set(on.as_bytes(), Object::Reference(on_id));
            n.set("Off", Object::Reference(off_id));
            d.set(
                "AP",
                Object::Dictionary(Dictionary::from_iter(vec![("N", Object::Dictionary(n))])),
            );
            let state =
                if value == on || (f.kind() == "check" && value != "Off" && !value.is_empty()) {
                    on
                } else {
                    "Off".into()
                };
            d.set("AS", Object::Name(state.into_bytes()));
            doc.touch();
            return Ok(());
        }
        "signature" => {
            let mut ops = Ops::new();
            ops.q();
            draw_frame(&mut ops, &style, w, h, false);
            ops.Q();
            let ap = form_xobject(
                &mut doc.inner,
                bbox,
                matrix,
                Resources::default(),
                ops.into_bytes(),
            );
            set_normal_ap(doc, widget, ap)?;
            return Ok(());
        }
        _ => {}
    }

    // Text-like fields: text, combo, list, push button caption.
    let (font, font_name, da_size, color) = resolve_font(doc, f, widget);
    // The font must be registered under the DA name, since /DA refers to it.
    let mut res = Resources::default();
    let mut font_dict = Dictionary::new();
    font_dict.set(font_name.as_bytes(), Object::Reference(doc.fonts[font].obj));
    res.dict.set("Font", Object::Dictionary(font_dict));

    let mut ops = Ops::new();
    ops.q();
    draw_frame(&mut ops, &style, w, h, false);
    let asc = doc.fonts[font].ascender() / 1000.0;
    let desc = doc.fonts[font].descender() / 1000.0;
    let inner = [pad, pad, w - pad, h - pad];

    let value = value_string(&doc.inner, f);
    let quadding = field_attr(&doc.inner, f.id, b"Q")
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0);

    if f.kind() == "list" {
        let opts = options(&doc.inner, f);
        let selected: Vec<&str> = value.split('\n').collect();
        let size = if da_size > 0.0 { da_size } else { 12.0 };
        let line_h = (asc - desc) * size * 1.1;
        ops.raw("/Tx BMC")
            .q()
            .rect(
                inner[0] - 1.0,
                inner[1] - 1.0,
                inner[2] - inner[0] + 2.0,
                inner[3] - inner[1] + 2.0,
            )
            .raw("W n");
        for (i, (export, display)) in opts.iter().enumerate() {
            let top = h - pad - i as f64 * line_h;
            if top - line_h < 0.0 {
                break;
            }
            if selected.contains(&export.as_str()) || selected.contains(&display.as_str()) {
                ops.fill(&[0.6, 0.75, 0.88])
                    .rect(pad - 1.0, top - line_h, w - 2.0 * pad + 2.0, line_h)
                    .op("f");
            }
            let enc = doc.fonts[font].encode(display);
            ops.begin_text()
                .font(&font_name, size)
                .fill(&color)
                .text_matrix([
                    1.0,
                    0.0,
                    0.0,
                    1.0,
                    pad + 1.0,
                    top - asc * size - (line_h - (asc - desc) * size) / 2.0,
                ])
                .show(&enc)
                .end_text();
        }
        ops.Q().raw("EMC");
    } else {
        let text = match f.kind() {
            "button" => doc
                .inner
                .get_dictionary(widget)
                .ok()
                .and_then(|d| d.get(b"MK").ok())
                .and_then(|o| deref(&doc.inner, o).as_dict().ok())
                .and_then(|mk| mk.get(b"CA").ok())
                .and_then(object_to_text)
                .unwrap_or_default(),
            "combo" => options(&doc.inner, f)
                .into_iter()
                .find(|(e, _)| *e == value)
                .map(|(_, d)| d)
                .unwrap_or(value.clone()),
            _ if f.flags & FF_PASSWORD != 0 => "*".repeat(value.chars().count()),
            _ => value.clone(),
        };
        let multiline = f.kind() == "text" && f.flags & FF_MULTILINE != 0;
        let comb = f.kind() == "text" && f.flags & FF_COMB != 0;
        let max_len = field_attr(&doc.inner, f.id, b"MaxLen")
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        let avail_h = inner[3] - inner[1];
        let avail_w = inner[2] - inner[0];
        let size = if da_size > 0.0 {
            da_size
        } else if multiline {
            12.0_f64.min(avail_h / (asc - desc)).max(4.0)
        } else {
            let by_h = avail_h / (asc - desc);
            let tw = doc.fonts[font].width(&text) / 1000.0;
            let by_w = if tw > 0.0 { avail_w / tw } else { f64::MAX };
            by_h.min(by_w).clamp(4.0, 12.0)
        };
        ops.raw("/Tx BMC")
            .q()
            .rect(inner[0] - 1.0, inner[1] - 1.0, avail_w + 2.0, avail_h + 2.0)
            .raw("W n");
        ops.begin_text().font(&font_name, size).fill(&color);
        if comb && max_len > 0 {
            let cell = w / max_len as f64;
            let y = (h - (asc - desc) * size) / 2.0 - desc * size;
            for (i, c) in text.chars().take(max_len as usize).enumerate() {
                let s = c.to_string();
                let cw = doc.fonts[font].width(&s) * size / 1000.0;
                let enc = doc.fonts[font].encode(&s);
                ops.text_matrix([1.0, 0.0, 0.0, 1.0, i as f64 * cell + (cell - cw) / 2.0, y])
                    .show(&enc);
            }
        } else if multiline {
            let lines = wrap(&doc.fonts[font], size, &text, Some(avail_w));
            let leading = (asc - desc) * size * 1.1;
            for (i, line) in lines.iter().enumerate() {
                let lw = doc.fonts[font].width(line) * size / 1000.0;
                let x = align(quadding, inner[0], avail_w, lw);
                let y = inner[3] - asc * size - i as f64 * leading;
                let enc = doc.fonts[font].encode(line);
                ops.text_matrix([1.0, 0.0, 0.0, 1.0, x, y]).show(&enc);
            }
        } else {
            let lw = doc.fonts[font].width(&text) * size / 1000.0;
            let q = if f.kind() == "button" { 1 } else { quadding };
            let x = align(q, inner[0], avail_w, lw);
            let y = (h - (asc - desc) * size) / 2.0 - desc * size;
            let enc = doc.fonts[font].encode(&text);
            ops.text_matrix([1.0, 0.0, 0.0, 1.0, x, y]).show(&enc);
        }
        ops.end_text().Q().raw("EMC");
    }
    ops.Q();
    let ap = form_xobject(&mut doc.inner, bbox, matrix, res, ops.into_bytes());
    set_normal_ap(doc, widget, ap)
}

fn align(quadding: i64, x0: f64, avail: f64, w: f64) -> f64 {
    match quadding {
        1 => x0 + (avail - w) / 2.0,
        2 => x0 + avail - w,
        _ => x0,
    }
}

fn set_normal_ap(doc: &mut Doc, widget: ObjectId, ap: ObjectId) -> Res<()> {
    doc.inner
        .get_dictionary_mut(widget)
        .map_err(err("invalid widget"))?
        .set(
            "AP",
            Object::Dictionary(Dictionary::from_iter(vec![("N", Object::Reference(ap))])),
        );
    doc.touch();
    Ok(())
}

/// Regenerates all field appearances and clears /NeedAppearances.
pub fn refresh_all(doc: &mut Doc) -> Res<usize> {
    let all = fields(&doc.inner);
    for f in &all {
        regenerate(doc, f)?;
    }
    if let Some(form) = acroform_mut(&mut doc.inner) {
        form.remove(b"NeedAppearances");
    }
    Ok(all.len())
}

fn acroform_mut(doc: &mut Document) -> Option<&mut Dictionary> {
    if let Some(id) = acroform_id(doc) {
        return doc.get_dictionary_mut(id).ok();
    }
    match doc.catalog_mut().ok()?.get_mut(b"AcroForm").ok()? {
        Object::Dictionary(d) => Some(d),
        _ => None,
    }
}

// ------------------------------------------------------------------ creation

/// Ensures the document has an AcroForm with Helvetica as /Helv in /DR. Returns its id.
fn ensure_acroform(doc: &mut Doc) -> Res<ObjectId> {
    if let Some(id) = acroform_id(&doc.inner) {
        return Ok(id);
    }
    let existing = doc
        .inner
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(|o| o.as_dict().ok())
        .cloned();
    let helv = doc.std_font(StdFont::Helvetica);
    let helv_obj = doc.fonts[helv].obj;
    let form = existing.unwrap_or_else(|| {
        Dictionary::from_iter(vec![
            ("Fields", Object::Array(vec![])),
            (
                "DR",
                Object::Dictionary(Dictionary::from_iter(vec![(
                    "Font",
                    Object::Dictionary(Dictionary::from_iter(vec![(
                        "Helv",
                        Object::Reference(helv_obj),
                    )])),
                )])),
            ),
            ("DA", Object::string_literal("/Helv 0 Tf 0 g")),
        ])
    });
    let id = doc.inner.add_object(form);
    doc.inner
        .catalog_mut()
        .map_err(err("no catalog"))?
        .set("AcroForm", Object::Reference(id));
    doc.touch();
    Ok(id)
}

fn push_ref(doc: &mut Document, holder: ObjectId, key: &[u8], value: ObjectId) -> Res<()> {
    let current = doc
        .get_dictionary(holder)
        .map_err(err("invalid dictionary"))?
        .get(key)
        .ok()
        .cloned();
    let mut arr = match current {
        Some(Object::Reference(r)) => doc
            .get_object(r)
            .ok()
            .and_then(|o| o.as_array().ok())
            .cloned()
            .unwrap_or_default(),
        Some(Object::Array(a)) => a,
        _ => vec![],
    };
    arr.push(Object::Reference(value));
    doc.get_dictionary_mut(holder)
        .map_err(err("invalid dictionary"))?
        .set(key, Object::Array(arr));
    Ok(())
}

/// Creates a field (with intermediate parents for dotted names). Returns the existing field
/// when one with the same name and type already exists.
pub fn create_field(doc: &mut Doc, name: &str, ftype: &str, flags: i64) -> Res<ObjectId> {
    if let Some(f) = find(&doc.inner, name) {
        if f.ftype == ftype {
            return Ok(f.id);
        }
        return Err(format!(
            "a field named '{name}' already exists with type {}",
            f.ftype
        ));
    }
    let form = ensure_acroform(doc)?;
    let parts: Vec<&str> = name.split('.').collect();
    let mut parent: Option<ObjectId> = None;
    for (i, part) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        let siblings: Vec<ObjectId> = match parent {
            None => acroform(&doc.inner)
                .and_then(|a| a.get(b"Fields").ok())
                .and_then(|o| deref(&doc.inner, o).as_array().ok())
                .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
                .unwrap_or_default(),
            Some(p) => doc
                .inner
                .get_dictionary(p)
                .ok()
                .and_then(|d| d.get(b"Kids").ok())
                .and_then(|o| deref(&doc.inner, o).as_array().ok())
                .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
                .unwrap_or_default(),
        };
        let existing = siblings.into_iter().find(|s| {
            doc.inner
                .get_dictionary(*s)
                .ok()
                .and_then(|d| d.get(b"T").ok())
                .and_then(object_to_text)
                .as_deref()
                == Some(part)
        });
        let id = match existing {
            Some(id) if !last => id,
            _ => {
                let mut d = Dictionary::from_iter(vec![
                    ("T", encode_text(part)),
                    ("Kids", Object::Array(vec![])),
                ]);
                if last {
                    d.set("FT", Object::Name(ftype.as_bytes().to_vec()));
                    if flags != 0 {
                        d.set("Ff", Object::Integer(flags));
                    }
                }
                if let Some(p) = parent {
                    d.set("Parent", Object::Reference(p));
                }
                let id = doc.inner.add_object(d);
                match parent {
                    None => push_ref(&mut doc.inner, form, b"Fields", id)?,
                    Some(p) => push_ref(&mut doc.inner, p, b"Kids", id)?,
                }
                id
            }
        };
        parent = Some(id);
    }
    doc.touch();
    Ok(parent.expect("name has at least one part"))
}

pub struct WidgetOpts {
    pub on_state: Option<String>,
    pub border: Vec<f64>,
    pub background: Vec<f64>,
    pub border_width: f64,
    pub caption: Option<String>,
}

/// Creates a widget annotation for `field` (not yet placed on a page) and its appearance.
pub fn create_widget(
    doc: &mut Doc,
    field: ObjectId,
    rect: [f64; 4],
    opts: WidgetOpts,
) -> Res<ObjectId> {
    let mut mk = Dictionary::new();
    if !opts.border.is_empty() {
        mk.set(
            "BC",
            Object::Array(opts.border.iter().map(|v| real(*v)).collect()),
        );
    }
    if !opts.background.is_empty() {
        mk.set(
            "BG",
            Object::Array(opts.background.iter().map(|v| real(*v)).collect()),
        );
    }
    if let Some(c) = &opts.caption {
        mk.set("CA", encode_text(c));
    }
    let mut d = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Annot".to_vec())),
        ("Subtype", Object::Name(b"Widget".to_vec())),
        (
            "Rect",
            Object::Array(rect.iter().map(|v| real(*v)).collect()),
        ),
        ("F", Object::Integer(4)),
        ("Parent", Object::Reference(field)),
        ("MK", Object::Dictionary(mk)),
        (
            "BS",
            Object::Dictionary(Dictionary::from_iter(vec![
                ("W", real(opts.border_width)),
                ("S", Object::Name(b"S".to_vec())),
            ])),
        ),
    ]);
    let ftype = field_attr(&doc.inner, field, b"FT")
        .and_then(|o| o.as_name().ok().map(|n| n.to_vec()))
        .unwrap_or_default();
    if ftype == b"Btn" {
        d.set("AS", Object::Name(b"Off".to_vec()));
    }
    let field_dict = doc
        .inner
        .get_dictionary(field)
        .map_err(err("invalid field"))?;
    let has_kids = field_dict
        .get(b"Kids")
        .ok()
        .and_then(|o| deref(&doc.inner, o).as_array().ok())
        .is_some_and(|k| !k.is_empty());
    let merged = is_widget(field_dict);
    let flags = field_attr(&doc.inner, field, b"Ff")
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0);
    let radio = ftype == b"Btn" && flags & FF_RADIO != 0;
    let widget = if !radio && !has_kids && !merged {
        // A single widget is merged into its field (what most producers write).
        d.remove(b"Parent");
        let fd = doc
            .inner
            .get_dictionary_mut(field)
            .map_err(err("invalid field"))?;
        fd.remove(b"Kids");
        for (k, v) in d.into_iter() {
            fd.set(k, v);
        }
        field
    } else {
        if merged {
            split_merged(doc, field)?;
        }
        let widget = doc.inner.add_object(d);
        push_ref(&mut doc.inner, field, b"Kids", widget)?;
        widget
    };
    doc.touch();

    let f = fields(&doc.inner)
        .into_iter()
        .find(|f| f.id == field)
        .ok_or("field not found after creation")?;
    if f.kind() == "check" || f.kind() == "radio" {
        // Build Off/On appearances under the requested on-state name.
        widget_appearance(doc, &f, widget)?;
        if let Some(on) = opts.on_state {
            rename_on_state(&mut doc.inner, widget, &on)?;
        }
    } else {
        widget_appearance(doc, &f, widget)?;
    }
    Ok(widget)
}

fn rename_on_state(doc: &mut Document, widget: ObjectId, on: &str) -> Res<()> {
    let d = doc
        .get_dictionary_mut(widget)
        .map_err(err("invalid widget"))?;
    if let Ok(Object::Dictionary(ap)) = d.get_mut(b"AP")
        && let Ok(Object::Dictionary(n)) = ap.get_mut(b"N")
    {
        let old: Option<Vec<u8>> = n.iter().map(|(k, _)| k.clone()).find(|k| k != b"Off");
        if let Some(old) = old
            && let Some(v) = n.remove(&old)
        {
            n.set(on.as_bytes(), v);
        }
    }
    Ok(())
}

/// Keys that belong to the widget part of a merged field/widget dictionary.
const WIDGET_KEYS: [&[u8]; 14] = [
    b"Type", b"Subtype", b"Rect", b"F", b"P", b"MK", b"BS", b"AP", b"AS", b"Border", b"C", b"H",
    b"NM", b"M",
];

/// Turns a merged field/widget into a field with one kid widget (before adding a second
/// widget), and re-points the page's /Annots entry to the new kid.
fn split_merged(doc: &mut Doc, field: ObjectId) -> Res<()> {
    let fd = doc
        .inner
        .get_dictionary_mut(field)
        .map_err(err("invalid field"))?;
    let mut kid = Dictionary::new();
    for key in WIDGET_KEYS {
        if let Some(v) = fd.remove(key) {
            kid.set(key, v);
        }
    }
    kid.set("Parent", Object::Reference(field));
    let page = kid.get(b"P").ok().and_then(|o| o.as_reference().ok());
    let kid_id = doc.inner.add_object(kid);
    doc.inner
        .get_dictionary_mut(field)
        .map_err(err("invalid field"))?
        .set("Kids", Object::Array(vec![Object::Reference(kid_id)]));
    let pages: Vec<ObjectId> = match page {
        Some(p) => vec![p],
        None => doc.page_ids(),
    };
    for p in pages {
        let annots = crate::annots::page_annots(&doc.inner, p);
        if annots.contains(&field) {
            let updated: Vec<Object> = annots
                .into_iter()
                .map(|a| Object::Reference(if a == field { kid_id } else { a }))
                .collect();
            doc.inner
                .get_dictionary_mut(p)
                .map_err(err("invalid page"))?
                .set("Annots", Object::Array(updated));
        }
    }
    Ok(())
}

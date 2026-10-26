//! XFDF import (ISO 19444-1), as exported by Acrobat and WebViewer: annotations, form field
//! values, and command XFDF (<add>/<modify>/<delete>). Imported annotations get generated
//! appearances so they display (and print) in any viewer.

use base64::Engine;
use lopdf::{Dictionary, Object, ObjectId};

use crate::annots;
use crate::appearance;
use crate::content::{Ops, Resources};
use crate::doc::{Doc, Res, err, real};
use crate::forms;
use crate::image;
use crate::strings::{encode_text, object_to_text};
use crate::xml::{El, Node, parse_xml};

#[derive(Default, Debug)]
pub struct MergeStats {
    pub added: usize,
    pub replaced: usize,
    pub deleted: usize,
    pub fields: usize,
    pub skipped: Vec<String>,
}

fn subtype(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "text" => "Text",
        "freetext" => "FreeText",
        "line" => "Line",
        "square" => "Square",
        "circle" => "Circle",
        "polygon" => "Polygon",
        "polyline" => "PolyLine",
        "highlight" => "Highlight",
        "underline" => "Underline",
        "squiggly" => "Squiggly",
        "strikeout" => "StrikeOut",
        "stamp" => "Stamp",
        "caret" => "Caret",
        "ink" => "Ink",
        "redact" => "Redact",
        "link" => "Link",
        _ => return None,
    })
}

/// Attribute value of an element (attributes are kept raw; values are unescaped here).
fn attr(e: &El, name: &str) -> Option<String> {
    let raw = &e.attrs;
    let mut rest = raw.as_str();
    while let Some(pos) = rest.find(name) {
        let before_ok = pos == 0 || rest[..pos].ends_with(char::is_whitespace);
        let after = &rest[pos + name.len()..];
        let after_trim = after.trim_start();
        if before_ok && after_trim.starts_with('=') {
            let v = after_trim[1..].trim_start();
            let quote = v.chars().next()?;
            if quote == '"' || quote == '\'' {
                let end = v[1..].find(quote)?;
                return Some(unescape(&v[1..1 + end]));
            }
        }
        rest = &rest[pos + name.len()..];
    }
    None
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let ent = &tail[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                u32::from_str_radix(&ent[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn text_of(e: &El) -> String {
    let mut s = String::new();
    fn walk(n: &Node, s: &mut String) {
        match n {
            Node::Text(t) => s.push_str(t),
            Node::El(e) => {
                if e.is("p") && !s.is_empty() {
                    s.push('\n');
                }
                e.children.iter().for_each(|c| walk(c, s));
            }
            Node::Raw(_) => {}
        }
    }
    e.children.iter().for_each(|c| walk(c, &mut s));
    s
}

fn nums(s: &str) -> Vec<f64> {
    s.split([',', ';', ' ', '\n', '\t'])
        .filter(|x| !x.trim().is_empty())
        .filter_map(|x| x.trim().parse().ok())
        .collect()
}

fn color(s: &str) -> Option<Vec<f64>> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let c = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16)
            .ok()
            .map(|v| v as f64 / 255.0)
    };
    Some(vec![c(0)?, c(2)?, c(4)?])
}

fn arr(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| real(*x)).collect())
}

fn flags(s: &str) -> i64 {
    s.split(',')
        .map(|f| match f.trim().to_ascii_lowercase().as_str() {
            "invisible" => 1,
            "hidden" => 2,
            "print" => 4,
            "nozoom" => 8,
            "norotate" => 16,
            "noview" => 32,
            "readonly" => 64,
            "locked" => 128,
            "togglenoview" => 256,
            "lockedcontents" => 512,
            _ => 0,
        })
        .fold(0, |a, b| a | b)
}

fn child<'e>(e: &'e El, name: &str) -> Option<&'e El> {
    e.children.iter().find_map(|c| match c {
        Node::El(x) if x.is(name) => Some(x),
        _ => None,
    })
}

fn elements(e: &El) -> impl Iterator<Item = &El> {
    e.children.iter().filter_map(|c| match c {
        Node::El(x) => Some(x),
        _ => None,
    })
}

/// "font: Helvetica 12pt; color: #E44234" → (size, color)
fn default_style(s: &str) -> (Option<f64>, Option<Vec<f64>>) {
    let mut size = None;
    let mut col = None;
    for part in s.split(';') {
        let Some((k, v)) = part.split_once(':') else {
            continue;
        };
        match k.trim() {
            "font" => {
                size = v
                    .split_whitespace()
                    .find_map(|w| w.trim_end_matches("pt").parse().ok())
            }
            "font-size" => size = v.trim().trim_end_matches("pt").parse().ok(),
            "color" => col = color(v.trim()),
            _ => {}
        }
    }
    (size, col)
}

fn find_by_name(doc: &Doc, page: ObjectId, name: &str) -> Option<ObjectId> {
    annots::page_annots(&doc.inner, page).into_iter().find(|a| {
        doc.inner
            .get_dictionary(*a)
            .ok()
            .and_then(|d| d.get(b"NM").ok())
            .and_then(object_to_text)
            .as_deref()
            == Some(name)
    })
}

/// Builds and places one annotation from its XFDF element.
fn import_annot(doc: &mut Doc, e: &El, stats: &mut MergeStats) -> Res<()> {
    let tag = e.name.rsplit(':').next().unwrap_or(&e.name);
    let Some(st) = subtype(tag) else {
        if tag != "popup" {
            stats.skipped.push(tag.to_string());
        }
        return Ok(());
    };
    let page_index: usize = attr(e, "page")
        .and_then(|p| p.trim().parse().ok())
        .ok_or_else(|| format!("<{tag}> without a valid page attribute"))?;
    let page = doc
        .page_id(page_index)
        .map_err(|_| format!("<{tag}> refers to page {page_index}, which does not exist"))?;
    let rect = attr(e, "rect")
        .map(|r| nums(&r))
        .filter(|r| r.len() == 4)
        .ok_or_else(|| format!("<{tag}> without a valid rect"))?;
    let rect = [
        rect[0].min(rect[2]),
        rect[1].min(rect[3]),
        rect[0].max(rect[2]),
        rect[1].max(rect[3]),
    ];

    let mut d = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Annot".to_vec())),
        ("Subtype", Object::Name(st.as_bytes().to_vec())),
        ("Rect", arr(&rect)),
        (
            "F",
            Object::Integer(attr(e, "flags").map(|f| flags(&f)).unwrap_or(4)),
        ),
    ]);
    let name = attr(e, "name");
    let text_attrs = [
        ("name", "NM"),
        ("title", "T"),
        ("subject", "Subj"),
        ("date", "M"),
        ("creationdate", "CreationDate"),
    ];
    for (a, k) in text_attrs {
        if let Some(v) = attr(e, a) {
            d.set(k, encode_text(&v));
        }
    }
    if let Some(c) = attr(e, "color").and_then(|c| color(&c)) {
        d.set("C", arr(&c));
    }
    if let Some(c) = attr(e, "interior-color").and_then(|c| color(&c)) {
        d.set("IC", arr(&c));
    }
    if let Some(o) = attr(e, "opacity").and_then(|o| o.trim().parse::<f64>().ok()) {
        d.set("CA", real(o));
    }
    let width = attr(e, "width").and_then(|w| w.trim().parse::<f64>().ok());
    let dashes = attr(e, "dashes").map(|s| nums(&s)).unwrap_or_default();
    if width.is_some() || !dashes.is_empty() || attr(e, "style").is_some() {
        let mut bs = Dictionary::from_iter(vec![("W", real(width.unwrap_or(1.0)))]);
        let dashed = !dashes.is_empty() || attr(e, "style").as_deref() == Some("dash");
        bs.set(
            "S",
            Object::Name(if dashed { b"D".to_vec() } else { b"S".to_vec() }),
        );
        if dashed {
            bs.set("D", arr(if dashes.is_empty() { &[3.0] } else { &dashes }));
        }
        d.set("BS", Object::Dictionary(bs));
    }
    if let Some(c) = child(e, "contents") {
        d.set("Contents", encode_text(&text_of(c)));
    } else if let Some(rc) = child(e, "contents-richtext") {
        d.set("Contents", encode_text(&text_of(rc)));
    }
    if let Some(icon) = attr(e, "icon") {
        d.set("Name", Object::Name(icon.replace(' ', "").into_bytes()));
    }
    match tag {
        "highlight" | "underline" | "squiggly" | "strikeout" | "redact" => {
            if let Some(q) = attr(e, "coords").map(|c| nums(&c)).filter(|q| q.len() >= 8) {
                d.set("QuadPoints", arr(&q[..q.len() / 8 * 8]));
            }
            if let Some(t) = attr(e, "overlay-text") {
                d.set("OverlayText", encode_text(&t));
            }
        }
        "ink" => {
            let paths: Vec<Object> = child(e, "inklist")
                .map(|l| {
                    elements(l)
                        .filter(|g| g.is("gesture"))
                        .map(|g| arr(&nums(&text_of(g))))
                        .collect()
                })
                .unwrap_or_default();
            d.set("InkList", Object::Array(paths));
        }
        "line" => {
            let start = attr(e, "start").map(|s| nums(&s)).unwrap_or_default();
            let end = attr(e, "end").map(|s| nums(&s)).unwrap_or_default();
            if start.len() >= 2 && end.len() >= 2 {
                d.set("L", arr(&[start[0], start[1], end[0], end[1]]));
            }
            let le =
                |a: &str| Object::Name(attr(e, a).unwrap_or_else(|| "None".into()).into_bytes());
            d.set("LE", Object::Array(vec![le("head"), le("tail")]));
        }
        "polygon" | "polyline" => {
            if let Some(v) = child(e, "vertices") {
                d.set("Vertices", arr(&nums(&text_of(v))));
            }
        }
        "freetext" => {
            let da = child(e, "defaultappearance").map(text_of);
            let (size, col) = child(e, "defaultstyle")
                .map(|s| default_style(&text_of(s)))
                .unwrap_or((None, None));
            let da = da.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
                let c = col.clone().unwrap_or(vec![0.0, 0.0, 0.0]);
                format!(
                    "/Helv {} Tf {} {} {} rg",
                    size.unwrap_or(12.0),
                    c[0],
                    c[1],
                    c[2]
                )
            });
            d.set("DA", Object::string_literal(da));
            if let Some(q) = attr(e, "justification").and_then(|q| q.trim().parse::<i64>().ok()) {
                d.set("Q", Object::Integer(q));
            }
            if width.is_none() {
                d.set(
                    "BS",
                    Object::Dictionary(Dictionary::from_iter(vec![("W", Object::Integer(0))])),
                );
            }
        }
        _ => {}
    }

    let replaced = match name.as_deref().and_then(|n| find_by_name(doc, page, n)) {
        Some(old) => {
            annots::remove_from_page(doc, page, old)?;
            true
        }
        None => false,
    };
    let id = doc.inner.add_object(d);
    annots::add_to_page(doc, page, id, None)?;

    // Appearance: custom stamp image, otherwise generated from the properties.
    let image_data = child(e, "imagedata")
        .map(text_of)
        .filter(|s| !s.trim().is_empty());
    match image_data {
        Some(uri) => {
            let b64 = uri.split_once(',').map(|x| x.1).unwrap_or(&uri);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| format!("invalid stamp image data: {e}"))?;
            let img = image::embed(&mut doc.inner, &bytes)?;
            let mut res = Resources::default();
            let xname = res.xobject(img.obj);
            let mut ops = Ops::new();
            ops.q()
                .cm([
                    rect[2] - rect[0],
                    0.0,
                    0.0,
                    rect[3] - rect[1],
                    rect[0],
                    rect[1],
                ])
                .do_xobject(&xname)
                .Q();
            appearance::set_appearance(doc, id, rect, res, ops.into_bytes())?;
        }
        None => {
            appearance::generate(doc, id)?;
        }
    }
    if replaced {
        stats.replaced += 1;
    } else {
        stats.added += 1;
    }
    Ok(())
}

fn import_fields(doc: &mut Doc, parent: &str, e: &El, stats: &mut MergeStats) -> Res<()> {
    for f in elements(e).filter(|x| x.is("field")) {
        let Some(n) = attr(f, "name") else { continue };
        let full = if parent.is_empty() {
            n
        } else {
            format!("{parent}.{n}")
        };
        if let Some(v) = child(f, "value")
            && let Some(field) = forms::find(&doc.inner, &full)
        {
            forms::set_value(doc, &field, &text_of(v))?;
            stats.fields += 1;
        }
        import_fields(doc, &full, f, stats)?;
    }
    Ok(())
}

fn delete_ids(doc: &mut Doc, e: &El, stats: &mut MergeStats) -> Res<()> {
    for id in elements(e).filter(|x| x.is("id")) {
        let name = text_of(id);
        let pages: Vec<ObjectId> = match attr(id, "page").and_then(|p| p.parse::<usize>().ok()) {
            Some(p) => doc.page_id(p).into_iter().collect(),
            None => doc.page_ids(),
        };
        for p in pages {
            if let Some(a) = find_by_name(doc, p, name.trim()) {
                annots::remove_from_page(doc, p, a)?;
                stats.deleted += 1;
            }
        }
    }
    Ok(())
}

fn root(xfdf: &str) -> Res<El> {
    let doc =
        parse_xml(xfdf.trim_start_matches('\u{feff}')).map_err(|e| format!("invalid XFDF: {e}"))?;
    let r = doc.root;
    if r.name.rsplit(':').next() != Some("xfdf") {
        return Err(format!(
            "invalid XFDF: root element is <{}>, expected <xfdf>",
            r.name
        ));
    }
    Ok(r)
}

/// Checks that the string is well-formed XFDF.
pub fn validate(xfdf: &str) -> Res<()> {
    root(xfdf).map(|_| ())
}

/// Imports the annotations / field values / commands of an XFDF document into `doc`.
pub fn merge(doc: &mut Doc, xfdf: &str) -> Res<MergeStats> {
    let r = root(xfdf)?;
    let mut stats = MergeStats::default();
    for section in elements(&r) {
        match section.name.as_str() {
            "annots" | "add" | "modify" => {
                for a in elements(section) {
                    import_annot(doc, a, &mut stats)?;
                }
            }
            "delete" => delete_ids(doc, section, &mut stats)?,
            "fields" => import_fields(doc, "", section, &mut stats)?,
            _ => {}
        }
    }
    doc.touch();
    let _ = err::<String>;
    Ok(stats)
}

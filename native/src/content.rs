//! Helpers to write content: a small content-stream builder, resource dictionaries,
//! Form XObjects, and appending/prepending content to pages.

use std::fmt::Write as _;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::doc::{Res, deref, err, inherited_in, real};

/// Fluent builder for content-stream operators.
#[derive(Default)]
pub struct Ops {
    pub buf: String,
}

fn n(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e9 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

impl Ops {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn raw(&mut self, s: &str) -> &mut Self {
        self.buf.push_str(s);
        self.buf.push('\n');
        self
    }

    pub fn q(&mut self) -> &mut Self {
        self.raw("q")
    }

    #[allow(non_snake_case)]
    pub fn Q(&mut self) -> &mut Self {
        self.raw("Q")
    }

    pub fn cm(&mut self, m: [f64; 6]) -> &mut Self {
        let _ = writeln!(
            self.buf,
            "{} {} {} {} {} {} cm",
            n(m[0]),
            n(m[1]),
            n(m[2]),
            n(m[3]),
            n(m[4]),
            n(m[5])
        );
        self
    }

    /// Fill color from 1 (gray), 3 (RGB) or 4 (CMYK) components. Empty = no change.
    pub fn fill(&mut self, c: &[f64]) -> &mut Self {
        self.color(c, false)
    }

    pub fn stroke(&mut self, c: &[f64]) -> &mut Self {
        self.color(c, true)
    }

    fn color(&mut self, c: &[f64], stroke: bool) -> &mut Self {
        let parts: Vec<String> = c.iter().map(|v| n(*v)).collect();
        let op = match (c.len(), stroke) {
            (1, false) => "g",
            (1, true) => "G",
            (3, false) => "rg",
            (3, true) => "RG",
            (4, false) => "k",
            (4, true) => "K",
            _ => return self,
        };
        let _ = writeln!(self.buf, "{} {op}", parts.join(" "));
        self
    }

    pub fn line_width(&mut self, w: f64) -> &mut Self {
        let _ = writeln!(self.buf, "{} w", n(w));
        self
    }

    pub fn dash(&mut self, pattern: &[f64], phase: f64) -> &mut Self {
        let p: Vec<String> = pattern.iter().map(|v| n(*v)).collect();
        let _ = writeln!(self.buf, "[{}] {} d", p.join(" "), n(phase));
        self
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        let _ = writeln!(self.buf, "{} {} {} {} re", n(x), n(y), n(w), n(h));
        self
    }

    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        let _ = writeln!(self.buf, "{} {} m", n(x), n(y));
        self
    }

    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        let _ = writeln!(self.buf, "{} {} l", n(x), n(y));
        self
    }

    pub fn curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) -> &mut Self {
        let _ = writeln!(
            self.buf,
            "{} {} {} {} {} {} c",
            n(x1),
            n(y1),
            n(x2),
            n(y2),
            n(x3),
            n(y3)
        );
        self
    }

    /// Ellipse inscribed in the rectangle (4 Bézier arcs).
    pub fn ellipse(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) -> &mut Self {
        const K: f64 = 0.552_284_75;
        let (cx, cy) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
        let (rx, ry) = ((x2 - x1) / 2.0, (y2 - y1) / 2.0);
        self.move_to(cx + rx, cy)
            .curve_to(cx + rx, cy + ry * K, cx + rx * K, cy + ry, cx, cy + ry)
            .curve_to(cx - rx * K, cy + ry, cx - rx, cy + ry * K, cx - rx, cy)
            .curve_to(cx - rx, cy - ry * K, cx - rx * K, cy - ry, cx, cy - ry)
            .curve_to(cx + rx * K, cy - ry, cx + rx, cy - ry * K, cx + rx, cy)
            .close()
    }

    pub fn close(&mut self) -> &mut Self {
        self.raw("h")
    }

    pub fn op(&mut self, op: &str) -> &mut Self {
        self.raw(op)
    }

    pub fn gs(&mut self, name: &str) -> &mut Self {
        let _ = writeln!(self.buf, "/{name} gs");
        self
    }

    pub fn do_xobject(&mut self, name: &str) -> &mut Self {
        let _ = writeln!(self.buf, "/{name} Do");
        self
    }

    pub fn begin_text(&mut self) -> &mut Self {
        self.raw("BT")
    }

    pub fn end_text(&mut self) -> &mut Self {
        self.raw("ET")
    }

    pub fn font(&mut self, name: &str, size: f64) -> &mut Self {
        let _ = writeln!(self.buf, "/{name} {} Tf", n(size));
        self
    }

    pub fn text_matrix(&mut self, m: [f64; 6]) -> &mut Self {
        let _ = writeln!(
            self.buf,
            "{} {} {} {} {} {} Tm",
            n(m[0]),
            n(m[1]),
            n(m[2]),
            n(m[3]),
            n(m[4]),
            n(m[5])
        );
        self
    }

    pub fn show(&mut self, encoded: &[u8]) -> &mut Self {
        let hex: String = encoded.iter().map(|b| format!("{b:02X}")).collect();
        let _ = writeln!(self.buf, "<{hex}> Tj");
        self
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf.into_bytes()
    }
}

// ------------------------------------------------------------------ resources

/// A resource dictionary being assembled for a new content stream or XObject.
#[derive(Default, Clone)]
pub struct Resources {
    pub dict: Dictionary,
}

impl Resources {
    fn sub(&mut self, key: &str) -> &mut Dictionary {
        if !matches!(self.dict.get(key.as_bytes()), Ok(Object::Dictionary(_))) {
            self.dict.set(key, Object::Dictionary(Dictionary::new()));
        }
        match self.dict.get_mut(key.as_bytes()) {
            Ok(Object::Dictionary(d)) => d,
            _ => unreachable!(),
        }
    }

    /// Adds `obj` under `key` and returns its resource name (reusing an existing entry).
    pub fn add(&mut self, key: &str, prefix: &str, obj: ObjectId) -> String {
        let sub = self.sub(key);
        if let Some((k, _)) = sub.iter().find(|(_, v)| v.as_reference().ok() == Some(obj)) {
            return String::from_utf8_lossy(k).into_owned();
        }
        let name = unique_name(sub, prefix);
        sub.set(name.as_bytes(), Object::Reference(obj));
        name
    }

    pub fn font(&mut self, obj: ObjectId) -> String {
        self.add("Font", "F", obj)
    }

    pub fn xobject(&mut self, obj: ObjectId) -> String {
        self.add("XObject", "X", obj)
    }

    /// Graphics state with constant alpha (fill and stroke) and an optional blend mode.
    pub fn opacity(&mut self, alpha: f64, blend: Option<&str>) -> String {
        let mut gs = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"ExtGState".to_vec())),
            ("CA", Object::Real(alpha as f32)),
            ("ca", Object::Real(alpha as f32)),
        ]);
        if let Some(b) = blend {
            gs.set("BM", Object::Name(b.as_bytes().to_vec()));
        }
        let sub = self.sub("ExtGState");
        let name = unique_name(sub, "GS");
        sub.set(name.as_bytes(), Object::Dictionary(gs));
        name
    }
}

pub fn unique_name(dict: &Dictionary, prefix: &str) -> String {
    (0..)
        .map(|i| format!("{prefix}{i}"))
        .find(|k| !dict.has(k.as_bytes()))
        .unwrap()
}

/// Creates a compressed Form XObject.
pub fn form_xobject(
    doc: &mut Document,
    bbox: [f64; 4],
    matrix: Option<[f64; 6]>,
    res: Resources,
    content: Vec<u8>,
) -> ObjectId {
    let mut dict = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"XObject".to_vec())),
        ("Subtype", Object::Name(b"Form".to_vec())),
        ("FormType", Object::Integer(1)),
        (
            "BBox",
            Object::Array(bbox.iter().map(|v| real(*v)).collect()),
        ),
        ("Resources", Object::Dictionary(res.dict)),
    ]);
    if let Some(m) = matrix {
        dict.set(
            "Matrix",
            Object::Array(m.iter().map(|v| real(*v)).collect()),
        );
    }
    let mut s = Stream::new(dict, content);
    let _ = s.compress();
    doc.add_object(s)
}

// ------------------------------------------------------------------ pages

/// Marker key put on the content streams we add to pages, so they can be found again
/// (Stamper.deleteStamps) and so the q/Q isolation wrappers are added only once.
pub const STAMP_KEY: &[u8] = b"BWStamp";
const WRAP_KEY: &[u8] = b"BWWrap";

/// Makes the page own a direct /Resources dictionary (copy-on-write of shared or inherited
/// resources) and returns it, with the sub-dictionary `key` also made direct.
pub fn page_resource_sub<'a>(
    doc: &'a mut Document,
    page: ObjectId,
    key: &str,
) -> Res<&'a mut Dictionary> {
    let resolved = match inherited_in(doc, page, b"Resources") {
        Some(o) => deref(doc, &o).as_dict().cloned().unwrap_or_default(),
        None => Dictionary::new(),
    };
    let sub = match resolved.get(key.as_bytes()) {
        Ok(o) => deref(doc, o).as_dict().cloned().unwrap_or_default(),
        Err(_) => Dictionary::new(),
    };
    let mut res = resolved;
    res.set(key.as_bytes(), Object::Dictionary(sub));
    let page_dict = doc.get_dictionary_mut(page).map_err(err("invalid page"))?;
    page_dict.set("Resources", Object::Dictionary(res));
    match page_dict.get_mut(b"Resources") {
        Ok(Object::Dictionary(r)) => match r.get_mut(key.as_bytes()) {
            Ok(Object::Dictionary(d)) => Ok(d),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
}

/// Registers an XObject in the page resources and returns its name.
pub fn page_add_xobject(doc: &mut Document, page: ObjectId, xobj: ObjectId) -> Res<String> {
    let sub = page_resource_sub(doc, page, "XObject")?;
    if let Some((k, _)) = sub
        .iter()
        .find(|(_, v)| v.as_reference().ok() == Some(xobj))
    {
        return Ok(String::from_utf8_lossy(k).into_owned());
    }
    let name = unique_name(sub, "BWX");
    sub.set(name.as_bytes(), Object::Reference(xobj));
    Ok(name)
}

/// Current /Contents of a page as a list of stream references.
pub fn page_content_refs(doc: &Document, page: ObjectId) -> Vec<ObjectId> {
    let Ok(dict) = doc.get_dictionary(page) else {
        return vec![];
    };
    match dict.get(b"Contents") {
        Ok(Object::Reference(id)) => match doc.get_object(*id) {
            Ok(Object::Array(items)) => {
                items.iter().filter_map(|o| o.as_reference().ok()).collect()
            }
            _ => vec![*id],
        },
        Ok(Object::Array(items)) => items.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => vec![],
    }
}

fn set_page_contents(doc: &mut Document, page: ObjectId, refs: Vec<ObjectId>) -> Res<()> {
    let arr = Object::Array(refs.into_iter().map(Object::Reference).collect());
    doc.get_dictionary_mut(page)
        .map_err(err("invalid page"))?
        .set("Contents", arr);
    Ok(())
}

fn stream_has_key(doc: &Document, id: ObjectId, key: &[u8]) -> bool {
    matches!(doc.get_object(id), Ok(Object::Stream(s)) if s.dict.has(key))
}

fn marker_stream(doc: &mut Document, content: &[u8], key: &[u8]) -> ObjectId {
    let mut dict = Dictionary::new();
    dict.set(key, Object::Boolean(true));
    doc.add_object(Stream::new(dict, content.to_vec()))
}

/// Adds a self-contained content stream (it must balance its own q/Q) to a page.
/// Foreground content goes after the page content, which is isolated in q … Q first so that
/// its leftover graphics state cannot affect us; background content goes before everything.
pub fn page_add_content(
    doc: &mut Document,
    page: ObjectId,
    content: Vec<u8>,
    foreground: bool,
    marker: Option<&[u8]>,
) -> Res<ObjectId> {
    let mut refs = page_content_refs(doc, page);
    let mut s = Stream::new(Dictionary::new(), content);
    if let Some(key) = marker {
        s.dict.set(key, Object::Boolean(true));
    }
    let _ = s.compress();
    let id = doc.add_object(s);
    if foreground {
        let wrapped = refs
            .first()
            .is_some_and(|r| stream_has_key(doc, *r, WRAP_KEY));
        if !wrapped && !refs.is_empty() {
            let open = marker_stream(doc, b"q\n", WRAP_KEY);
            let close = marker_stream(doc, b"\nQ\n", WRAP_KEY);
            refs.insert(0, open);
            refs.push(close);
        }
        refs.push(id);
    } else {
        refs.insert(0, id);
    }
    set_page_contents(doc, page, refs)?;
    Ok(id)
}

/// Removes the content streams carrying `marker`. Returns how many were removed.
pub fn page_remove_marked(doc: &mut Document, page: ObjectId, marker: &[u8]) -> Res<usize> {
    let refs = page_content_refs(doc, page);
    let keep: Vec<ObjectId> = refs
        .iter()
        .copied()
        .filter(|r| !stream_has_key(doc, *r, marker))
        .collect();
    let removed = refs.len() - keep.len();
    if removed > 0 {
        set_page_contents(doc, page, keep)?;
    }
    Ok(removed)
}

pub fn page_has_marked(doc: &Document, page: ObjectId, marker: &[u8]) -> bool {
    page_content_refs(doc, page)
        .iter()
        .any(|r| stream_has_key(doc, *r, marker))
}

/// Matrix (PDF order) that maps display coordinates of a page — origin at the bottom-left of
/// the visible box, page rotation applied — back to user space.
pub fn display_to_user(crop: [f64; 4], rotation: i64) -> [f64; 6] {
    let [x1, y1, x2, y2] = crop;
    match rotation {
        90 => [0.0, 1.0, -1.0, 0.0, x2, y1],
        180 => [-1.0, 0.0, 0.0, -1.0, x2, y2],
        270 => [0.0, -1.0, 1.0, 0.0, x1, y2],
        _ => [1.0, 0.0, 0.0, 1.0, x1, y1],
    }
}

/// a × b in PDF row-vector convention (apply a, then b).
pub fn concat(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

pub fn apply(m: [f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// Bounding box of a rectangle transformed by `m`.
pub fn transform_rect(m: [f64; 6], r: [f64; 4]) -> [f64; 4] {
    let pts = [
        apply(m, r[0], r[1]),
        apply(m, r[2], r[1]),
        apply(m, r[2], r[3]),
        apply(m, r[0], r[3]),
    ];
    let xs = pts.iter().map(|p| p.0);
    let ys = pts.iter().map(|p| p.1);
    [
        xs.clone().fold(f64::INFINITY, f64::min),
        ys.clone().fold(f64::INFINITY, f64::min),
        xs.fold(f64::NEG_INFINITY, f64::max),
        ys.fold(f64::NEG_INFINITY, f64::max),
    ]
}

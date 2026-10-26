//! Fonts used to *write* text (stamps, annotation and form-field appearances).
//!
//! - The 14 standard fonts, WinAnsi-encoded, with their AFM metrics.
//! - Simple fonts already present in a document (e.g. form /DR fonts), measured with /Widths.
//! - TrueType fonts embedded as Type0/CIDFontType2 (Identity-H) for full Unicode coverage.

mod std_metrics;

use std::collections::BTreeMap;
use std::sync::Arc;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::doc::{Res, deref, number};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdFont {
    TimesRoman,
    TimesBold,
    TimesItalic,
    TimesBoldItalic,
    Helvetica,
    HelveticaBold,
    HelveticaOblique,
    HelveticaBoldOblique,
    Courier,
    CourierBold,
    CourierOblique,
    CourierBoldOblique,
    Symbol,
    ZapfDingbats,
}

impl StdFont {
    /// Same numbering as PDFNet's Font.StandardType1Font.
    pub fn from_code(code: u32) -> Res<Self> {
        use StdFont::*;
        Ok(match code {
            0 => TimesRoman,
            1 => TimesBold,
            2 => TimesItalic,
            3 => TimesBoldItalic,
            4 => Helvetica,
            5 => HelveticaBold,
            6 => HelveticaOblique,
            7 => HelveticaBoldOblique,
            8 => Courier,
            9 => CourierBold,
            10 => CourierOblique,
            11 => CourierBoldOblique,
            12 => Symbol,
            13 => ZapfDingbats,
            _ => return Err(format!("unknown standard font {code}")),
        })
    }

    pub fn base_font(self) -> &'static str {
        use StdFont::*;
        match self {
            TimesRoman => "Times-Roman",
            TimesBold => "Times-Bold",
            TimesItalic => "Times-Italic",
            TimesBoldItalic => "Times-BoldItalic",
            Helvetica => "Helvetica",
            HelveticaBold => "Helvetica-Bold",
            HelveticaOblique => "Helvetica-Oblique",
            HelveticaBoldOblique => "Helvetica-BoldOblique",
            Courier => "Courier",
            CourierBold => "Courier-Bold",
            CourierOblique => "Courier-Oblique",
            CourierBoldOblique => "Courier-BoldOblique",
            Symbol => "Symbol",
            ZapfDingbats => "ZapfDingbats",
        }
    }

    /// Recognizes standard font names, including common aliases (Arial, TimesNewRoman…).
    pub fn from_base_font(name: &str) -> Option<Self> {
        use StdFont::*;
        let n = name
            .split('+')
            .next_back()
            .unwrap_or(name)
            .replace([' ', ','], "-")
            .to_ascii_lowercase();
        let bold = n.contains("bold");
        let italic = n.contains("italic") || n.contains("oblique");
        let family = if n.starts_with("helv") || n.starts_with("arial") {
            0
        } else if n.starts_with("times") {
            1
        } else if n.starts_with("cour") {
            2
        } else if n.starts_with("symbol") {
            return Some(Symbol);
        } else if n.starts_with("zapf") || n.starts_with("zadb") {
            return Some(ZapfDingbats);
        } else {
            return None;
        };
        Some(match (family, bold, italic) {
            (0, false, false) => Helvetica,
            (0, true, false) => HelveticaBold,
            (0, false, true) => HelveticaOblique,
            (0, true, true) => HelveticaBoldOblique,
            (1, false, false) => TimesRoman,
            (1, true, false) => TimesBold,
            (1, false, true) => TimesItalic,
            (1, true, true) => TimesBoldItalic,
            (2, false, false) => Courier,
            (2, true, false) => CourierBold,
            (2, false, true) => CourierOblique,
            _ => CourierBoldOblique,
        })
    }

    fn widths(self) -> Option<&'static [u16; 256]> {
        use StdFont::*;
        Some(match self {
            TimesRoman => &std_metrics::TIMES_ROMAN,
            TimesBold => &std_metrics::TIMES_BOLD,
            TimesItalic => &std_metrics::TIMES_ITALIC,
            TimesBoldItalic => &std_metrics::TIMES_BOLD_ITALIC,
            Helvetica => &std_metrics::HELVETICA,
            HelveticaBold => &std_metrics::HELVETICA_BOLD,
            HelveticaOblique => &std_metrics::HELVETICA_OBLIQUE,
            HelveticaBoldOblique => &std_metrics::HELVETICA_BOLD_OBLIQUE,
            Courier => &std_metrics::COURIER,
            CourierBold => &std_metrics::COURIER_BOLD,
            CourierOblique => &std_metrics::COURIER_OBLIQUE,
            CourierBoldOblique => &std_metrics::COURIER_BOLD_OBLIQUE,
            Symbol | ZapfDingbats => return None,
        })
    }

    /// (ascender, descender) in 1/1000 em, from the AFM files.
    fn vertical_metrics(self) -> (f64, f64) {
        use StdFont::*;
        match self {
            Helvetica | HelveticaOblique => (718.0, -207.0),
            HelveticaBold | HelveticaBoldOblique => (718.0, -207.0),
            TimesRoman => (683.0, -217.0),
            TimesBold => (683.0, -217.0),
            TimesItalic => (683.0, -217.0),
            TimesBoldItalic => (683.0, -217.0),
            Courier | CourierBold | CourierOblique | CourierBoldOblique => (629.0, -157.0),
            Symbol | ZapfDingbats => (800.0, -200.0),
        }
    }

    pub fn dictionary(self) -> Dictionary {
        let mut d = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Font".to_vec())),
            ("Subtype", Object::Name(b"Type1".to_vec())),
            (
                "BaseFont",
                Object::Name(self.base_font().as_bytes().to_vec()),
            ),
        ]);
        if self.widths().is_some() {
            d.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        }
        d
    }
}

/// Unicode → WinAnsi (cp1252) code.
pub fn win_ansi_code(c: char) -> Option<u8> {
    let u = c as u32;
    if (0x20..0x7F).contains(&u) || (0xA0..=0xFF).contains(&u) {
        return Some(u as u8);
    }
    Some(match c {
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        '\u{a0}' => 0xA0,
        _ => return None,
    })
}

/// An embedded TrueType/OpenType (glyf or CFF) font program.
pub struct TrueType {
    data: Arc<Vec<u8>>,
    pub units_per_em: f64,
    pub ascender: f64,
    pub descender: f64,
    pub postscript_name: String,
}

impl TrueType {
    pub fn parse(data: Vec<u8>) -> Res<Self> {
        let face =
            ttf_parser::Face::parse(&data, 0).map_err(|e| format!("invalid font file: {e}"))?;
        let upem = face.units_per_em() as f64;
        let name = face
            .names()
            .into_iter()
            .filter(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME)
            .find_map(|n| n.to_string())
            .unwrap_or_else(|| "EmbeddedFont".into());
        let postscript_name: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        Ok(TrueType {
            units_per_em: upem,
            ascender: face.ascender() as f64 * 1000.0 / upem,
            descender: face.descender() as f64 * 1000.0 / upem,
            postscript_name,
            data: Arc::new(data),
        })
    }

    fn with_face<T>(&self, f: impl FnOnce(&ttf_parser::Face) -> T) -> T {
        let face = ttf_parser::Face::parse(&self.data, 0).expect("validated at load");
        f(&face)
    }
}

pub enum FontKind {
    Std(StdFont),
    /// A simple font found in the document, measured with its /Widths (WinAnsi assumed).
    Simple {
        widths: Box<[u16; 256]>,
        ascender: f64,
        descender: f64,
    },
    TrueType(TrueType),
}

/// A font usable for writing, bound to a font dictionary in a document.
pub struct DocFont {
    pub kind: FontKind,
    pub obj: ObjectId,
    /// For TrueType fonts: glyphs used so far (gid → unicode), written to /W and /ToUnicode.
    used: BTreeMap<u16, (char, u16)>,
    /// Extra objects of an embedded TrueType font: (CIDFont dict, ToUnicode stream).
    cid_parts: Option<(ObjectId, ObjectId)>,
}

impl DocFont {
    pub fn std(doc: &mut Document, f: StdFont) -> DocFont {
        let obj = doc.add_object(f.dictionary());
        DocFont {
            kind: FontKind::Std(f),
            obj,
            used: BTreeMap::new(),
            cid_parts: None,
        }
    }

    /// Wraps an existing font dictionary of the document for measuring/encoding.
    /// Unknown or composite fonts fall back to Helvetica metrics.
    pub fn existing(doc: &Document, obj: ObjectId) -> DocFont {
        let dict = doc.get_dictionary(obj).ok();
        let base = dict
            .and_then(|d| d.get(b"BaseFont").ok())
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        let kind = if let Some(widths) = dict.and_then(|d| simple_widths(doc, d)) {
            let (ascender, descender) = StdFont::from_base_font(&base)
                .map(|s| s.vertical_metrics())
                .unwrap_or((750.0, -250.0));
            FontKind::Simple {
                widths: Box::new(widths),
                ascender,
                descender,
            }
        } else {
            FontKind::Std(StdFont::from_base_font(&base).unwrap_or(StdFont::Helvetica))
        };
        DocFont {
            kind,
            obj,
            used: BTreeMap::new(),
            cid_parts: None,
        }
    }

    pub fn embed_truetype(doc: &mut Document, data: Vec<u8>) -> Res<DocFont> {
        let tt = TrueType::parse(data)?;
        let (bbox, italic_angle, cap_height) = tt.with_face(|f| {
            let b = f.global_bounding_box();
            let s = 1000.0 / tt.units_per_em;
            (
                [
                    b.x_min as f64 * s,
                    b.y_min as f64 * s,
                    b.x_max as f64 * s,
                    b.y_max as f64 * s,
                ],
                f.italic_angle() as f64,
                f.capital_height()
                    .map(|c| c as f64 * s)
                    .unwrap_or(tt.ascender),
            )
        });
        let is_cff = tt.with_face(|f| f.tables().cff.is_some());
        let mut file = Stream::new(Dictionary::new(), tt.data.as_ref().clone());
        if is_cff {
            file.dict.set("Subtype", Object::Name(b"OpenType".to_vec()));
        }
        let _ = file.compress();
        let file_id = doc.add_object(file);
        let name = format!("BWAAAA+{}", tt.postscript_name);
        let descriptor = doc.add_object(Dictionary::from_iter(vec![
            ("Type", Object::Name(b"FontDescriptor".to_vec())),
            ("FontName", Object::Name(name.as_bytes().to_vec())),
            ("Flags", Object::Integer(4)),
            (
                "FontBBox",
                Object::Array(bbox.iter().map(|v| Object::Integer(*v as i64)).collect()),
            ),
            ("ItalicAngle", Object::Real(italic_angle as f32)),
            ("Ascent", Object::Integer(tt.ascender as i64)),
            ("Descent", Object::Integer(tt.descender as i64)),
            ("CapHeight", Object::Integer(cap_height as i64)),
            ("StemV", Object::Integer(80)),
            (
                if is_cff { "FontFile3" } else { "FontFile2" },
                Object::Reference(file_id),
            ),
        ]));
        let mut cid = Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Font".to_vec())),
            (
                "Subtype",
                Object::Name(if is_cff {
                    b"CIDFontType0".to_vec()
                } else {
                    b"CIDFontType2".to_vec()
                }),
            ),
            ("BaseFont", Object::Name(name.as_bytes().to_vec())),
            (
                "CIDSystemInfo",
                Object::Dictionary(Dictionary::from_iter(vec![
                    ("Registry", Object::string_literal("Adobe")),
                    ("Ordering", Object::string_literal("Identity")),
                    ("Supplement", Object::Integer(0)),
                ])),
            ),
            ("FontDescriptor", Object::Reference(descriptor)),
            ("DW", Object::Integer(1000)),
        ]);
        if !is_cff {
            cid.set("CIDToGIDMap", Object::Name(b"Identity".to_vec()));
        }
        let cid_id = doc.add_object(cid);
        let to_unicode = doc.add_object(Stream::new(Dictionary::new(), Vec::new()));
        let obj = doc.add_object(Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Font".to_vec())),
            ("Subtype", Object::Name(b"Type0".to_vec())),
            ("BaseFont", Object::Name(name.as_bytes().to_vec())),
            ("Encoding", Object::Name(b"Identity-H".to_vec())),
            (
                "DescendantFonts",
                Object::Array(vec![Object::Reference(cid_id)]),
            ),
            ("ToUnicode", Object::Reference(to_unicode)),
        ]));
        Ok(DocFont {
            kind: FontKind::TrueType(tt),
            obj,
            used: BTreeMap::new(),
            cid_parts: Some((cid_id, to_unicode)),
        })
    }

    pub fn ascender(&self) -> f64 {
        match &self.kind {
            FontKind::Std(s) => s.vertical_metrics().0,
            FontKind::Simple { ascender, .. } => *ascender,
            FontKind::TrueType(t) => t.ascender,
        }
    }

    pub fn descender(&self) -> f64 {
        match &self.kind {
            FontKind::Std(s) => s.vertical_metrics().1,
            FontKind::Simple { descender, .. } => *descender,
            FontKind::TrueType(t) => t.descender,
        }
    }

    /// Width of `text` in 1/1000 em.
    pub fn width(&self, text: &str) -> f64 {
        match &self.kind {
            FontKind::Std(_) | FontKind::Simple { .. } => {
                let w = self.simple_width_table();
                text.chars()
                    .map(|c| {
                        let code = win_ansi_code(c).unwrap_or(b'?');
                        w.map(|t| t[code as usize] as f64).unwrap_or(500.0)
                    })
                    .sum()
            }
            FontKind::TrueType(t) => t.with_face(|f| {
                text.chars()
                    .map(|c| {
                        let gid = f.glyph_index(c).unwrap_or(ttf_parser::GlyphId(0));
                        f.glyph_hor_advance(gid).unwrap_or(0) as f64 * 1000.0 / t.units_per_em
                    })
                    .sum()
            }),
        }
    }

    fn simple_width_table(&self) -> Option<&[u16; 256]> {
        match &self.kind {
            FontKind::Std(s) => s.widths(),
            FontKind::Simple { widths, .. } => Some(widths),
            FontKind::TrueType(_) => None,
        }
    }

    /// Encodes text as the bytes of a PDF string operand for this font. Characters the font
    /// cannot represent become '?' (simple fonts) or .notdef (TrueType).
    pub fn encode(&mut self, text: &str) -> Vec<u8> {
        match &self.kind {
            FontKind::Std(_) | FontKind::Simple { .. } => text
                .chars()
                .map(|c| win_ansi_code(c).unwrap_or(b'?'))
                .collect(),
            FontKind::TrueType(t) => {
                let upem = t.units_per_em;
                let glyphs: Vec<(u16, char, u16)> = t.with_face(|f| {
                    text.chars()
                        .map(|c| {
                            let gid = f.glyph_index(c).map(|g| g.0).unwrap_or(0);
                            let adv =
                                f.glyph_hor_advance(ttf_parser::GlyphId(gid)).unwrap_or(0) as f64;
                            (gid, c, (adv * 1000.0 / upem).round() as u16)
                        })
                        .collect()
                });
                let mut out = Vec::with_capacity(glyphs.len() * 2);
                for (gid, c, w) in glyphs {
                    out.extend_from_slice(&gid.to_be_bytes());
                    if gid != 0 {
                        self.used.entry(gid).or_insert((c, w));
                    }
                }
                out
            }
        }
    }

    /// Writes /W and /ToUnicode for embedded TrueType fonts (call before saving).
    pub fn finalize(&self, doc: &mut Document) {
        let Some((cid_id, tu_id)) = self.cid_parts else {
            return;
        };
        let mut w = Vec::new();
        for (gid, (_, width)) in &self.used {
            w.push(Object::Integer(*gid as i64));
            w.push(Object::Array(vec![Object::Integer(*width as i64)]));
        }
        if let Ok(d) = doc.get_dictionary_mut(cid_id) {
            d.set("W", Object::Array(w));
        }
        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        );
        let entries: Vec<_> = self.used.iter().collect();
        for chunk in entries.chunks(100) {
            cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (gid, (c, _)) in chunk {
                let mut units = [0u16; 2];
                let hex: String = c
                    .encode_utf16(&mut units)
                    .iter()
                    .map(|u| format!("{u:04X}"))
                    .collect();
                cmap.push_str(&format!("<{gid:04X}> <{hex}>\n"));
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        let mut s = Stream::new(Dictionary::new(), cmap.into_bytes());
        let _ = s.compress();
        doc.objects.insert(tu_id, Object::Stream(s));
    }
}

/// /Widths of a simple (Type1/TrueType) font, re-indexed by code. None for composite fonts.
fn simple_widths(doc: &Document, dict: &Dictionary) -> Option<[u16; 256]> {
    let subtype = dict.get(b"Subtype").ok()?.as_name().ok()?;
    if subtype == b"Type0" {
        return None;
    }
    let widths = deref(doc, dict.get(b"Widths").ok()?).as_array().ok()?;
    let first = dict
        .get(b"FirstChar")
        .ok()
        .and_then(|o| number(doc, o))
        .unwrap_or(0.0) as usize;
    let mut out = [0u16; 256];
    for (i, w) in widths.iter().enumerate() {
        if let (Some(slot), Some(v)) = (out.get_mut(first + i), number(doc, w)) {
            *slot = v.max(0.0) as u16;
        }
    }
    Some(out)
}

/// Splits text into lines that fit `max_width` (points) at `size`, honouring hard line breaks.
/// Words longer than a line are broken between characters.
pub fn wrap(font: &DocFont, size: f64, text: &str, max_width: Option<f64>) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let para = para.trim_end_matches('\r');
        let Some(max) = max_width.filter(|m| *m > 0.0) else {
            lines.push(para.to_string());
            continue;
        };
        let fits = |s: &str| font.width(s) * size / 1000.0 <= max;
        let mut current = String::new();
        for word in para.split(' ') {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if fits(&candidate) {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            // The word alone may still be too long: break it.
            let mut piece = String::new();
            for c in word.chars() {
                piece.push(c);
                if !fits(&piece) && piece.chars().count() > 1 {
                    piece.pop();
                    lines.push(std::mem::take(&mut piece));
                    piece.push(c);
                }
            }
            current = piece;
        }
        lines.push(current);
    }
    lines
}

/// AFM width (1/1000 em) of a WinAnsi code in a standard font; 0 when unknown.
pub fn std_width(f: StdFont, code: u8) -> f64 {
    f.widths().map(|t| t[code as usize] as f64).unwrap_or(0.0)
}

//! N-API surface of the Rust core. The TypeScript layer (src/) builds the PDFNet-compatible
//! API on top of these low-level, synchronous primitives.

mod annots;
mod appearance;
mod content;
mod doc;
mod docgen;
mod fonts;
mod forms;
mod image;
mod linearize;
mod optimize;
mod redact;
mod render;
mod sdf;
mod security;
mod sign;
mod stamper;
mod strings;
mod text;
mod verify;
mod xfdf;
mod xml;

use std::collections::HashMap;
use std::sync::Arc;

use lopdf::ObjectId;
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::doc::{BoxKind, Doc, SaveOpts};

fn to_napi(e: String) -> Error {
    Error::from_reason(e)
}

#[napi]
pub fn native_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

#[napi(object)]
#[derive(Clone, Copy)]
pub struct ObjRef {
    pub num: u32,
    pub generation: u32,
}

impl From<ObjectId> for ObjRef {
    fn from(id: ObjectId) -> Self {
        ObjRef {
            num: id.0,
            generation: id.1 as u32,
        }
    }
}

impl ObjRef {
    fn id(&self) -> ObjectId {
        (self.num, self.generation as u16)
    }
}

#[napi(object)]
#[derive(Default)]
pub struct SaveOptions {
    pub remove_unused: Option<bool>,
    pub compress: Option<bool>,
    pub object_streams: Option<bool>,
    pub incremental: Option<bool>,
    pub linearize: Option<bool>,
}

#[napi(object)]
#[derive(Default)]
pub struct RenderOptions {
    pub dpi: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub preserve_aspect_ratio: Option<bool>,
    pub transparent: Option<bool>,
    pub annotations: Option<bool>,
    /// "PNG" | "JPEG" | "RGBA"
    pub format: Option<String>,
    pub jpeg_quality: Option<u32>,
}

#[napi(object)]
pub struct RenderResult {
    pub width: u32,
    pub height: u32,
    pub data: Buffer,
}

#[napi(object)]
pub struct TextChar {
    pub text: String,
    pub bbox: Vec<f64>,
    pub font_size: f64,
}

#[napi(object)]
pub struct TextWord {
    pub text: String,
    pub bbox: Vec<f64>,
    pub quad: Vec<f64>,
    pub chars: Vec<TextChar>,
}

#[napi(object)]
pub struct TextLine {
    pub bbox: Vec<f64>,
    pub words: Vec<TextWord>,
}

#[napi(object)]
pub struct PageTextResult {
    pub text: String,
    pub lines: Vec<TextLine>,
}

#[napi(object)]
#[derive(Default)]
pub struct SearchOptions {
    pub regex: Option<bool>,
    pub case_sensitive: Option<bool>,
    pub whole_word: Option<bool>,
    pub ambient_chars: Option<u32>,
}

#[napi(object)]
pub struct SearchHit {
    pub text: String,
    pub ambient: String,
    pub quads: Vec<Vec<f64>>,
}

#[napi]
pub struct NativeDoc {
    doc: Doc,
    text_cache: HashMap<usize, (u64, Arc<text::PageText>)>,
}

impl NativeDoc {
    fn wrap(doc: Doc) -> Self {
        NativeDoc {
            doc,
            text_cache: HashMap::new(),
        }
    }

    fn page_text(&mut self, index: usize) -> Result<Arc<text::PageText>> {
        let rev = self.doc.revision();
        if let Some((r, t)) = self.text_cache.get(&index)
            && *r == rev
        {
            return Ok(t.clone());
        }
        let bytes = self.doc.snapshot().map_err(to_napi)?;
        let t = Arc::new(text::extract(bytes, index).map_err(to_napi)?);
        self.text_cache.insert(index, (rev, t.clone()));
        Ok(t)
    }
}

#[napi]
impl NativeDoc {
    #[napi(factory)]
    pub fn open(data: Buffer, password: Option<String>) -> Result<Self> {
        Doc::open(&data, password.as_deref())
            .map(Self::wrap)
            .map_err(to_napi)
    }

    #[napi(factory)]
    pub fn create() -> Self {
        Self::wrap(Doc::create())
    }

    #[napi]
    pub fn version(&self) -> String {
        self.doc.inner.version.clone()
    }

    #[napi]
    pub fn was_encrypted(&self) -> bool {
        self.doc.was_encrypted()
    }

    // ------------------------------------------------------------ pages

    #[napi]
    pub fn page_count(&self) -> u32 {
        self.doc.page_count() as u32
    }

    #[napi]
    pub fn page_ref(&self, index: u32) -> Result<ObjRef> {
        self.doc
            .page_id(index as usize)
            .map(ObjRef::from)
            .map_err(to_napi)
    }

    #[napi]
    pub fn page_refs(&self) -> Vec<ObjRef> {
        self.doc.page_ids().into_iter().map(ObjRef::from).collect()
    }

    /// 0-based index of the page in the page tree, or -1 when detached.
    #[napi]
    pub fn page_index(&self, page: ObjRef) -> i32 {
        self.doc.page_index(page.id()).map_or(-1, |i| i as i32)
    }

    /// box: 0 media, 1 crop, 2 bleed, 3 trim, 4 art. Returns [x1, y1, x2, y2].
    #[napi]
    pub fn page_box(&self, page: ObjRef, kind: u32) -> Result<Vec<f64>> {
        let kind = BoxKind::from_code(kind).map_err(to_napi)?;
        Ok(self.doc.page_box(page.id(), kind).to_vec())
    }

    #[napi]
    pub fn set_page_box(&mut self, page: ObjRef, kind: u32, rect: Vec<f64>) -> Result<()> {
        let kind = BoxKind::from_code(kind).map_err(to_napi)?;
        let r = rect4(&rect)?;
        self.doc.set_page_box(page.id(), kind, r).map_err(to_napi)
    }

    #[napi]
    pub fn rotation(&self, page: ObjRef) -> i32 {
        self.doc.rotation(page.id()) as i32
    }

    #[napi]
    pub fn set_rotation(&mut self, page: ObjRef, degrees: i32) -> Result<()> {
        self.doc
            .set_rotation(page.id(), degrees as i64)
            .map_err(to_napi)
    }

    /// Creates a detached page with the given media box.
    #[napi]
    pub fn new_page(&mut self, media_box: Vec<f64>) -> Result<ObjRef> {
        Ok(self.doc.new_page(rect4(&media_box)?).into())
    }

    #[napi]
    pub fn insert_page(&mut self, page: ObjRef, index: u32) -> Result<()> {
        self.doc
            .insert_page(page.id(), index as usize)
            .map_err(to_napi)
    }

    #[napi]
    pub fn remove_page(&mut self, index: u32) -> Result<()> {
        self.doc.remove_page(index as usize).map_err(to_napi)
    }

    #[napi]
    pub fn move_page(&mut self, from: u32, to: u32) -> Result<()> {
        self.doc
            .move_page(from as usize, to as usize)
            .map_err(to_napi)
    }

    /// Copies pages of another document (attached or detached) into this one at `at`.
    #[napi]
    pub fn import_pages(
        &mut self,
        src: &NativeDoc,
        pages: Vec<ObjRef>,
        at: u32,
    ) -> Result<Vec<ObjRef>> {
        let ids: Vec<ObjectId> = pages.iter().map(ObjRef::id).collect();
        let out = self
            .doc
            .import_pages(&src.doc.inner, &ids, at as usize)
            .map_err(to_napi)?;
        Ok(out.into_iter().map(ObjRef::from).collect())
    }

    /// Same as importPages with this document as the source (page duplication).
    #[napi]
    pub fn duplicate_pages(&mut self, pages: Vec<ObjRef>, at: u32) -> Result<Vec<ObjRef>> {
        let src = self.doc.inner.clone();
        let ids: Vec<ObjectId> = pages.iter().map(ObjRef::id).collect();
        let out = self
            .doc
            .import_pages(&src, &ids, at as usize)
            .map_err(to_napi)?;
        Ok(out.into_iter().map(ObjRef::from).collect())
    }

    // ------------------------------------------------------------ info

    #[napi]
    pub fn get_info(&self, key: String) -> Option<String> {
        self.doc.get_info(&key)
    }

    #[napi]
    pub fn set_info(&mut self, key: String, value: Option<String>) -> Result<()> {
        self.doc.set_info(&key, value.as_deref()).map_err(to_napi)
    }

    #[napi]
    pub fn info_keys(&self) -> Vec<String> {
        self.doc.info_keys()
    }

    // ------------------------------------------------------------ output

    #[napi]
    pub fn save(&mut self, options: Option<SaveOptions>) -> Result<Buffer> {
        let o = options.unwrap_or_default();
        let opts = SaveOpts {
            remove_unused: o.remove_unused.unwrap_or(false),
            compress: o.compress.unwrap_or(false),
            object_streams: o.object_streams.unwrap_or(false),
            incremental: o.incremental.unwrap_or(false),
            linearize: o.linearize.unwrap_or(false),
        };
        self.doc.save(opts).map(Buffer::from).map_err(to_napi)
    }

    #[napi]
    pub fn render(
        &mut self,
        page_index: u32,
        options: Option<RenderOptions>,
    ) -> Result<RenderResult> {
        let o = options.unwrap_or_default();
        let size = match (o.width, o.height) {
            (Some(w), Some(h)) => Some((w, h, o.preserve_aspect_ratio.unwrap_or(true))),
            (None, None) => None,
            _ => {
                return Err(to_napi(
                    "render: width and height must be given together".into(),
                ));
            }
        };
        let opts = render::RenderOpts {
            dpi: o.dpi.unwrap_or(92.0),
            size,
            transparent: o.transparent.unwrap_or(false),
            annotations: o.annotations.unwrap_or(true),
            format: render::Format::parse(o.format.as_deref().unwrap_or("PNG")).map_err(to_napi)?,
            jpeg_quality: o.jpeg_quality.unwrap_or(80).min(100) as u8,
        };
        let bytes = self.doc.snapshot().map_err(to_napi)?;
        let r = render::render_page(bytes, page_index as usize, &opts).map_err(to_napi)?;
        Ok(RenderResult {
            width: r.width,
            height: r.height,
            data: r.data.into(),
        })
    }

    #[napi]
    pub fn extract_text(&mut self, page_index: u32) -> Result<PageTextResult> {
        let t = self.page_text(page_index as usize)?;
        let lines = t
            .lines
            .iter()
            .map(|l| TextLine {
                bbox: l.bbox.to_vec(),
                words: l
                    .words
                    .iter()
                    .map(|w| TextWord {
                        text: w.text.clone(),
                        bbox: w.bbox.to_vec(),
                        quad: w.quad.to_vec(),
                        chars: t.glyphs[w.glyph_start..w.glyph_end]
                            .iter()
                            .filter(|g| !g.text.chars().all(char::is_whitespace))
                            .map(|g| TextChar {
                                text: g.text.clone(),
                                bbox: g.bbox.to_vec(),
                                font_size: g.size,
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        Ok(PageTextResult {
            text: t.text(),
            lines,
        })
    }

    #[napi]
    pub fn search(
        &mut self,
        page_index: u32,
        pattern: String,
        options: Option<SearchOptions>,
    ) -> Result<Vec<SearchHit>> {
        let o = options.unwrap_or_default();
        let opts = text::SearchOpts {
            regex: o.regex.unwrap_or(false),
            case_sensitive: o.case_sensitive.unwrap_or(false),
            whole_word: o.whole_word.unwrap_or(false),
            ambient_chars: o.ambient_chars.unwrap_or(20) as usize,
        };
        let t = self.page_text(page_index as usize)?;
        let hits = text::search(&t, &pattern, &opts).map_err(to_napi)?;
        Ok(hits
            .into_iter()
            .map(|h| SearchHit {
                text: h.text,
                ambient: h.ambient,
                quads: h.quads.iter().map(|q| q.to_vec()).collect(),
            })
            .collect())
    }
}

fn rect4(v: &[f64]) -> Result<[f64; 4]> {
    if v.len() != 4 {
        return Err(Error::from_reason(
            "rectangle must have 4 numbers [x1, y1, x2, y2]",
        ));
    }
    Ok([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

// ====================================================================== phase 2

#[napi(object)]
pub struct FontInfo {
    pub index: u32,
    pub name: String,
    pub ascender: f64,
    pub descender: f64,
}

#[napi(object)]
pub struct ImageInfo {
    pub obj: ObjRef,
    pub width: u32,
    pub height: u32,
}

#[napi(object)]
pub struct FieldOption {
    pub export_value: String,
    pub display: String,
}

#[napi(object)]
pub struct FieldInfo {
    pub obj: ObjRef,
    pub name: String,
    /// text | check | radio | button | combo | list | signature | unknown
    pub kind: String,
    pub flags: i64,
    pub value: String,
    pub widgets: Vec<ObjRef>,
    pub options: Vec<FieldOption>,
    /// On-state names of each widget (buttons only).
    pub on_states: Vec<Vec<String>>,
}

#[napi(object)]
#[derive(Default)]
pub struct WidgetOptions {
    pub on_state: Option<String>,
    pub border_color: Option<Vec<f64>>,
    pub background_color: Option<Vec<f64>>,
    pub border_width: Option<f64>,
    pub caption: Option<String>,
}

#[napi(object)]
#[derive(Default)]
pub struct StampOptions {
    pub size_type: Option<u32>,
    pub a: Option<f64>,
    pub b: Option<f64>,
    pub h_align: Option<i32>,
    pub v_align: Option<i32>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub percentage: Option<bool>,
    pub rotation: Option<f64>,
    pub opacity: Option<f64>,
    pub background: Option<bool>,
    pub as_annotation: Option<bool>,
    pub text_align: Option<i32>,
    pub font: Option<u32>,
    pub color: Option<Vec<f64>>,
    pub underline: Option<bool>,
}

impl StampOptions {
    fn params(&self) -> stamper::StampParams {
        stamper::StampParams {
            size_type: self.size_type.unwrap_or(1),
            a: self.a.unwrap_or(0.5),
            b: self.b.unwrap_or(0.5),
            h_align: self.h_align.unwrap_or(0),
            v_align: self.v_align.unwrap_or(0),
            x: self.x.unwrap_or(0.0),
            y: self.y.unwrap_or(0.0),
            percentage: self.percentage.unwrap_or(false),
            rotation: self.rotation.unwrap_or(0.0),
            opacity: self.opacity.unwrap_or(1.0),
            background: self.background.unwrap_or(false),
            as_annotation: self.as_annotation.unwrap_or(false),
            text_align: self.text_align.unwrap_or(0),
            font: self.font.map(|f| f as usize),
            color: self.color.clone().unwrap_or_default(),
            underline: self.underline.unwrap_or(false),
        }
    }
}

fn indices(pages: &[u32]) -> Vec<usize> {
    pages.iter().map(|p| *p as usize).collect()
}

#[napi]
impl NativeDoc {
    // ------------------------------------------------------------ SDF (low-level objects)

    /// The object as JSON (see sdf.rs for the encoding).
    #[napi]
    pub fn obj_get(&self, obj: ObjRef) -> Result<serde_json::Value> {
        let o = self
            .doc
            .inner
            .get_object(obj.id())
            .map_err(|e| to_napi(format!("object {}: {e}", obj.num)))?;
        Ok(sdf::to_json(o))
    }

    #[napi]
    pub fn obj_set(&mut self, obj: ObjRef, key: String, value: serde_json::Value) -> Result<()> {
        let v = sdf::from_json(&value).map_err(to_napi)?;
        let dict = match self.doc.inner.get_object_mut(obj.id()) {
            Ok(lopdf::Object::Dictionary(d)) => d,
            Ok(lopdf::Object::Stream(s)) => &mut s.dict,
            _ => return Err(to_napi(format!("object {} is not a dictionary", obj.num))),
        };
        if v == lopdf::Object::Null {
            dict.remove(key.as_bytes());
        } else {
            dict.set(key.into_bytes(), v);
        }
        self.doc.touch();
        Ok(())
    }

    #[napi]
    pub fn obj_create(&mut self, value: serde_json::Value) -> Result<ObjRef> {
        let v = sdf::from_json(&value).map_err(to_napi)?;
        self.doc.touch();
        Ok(self.doc.inner.add_object(v).into())
    }

    #[napi]
    pub fn catalog_ref(&self) -> Result<ObjRef> {
        self.doc
            .inner
            .trailer
            .get(b"Root")
            .and_then(lopdf::Object::as_reference)
            .map(ObjRef::from)
            .map_err(|e| to_napi(format!("no catalog: {e}")))
    }

    // ------------------------------------------------------------ fonts & images

    /// Standard Type1 font (PDFNet Font.StandardType1Font code); returns a font index.
    #[napi]
    pub fn font_standard(&mut self, code: u32) -> Result<FontInfo> {
        let f = fonts::StdFont::from_code(code).map_err(to_napi)?;
        let i = self.doc.std_font(f);
        Ok(self.font_info(i as u32))
    }

    /// Embeds a TrueType/OpenType font; returns a font index.
    #[napi]
    pub fn font_truetype(&mut self, data: Buffer) -> Result<FontInfo> {
        let i = self.doc.truetype_font(data.to_vec()).map_err(to_napi)?;
        Ok(self.font_info(i as u32))
    }

    #[napi]
    pub fn font_info(&self, index: u32) -> FontInfo {
        let f = &self.doc.fonts[index as usize];
        let name = self
            .doc
            .inner
            .get_dictionary(f.obj)
            .ok()
            .and_then(|d| d.get(b"BaseFont").ok())
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        FontInfo {
            index,
            name,
            ascender: f.ascender(),
            descender: f.descender(),
        }
    }

    /// Width of `text` in 1/1000 em.
    #[napi]
    pub fn font_width(&self, index: u32, text: String) -> f64 {
        self.doc.fonts[index as usize].width(&text)
    }

    #[napi]
    pub fn image_embed(&mut self, data: Buffer) -> Result<ImageInfo> {
        let img = image::embed(&mut self.doc.inner, &data).map_err(to_napi)?;
        self.doc.touch();
        Ok(ImageInfo {
            obj: img.obj.into(),
            width: img.width,
            height: img.height,
        })
    }

    // ------------------------------------------------------------ annotations

    #[napi]
    pub fn page_annots(&self, page: ObjRef) -> Vec<ObjRef> {
        annots::page_annots(&self.doc.inner, page.id())
            .into_iter()
            .map(ObjRef::from)
            .collect()
    }

    #[napi]
    pub fn annot_create(&mut self, subtype: String, rect: Vec<f64>) -> Result<ObjRef> {
        Ok(annots::create(&mut self.doc, &subtype, rect4(&rect)?).into())
    }

    #[napi]
    pub fn annot_add(&mut self, page: ObjRef, annot: ObjRef, index: Option<u32>) -> Result<()> {
        annots::add_to_page(
            &mut self.doc,
            page.id(),
            annot.id(),
            index.map(|i| i as usize),
        )
        .map_err(to_napi)
    }

    #[napi]
    pub fn annot_remove(&mut self, page: ObjRef, annot: ObjRef) -> Result<bool> {
        annots::remove_from_page(&mut self.doc, page.id(), annot.id()).map_err(to_napi)
    }

    /// Regenerates the appearance of a markup annotation or of a widget's field.
    #[napi]
    pub fn annot_refresh(&mut self, annot: ObjRef) -> Result<bool> {
        if let Some(f) = forms::fields(&self.doc.inner)
            .into_iter()
            .find(|f| f.widgets.contains(&annot.id()))
        {
            forms::regenerate(&mut self.doc, &f).map_err(to_napi)?;
            return Ok(true);
        }
        appearance::generate(&mut self.doc, annot.id()).map_err(to_napi)
    }

    /// Uses an embedded image (fitted to /Rect) as the appearance of an annotation.
    #[napi]
    pub fn annot_image_appearance(&mut self, annot: ObjRef, image: ObjRef) -> Result<()> {
        let rect = self
            .doc
            .inner
            .get_dictionary(annot.id())
            .ok()
            .and_then(|d| d.get(b"Rect").ok())
            .and_then(|o| doc::rect_of(&self.doc.inner, o))
            .ok_or_else(|| to_napi("annotation has no /Rect".into()))?;
        let mut res = content::Resources::default();
        let name = res.xobject(image.id());
        let mut ops = content::Ops::new();
        ops.q()
            .cm([
                rect[2] - rect[0],
                0.0,
                0.0,
                rect[3] - rect[1],
                rect[0],
                rect[1],
            ])
            .do_xobject(&name)
            .Q();
        appearance::set_appearance(&mut self.doc, annot.id(), rect, res, ops.into_bytes())
            .map_err(to_napi)
    }

    #[napi]
    pub fn annot_flatten(&mut self, page: ObjRef, annot: ObjRef) -> Result<bool> {
        let target = annot.id();
        let n =
            annots::flatten_page(&mut self.doc, page.id(), &|_, a| a == target).map_err(to_napi)?;
        Ok(n > 0)
    }

    /// Flattens annotations of all pages (only widgets when `formsOnly`).
    #[napi]
    pub fn flatten_annotations(&mut self, forms_only: bool) -> Result<u32> {
        annots::flatten_document(&mut self.doc, forms_only)
            .map(|n| n as u32)
            .map_err(to_napi)
    }

    // ------------------------------------------------------------ forms

    #[napi]
    pub fn fields(&self) -> Vec<FieldInfo> {
        forms::fields(&self.doc.inner)
            .iter()
            .map(|f| self.field_info(f))
            .collect()
    }

    fn field_info(&self, f: &forms::Field) -> FieldInfo {
        let d = &self.doc.inner;
        FieldInfo {
            obj: f.id.into(),
            name: f.name.clone(),
            kind: f.kind().into(),
            flags: f.flags,
            value: forms::value_string(d, f),
            widgets: f.widgets.iter().map(|w| ObjRef::from(*w)).collect(),
            options: forms::options(d, f)
                .into_iter()
                .map(|(e, s)| FieldOption {
                    export_value: e,
                    display: s,
                })
                .collect(),
            on_states: f.widgets.iter().map(|w| forms::on_states(d, *w)).collect(),
        }
    }

    fn field(&self, obj: ObjRef) -> Result<forms::Field> {
        forms::fields(&self.doc.inner)
            .into_iter()
            .find(|f| f.id == obj.id())
            .ok_or_else(|| to_napi(format!("object {} is not a form field", obj.num)))
    }

    #[napi]
    pub fn field_set_value(&mut self, field: ObjRef, value: String) -> Result<FieldInfo> {
        let f = self.field(field)?;
        forms::set_value(&mut self.doc, &f, &value).map_err(to_napi)?;
        let f = self.field(field)?;
        Ok(self.field_info(&f))
    }

    #[napi]
    pub fn field_refresh(&mut self, field: ObjRef) -> Result<()> {
        let f = self.field(field)?;
        forms::regenerate(&mut self.doc, &f).map_err(to_napi)
    }

    #[napi]
    pub fn refresh_field_appearances(&mut self) -> Result<u32> {
        forms::refresh_all(&mut self.doc)
            .map(|n| n as u32)
            .map_err(to_napi)
    }

    /// ftype: Tx | Btn | Ch | Sig.
    #[napi]
    pub fn field_create(&mut self, name: String, ftype: String, flags: i64) -> Result<ObjRef> {
        forms::create_field(&mut self.doc, &name, &ftype, flags)
            .map(ObjRef::from)
            .map_err(to_napi)
    }

    #[napi]
    pub fn widget_create(
        &mut self,
        field: ObjRef,
        rect: Vec<f64>,
        options: Option<WidgetOptions>,
    ) -> Result<ObjRef> {
        let o = options.unwrap_or_default();
        let opts = forms::WidgetOpts {
            on_state: o.on_state,
            border: o.border_color.unwrap_or_default(),
            background: o.background_color.unwrap_or_default(),
            border_width: o.border_width.unwrap_or(1.0),
            caption: o.caption,
        };
        forms::create_widget(&mut self.doc, field.id(), rect4(&rect)?, opts)
            .map(ObjRef::from)
            .map_err(to_napi)
    }

    // ------------------------------------------------------------ stamper

    #[napi]
    pub fn stamp_text(
        &mut self,
        text: String,
        pages: Vec<u32>,
        options: StampOptions,
    ) -> Result<()> {
        stamper::stamp(
            &mut self.doc,
            stamper::Source::Text(text),
            &indices(&pages),
            &options.params(),
        )
        .map_err(to_napi)
    }

    #[napi]
    pub fn stamp_image(
        &mut self,
        image: ImageInfo,
        pages: Vec<u32>,
        options: StampOptions,
    ) -> Result<()> {
        let src = stamper::Source::Image {
            obj: image.obj.id(),
            width: image.width,
            height: image.height,
        };
        stamper::stamp(&mut self.doc, src, &indices(&pages), &options.params()).map_err(to_napi)
    }

    /// Stamps a page of this document.
    #[napi]
    pub fn stamp_page(
        &mut self,
        page: ObjRef,
        pages: Vec<u32>,
        options: StampOptions,
    ) -> Result<()> {
        let (obj, bbox) = stamper::page_to_form(&mut self.doc, None, page.id()).map_err(to_napi)?;
        stamper::stamp(
            &mut self.doc,
            stamper::Source::Form { obj, bbox },
            &indices(&pages),
            &options.params(),
        )
        .map_err(to_napi)
    }

    /// Stamps a page of another document.
    #[napi]
    pub fn stamp_page_from(
        &mut self,
        src: &NativeDoc,
        page: ObjRef,
        pages: Vec<u32>,
        options: StampOptions,
    ) -> Result<()> {
        let (obj, bbox) = stamper::page_to_form(&mut self.doc, Some(&src.doc.inner), page.id())
            .map_err(to_napi)?;
        stamper::stamp(
            &mut self.doc,
            stamper::Source::Form { obj, bbox },
            &indices(&pages),
            &options.params(),
        )
        .map_err(to_napi)
    }

    #[napi]
    pub fn delete_stamps(&mut self, pages: Vec<u32>) -> Result<u32> {
        stamper::delete_stamps(&mut self.doc, &indices(&pages))
            .map(|n| n as u32)
            .map_err(to_napi)
    }

    #[napi]
    pub fn has_stamps(&self, pages: Vec<u32>) -> Result<bool> {
        stamper::has_stamps(&self.doc, &indices(&pages)).map_err(to_napi)
    }
}

// ====================================================================== phase 3

#[napi(object)]
pub struct SecurityInfo {
    pub encrypted: bool,
    /// 0 none, 1 RC4-40, 2 RC4-128, 3 AES-128, 4 AES-256
    pub algorithm: u32,
    pub key_length: u32,
    /// /P permission bits (as a float: JS numbers cannot hold all 64 bits).
    pub permissions: f64,
    pub revision: i64,
}

#[napi(object)]
pub struct EncryptionOptions {
    pub algorithm: u32,
    pub user_password: String,
    pub owner_password: String,
    /// /P permission bits (ISO 32000-1 table 22).
    pub permissions: f64,
    pub encrypt_metadata: Option<bool>,
}

#[napi(object)]
#[derive(Default)]
pub struct SignOptions {
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact_info: Option<String>,
    /// adbe.pkcs7.detached (default) or ETSI.CAdES.detached
    pub subfilter: Option<String>,
}

#[napi(object)]
#[derive(Default)]
pub struct VerifyOptions {
    pub trusted_certificates: Option<Vec<Buffer>>,
    pub check_trust: Option<bool>,
}

#[napi(object)]
pub struct SignatureReport {
    pub field: Option<ObjRef>,
    pub field_name: String,
    pub signed: bool,
    pub subfilter: String,
    pub signer_name: String,
    pub signing_time: String,
    pub reason: String,
    pub location: String,
    pub contact_info: String,
    pub document_status: u32,
    pub digest_status: u32,
    pub trust_status: u32,
    pub permissions_status: u32,
    pub digest_algorithm: String,
    pub verified: bool,
    pub message: String,
    pub certificates: Vec<Buffer>,
    pub byte_range: Vec<i64>,
}

impl From<verify::Report> for SignatureReport {
    fn from(r: verify::Report) -> Self {
        SignatureReport {
            field: r.field.map(ObjRef::from),
            field_name: r.field_name,
            signed: r.signed,
            subfilter: r.subfilter,
            signer_name: r.signer_name,
            signing_time: r.signing_time,
            reason: r.reason,
            location: r.location,
            contact_info: r.contact_info,
            document_status: r.document_status,
            digest_status: r.digest_status,
            trust_status: r.trust_status,
            permissions_status: r.permissions_status,
            digest_algorithm: r.digest_algorithm,
            verified: r.verified,
            message: r.message,
            certificates: r.certificates.into_iter().map(Buffer::from).collect(),
            byte_range: r.byte_range,
        }
    }
}

impl VerifyOptions {
    fn to_native(&self) -> verify::Options {
        verify::Options {
            trusted: self
                .trusted_certificates
                .as_ref()
                .map(|v| v.iter().map(|b| b.to_vec()).collect())
                .unwrap_or_default(),
            check_trust: self.check_trust.unwrap_or(true),
        }
    }
}

#[napi]
impl NativeDoc {
    #[napi]
    pub fn security_info(&self) -> SecurityInfo {
        let i = security::info(&self.doc.inner);
        SecurityInfo {
            encrypted: i.encrypted,
            algorithm: i.algorithm,
            key_length: i.key_length,
            permissions: i.permissions as u32 as f64,
            revision: i.revision,
        }
    }

    /// Encrypts the document on the next (full) save.
    #[napi]
    pub fn set_encryption(&mut self, o: EncryptionOptions) -> Result<()> {
        if !(1..=4).contains(&o.algorithm) {
            return Err(to_napi(format!(
                "unsupported encryption algorithm {}",
                o.algorithm
            )));
        }
        self.doc.security = security::Security::Set(security::SecurityParams {
            algorithm: o.algorithm,
            user_password: o.user_password,
            owner_password: o.owner_password,
            permissions: o.permissions as i64 as u64,
            encrypt_metadata: o.encrypt_metadata.unwrap_or(true),
        });
        self.doc.touch();
        Ok(())
    }

    /// Saves without encryption from now on.
    #[napi]
    pub fn remove_security(&mut self) {
        self.doc.security = security::Security::Remove;
        self.doc.touch();
    }

    /// Schedules a signature of `field` (a signature field) for the next save.
    #[napi]
    pub fn sign_on_next_save(
        &mut self,
        field: ObjRef,
        pkcs12: Buffer,
        password: String,
        options: Option<SignOptions>,
    ) -> Result<String> {
        let f = self.field(field)?;
        if f.kind() != "signature" {
            return Err(to_napi(format!("'{}' is not a signature field", f.name)));
        }
        let key = sign::load_p12(&pkcs12, &password).map_err(to_napi)?;
        let name = key.common_name.clone();
        let o = options.unwrap_or_default();
        self.doc.pending_signature = Some(sign::Pending {
            field: f.id,
            key,
            reason: o.reason,
            location: o.location,
            contact_info: o.contact_info,
            subfilter: o.subfilter.unwrap_or_else(|| "adbe.pkcs7.detached".into()),
        });
        Ok(name)
    }

    #[napi]
    pub fn has_pending_signature(&self) -> bool {
        self.doc.pending_signature.is_some()
    }

    /// Verifies every signature field against the last loaded/saved bytes.
    #[napi]
    pub fn verify_signatures(
        &self,
        options: Option<VerifyOptions>,
    ) -> Result<Vec<SignatureReport>> {
        let Some((bytes, _)) = &self.doc.original else {
            return Ok(vec![]);
        };
        let opts = options.unwrap_or_default().to_native();
        Ok(verify::verify_document(&self.doc.inner, bytes, &opts)
            .into_iter()
            .map(SignatureReport::from)
            .collect())
    }
}

#[napi(object)]
pub struct RedactRegion {
    /// 0-based page index
    pub page: u32,
    /// [x1, y1, x2, y2] in PDF user space
    pub rect: Vec<f64>,
}

#[napi(object)]
#[derive(Default)]
pub struct RedactOverlay {
    pub enabled: Option<bool>,
    pub color: Option<Vec<f64>>,
    pub border: Option<bool>,
    pub border_color: Option<Vec<f64>>,
    pub text: Option<String>,
    pub text_color: Option<Vec<f64>>,
    pub min_font_size: Option<f64>,
    pub max_font_size: Option<f64>,
    pub text_align: Option<i32>,
}

#[napi(object)]
pub struct RedactStats {
    pub glyphs: u32,
    pub images: u32,
    pub paths: u32,
    pub annotations: u32,
}

#[napi]
impl NativeDoc {
    /// Removes the content under the regions and draws the overlay.
    #[napi]
    pub fn redact(
        &mut self,
        regions: Vec<RedactRegion>,
        overlay: Option<RedactOverlay>,
    ) -> Result<RedactStats> {
        let o = overlay.unwrap_or_default();
        let ov = redact::Overlay {
            enabled: o.enabled.unwrap_or(true),
            color: o.color.unwrap_or_else(|| vec![0.0]),
            border: o.border.unwrap_or(false),
            border_color: o.border_color.unwrap_or_default(),
            text: o.text,
            text_color: o.text_color.unwrap_or_else(|| vec![1.0]),
            min_font_size: o.min_font_size.unwrap_or(2.0),
            max_font_size: o.max_font_size.unwrap_or(24.0),
            text_align: o.text_align.unwrap_or(0),
        };
        let mut by_page: std::collections::BTreeMap<u32, Vec<[f64; 4]>> = Default::default();
        for r in regions {
            by_page.entry(r.page).or_default().push(rect4(&r.rect)?);
        }
        let mut total = RedactStats {
            glyphs: 0,
            images: 0,
            paths: 0,
            annotations: 0,
        };
        for (index, rects) in by_page {
            let page = self.doc.page_id(index as usize).map_err(to_napi)?;
            let s = redact::redact_page(&mut self.doc, page, &rects, &ov).map_err(to_napi)?;
            total.glyphs += s.glyphs as u32;
            total.images += s.images as u32;
            total.paths += s.paths as u32;
            total.annotations += s.annotations as u32;
        }
        Ok(total)
    }

    /// Applies every /Redact annotation of the document.
    #[napi]
    pub fn apply_redact_annotations(&mut self) -> Result<RedactStats> {
        let mut total = RedactStats {
            glyphs: 0,
            images: 0,
            paths: 0,
            annotations: 0,
        };
        for page in self.doc.page_ids() {
            let s = redact::apply_redact_annotations(&mut self.doc, page).map_err(to_napi)?;
            total.glyphs += s.glyphs as u32;
            total.images += s.images as u32;
            total.paths += s.paths as u32;
            total.annotations += s.annotations as u32;
        }
        Ok(total)
    }
}

// ====================================================================== phase 4

#[napi]
impl NativeDoc {
    /// Appends a page showing the image at `dpi` (page size = pixels × 72 / dpi).
    #[napi]
    pub fn add_image_page(&mut self, data: Buffer, dpi: Option<f64>) -> Result<ObjRef> {
        let img = image::embed(&mut self.doc.inner, &data).map_err(to_napi)?;
        let scale = 72.0 / dpi.filter(|d| *d > 0.0).unwrap_or(96.0);
        let (w, h) = (img.width as f64 * scale, img.height as f64 * scale);
        let page = self.doc.new_page([0.0, 0.0, w, h]);
        let count = self.doc.page_count();
        self.doc.insert_page(page, count).map_err(to_napi)?;
        let name =
            content::page_add_xobject(&mut self.doc.inner, page, img.obj).map_err(to_napi)?;
        let mut ops = content::Ops::new();
        ops.q().cm([w, 0.0, 0.0, h, 0.0, 0.0]).do_xobject(&name).Q();
        content::page_add_content(&mut self.doc.inner, page, ops.into_bytes(), true, None)
            .map_err(to_napi)?;
        self.doc.touch();
        Ok(page.into())
    }
}

// ====================================================================== DocGen

#[napi(object)]
pub struct DocGenResult {
    pub docx: Buffer,
    pub warnings: Vec<String>,
}

/// Validates a .docx template and returns the JSON schema of the keys it expects.
#[napi]
pub fn docgen_template_keys(template: Buffer) -> Result<serde_json::Value> {
    docgen::template_keys(&template).map_err(to_napi)
}

/// Fills a .docx template with JSON data (image values carry "__image_b64").
#[napi]
pub fn docgen_fill(template: Buffer, data: serde_json::Value) -> Result<DocGenResult> {
    let r = docgen::fill(&template, &data).map_err(to_napi)?;
    Ok(DocGenResult {
        docx: r.docx.into(),
        warnings: r.warnings,
    })
}

// ====================================================================== XFDF & Optimizer

#[napi(object)]
pub struct XfdfMergeResult {
    pub added: u32,
    pub replaced: u32,
    pub deleted: u32,
    pub fields: u32,
    /// Element names that were not imported (unsupported annotation types).
    pub skipped: Vec<String>,
}

/// Throws when the string is not well-formed XFDF.
#[napi]
pub fn xfdf_validate(xfdf: String) -> Result<()> {
    xfdf::validate(&xfdf).map_err(to_napi)
}

#[napi(object)]
#[derive(Default, Clone)]
pub struct OptimizerImageOptions {
    /// 0 retain, 1 flate, 2 jpeg, 3 jpeg2000 (written as jpeg), 4 none
    pub compression_mode: Option<u32>,
    /// 1-10
    pub quality: Option<u32>,
    pub max_dpi: Option<f64>,
    pub resample_dpi: Option<f64>,
    pub downsample: Option<bool>,
    pub force_recompression: Option<bool>,
    pub force_changes: Option<bool>,
}

impl OptimizerImageOptions {
    fn to_native(&self) -> optimize::ImageSettings {
        let d = optimize::ImageSettings::default();
        optimize::ImageSettings {
            mode: self.compression_mode.unwrap_or(d.mode),
            quality: self.quality.unwrap_or(d.quality),
            max_dpi: self.max_dpi.unwrap_or(d.max_dpi),
            resample_dpi: self.resample_dpi.unwrap_or(d.resample_dpi),
            downsample: self.downsample.unwrap_or(d.downsample),
            force_recompression: self.force_recompression.unwrap_or(d.force_recompression),
            force_changes: self.force_changes.unwrap_or(d.force_changes),
        }
    }
}

#[napi(object)]
pub struct OptimizeResult {
    pub images: u32,
    pub optimized: u32,
    pub bytes_before: f64,
    pub bytes_after: f64,
}

#[napi]
impl NativeDoc {
    /// Imports annotations, field values and commands from an XFDF string.
    #[napi]
    pub fn merge_xfdf(&mut self, xfdf: String) -> Result<XfdfMergeResult> {
        let s = xfdf::merge(&mut self.doc, &xfdf).map_err(to_napi)?;
        Ok(XfdfMergeResult {
            added: s.added as u32,
            replaced: s.replaced as u32,
            deleted: s.deleted as u32,
            fields: s.fields as u32,
            skipped: s.skipped,
        })
    }

    /// Downsamples / recompresses images (PDFNet.Optimizer.optimize).
    #[napi]
    pub fn optimize_images(
        &mut self,
        color: Option<OptimizerImageOptions>,
        gray: Option<OptimizerImageOptions>,
    ) -> Result<OptimizeResult> {
        let c = color.unwrap_or_default().to_native();
        let g = gray.unwrap_or_default().to_native();
        let s = optimize::optimize(&mut self.doc, c, g).map_err(to_napi)?;
        Ok(OptimizeResult {
            images: s.images as u32,
            optimized: s.optimized as u32,
            bytes_before: s.bytes_before as f64,
            bytes_after: s.bytes_after as f64,
        })
    }
}

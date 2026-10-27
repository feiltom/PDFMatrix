//! Document model on top of lopdf: page tree, page boxes, cross-document page import,
//! document info and serialization.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId, SaveOptions, Stream};

use crate::fonts::{DocFont, FontKind, StdFont};
use crate::security::{self, Security};
use crate::strings::{encode_text, object_to_text};

pub type Res<T> = Result<T, String>;

pub fn err<E: std::fmt::Display>(context: &str) -> impl FnOnce(E) -> String + '_ {
    move |e| format!("{context}: {e}")
}

/// Page attributes that may be inherited from ancestor /Pages nodes (ISO 32000-1 §7.7.3.4).
pub(crate) const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// US Letter, the default MediaBox used by most processors when none is present.
const DEFAULT_MEDIA_BOX: [f64; 4] = [0.0, 0.0, 612.0, 792.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxKind {
    Media,
    Crop,
    Bleed,
    Trim,
    Art,
}

impl BoxKind {
    pub fn from_code(code: u32) -> Res<Self> {
        // Same numbering as PDFNet's Page.Box enum.
        Ok(match code {
            0 => BoxKind::Media,
            1 => BoxKind::Crop,
            2 => BoxKind::Bleed,
            3 => BoxKind::Trim,
            4 => BoxKind::Art,
            _ => return Err(format!("unknown page box {code}")),
        })
    }

    fn key(self) -> &'static [u8] {
        match self {
            BoxKind::Media => b"MediaBox",
            BoxKind::Crop => b"CropBox",
            BoxKind::Bleed => b"BleedBox",
            BoxKind::Trim => b"TrimBox",
            BoxKind::Art => b"ArtBox",
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct SaveOpts {
    pub remove_unused: bool,
    pub compress: bool,
    pub object_streams: bool,
    /// Append changes to the original bytes (required to keep existing signatures valid).
    pub incremental: bool,
    /// Linearized output ("fast web view", ISO 32000-1 Annex F).
    pub linearize: bool,
}

pub struct Doc {
    pub inner: Document,
    /// Fonts registered for writing text in this session (see `fonts`).
    pub fonts: Vec<DocFont>,
    revision: u64,
    snapshot: Option<(u64, Arc<Vec<u8>>)>,
    /// Bytes of the last loaded or saved revision and the password that opened them; the base
    /// of incremental saves and of signature verification.
    pub original: Option<(Arc<Vec<u8>>, Option<String>)>,
    /// Encryption to apply when saving.
    pub security: Security,
    /// Signature to apply on the next save.
    pub pending_signature: Option<crate::sign::Pending>,
}

impl Doc {
    pub fn open(bytes: &[u8], password: Option<&str>) -> Res<Doc> {
        let options = match password {
            Some(p) => LoadOptions::with_password(p),
            None => LoadOptions::default(),
        };
        let inner =
            Document::load_mem_with_options(bytes, options).map_err(err("cannot open PDF"))?;
        if inner.is_encrypted() {
            // lopdf leaves /Encrypt in place when the password did not authenticate.
            return Err(
                "cannot open PDF: document is encrypted and the password is missing or wrong"
                    .into(),
            );
        }
        Ok(Doc {
            inner,
            fonts: Vec::new(),
            revision: 0,
            snapshot: None,
            original: Some((Arc::new(bytes.to_vec()), password.map(str::to_string))),
            security: Security::Keep,
            pending_signature: None,
        })
    }

    pub fn create() -> Doc {
        let mut inner = Document::with_version("1.7");
        let pages_id = inner.add_object(Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Pages".to_vec())),
            ("Kids", Object::Array(vec![])),
            ("Count", Object::Integer(0)),
        ]));
        let catalog_id = inner.add_object(Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", Object::Reference(pages_id)),
        ]));
        inner.trailer.set("Root", Object::Reference(catalog_id));
        Doc {
            inner,
            fonts: Vec::new(),
            revision: 0,
            snapshot: None,
            original: None,
            security: Security::Keep,
            pending_signature: None,
        }
    }

    /// Must be called by every mutating operation so that cached snapshots are invalidated.
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn was_encrypted(&self) -> bool {
        self.inner.was_encrypted()
    }

    // ---------------------------------------------------------------- pages

    pub fn page_ids(&self) -> Vec<ObjectId> {
        self.inner.page_iter().collect()
    }

    pub fn page_count(&self) -> usize {
        self.inner.page_iter().count()
    }

    pub fn page_id(&self, index: usize) -> Res<ObjectId> {
        self.inner
            .page_iter()
            .nth(index)
            .ok_or_else(|| format!("page index {index} out of range"))
    }

    pub fn page_index(&self, id: ObjectId) -> Option<usize> {
        self.inner.page_iter().position(|p| p == id)
    }

    fn pages_root(&self) -> Res<ObjectId> {
        self.inner
            .catalog()
            .and_then(|c| c.get(b"Pages"))
            .and_then(Object::as_reference)
            .map_err(err("document has no page tree"))
    }

    /// Value of `key` on the page or the nearest ancestor that defines it (references are kept).
    pub fn inherited(&self, page: ObjectId, key: &[u8]) -> Option<Object> {
        inherited_in(&self.inner, page, key)
    }

    pub fn page_box(&self, page: ObjectId, kind: BoxKind) -> [f64; 4] {
        let read = |k: BoxKind| {
            self.inherited(page, k.key())
                .and_then(|o| rect_of(&self.inner, &o))
        };
        let media = read(BoxKind::Media).unwrap_or(DEFAULT_MEDIA_BOX);
        match kind {
            BoxKind::Media => media,
            BoxKind::Crop => read(BoxKind::Crop).unwrap_or(media),
            other => read(other).or_else(|| read(BoxKind::Crop)).unwrap_or(media),
        }
    }

    pub fn set_page_box(&mut self, page: ObjectId, kind: BoxKind, r: [f64; 4]) -> Res<()> {
        let arr = Object::Array(r.iter().map(|&v| real(v)).collect());
        self.page_dict_mut(page)?.set(kind.key(), arr);
        self.touch();
        Ok(())
    }

    /// Normalized rotation in degrees: 0, 90, 180 or 270.
    pub fn rotation(&self, page: ObjectId) -> i64 {
        let raw = self
            .inherited(page, b"Rotate")
            .and_then(|o| deref(&self.inner, &o).as_i64().ok())
            .unwrap_or(0);
        ((raw % 360) + 360) % 360 / 90 * 90
    }

    pub fn set_rotation(&mut self, page: ObjectId, degrees: i64) -> Res<()> {
        let normalized = degrees.rem_euclid(360);
        if normalized % 90 != 0 {
            return Err(format!("rotation must be a multiple of 90, got {degrees}"));
        }
        self.page_dict_mut(page)?
            .set("Rotate", Object::Integer(normalized));
        self.touch();
        Ok(())
    }

    fn page_dict_mut(&mut self, page: ObjectId) -> Res<&mut Dictionary> {
        self.inner
            .get_dictionary_mut(page)
            .map_err(err("invalid page object"))
    }

    /// Rewrites the page tree as a single /Pages node whose /Kids are all the pages, after copying
    /// inherited attributes down onto every page. Makes insert/remove/move trivial.
    pub fn flatten_tree(&mut self) -> Res<ObjectId> {
        let root = self.pages_root()?;
        let pages = self.page_ids();

        for &pid in &pages {
            let missing: Vec<(&[u8], Object)> = INHERITABLE
                .iter()
                .filter(|k| !self.inner.get_dictionary(pid).is_ok_and(|d| d.has(k)))
                .filter_map(|k| self.inherited(pid, k).map(|v| (*k, v)))
                .collect();
            let dict = self.page_dict_mut(pid)?;
            for (k, v) in missing {
                dict.set(k, v);
            }
        }

        let mut intermediates = Vec::new();
        let mut stack = vec![root];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            let Ok(dict) = self.inner.get_dictionary(node) else {
                continue;
            };
            if !is_type(dict, b"Pages") {
                continue;
            }
            if node != root {
                intermediates.push(node);
            }
            if let Ok(kids) = dict.get(b"Kids").and_then(Object::as_array) {
                stack.extend(kids.iter().filter_map(|k| k.as_reference().ok()));
            }
        }
        for id in intermediates {
            self.inner.objects.remove(&id);
        }

        let kids: Vec<Object> = pages.iter().map(|&p| Object::Reference(p)).collect();
        let count = kids.len() as i64;
        let root_dict = self
            .inner
            .get_dictionary_mut(root)
            .map_err(err("invalid page tree"))?;
        root_dict.set("Kids", Object::Array(kids));
        root_dict.set("Count", Object::Integer(count));
        root_dict.remove(b"Parent");
        for &pid in &pages {
            self.page_dict_mut(pid)?
                .set("Parent", Object::Reference(root));
        }
        Ok(root)
    }

    fn with_kids<T>(&mut self, f: impl FnOnce(&mut Vec<Object>) -> Res<T>) -> Res<T> {
        let root = self.flatten_tree()?;
        let root_dict = self
            .inner
            .get_dictionary_mut(root)
            .map_err(err("invalid page tree"))?;
        let mut kids = root_dict
            .get(b"Kids")
            .and_then(Object::as_array)
            .cloned()
            .unwrap_or_default();
        let out = f(&mut kids)?;
        root_dict.set("Count", Object::Integer(kids.len() as i64));
        root_dict.set("Kids", Object::Array(kids));
        self.touch();
        Ok(out)
    }

    /// Creates a detached page (not yet part of the page tree).
    pub fn new_page(&mut self, media_box: [f64; 4]) -> ObjectId {
        let id = self.inner.add_object(Dictionary::from_iter(vec![
            ("Type", Object::Name(b"Page".to_vec())),
            (
                "MediaBox",
                Object::Array(media_box.iter().map(|&v| real(v)).collect()),
            ),
            ("Resources", Object::Dictionary(Dictionary::new())),
        ]));
        self.touch();
        id
    }

    /// Inserts an existing page object of this document at `index` (clamped to the page count).
    pub fn insert_page(&mut self, page: ObjectId, index: usize) -> Res<()> {
        if self.page_index(page).is_some() {
            return Err("page is already part of the document; use movePage".into());
        }
        self.with_kids(|kids| {
            let at = index.min(kids.len());
            kids.insert(at, Object::Reference(page));
            Ok(())
        })?;
        let root = self.pages_root()?;
        self.page_dict_mut(page)?
            .set("Parent", Object::Reference(root));
        Ok(())
    }

    pub fn remove_page(&mut self, index: usize) -> Res<()> {
        self.with_kids(|kids| {
            if index >= kids.len() {
                return Err(format!("page index {index} out of range"));
            }
            kids.remove(index);
            Ok(())
        })
    }

    pub fn move_page(&mut self, from: usize, to: usize) -> Res<()> {
        self.with_kids(|kids| {
            if from >= kids.len() {
                return Err(format!("page index {from} out of range"));
            }
            let page = kids.remove(from);
            let at = to.min(kids.len());
            kids.insert(at, page);
            Ok(())
        })
    }

    /// Deep-copies pages (and everything they reference) from `src` into this document and
    /// inserts them at `at`. Objects shared between the copied pages (fonts, images…) are copied
    /// once. References to source pages that are not imported (e.g. link targets) become null.
    pub fn import_pages(
        &mut self,
        src: &Document,
        src_pages: &[ObjectId],
        at: usize,
    ) -> Res<Vec<ObjectId>> {
        let imported: HashSet<ObjectId> = src_pages.iter().copied().collect();
        let foreign: HashSet<ObjectId> =
            src.page_iter().filter(|p| !imported.contains(p)).collect();
        let mut map: HashMap<ObjectId, ObjectId> = HashMap::new();

        // Reserve ids first so that cross references between imported pages resolve.
        for &sp in src_pages {
            map.entry(sp).or_insert_with(|| self.inner.new_object_id());
        }

        let mut new_pages = Vec::with_capacity(src_pages.len());
        let mut done = HashSet::new();
        for &sp in src_pages {
            let new_id = map[&sp];
            new_pages.push(new_id);
            if !done.insert(sp) {
                continue;
            }
            let mut dict = src
                .get_dictionary(sp)
                .map_err(err("invalid source page"))?
                .clone();
            for key in INHERITABLE {
                if !dict.has(key)
                    && let Some(v) = inherited_in(src, sp, key)
                {
                    dict.set(key, v);
                }
            }
            for key in [&b"Parent"[..], b"StructParents", b"B"] {
                dict.remove(key);
            }
            let copied = self.graft(src, Object::Dictionary(dict), &mut map, &foreign);
            self.inner.objects.insert(new_id, copied);
        }

        // Duplicated source pages need distinct page objects in the destination.
        let mut seen = HashSet::new();
        for id in new_pages.iter_mut() {
            if !seen.insert(*id) {
                let clone = self.inner.get_object(*id).map_err(err("import"))?.clone();
                *id = self.inner.add_object(clone);
            }
        }

        self.with_kids(|kids| {
            let at = at.min(kids.len());
            for (i, &p) in new_pages.iter().enumerate() {
                kids.insert(at + i, Object::Reference(p));
            }
            Ok(())
        })?;
        let root = self.pages_root()?;
        for &p in &new_pages {
            self.page_dict_mut(p)?
                .set("Parent", Object::Reference(root));
        }
        Ok(new_pages)
    }

    pub(crate) fn graft(
        &mut self,
        src: &Document,
        obj: Object,
        map: &mut HashMap<ObjectId, ObjectId>,
        foreign: &HashSet<ObjectId>,
    ) -> Object {
        match obj {
            Object::Reference(id) => {
                if let Some(&mapped) = map.get(&id) {
                    return Object::Reference(mapped);
                }
                if foreign.contains(&id) {
                    return Object::Null;
                }
                let new_id = self.inner.new_object_id();
                map.insert(id, new_id);
                let target = src.get_object(id).cloned().unwrap_or(Object::Null);
                let copied = self.graft(src, target, map, foreign);
                self.inner.objects.insert(new_id, copied);
                Object::Reference(new_id)
            }
            Object::Array(items) => Object::Array(
                items
                    .into_iter()
                    .map(|o| self.graft(src, o, map, foreign))
                    .collect(),
            ),
            Object::Dictionary(dict) => {
                Object::Dictionary(self.graft_dict(src, dict, map, foreign))
            }
            Object::Stream(stream) => {
                let dict = self.graft_dict(src, stream.dict, map, foreign);
                let mut copy = Stream::new(dict, stream.content);
                copy.allows_compression = stream.allows_compression;
                copy.start_position = None;
                Object::Stream(copy)
            }
            other => other,
        }
    }

    fn graft_dict(
        &mut self,
        src: &Document,
        dict: Dictionary,
        map: &mut HashMap<ObjectId, ObjectId>,
        foreign: &HashSet<ObjectId>,
    ) -> Dictionary {
        let mut out = Dictionary::new();
        for (k, v) in dict.into_iter() {
            let v = self.graft(src, v, map, foreign);
            out.set(k, v);
        }
        out
    }

    // ---------------------------------------------------------------- document info

    fn info_id(&mut self, create: bool) -> Option<ObjectId> {
        match self.inner.trailer.get(b"Info") {
            Ok(Object::Reference(id)) => return Some(*id),
            Ok(Object::Dictionary(d)) => {
                let d = d.clone();
                let id = self.inner.add_object(d);
                self.inner.trailer.set("Info", Object::Reference(id));
                return Some(id);
            }
            _ => {}
        }
        if !create {
            return None;
        }
        let id = self.inner.add_object(Dictionary::new());
        self.inner.trailer.set("Info", Object::Reference(id));
        Some(id)
    }

    pub fn get_info(&self, key: &str) -> Option<String> {
        let info = match self.inner.trailer.get(b"Info").ok()? {
            Object::Reference(id) => self.inner.get_dictionary(*id).ok()?,
            Object::Dictionary(d) => d,
            _ => return None,
        };
        let value = info.get(key.as_bytes()).ok()?;
        object_to_text(deref(&self.inner, value))
    }

    pub fn set_info(&mut self, key: &str, value: Option<&str>) -> Res<()> {
        let id = self.info_id(value.is_some());
        if let Some(id) = id {
            let dict = self
                .inner
                .get_dictionary_mut(id)
                .map_err(err("invalid /Info"))?;
            match value {
                Some(v) => dict.set(key.as_bytes(), encode_text(v)),
                None => {
                    dict.remove(key.as_bytes());
                }
            }
            self.touch();
        }
        Ok(())
    }

    pub fn info_keys(&self) -> Vec<String> {
        let info = match self.inner.trailer.get(b"Info") {
            Ok(Object::Reference(id)) => self.inner.get_dictionary(*id).ok(),
            Ok(Object::Dictionary(d)) => Some(d),
            _ => None,
        };
        info.map(|d| {
            d.iter()
                .map(|(k, _)| String::from_utf8_lossy(k).into_owned())
                .collect()
        })
        .unwrap_or_default()
    }

    // ---------------------------------------------------------------- serialization

    /// Serializes the document: full or incremental, with the requested security, then applies
    /// the pending signature if any. The result becomes the new base for incremental saves.
    pub fn save(&mut self, opts: SaveOpts) -> Res<Vec<u8>> {
        self.finalize_fonts();
        let pending = self.pending_signature.take();
        // Checked before `prepare`, which adds the new signature.
        let signed_before = pending.is_some() && crate::sign::has_signatures(&self.inner);
        if let Some(p) = &pending
            && let Err(e) = crate::sign::prepare(self, p)
        {
            self.pending_signature = pending;
            return Err(e);
        }
        // Signing a document that already has signatures must not invalidate them.
        let incremental = opts.incremental || (signed_before && self.original.is_some());
        let result = if incremental {
            self.save_incremental()
        } else {
            self.save_full(opts)
        };
        let bytes = match (result, pending) {
            (Ok(b), Some(p)) => crate::sign::sign(b, &p)?,
            (Ok(b), None) => b,
            (Err(e), p) => {
                self.pending_signature = p;
                return Err(e);
            }
        };
        let password = self.original.as_ref().and_then(|(_, p)| p.clone());
        self.original = Some((Arc::new(bytes.clone()), password));
        Ok(bytes)
    }

    fn save_full(&mut self, opts: SaveOpts) -> Res<Vec<u8>> {
        if opts.remove_unused {
            self.inner.prune_objects();
            let inner = &self.inner;
            self.fonts.retain(|f| inner.objects.contains_key(&f.obj));
        }
        if opts.compress {
            self.inner.compress();
        }
        let options = SaveOptions::builder()
            .use_object_streams(opts.object_streams)
            .use_xref_streams(opts.object_streams)
            .build();
        let state = match &self.security {
            Security::Keep => self.inner.encryption_state.clone(),
            Security::Remove => None,
            Security::Set(p) => {
                security::ensure_id(&mut self.inner)?;
                Some(security::state_for(&self.inner, p)?)
            }
        };
        if opts.linearize
            && let Some(out) = crate::linearize::linearize(&self.inner, state.as_ref())?
        {
            if let (Security::Set(_), Some(state)) = (&self.security, state) {
                self.inner.encryption_state = Some(state);
                self.security = Security::Keep;
            }
            return Ok(out);
        }
        let mut out = Vec::new();
        match state {
            None => {
                // Never write a stale state: the in-memory objects are plaintext.
                let saved = self.inner.encryption_state.take();
                let r = self.inner.save_with_options(&mut out, options);
                self.inner.encryption_state = saved;
                r.map_err(err("cannot save PDF"))?;
            }
            Some(state) => {
                let mut copy = self.inner.clone();
                copy.encryption_state = None;
                copy.encrypt(&state).map_err(err("cannot encrypt PDF"))?;
                if let Ok(id) = copy.trailer.get(b"Encrypt").and_then(Object::as_reference) {
                    let dict = security::encrypt_dict(&state)?;
                    copy.objects.insert(id, Object::Dictionary(dict));
                }
                copy.save_with_options(&mut out, options)
                    .map_err(err("cannot save PDF"))?;
                if let Security::Set(_) = self.security {
                    // Later saves keep the new security.
                    self.inner.encryption_state = Some(state);
                    self.security = Security::Keep;
                }
            }
        }
        Ok(out)
    }

    /// Appends the objects that changed since the last loaded/saved revision (ISO 32000-1 §7.5.6).
    fn save_incremental(&mut self) -> Res<Vec<u8>> {
        let Some((orig, password)) = self.original.clone() else {
            return Err("incremental save needs a document loaded from a file or buffer".into());
        };
        let changes_security = match self.security {
            Security::Keep => false,
            Security::Remove => self.inner.was_encrypted(),
            Security::Set(_) => true,
        };
        if changes_security {
            return Err(
                "security cannot be changed in an incremental save; save without e_incremental"
                    .into(),
            );
        }
        let options = match &password {
            Some(p) => LoadOptions::with_password(p),
            None => LoadOptions::default(),
        };
        let prev = Document::load_mem_with_options(&orig, options)
            .map_err(err("cannot reload previous revision"))?;
        let mut inc = lopdf::IncrementalDocument::create_from(orig.as_ref().clone(), prev);
        let prev = inc.get_prev_documents().clone();
        for (id, obj) in &self.inner.objects {
            if prev.objects.get(id) != Some(obj) {
                inc.new_document.set_object(*id, obj.clone());
            }
        }
        for key in [&b"Root"[..], b"Info", b"ID"] {
            if let Ok(v) = self.inner.trailer.get(key) {
                inc.new_document.trailer.set(key, v.clone());
            }
        }
        inc.new_document.max_id = inc.new_document.max_id.max(self.inner.max_id);
        let mut out = Vec::new();
        inc.save_to(&mut out)
            .map_err(err("cannot save incrementally"))?;
        Ok(out)
    }

    /// Serialized, unencrypted copy of the current state, cached until the next mutation.
    /// Used by the renderer and the text extractor, which read PDF bytes.
    pub fn snapshot(&mut self) -> Res<Arc<Vec<u8>>> {
        if let Some((rev, bytes)) = &self.snapshot
            && *rev == self.revision
        {
            return Ok(bytes.clone());
        }
        self.finalize_fonts();
        let mut out = Vec::new();
        self.inner
            .save_to(&mut out)
            .map_err(err("cannot serialize PDF"))?;
        let bytes = Arc::new(out);
        self.snapshot = Some((self.revision, bytes.clone()));
        Ok(bytes)
    }
}

// -------------------------------------------------------------------- helpers

pub fn deref<'a>(doc: &'a Document, obj: &'a Object) -> &'a Object {
    doc.dereference(obj).map(|(_, o)| o).unwrap_or(obj)
}

fn is_type(dict: &Dictionary, name: &[u8]) -> bool {
    dict.get(b"Type")
        .and_then(Object::as_name)
        .is_ok_and(|n| n == name)
}

pub fn inherited_in(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page;
    for _ in 0..64 {
        let dict = doc.get_dictionary(current).ok()?;
        if let Ok(v) = dict.get(key) {
            return Some(v.clone());
        }
        current = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

pub fn number(doc: &Document, obj: &Object) -> Option<f64> {
    match deref(doc, obj) {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

/// Reads a PDF rectangle and normalizes it to [llx lly urx ury].
pub fn rect_of(doc: &Document, obj: &Object) -> Option<[f64; 4]> {
    let arr = deref(doc, obj).as_array().ok()?;
    if arr.len() != 4 {
        return None;
    }
    let v: Vec<f64> = arr.iter().map(|o| number(doc, o)).collect::<Option<_>>()?;
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

pub fn real(v: f64) -> Object {
    if v.fract() == 0.0 && v.abs() < i64::MAX as f64 {
        Object::Integer(v as i64)
    } else {
        Object::Real(v as f32)
    }
}

// -------------------------------------------------------------------- fonts

impl Doc {
    /// Index of a standard font usable for writing (created once per document).
    pub fn std_font(&mut self, f: StdFont) -> usize {
        if let Some(i) = self
            .fonts
            .iter()
            .position(|d| matches!(d.kind, FontKind::Std(s) if s == f))
        {
            return i;
        }
        self.fonts.push(DocFont::std(&mut self.inner, f));
        self.touch();
        self.fonts.len() - 1
    }

    /// Embeds a TrueType/OpenType font (full Unicode support through Identity-H).
    pub fn truetype_font(&mut self, data: Vec<u8>) -> Res<usize> {
        let f = DocFont::embed_truetype(&mut self.inner, data)?;
        self.fonts.push(f);
        self.touch();
        Ok(self.fonts.len() - 1)
    }

    /// Index of a font dictionary that already exists in the document.
    pub fn existing_font(&mut self, obj: ObjectId) -> usize {
        if let Some(i) = self.fonts.iter().position(|d| d.obj == obj) {
            return i;
        }
        self.fonts.push(DocFont::existing(&self.inner, obj));
        self.fonts.len() - 1
    }

    /// Writes the glyph-dependent parts (/W, /ToUnicode) of embedded fonts.
    pub fn finalize_fonts(&mut self) {
        for f in &self.fonts {
            f.finalize(&mut self.inner);
        }
    }

    /// Deep-copies an object graph of another document into this one.
    pub fn graft_from(&mut self, src: &Document, obj: Object) -> Object {
        let foreign: HashSet<ObjectId> = src.page_iter().collect();
        self.graft(src, obj, &mut HashMap::new(), &foreign)
    }
}

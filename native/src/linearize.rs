//! Linearized ("fast web view") output, ISO 32000-1 Annex F.
//!
//! The objects are split into the parts of Annex F.3 with the same rules as qpdf, so that
//! `qpdf --check-linearization` agrees with the hint tables:
//!
//! - part 4: catalog, objects of the document-level catalog keys and the encryption dictionary;
//! - part 6: first page object, then the other objects used by the first page;
//! - part 7: each other page followed by its private objects;
//! - part 8: objects shared by several pages other than the first one;
//! - part 9: page tree, outlines, document information and everything else.
//!
//! The file is written as: header, linearization dictionary, first-page cross-reference section
//! and trailer, part 4, primary hint stream, parts 6 to 9, main cross-reference section.
//! Objects are renumbered so that every part is a run of consecutive numbers in file order:
//! parts 7–9 get 1..k, the first-page section gets the numbers above.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;

use lopdf::encryption::{self, EncryptionState};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};

use crate::doc::{inherited_in, Res, INHERITABLE};

/// Catalog keys whose objects are needed to open the document (qpdf's list).
const OPEN_DOCUMENT_KEYS: [&[u8]; 5] =
    [b"ViewerPreferences", b"PageMode", b"Threads", b"OpenAction", b"AcroForm"];

/// Width of the numbers that are only known once the layout is done.
const PAD: usize = 10;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum User {
    Page(usize),
    Thumb(usize),
    RootKey(Vec<u8>),
    TrailerKey(Vec<u8>),
    Root,
}

#[derive(Default)]
struct Usage {
    user_objects: HashMap<User, BTreeSet<ObjectId>>,
    object_users: HashMap<ObjectId, BTreeSet<User>>,
}

impl Usage {
    fn record(&mut self, user: &User, id: ObjectId) {
        self.user_objects.entry(user.clone()).or_default().insert(id);
        self.object_users.entry(id).or_default().insert(user.clone());
    }

    /// Records every indirect object reachable from `start` as used by `user`. Other pages are
    /// not followed (they have their own user), nor is a page's /Parent; a page's /Thumb belongs
    /// to a thumbnail user.
    fn traverse(&mut self, doc: &Document, user: User, start: &Object) {
        let mut visited = HashSet::new();
        let mut pending = vec![(user, start, true)];
        while let Some((user, obj, top)) = pending.pop() {
            let obj = match obj {
                Object::Reference(id) => {
                    if !visited.insert(*id) {
                        continue;
                    }
                    let Ok(target) = doc.get_object(*id) else {
                        continue;
                    };
                    if !top && is_page(target) {
                        continue;
                    }
                    self.record(&user, *id);
                    target
                }
                direct => direct,
            };
            let dict = match obj {
                Object::Array(items) => {
                    pending.extend(items.iter().rev().map(|o| (user.clone(), o, false)));
                    continue;
                }
                Object::Dictionary(d) => d,
                Object::Stream(s) => &s.dict,
                _ => continue,
            };
            let page = is_page(obj);
            for (key, value) in dict.iter().rev() {
                match (page, key.as_slice(), &user) {
                    (true, b"Parent", _) => {}
                    (true, b"Thumb", User::Page(n)) => pending.push((User::Thumb(*n), value, false)),
                    _ => pending.push((user.clone(), value, false)),
                }
            }
        }
    }
}

fn is_page(obj: &Object) -> bool {
    matches!(obj, Object::Dictionary(d) if d.get(b"Type").and_then(Object::as_name).is_ok_and(|n| n == b"Page"))
}

/// Object categories of Annex F.3 (qpdf's `lc_*` sets).
#[derive(Default)]
struct Classes {
    root: Option<ObjectId>,
    open_document: BTreeSet<ObjectId>,
    outlines: BTreeSet<ObjectId>,
    first_page_private: BTreeSet<ObjectId>,
    first_page_shared: BTreeSet<ObjectId>,
    other_page_private: BTreeSet<ObjectId>,
    other_page_shared: BTreeSet<ObjectId>,
    thumbnail: BTreeSet<ObjectId>,
    other: BTreeSet<ObjectId>,
}

fn classify(usage: &Usage) -> Classes {
    let mut c = Classes::default();
    let mut ids: Vec<_> = usage.object_users.keys().copied().collect();
    ids.sort();
    for id in ids {
        let (mut open, mut outlines, mut root, mut first) = (false, false, false, false);
        let (mut other_pages, mut thumbs, mut others) = (0, 0, 0);
        for user in &usage.object_users[&id] {
            match user {
                User::TrailerKey(k) if k == b"Encrypt" => open = true,
                User::TrailerKey(_) => others += 1,
                User::Thumb(_) => thumbs += 1,
                User::RootKey(k) if OPEN_DOCUMENT_KEYS.contains(&k.as_slice()) => open = true,
                User::RootKey(k) if k == b"Outlines" => outlines = true,
                User::RootKey(_) => others += 1,
                User::Page(0) => first = true,
                User::Page(_) => other_pages += 1,
                User::Root => root = true,
            }
        }
        let set = if root {
            c.root = Some(id);
            continue;
        } else if outlines {
            &mut c.outlines
        } else if open {
            &mut c.open_document
        } else if first && others == 0 && other_pages == 0 && thumbs == 0 {
            &mut c.first_page_private
        } else if first {
            &mut c.first_page_shared
        } else if other_pages == 1 && others == 0 && thumbs == 0 {
            &mut c.other_page_private
        } else if other_pages > 1 {
            &mut c.other_page_shared
        } else if thumbs > 0 && others == 0 {
            &mut c.thumbnail
        } else {
            &mut c.other
        };
        set.insert(id);
    }
    c
}

/// A serialized indirect object ("n 0 obj … endobj\n").
struct Chunk {
    bytes: Vec<u8>,
}

/// MSB-first bit writer for the hint tables.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, value: u64, bits: u32) {
        for i in (0..bits).rev() {
            self.acc = (self.acc << 1) | ((value >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                self.out.push(self.acc as u8);
                self.acc = 0;
                self.n = 0;
            }
        }
    }

    fn align(&mut self) {
        if self.n > 0 {
            self.put(0, 8 - self.n);
        }
    }

    fn vector(&mut self, values: impl IntoIterator<Item = u64>, bits: u32) {
        for v in values {
            self.put(v, bits);
        }
        self.align();
    }
}

fn nbits(v: u64) -> u32 {
    64 - v.leading_zeros()
}

/// Writes `doc` linearized, encrypted with `state` when given. Returns `None` when the
/// document cannot be linearized (no page, or a page object used twice in the page tree);
/// the caller then saves it normally.
pub fn linearize(src: &Document, state: Option<&EncryptionState>) -> Res<Option<Vec<u8>>> {
    let mut doc = src.clone();
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    if pages.is_empty() || pages.iter().collect::<HashSet<_>>().len() != pages.len() {
        return Ok(None);
    }
    let root_id = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| "cannot linearize: the trailer has no /Root reference".to_string())?;
    prepare(&mut doc, &pages);

    // Who uses what.
    let mut usage = Usage::default();
    for (n, page) in pages.iter().enumerate() {
        usage.traverse(&doc, User::Page(n), &Object::Reference(*page));
    }
    for (key, value) in doc.trailer.iter() {
        if key != b"Root" {
            usage.traverse(&doc, User::TrailerKey(key.clone()), value);
        }
    }
    let catalog = doc.get_dictionary(root_id).map_err(|e| format!("cannot linearize: {e}"))?;
    for (key, value) in catalog.iter() {
        usage.traverse(&doc, User::RootKey(key.clone()), value);
    }
    usage.record(&User::Root, root_id);
    let mut c = classify(&usage);

    // Parts.
    let mut part4 = vec![root_id];
    part4.extend(&c.open_document);
    if !c.first_page_private.remove(&pages[0]) {
        return Ok(None);
    }
    let mut part6 = vec![pages[0]];
    part6.extend(&c.first_page_private);
    part6.extend(&c.first_page_shared);
    let outlines_root = catalog.get(b"Outlines").and_then(Object::as_reference).ok();
    let outlines_first = outlines_root.is_some()
        && catalog
            .get(b"PageMode")
            .and_then(Object::as_name)
            .is_ok_and(|m| m == b"UseOutlines");
    let mut outline_part = Vec::new();
    if let Some(o) = outlines_root
        && c.outlines.remove(&o)
    {
        outline_part.push(o);
        outline_part.extend(std::mem::take(&mut c.outlines));
    }
    if outlines_first {
        part6.extend(&outline_part);
    }
    let mut part7 = Vec::new();
    let mut page_objects = vec![part6.len()];
    for (n, page) in pages.iter().enumerate().skip(1) {
        if !c.other_page_private.remove(page) {
            return Ok(None);
        }
        part7.push(*page);
        let mut count = 1;
        for id in &usage.user_objects[&User::Page(n)] {
            if c.other_page_private.remove(id) {
                part7.push(*id);
                count += 1;
            }
        }
        page_objects.push(count);
    }
    let part8: Vec<ObjectId> = c.other_page_shared.iter().copied().collect();
    let mut part9 = Vec::new();
    if let Some(tree) = usage.user_objects.get(&User::RootKey(b"Pages".to_vec())) {
        for id in tree {
            if c.other.remove(id) {
                part9.push(*id);
            }
        }
    }
    part9.extend(&c.thumbnail);
    if !outlines_first {
        part9.extend(&outline_part);
    }
    part9.extend(&c.other);
    part9.extend(c.other_page_private.iter().chain(&c.outlines));

    // Numbering: parts 7–9 from 1, then the linearization dictionary, part 4, the encryption
    // dictionary, the hint stream and part 6.
    let mut renumber: HashMap<ObjectId, u32> = HashMap::new();
    let mut next = 1u32;
    for id in part7.iter().chain(&part8).chain(&part9) {
        renumber.insert(*id, next);
        next += 1;
    }
    let main_size = next;
    let lin_num = next;
    next += 1;
    for id in &part4 {
        renumber.insert(*id, next);
        next += 1;
    }
    let encrypt_num = state.map(|_| {
        next += 1;
        next - 1
    });
    let hint_num = next;
    next += 1;
    for id in &part6 {
        renumber.insert(*id, next);
        next += 1;
    }
    let size = next;

    let chunk = |id: &ObjectId| -> Res<Chunk> {
        let mut obj = remap(doc.get_object(*id).map_err(|e| format!("cannot linearize: {e}"))?, &renumber);
        let num = renumber[id];
        if let Some(state) = state {
            encryption::encrypt_object(state, (num, 0), &mut obj)
                .map_err(|e| format!("cannot encrypt PDF: {e}"))?;
        }
        Ok(serialize(num, &obj))
    };
    let chunks = |ids: &[ObjectId]| ids.iter().map(chunk).collect::<Res<Vec<_>>>();
    let mut c4 = chunks(&part4)?;
    if let (Some(state), Some(num)) = (state, encrypt_num) {
        let dict = crate::security::encrypt_dict(state)?;
        c4.push(serialize(num, &Object::Dictionary(dict)));
    }
    let c6 = chunks(&part6)?;
    let c7 = chunks(&part7)?;
    let c8 = chunks(&part8)?;
    let c9 = chunks(&part9)?;

    // Fixed-size head: header, linearization dictionary, first-page xref and trailer.
    let header = format!("%PDF-{}\n%\u{e2}\u{e3}\u{cf}\u{d3}\n", doc.version);
    let header = header
        .chars()
        .map(|ch| ch as u32 as u8)
        .collect::<Vec<u8>>();
    let lin_len = lin_dict(lin_num, 0, [0; 2], 0, 0, 0, 0).len();
    let first_count = size - lin_num;
    let xref1_len = format!("xref\n{lin_num} {first_count}\n").len() + 20 * first_count as usize;
    let mut trailer_dict = Dictionary::new();
    trailer_dict.set("Size", Object::Integer(size as i64));
    trailer_dict.set("Root", Object::Reference((renumber[&root_id], 0)));
    for key in [&b"Info"[..], b"ID"] {
        if let Ok(v) = doc.trailer.get(key) {
            trailer_dict.set(key, remap(v, &renumber));
        }
    }
    if let Some(num) = encrypt_num {
        trailer_dict.set("Encrypt", Object::Reference((num, 0)));
    }
    let trailer1 = |prev: usize| -> Vec<u8> {
        let mut t = b"trailer\n".to_vec();
        let mut body = Vec::new();
        write_object(&mut body, &Object::Dictionary(trailer_dict.clone()));
        t.extend_from_slice(&body[..body.len() - 2]);
        t.extend_from_slice(format!("/Prev {prev:<PAD$}>>\nstartxref\n0\n%%EOF\n").as_bytes());
        t
    };
    let trailer1_len = trailer1(0).len();

    let len = |c: &[Chunk]| c.iter().map(|x| x.bytes.len()).sum::<usize>();
    let off_lin = header.len();
    let off_xref1 = off_lin + lin_len;
    let off_part4 = off_xref1 + xref1_len + trailer1_len;
    let off_hint = off_part4 + len(&c4);
    // Offsets in the hint tables ignore the hint stream (Annex F.4).
    let off_part6 = off_hint;
    let off_part7 = off_part6 + len(&c6);
    let off_part8 = off_part7 + len(&c7);

    // Page offset hint table (F.4.1).
    let mut page_lengths = vec![len(&c6) as u64];
    let mut at = 0;
    for &n in &page_objects[1..] {
        page_lengths.push(len(&c7[at..at + n]) as u64);
        at += n;
    }
    let shared_index: HashMap<ObjectId, u64> = part6
        .iter()
        .chain(&part8)
        .enumerate()
        .map(|(i, id)| (*id, i as u64))
        .collect();
    let mut shared_refs: Vec<Vec<u64>> = vec![Vec::new()];
    for n in 1..pages.len() {
        shared_refs.push(
            usage.user_objects[&User::Page(n)]
                .iter()
                .filter(|id| usage.object_users[id].len() > 1)
                .filter_map(|id| shared_index.get(id).copied())
                .collect(),
        );
    }
    let counts: Vec<u64> = page_objects.iter().map(|&n| n as u64).collect();
    let (min_objects, max_objects) = min_max(&counts);
    let (min_length, max_length) = min_max(&page_lengths);
    let nshared_total = (part6.len() + part8.len()) as u64;
    let bits_objects = nbits(max_objects - min_objects);
    let bits_length = nbits(max_length - min_length);
    let bits_nshared = nbits(shared_refs.iter().map(|r| r.len() as u64).max().unwrap_or(0));
    let bits_shared_id = nbits(nshared_total);
    let mut h = Bits::default();
    h.put(min_objects, 32);
    h.put(off_part6 as u64, 32);
    h.put(bits_objects as u64, 16);
    h.put(min_length, 32);
    h.put(bits_length as u64, 16);
    // Content stream offsets and lengths are written the way Acrobat does: offset 0 and the
    // page length (implementation note 127 of the PDF Reference).
    h.put(0, 32);
    h.put(0, 16);
    h.put(min_length, 32);
    h.put(bits_length as u64, 16);
    h.put(bits_nshared as u64, 16);
    h.put(bits_shared_id as u64, 16);
    h.put(0, 16);
    h.put(4, 16);
    h.vector(counts.iter().map(|n| n - min_objects), bits_objects);
    h.vector(page_lengths.iter().map(|n| n - min_length), bits_length);
    h.vector(shared_refs.iter().map(|r| r.len() as u64), bits_nshared);
    h.vector(shared_refs.iter().flatten().copied(), bits_shared_id);
    h.vector(std::iter::empty(), 0);
    h.vector(page_lengths.iter().map(|_| 0), 0);
    h.vector(page_lengths.iter().map(|n| n - min_length), bits_length);

    // Shared object hint table (F.4.2): one group per object.
    let shared_offset = h.out.len();
    let group_lengths: Vec<u64> = c6.iter().chain(&c8).map(|x| x.bytes.len() as u64).collect();
    let (min_group, max_group) = min_max(&group_lengths);
    let bits_group = nbits(max_group - min_group);
    let first_shared = part8.first().map_or((0, 0), |id| (renumber[id] as u64, off_part8 as u64));
    h.put(first_shared.0, 32);
    h.put(first_shared.1, 32);
    h.put(part6.len() as u64, 32);
    h.put(nshared_total, 32);
    h.put(0, 16);
    h.put(min_group, 32);
    h.put(bits_group as u64, 16);
    h.vector(group_lengths.iter().map(|n| n - min_group), bits_group);
    h.vector(group_lengths.iter().map(|_| 0), 1);
    h.vector(group_lengths.iter().map(|_| 0), 0);

    // Outline hint table (F.4.6).
    let mut hint_dict = Dictionary::new();
    hint_dict.set("S", Object::Integer(shared_offset as i64));
    if let Some(first) = outline_part.first() {
        let num = renumber[first];
        let (offset, length) = if outlines_first {
            let start = part6.len() - outline_part.len();
            (off_part6 + len(&c6[..start]), len(&c6[start..]))
        } else {
            let start = part9.iter().position(|id| id == first).unwrap_or(0);
            let before = len(&c9[..start]);
            (off_part8 + len(&c8) + before, len(&c9[start..start + outline_part.len()]))
        };
        hint_dict.set("O", Object::Integer(h.out.len() as i64));
        h.put(num as u64, 32);
        h.put(offset as u64, 32);
        h.put(outline_part.len() as u64, 32);
        h.put(length as u64, 32);
    }
    let mut hint = Object::Stream(lopdf::Stream::new(hint_dict, h.out));
    if let Some(state) = state {
        encryption::encrypt_object(state, (hint_num, 0), &mut hint)
            .map_err(|e| format!("cannot encrypt PDF: {e}"))?;
    }
    let hint = serialize(hint_num, &hint);
    let hint_len = hint.bytes.len();

    // Real offsets.
    let mut offsets: HashMap<u32, usize> = HashMap::new();
    let mut cursor = off_part4;
    let mut body = Vec::new();
    let nums = part4.iter().map(|id| renumber[id]).chain(encrypt_num).chain([hint_num]);
    let nums = nums.chain(part6.iter().chain(&part7).chain(&part8).chain(&part9).map(|id| renumber[id]));
    let all = c4.iter().chain([&hint]).chain(&c6).chain(&c7).chain(&c8).chain(&c9);
    for (num, ch) in nums.zip(all) {
        offsets.insert(num, cursor);
        cursor += ch.bytes.len();
        body.extend_from_slice(&ch.bytes);
    }
    let first_page_end = off_hint + hint_len + len(&c6);
    let off_xref0 = cursor;

    let mut tail = format!("xref\n0 {main_size}\n").into_bytes();
    let t = off_xref0 + tail.len() - 1;
    tail.extend_from_slice(b"0000000000 65535 f \n");
    for num in 1..main_size {
        tail.extend_from_slice(format!("{:010} 00000 n \n", offsets[&num]).as_bytes());
    }
    tail.extend_from_slice(
        format!("trailer\n<</Size {main_size}>>\nstartxref\n{off_xref1}\n%%EOF\n").as_bytes(),
    );
    let file_len = off_xref0 + tail.len();

    let mut out = header;
    out.extend_from_slice(&lin_dict(
        lin_num,
        file_len,
        [off_hint, hint_len],
        renumber[&pages[0]],
        first_page_end,
        pages.len(),
        t,
    ));
    out.extend_from_slice(format!("xref\n{lin_num} {first_count}\n").as_bytes());
    out.extend_from_slice(format!("{:010} 00000 n \n", off_lin).as_bytes());
    for num in lin_num + 1..size {
        out.extend_from_slice(format!("{:010} 00000 n \n", offsets[&num]).as_bytes());
    }
    out.extend_from_slice(&trailer1(off_xref0));
    debug_assert_eq!(out.len(), off_part4);
    out.extend_from_slice(&body);
    out.extend_from_slice(&tail);
    debug_assert_eq!(out.len(), file_len);
    Ok(Some(out))
}

/// Pushes inherited page attributes down to the pages (as qpdf does before linearizing, so
/// that resources count as page objects) and makes stream lengths direct.
fn prepare(doc: &mut Document, pages: &[ObjectId]) {
    let mut ancestors = HashSet::new();
    for &page in pages {
        let mut parent = doc
            .get_dictionary(page)
            .ok()
            .and_then(|d| d.get(b"Parent").and_then(Object::as_reference).ok());
        while let Some(p) = parent {
            if !ancestors.insert(p) {
                break;
            }
            parent = doc
                .get_dictionary(p)
                .ok()
                .and_then(|d| d.get(b"Parent").and_then(Object::as_reference).ok());
        }
        for key in INHERITABLE {
            let has = doc.get_dictionary(page).is_ok_and(|d| d.has(key));
            if !has && let Some(value) = inherited_in(doc, page, key) {
                if let Ok(d) = doc.get_dictionary_mut(page) {
                    d.set(key, value);
                }
            }
        }
    }
    for id in ancestors {
        if let Ok(d) = doc.get_dictionary_mut(id) {
            for key in INHERITABLE {
                d.remove(key);
            }
        }
    }
    for obj in doc.objects.values_mut() {
        if let Object::Stream(s) = obj {
            // An indirect /Length would make its object part of the page's objects.
            s.dict.set("Length", Object::Integer(s.content.len() as i64));
        }
    }
}

fn min_max(values: &[u64]) -> (u64, u64) {
    let min = values.iter().copied().min().unwrap_or(0);
    let max = values.iter().copied().max().unwrap_or(0);
    (min, max)
}

#[allow(clippy::too_many_arguments)]
fn lin_dict(num: u32, l: usize, h: [usize; 2], o: u32, e: usize, n: usize, t: usize) -> Vec<u8> {
    format!(
        "{num} 0 obj\n<</Linearized 1/L {l:<PAD$}/H [{:<PAD$} {:<PAD$}]/O {o:<PAD$}/E {e:<PAD$}/N {n:<PAD$}/T {t:<PAD$}>>\nendobj\n",
        h[0], h[1]
    )
    .into_bytes()
}

/// Rewrites references with the new numbers; references to objects that are not written
/// (unreachable, or a page outside the page tree) become null.
fn remap(obj: &Object, renumber: &HashMap<ObjectId, u32>) -> Object {
    match obj {
        Object::Reference(id) => renumber
            .get(id)
            .map_or(Object::Null, |n| Object::Reference((*n, 0))),
        Object::Array(items) => Object::Array(items.iter().map(|o| remap(o, renumber)).collect()),
        Object::Dictionary(d) => Object::Dictionary(remap_dict(d, renumber)),
        Object::Stream(s) => {
            let mut s = s.clone();
            s.dict = remap_dict(&s.dict, renumber);
            Object::Stream(s)
        }
        other => other.clone(),
    }
}

fn remap_dict(d: &Dictionary, renumber: &HashMap<ObjectId, u32>) -> Dictionary {
    let mut out = Dictionary::new();
    for (k, v) in d.iter() {
        out.set(k.clone(), remap(v, renumber));
    }
    out
}

fn serialize(num: u32, obj: &Object) -> Chunk {
    let mut bytes = format!("{num} 0 obj\n").into_bytes();
    write_object(&mut bytes, obj);
    bytes.extend_from_slice(b"\nendobj\n");
    Chunk { bytes }
}

/// PDF syntax for an object, with the same conventions as lopdf's writer (which is private).
fn write_object(out: &mut Vec<u8>, obj: &Object) {
    match obj {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Integer(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Object::Real(r) => {
            let _ = write!(out, "{r}");
        }
        Object::Name(n) => write_name(out, n),
        Object::String(s, StringFormat::Literal) => {
            out.push(b'(');
            let unbalanced: HashSet<usize> = {
                let mut set = HashSet::new();
                let mut stack = Vec::new();
                for (i, &b) in s.iter().enumerate() {
                    match b {
                        b'(' => stack.push(i),
                        b')' if stack.pop().is_none() => {
                            set.insert(i);
                        }
                        _ => {}
                    }
                }
                set.extend(stack);
                set
            };
            for (i, &b) in s.iter().enumerate() {
                match b {
                    b'\\' => out.extend_from_slice(b"\\\\"),
                    b'\r' => out.extend_from_slice(b"\\r"),
                    b'(' | b')' if unbalanced.contains(&i) => {
                        out.push(b'\\');
                        out.push(b);
                    }
                    _ => out.push(b),
                }
            }
            out.push(b')');
        }
        Object::String(s, StringFormat::Hexadecimal) => {
            out.push(b'<');
            for b in s {
                let _ = write!(out, "{b:02X}");
            }
            out.push(b'>');
        }
        Object::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 && needs_separator(item) {
                    out.push(b' ');
                }
                write_object(out, item);
            }
            out.push(b']');
        }
        Object::Dictionary(d) => write_dict(out, d),
        Object::Stream(s) => {
            // Encryption changes the content length.
            let mut dict = s.dict.clone();
            dict.set("Length", Object::Integer(s.content.len() as i64));
            write_dict(out, &dict);
            out.extend_from_slice(b"stream\n");
            out.extend_from_slice(&s.content);
            out.extend_from_slice(b"\nendstream");
        }
        Object::Reference((n, g)) => {
            let _ = write!(out, "{n} {g} R");
        }
    }
}

fn write_dict(out: &mut Vec<u8>, d: &Dictionary) {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        write_name(out, k);
        if needs_separator(v) {
            out.push(b' ');
        }
        write_object(out, v);
    }
    out.extend_from_slice(b">>");
}

fn needs_separator(obj: &Object) -> bool {
    matches!(
        obj,
        Object::Null | Object::Boolean(_) | Object::Integer(_) | Object::Real(_) | Object::Reference(_)
    )
}

fn write_name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &b in name {
        if b" \t\n\r\x0C()<>[]{}/%#".contains(&b) || !(33..=126).contains(&b) {
            let _ = write!(out, "#{b:02X}");
        } else {
            out.push(b);
        }
    }
}

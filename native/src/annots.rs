//! Page annotations: listing, creation, placement, removal and flattening.

use std::time::{SystemTime, UNIX_EPOCH};

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::appearance;
use crate::content::{Ops, concat, page_add_content, page_add_xobject, transform_rect};
use crate::doc::{Doc, Res, deref, err, number, real, rect_of};
use crate::forms;

pub const FLAG_INVISIBLE: i64 = 1;
pub const FLAG_HIDDEN: i64 = 2;

/// Annotation references of a page, in /Annots order.
pub fn page_annots(doc: &Document, page: ObjectId) -> Vec<ObjectId> {
    doc.get_dictionary(page)
        .ok()
        .and_then(|d| d.get(b"Annots").ok())
        .and_then(|o| deref(doc, o).as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default()
}

fn set_page_annots(doc: &mut Document, page: ObjectId, annots: Vec<ObjectId>) -> Res<()> {
    let d = doc.get_dictionary_mut(page).map_err(err("invalid page"))?;
    if annots.is_empty() {
        d.remove(b"Annots");
    } else {
        d.set(
            "Annots",
            Object::Array(annots.into_iter().map(Object::Reference).collect()),
        );
    }
    Ok(())
}

/// PDF date string for "now" (UTC).
pub fn pdf_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Creates a detached annotation of the given subtype.
pub fn create(doc: &mut Doc, subtype: &str, rect: [f64; 4]) -> ObjectId {
    let now = pdf_now();
    let mut d = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Annot".to_vec())),
        ("Subtype", Object::Name(subtype.as_bytes().to_vec())),
        (
            "Rect",
            Object::Array(rect.iter().map(|v| real(*v)).collect()),
        ),
        ("F", Object::Integer(4)),
        ("M", Object::string_literal(now.clone())),
    ]);
    let markup = !matches!(subtype, "Link" | "Popup" | "Widget");
    if markup {
        d.set("CreationDate", Object::string_literal(now));
    }
    let id = doc.inner.add_object(d);
    doc.inner.get_dictionary_mut(id).expect("just added").set(
        "NM",
        Object::string_literal(format!("bw-{}-{}", id.0, id.1)),
    );
    doc.touch();
    id
}

/// Places an annotation on a page at `index` (clamped) and sets its /P.
pub fn add_to_page(
    doc: &mut Doc,
    page: ObjectId,
    annot: ObjectId,
    index: Option<usize>,
) -> Res<()> {
    let mut annots = page_annots(&doc.inner, page);
    if annots.contains(&annot) {
        return Ok(());
    }
    let at = index.unwrap_or(annots.len()).min(annots.len());
    annots.insert(at, annot);
    set_page_annots(&mut doc.inner, page, annots)?;
    doc.inner
        .get_dictionary_mut(annot)
        .map_err(err("invalid annotation"))?
        .set("P", Object::Reference(page));
    // A popup belongs on the same page as its parent.
    if let Some(popup) = doc
        .inner
        .get_dictionary(annot)
        .ok()
        .and_then(|d| d.get(b"Popup").ok())
        .and_then(|o| o.as_reference().ok())
    {
        let mut annots = page_annots(&doc.inner, page);
        if !annots.contains(&popup) {
            annots.push(popup);
            set_page_annots(&mut doc.inner, page, annots)?;
        }
    }
    doc.touch();
    Ok(())
}

pub fn remove_from_page(doc: &mut Doc, page: ObjectId, annot: ObjectId) -> Res<bool> {
    let annots = page_annots(&doc.inner, page);
    let popup = doc
        .inner
        .get_dictionary(annot)
        .ok()
        .and_then(|d| d.get(b"Popup").ok())
        .and_then(|o| o.as_reference().ok());
    let keep: Vec<ObjectId> = annots
        .iter()
        .copied()
        .filter(|a| *a != annot && Some(*a) != popup)
        .collect();
    let removed = keep.len() != annots.len();
    set_page_annots(&mut doc.inner, page, keep)?;
    if removed {
        doc.touch();
    }
    Ok(removed)
}

/// The normal appearance stream to use for an annotation (state-aware), if any.
fn normal_appearance(doc: &Document, annot: ObjectId) -> Option<ObjectId> {
    let d = doc.get_dictionary(annot).ok()?;
    let n = deref(doc, d.get(b"AP").ok()?)
        .as_dict()
        .ok()?
        .get(b"N")
        .ok()?;
    match n {
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Stream(_) => Some(*id),
            Object::Dictionary(states) => state_entry(d, states),
            _ => None,
        },
        Object::Dictionary(states) => state_entry(d, states),
        _ => None,
    }
}

fn state_entry(annot: &Dictionary, states: &Dictionary) -> Option<ObjectId> {
    let state = annot.get(b"AS").ok()?.as_name().ok()?;
    states.get(state).ok()?.as_reference().ok()
}

/// Flattens annotations of a page into its content. `filter` selects which ones.
/// Returns the number of annotations flattened (removed ones without appearance included).
pub fn flatten_page(
    doc: &mut Doc,
    page: ObjectId,
    filter: &dyn Fn(&Document, ObjectId) -> bool,
) -> Res<usize> {
    let annots = page_annots(&doc.inner, page);
    let mut ops = Ops::new();
    let mut flattened = Vec::new();
    for a in annots {
        if !filter(&doc.inner, a) {
            continue;
        }
        let d = doc
            .inner
            .get_dictionary(a)
            .map_err(err("invalid annotation"))?
            .clone();
        let subtype = d
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok())
            .unwrap_or(b"")
            .to_vec();
        let flags = d.get(b"F").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0);
        flattened.push(a);
        if subtype == b"Popup" || flags & (FLAG_HIDDEN | FLAG_INVISIBLE) != 0 {
            continue;
        }
        if normal_appearance(&doc.inner, a).is_none() {
            if subtype == b"Widget" {
                if let Some(f) = forms::fields(&doc.inner)
                    .into_iter()
                    .find(|f| f.widgets.contains(&a))
                {
                    forms::regenerate(doc, &f)?;
                }
            } else {
                appearance::generate(doc, a)?;
            }
        }
        let Some(ap) = normal_appearance(&doc.inner, a) else {
            continue;
        };
        let Some(rect) = d.get(b"Rect").ok().and_then(|o| rect_of(&doc.inner, o)) else {
            continue;
        };
        let (bbox, matrix) = {
            let Ok(Object::Stream(s)) = doc.inner.get_object_mut(ap) else {
                continue;
            };
            s.dict.set("Type", Object::Name(b"XObject".to_vec()));
            s.dict.set("Subtype", Object::Name(b"Form".to_vec()));
            let bbox = s.dict.get(b"BBox").ok().cloned();
            let matrix = s.dict.get(b"Matrix").ok().cloned();
            (bbox, matrix)
        };
        let bbox = bbox.and_then(|b| rect_of(&doc.inner, &b)).unwrap_or([
            0.0,
            0.0,
            rect[2] - rect[0],
            rect[3] - rect[1],
        ]);
        let matrix: [f64; 6] = matrix
            .and_then(|m| {
                let v: Vec<f64> = deref(&doc.inner, &m)
                    .as_array()
                    .ok()?
                    .iter()
                    .filter_map(|o| number(&doc.inner, o))
                    .collect();
                v.try_into().ok()
            })
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        // ISO 32000-1 §12.5.5: map the transformed BBox onto /Rect.
        let t = transform_rect(matrix, bbox);
        let (tw, th) = ((t[2] - t[0]).max(1e-6), (t[3] - t[1]).max(1e-6));
        let (sx, sy) = ((rect[2] - rect[0]) / tw, (rect[3] - rect[1]) / th);
        let a_m = concat(
            [1.0, 0.0, 0.0, 1.0, -t[0], -t[1]],
            [sx, 0.0, 0.0, sy, rect[0], rect[1]],
        );
        let name = page_add_xobject(&mut doc.inner, page, ap)?;
        ops.q().cm(a_m).do_xobject(&name).Q();
    }
    if !ops.buf.is_empty() {
        page_add_content(&mut doc.inner, page, ops.into_bytes(), true, None)?;
    }
    if !flattened.is_empty() {
        let keep: Vec<ObjectId> = page_annots(&doc.inner, page)
            .into_iter()
            .filter(|a| !flattened.contains(a))
            .collect();
        set_page_annots(&mut doc.inner, page, keep)?;
        doc.touch();
    }
    Ok(flattened.len())
}

fn is_widget(doc: &Document, a: ObjectId) -> bool {
    doc.get_dictionary(a)
        .ok()
        .and_then(|d| d.get(b"Subtype").ok())
        .and_then(|o| o.as_name().ok())
        == Some(b"Widget")
}

/// Flattens every page. With `forms_only`, only widgets are flattened. Flattened forms also
/// lose their AcroForm.
pub fn flatten_document(doc: &mut Doc, forms_only: bool) -> Res<usize> {
    let pages = doc.page_ids();
    let mut total = 0;
    let need_appearances = doc
        .inner
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(|o| deref(&doc.inner, o).as_dict().ok())
        .and_then(|a| a.get(b"NeedAppearances").ok())
        .and_then(|o| o.as_bool().ok())
        .unwrap_or(false);
    if need_appearances {
        forms::refresh_all(doc)?;
    }
    for p in pages {
        total += if forms_only {
            flatten_page(doc, p, &is_widget)?
        } else {
            flatten_page(doc, p, &|_, _| true)?
        };
    }
    if let Ok(c) = doc.inner.catalog_mut() {
        c.remove(b"AcroForm");
    }
    doc.touch();
    Ok(total)
}

//! DocGen: fills .docx templates from JSON (PDFNet Convert.createOfficeTemplate…).
//!
//! Template syntax (Apryse DocGen compatible subset):
//!   {{name}} {{client.address.city}} {{this}}         values (dotted paths, scoped in loops)
//!   {{loop items}} … {{endloop}}                        repeat paragraphs, table rows or runs
//!   {{if expr}} … {{elif expr}} … {{else}} … {{endif}} conditions (== != > < >= <= && || ! and or not)
//!   {{logo}} with { "logo": { "image_url": …, "width": pt, "height": pt } }   images
//!
//! A tag may be split by Word over several runs; runs are first normalized so that every tag
//! sits alone in its own run. Loops/conditions whose tags are in different paragraphs (or table
//! rows) repeat/keep whole paragraphs (rows); tags inside one paragraph act on its runs; a loop
//! starting in a table row repeats that row (or the rows up to {{endloop}}).

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use base64::Engine;

use regex::Regex;
use serde_json::{Map, Value};

use crate::doc::Res;
use crate::xml::{El, Node, XmlDoc, parse_xml, write_xml};

// ============================================================================ tags

#[derive(Clone, Debug, PartialEq)]
enum Tag {
    Value(String),
    LoopStart(String),
    LoopEnd,
    IfStart(String),
    Elif(String),
    Else,
    IfEnd,
}

impl Tag {
    fn parse(raw: &str) -> Tag {
        let inner = raw.trim_start_matches("{{").trim_end_matches("}}").trim();
        let (head, rest) = inner
            .split_once(char::is_whitespace)
            .map(|(h, r)| (h, r.trim()))
            .unwrap_or((inner, ""));
        match head {
            "loop" if !rest.is_empty() => Tag::LoopStart(rest.into()),
            "endloop" | "/loop" => Tag::LoopEnd,
            "if" if !rest.is_empty() => Tag::IfStart(rest.into()),
            "elif" | "elseif" if !rest.is_empty() => Tag::Elif(rest.into()),
            "else" if rest.starts_with("if ") => Tag::Elif(rest[3..].trim().into()),
            "else" => Tag::Else,
            "endif" | "/if" => Tag::IfEnd,
            _ => Tag::Value(inner.into()),
        }
    }

    fn depth_delta(&self) -> i32 {
        match self {
            Tag::LoopStart(_) | Tag::IfStart(_) => 1,
            Tag::LoopEnd | Tag::IfEnd => -1,
            _ => 0,
        }
    }

    fn is_control(&self) -> bool {
        !matches!(self, Tag::Value(_))
    }
}

fn tag_re() -> Regex {
    Regex::new(r"\{\{.*?\}\}").unwrap()
}

/// Text of a run when it is a single w:t (after normalization), else None.
fn run_text(r: &El) -> Option<&str> {
    let mut texts = r.children.iter().filter_map(|c| match c {
        Node::El(e) if e.is("w:t") => Some(e),
        _ => None,
    });
    let t = texts.next()?;
    if texts.next().is_some() {
        return None;
    }
    Some(match t.children.first() {
        Some(Node::Text(s)) => s.as_str(),
        _ => "",
    })
}

/// The tag a run holds, when the run is exactly one tag.
fn run_tag(n: &Node) -> Option<Tag> {
    match n {
        Node::El(e) if e.is("w:r") => {
            let t = run_text(e)?;
            (t.starts_with("{{") && t.ends_with("}}") && t.len() >= 4).then(|| Tag::parse(t))
        }
        _ => None,
    }
}

fn make_run(rpr: Option<&El>, text: &str) -> Node {
    let mut children = Vec::new();
    if let Some(p) = rpr {
        children.push(Node::El(p.clone()));
    }
    children.push(Node::El(El::new(
        "w:t",
        r#"xml:space="preserve""#,
        vec![Node::Text(text.into())],
    )));
    Node::El(El::new("w:r", "", children))
}

/// Splits runs with several content elements into one-content runs.
fn split_runs(p: &mut El) {
    let mut out = Vec::with_capacity(p.children.len());
    for c in std::mem::take(&mut p.children) {
        match c {
            Node::El(r) if r.is("w:r") => {
                let rpr = r.child("w:rPr").cloned();
                let contents: Vec<Node> = r
                    .children
                    .iter()
                    .filter(|n| !matches!(n, Node::El(e) if e.is("w:rPr")))
                    .cloned()
                    .collect();
                let real: Vec<&Node> = contents
                    .iter()
                    .filter(|n| matches!(n, Node::El(_)))
                    .collect();
                if real.len() <= 1 {
                    out.push(Node::El(r));
                    continue;
                }
                for n in contents.into_iter().filter(|n| matches!(n, Node::El(_))) {
                    let mut children = Vec::new();
                    if let Some(p) = &rpr {
                        children.push(Node::El(p.clone()));
                    }
                    children.push(n);
                    out.push(Node::El(El {
                        name: r.name.clone(),
                        attrs: r.attrs.clone(),
                        children,
                        empty: false,
                    }));
                }
            }
            Node::El(e) if e.is("w:proofErr") => {} // spell-check markers often split tags
            other => out.push(other),
        }
    }
    p.children = out;
}

/// Makes every tag of a paragraph sit alone in its own run (Word splits tags across runs).
fn normalize_paragraph(p: &mut El) {
    split_runs(p);
    let re = tag_re();
    // Groups of consecutive text runs (bookmarks are transparent).
    let n = p.children.len();
    let mut i = 0;
    while i < n {
        let is_text_run =
            |c: &Node| matches!(c, Node::El(e) if e.is("w:r") && run_text(e).is_some());
        let transparent = |c: &Node| {
            matches!(c, Node::El(e) if e.is("w:bookmarkStart") || e.is("w:bookmarkEnd"))
                || matches!(c, Node::Text(_))
        };
        if !is_text_run(&p.children[i]) {
            i += 1;
            continue;
        }
        let mut group = vec![i];
        let mut j = i + 1;
        while j < n && (is_text_run(&p.children[j]) || transparent(&p.children[j])) {
            if is_text_run(&p.children[j]) {
                group.push(j);
            }
            j += 1;
        }
        // Reassign characters of multi-run tags to the run where the tag starts.
        let texts: Vec<String> = group
            .iter()
            .map(|&k| match &p.children[k] {
                Node::El(e) => run_text(e).unwrap_or("").to_string(),
                _ => String::new(),
            })
            .collect();
        let full: String = texts.concat();
        let mut owner: Vec<usize> = Vec::new(); // byte → group index
        for (g, t) in texts.iter().enumerate() {
            owner.extend(std::iter::repeat_n(g, t.len()));
        }
        let mut dest = owner.clone();
        for m in re.find_iter(&full) {
            let g = owner[m.start()];
            for d in &mut dest[m.start()..m.end()] {
                *d = g;
            }
        }
        let mut new_texts = vec![String::new(); texts.len()];
        let mut pos = 0;
        for ch in full.chars() {
            new_texts[dest[pos]].push(ch);
            pos += ch.len_utf8();
        }
        for (g, &k) in group.iter().enumerate() {
            if let Node::El(r) = &mut p.children[k] {
                set_run_text(r, &new_texts[g]);
            }
        }
        i = j;
    }
    // Isolate each tag in its own run.
    let mut out = Vec::with_capacity(p.children.len());
    for c in std::mem::take(&mut p.children) {
        let Node::El(r) = &c else {
            out.push(c);
            continue;
        };
        let Some(text) = r
            .is("w:r")
            .then(|| run_text(r))
            .flatten()
            .map(str::to_string)
        else {
            out.push(c);
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if !re.is_match(&text) {
            out.push(c);
            continue;
        }
        let rpr = r.child("w:rPr");
        let mut last = 0;
        for m in re.find_iter(&text) {
            if m.start() > last {
                out.push(make_run(rpr, &text[last..m.start()]));
            }
            out.push(make_run(rpr, m.as_str()));
            last = m.end();
        }
        if last < text.len() {
            out.push(make_run(rpr, &text[last..]));
        }
    }
    p.children = out;
}

fn set_run_text(r: &mut El, text: &str) {
    for c in &mut r.children {
        if let Node::El(t) = c
            && t.is("w:t")
        {
            t.children = vec![Node::Text(text.to_string())];
            t.attrs = r#"xml:space="preserve""#.into();
            t.empty = false;
        }
    }
}

fn normalize_tree(e: &mut El) {
    if e.is("w:p") {
        normalize_paragraph(e);
    }
    for c in &mut e.children {
        if let Node::El(child) = c {
            normalize_tree(child);
        }
    }
}

/// Control tags contained in a node, in document order.
fn controls(n: &Node, out: &mut Vec<Tag>) {
    if let Some(t) = run_tag(n) {
        if t.is_control() {
            out.push(t);
        }
        return;
    }
    if let Node::El(e) = n {
        for c in &e.children {
            controls(c, out);
        }
    }
}

fn all_tags(n: &Node, out: &mut Vec<Tag>) {
    if let Some(t) = run_tag(n) {
        out.push(t);
        return;
    }
    if let Node::El(e) = n {
        for c in &e.children {
            all_tags(c, out);
        }
    }
}

/// Removes the first (or last) run holding a control tag inside `n`.
fn remove_control(n: &mut Node, last: bool) -> bool {
    let Node::El(e) = n else { return false };
    let idxs: Vec<usize> = if last {
        (0..e.children.len()).rev().collect()
    } else {
        (0..e.children.len()).collect()
    };
    for i in idxs {
        if run_tag(&e.children[i]).is_some_and(|t| t.is_control()) {
            e.children.remove(i);
            return true;
        }
        if remove_control(&mut e.children[i], last) {
            return true;
        }
    }
    false
}

/// Removes a control tag from a sibling: the sibling itself when it is the tag run, otherwise
/// the first (or last) tag run inside it.
fn strip(slot: &mut Option<Node>, last: bool) {
    if let Some(n) = slot {
        if run_tag(n).is_some_and(|t| t.is_control()) {
            *slot = None;
        } else {
            remove_control(n, last);
        }
    }
}

/// True when a paragraph/row has no visible content left (only properties / empty runs).
fn is_blank(n: &Node) -> bool {
    match n {
        Node::El(e) if e.is("w:p") => !e.children.iter().any(|c| match c {
            Node::El(r) if r.is("w:r") => run_text(r).is_none_or(|t| !t.is_empty()),
            Node::El(x) => x.is("w:hyperlink") || x.is("w:fldSimple") || x.is("w:sdt"),
            _ => false,
        }),
        Node::El(e) if e.is("w:tr") => e.children.iter().all(|c| match c {
            Node::El(tc) if tc.is("w:tc") => tc
                .children
                .iter()
                .all(|x| !matches!(x, Node::El(p) if p.is("w:p")) || is_blank(x)),
            _ => true,
        }),
        _ => false,
    }
}

// ============================================================================ data

struct Ctx<'a> {
    scopes: Vec<Value>,
    images: &'a mut Images,
    rels: &'a mut Vec<(String, String)>,
    warnings: &'a mut Vec<String>,
}

fn lookup(scopes: &[Value], path: &str) -> Option<Value> {
    let path = path.trim();
    if path == "this" || path == "." {
        return scopes.last().cloned();
    }
    let parts: Vec<&str> = path.trim_start_matches("this.").split('.').collect();
    for scope in scopes.iter().rev() {
        let mut cur = scope;
        let mut ok = true;
        for (i, part) in parts.iter().enumerate() {
            let next = match cur {
                Value::Object(m) => m.get(*part),
                Value::Array(a) => part.parse::<usize>().ok().and_then(|k| a.get(k)),
                _ => None,
            };
            match next {
                Some(v) => cur = v,
                None => {
                    ok = false;
                    // Only fall back to outer scopes when the first segment is missing here.
                    if i > 0 {
                        return None;
                    }
                    break;
                }
            }
        }
        if ok {
            return Some(cur.clone());
        }
    }
    None
}

fn display(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

// ---------------------------------------------------------------- expressions

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(String),
    LParen,
    RParen,
}

fn tokenize(s: &str) -> Res<Vec<Tok>> {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '(' {
            out.push(Tok::LParen);
            i += 1;
        } else if ch == ')' {
            out.push(Tok::RParen);
            i += 1;
        } else if ch == '"' || ch == '\'' || ch == '“' || ch == '”' || ch == '‘' || ch == '’'
        {
            // Word often turns quotes into typographic ones.
            let close = |x: char| {
                x == ch
                    || (matches!(ch, '“' | '”') && matches!(x, '“' | '”'))
                    || (matches!(ch, '‘' | '’') && matches!(x, '‘' | '’'))
            };
            let mut j = i + 1;
            while j < c.len() && !close(c[j]) {
                j += 1;
            }
            out.push(Tok::Str(c[i + 1..j.min(c.len())].iter().collect()));
            i = j + 1;
        } else if ch.is_ascii_digit()
            || (ch == '-'
                && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())
                && !matches!(
                    out.last(),
                    Some(Tok::Num(_) | Tok::Ident(_) | Tok::Str(_) | Tok::RParen)
                ))
        {
            let mut j = i + 1;
            while j < c.len() && (c[j].is_ascii_digit() || c[j] == '.') {
                j += 1;
            }
            let s: String = c[i..j].iter().collect();
            out.push(Tok::Num(s.parse().map_err(|_| format!("bad number {s}"))?));
            i = j;
        } else if ch.is_alphanumeric() || ch == '_' || ch == '.' {
            let mut j = i + 1;
            while j < c.len() && (c[j].is_alphanumeric() || c[j] == '_' || c[j] == '.') {
                j += 1;
            }
            let w: String = c[i..j].iter().collect();
            out.push(match w.as_str() {
                "and" => Tok::Op("&&".into()),
                "or" => Tok::Op("||".into()),
                "not" => Tok::Op("!".into()),
                _ => Tok::Ident(w),
            });
            i = j;
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            if ["==", "!=", ">=", "<=", "&&", "||"].contains(&two.as_str()) {
                out.push(Tok::Op(two));
                i += 2;
            } else if "<>!=".contains(ch) {
                out.push(Tok::Op(if ch == '=' {
                    "==".into()
                } else {
                    ch.to_string()
                }));
                i += 1;
            } else {
                return Err(format!("unexpected character '{ch}' in condition '{s}'"));
            }
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    scopes: &'a [Value],
}

impl Parser<'_> {
    fn peek_op(&self, ops: &[&str]) -> Option<String> {
        match self.toks.get(self.pos) {
            Some(Tok::Op(o)) if ops.contains(&o.as_str()) => Some(o.clone()),
            _ => None,
        }
    }

    fn or(&mut self) -> Res<Value> {
        let mut v = self.and()?;
        while self.peek_op(&["||"]).is_some() {
            self.pos += 1;
            let r = self.and()?;
            v = Value::Bool(truthy(&v) || truthy(&r));
        }
        Ok(v)
    }

    fn and(&mut self) -> Res<Value> {
        let mut v = self.not()?;
        while self.peek_op(&["&&"]).is_some() {
            self.pos += 1;
            let r = self.not()?;
            v = Value::Bool(truthy(&v) && truthy(&r));
        }
        Ok(v)
    }

    fn not(&mut self) -> Res<Value> {
        if self.peek_op(&["!"]).is_some() {
            self.pos += 1;
            return Ok(Value::Bool(!truthy(&self.not()?)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Res<Value> {
        let l = self.primary()?;
        let Some(op) = self.peek_op(&["==", "!=", ">", "<", ">=", "<="]) else {
            return Ok(l);
        };
        self.pos += 1;
        let r = self.primary()?;
        let num = |v: &Value| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().replace(',', ".").parse().ok(),
            Value::Bool(b) => Some(*b as u8 as f64),
            _ => None,
        };
        let ord = match (num(&l), num(&r)) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => Some(display(&l).cmp(&display(&r))),
        };
        use std::cmp::Ordering::*;
        Ok(Value::Bool(match op.as_str() {
            "==" => ord == Some(Equal),
            "!=" => ord != Some(Equal),
            ">" => ord == Some(Greater),
            "<" => ord == Some(Less),
            ">=" => matches!(ord, Some(Greater | Equal)),
            _ => matches!(ord, Some(Less | Equal)),
        }))
    }

    fn primary(&mut self) -> Res<Value> {
        let t = self
            .toks
            .get(self.pos)
            .cloned()
            .ok_or("incomplete condition")?;
        self.pos += 1;
        Ok(match t {
            Tok::Num(n) => serde_json::Number::from_f64(n)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            Tok::Str(s) => Value::String(s),
            Tok::Ident(w) => match w.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "null" => Value::Null,
                path => lookup(self.scopes, path).unwrap_or(Value::Null),
            },
            Tok::LParen => {
                let v = self.or()?;
                if self.toks.get(self.pos) != Some(&Tok::RParen) {
                    return Err("missing ')' in condition".into());
                }
                self.pos += 1;
                v
            }
            other => return Err(format!("unexpected {other:?} in condition")),
        })
    }
}

fn eval_condition(expr: &str, scopes: &[Value]) -> Res<bool> {
    let toks = tokenize(expr)?;
    let mut p = Parser {
        toks,
        pos: 0,
        scopes,
    };
    let v = p.or()?;
    if p.pos != p.toks.len() {
        return Err(format!(
            "unexpected tokens at the end of condition '{expr}'"
        ));
    }
    Ok(truthy(&v))
}

fn condition_idents(expr: &str) -> Vec<String> {
    tokenize(expr)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| match t {
            Tok::Ident(w) if !matches!(w.as_str(), "true" | "false" | "null" | "this") => Some(w),
            _ => None,
        })
        .collect()
}

// ============================================================================ images

#[derive(Default)]
struct Images {
    /// (zip path, bytes)
    media: Vec<(String, Vec<u8>)>,
    extensions: Vec<String>,
    counter: usize,
}

const EMU_PER_PT: f64 = 12700.0;

/// Builds the drawing run for an image value { "__image_b64", "width", "height" } (points).
fn image_run(v: &Map<String, Value>, rpr: Option<&El>, ctx: &mut Ctx) -> Res<Node> {
    let b64 = v.get("__image_b64").and_then(Value::as_str).unwrap_or("");
    let mut data = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("invalid image data: {e}"))?;
    let format = image::guess_format(&data).map_err(|e| format!("unsupported image: {e}"))?;
    let reader = image::ImageReader::with_format(Cursor::new(&data), format);
    let (pw, ph) = reader
        .into_dimensions()
        .map_err(|e| format!("cannot read image: {e}"))?;
    let ext = match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpeg",
        image::ImageFormat::Gif => "gif",
        _ => {
            // Re-encode exotic formats as PNG for maximum compatibility.
            let img =
                image::load_from_memory(&data).map_err(|e| format!("cannot decode image: {e}"))?;
            let mut out = Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png)
                .map_err(|e| format!("cannot encode image: {e}"))?;
            data = out.into_inner();
            "png"
        }
    };
    let natural = (pw as f64 * 0.75, ph as f64 * 0.75); // 96 dpi
    let (w, h) = match (
        v.get("width").and_then(Value::as_f64),
        v.get("height").and_then(Value::as_f64),
    ) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w * natural.1 / natural.0.max(1e-9)),
        (None, Some(h)) => (h * natural.0 / natural.1.max(1e-9), h),
        (None, None) => natural,
    };
    ctx.images.counter += 1;
    let n = ctx.images.counter;
    let file = format!("docgen{n}.{ext}");
    let rid = format!("rIdDocGen{n}");
    ctx.images.media.push((format!("word/media/{file}"), data));
    if !ctx.images.extensions.contains(&ext.to_string()) {
        ctx.images.extensions.push(ext.into());
    }
    ctx.rels.push((rid.clone(), format!("media/{file}")));
    let (cx, cy) = (
        (w * EMU_PER_PT).round() as i64,
        (h * EMU_PER_PT).round() as i64,
    );
    let id = 9000 + n;
    let drawing = format!(
        r#"<w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{id}" name="Image {n}"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{id}" name="{file}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing>"#
    );
    let mut children = Vec::new();
    if let Some(p) = rpr {
        children.push(Node::El(p.clone()));
    }
    children.push(Node::Raw(drawing));
    Ok(Node::El(El::new("w:r", "", children)))
}

// ============================================================================ rendering

fn render_value_run(r: &El, path: &str, ctx: &mut Ctx) -> Res<Vec<Node>> {
    let rpr = r.child("w:rPr");
    let value = match lookup(&ctx.scopes, path) {
        Some(v) => v,
        None => {
            ctx.warnings.push(format!("no value for {{{{{path}}}}}"));
            Value::Null
        }
    };
    if let Value::Object(m) = &value
        && m.contains_key("__image_b64")
    {
        return Ok(vec![image_run(m, rpr, ctx)?]);
    }
    let text = display(&value);
    // Line breaks in values become <w:br/>.
    let mut children = Vec::new();
    if let Some(p) = rpr {
        children.push(Node::El(p.clone()));
    }
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            children.push(Node::El(El {
                name: "w:br".into(),
                attrs: String::new(),
                children: vec![],
                empty: true,
            }));
        }
        children.push(Node::El(El::new(
            "w:t",
            r#"xml:space="preserve""#,
            vec![Node::Text(line.trim_end_matches('\r').into())],
        )));
    }
    Ok(vec![Node::El(El::new("w:r", &r.attrs, children))])
}

fn render_node(n: &Node, ctx: &mut Ctx) -> Res<Vec<Node>> {
    if let Some(Tag::Value(path)) = run_tag(n)
        && let Node::El(r) = n
    {
        return render_value_run(r, &path, ctx);
    }
    match n {
        Node::El(e) => {
            let mut copy = El {
                name: e.name.clone(),
                attrs: e.attrs.clone(),
                children: vec![],
                empty: e.empty,
            };
            copy.children = render_list(&e.children, ctx)?;
            // A table cell must keep at least one paragraph.
            if copy.is("w:tc")
                && !copy
                    .children
                    .iter()
                    .any(|c| matches!(c, Node::El(p) if p.is("w:p")))
            {
                copy.children.push(Node::El(El::new("w:p", "", vec![])));
            }
            Ok(vec![Node::El(copy)])
        }
        other => Ok(vec![other.clone()]),
    }
}

fn node_controls(n: &Node) -> Vec<Tag> {
    let mut v = Vec::new();
    controls(n, &mut v);
    v
}

fn is_el(n: &Node, name: &str) -> bool {
    matches!(n, Node::El(e) if e.is(name))
}

/// Whether the if/loop opened by the first tag of a row closes in the same paragraph (inline).
fn first_construct_is_inline(row: &Node) -> bool {
    fn walk(n: &Node, found: &mut Option<bool>) {
        if found.is_some() {
            return;
        }
        if let Node::El(e) = n {
            if e.is("w:p") {
                let tags = node_controls(n);
                if let Some(first) = tags.first() {
                    let mut depth = 0;
                    let mut closed = false;
                    for t in &tags {
                        depth += t.depth_delta();
                        if depth == 0 {
                            closed = true;
                            break;
                        }
                    }
                    *found = Some(first.depth_delta() > 0 && closed);
                }
                return;
            }
            for c in &e.children {
                walk(c, found);
            }
        }
    }
    let mut found = None;
    walk(row, &mut found);
    found.unwrap_or(true)
}

fn render_list(children: &[Node], ctx: &mut Ctx) -> Res<Vec<Node>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < children.len() {
        let c = &children[i];
        let ctl = node_controls(c);
        let balance: i32 = ctl.iter().map(Tag::depth_delta).sum();
        let row_construct = is_el(c, "w:tr")
            && match ctl.first() {
                Some(Tag::LoopStart(_)) => true,
                Some(Tag::IfStart(_)) => !first_construct_is_inline(c),
                _ => false,
            };
        if ctl.is_empty() || (balance == 0 && !row_construct) {
            out.extend(render_node(c, ctx)?);
            i += 1;
            continue;
        }
        let Some(open) = ctl.first().cloned().filter(|t| t.depth_delta() > 0) else {
            return Err(format!(
                "unexpected {:?} (no matching opening tag)",
                ctl.first().unwrap()
            ));
        };
        // Find the sibling that closes the construct.
        let mut depth = 0;
        let mut j = i;
        let mut separators: Vec<(usize, Tag)> = Vec::new();
        'outer: loop {
            if j >= children.len() {
                return Err(format!("{open:?} is never closed"));
            }
            for t in node_controls(&children[j]) {
                if depth == 1 && matches!(t, Tag::Else | Tag::Elif(_)) && j != i {
                    separators.push((j, t.clone()));
                }
                depth += t.depth_delta();
                if depth == 0 {
                    break 'outer;
                }
            }
            j += 1;
        }
        // Remove the opening tag (first control of the first sibling) and the closing tag
        // (last control of the last sibling); a sibling that is the tag run itself disappears.
        let mut body: Vec<Option<Node>> = children[i..=j].iter().cloned().map(Some).collect();
        let last = body.len() - 1;
        strip(&mut body[0], false);
        strip(&mut body[last], true);
        // Branches of an if (separator siblings hold only the else/elif tag).
        let mut branches: Vec<(Option<String>, Vec<Node>)> = vec![(
            match &open {
                Tag::IfStart(e) => Some(e.clone()),
                _ => None,
            },
            vec![],
        )];
        for (k, slot) in body.into_iter().enumerate() {
            let abs = i + k;
            if let Some((_, sep)) = separators.iter().find(|(s, _)| *s == abs) {
                let mut rest = slot;
                strip(&mut rest, false);
                if rest.as_ref().is_some_and(|n| !is_blank(n)) {
                    return Err("{{else}} / {{elif}} must be alone in its paragraph or row when the condition spans several".into());
                }
                branches.push((
                    match sep {
                        Tag::Elif(e) => Some(e.clone()),
                        _ => None,
                    },
                    vec![],
                ));
                continue;
            }
            let Some(node) = slot else { continue };
            // Paragraphs/rows that only held the opening or closing tag are dropped.
            if (k == 0 || k == last)
                && (is_el(&node, "w:p") || is_el(&node, "w:tr"))
                && is_blank(&node)
            {
                continue;
            }
            branches.last_mut().unwrap().1.push(node);
        }
        match open {
            Tag::LoopStart(path) => {
                let items = match lookup(&ctx.scopes, &path) {
                    Some(Value::Array(a)) => a,
                    Some(Value::Null) | None => {
                        ctx.warnings
                            .push(format!("no array for {{{{loop {path}}}}}"));
                        vec![]
                    }
                    Some(other) => vec![other],
                };
                let content = branches.into_iter().next().map(|b| b.1).unwrap_or_default();
                for (idx, item) in items.into_iter().enumerate() {
                    let mut scoped = item;
                    if let Value::Object(m) = &mut scoped {
                        m.entry("loop_index").or_insert(Value::from(idx));
                        m.entry("loop_number").or_insert(Value::from(idx + 1));
                    }
                    ctx.scopes.push(scoped);
                    let r = render_list(&content, ctx);
                    ctx.scopes.pop();
                    out.extend(r?);
                }
            }
            Tag::IfStart(_) => {
                for (cond, nodes) in branches {
                    let take = match cond {
                        Some(expr) => eval_condition(&expr, &ctx.scopes)?,
                        None => true,
                    };
                    if take {
                        out.extend(render_list(&nodes, ctx)?);
                        break;
                    }
                }
            }
            _ => unreachable!(),
        }
        i = j + 1;
    }
    Ok(out)
}

// ============================================================================ validation & keys

fn validate(tags: &[Tag], part: &str) -> Res<()> {
    let mut stack: Vec<&Tag> = Vec::new();
    for t in tags {
        match t {
            Tag::LoopStart(_) | Tag::IfStart(_) => stack.push(t),
            Tag::LoopEnd => {
                if !matches!(stack.pop(), Some(Tag::LoopStart(_))) {
                    return Err(format!(
                        "{part}: {{{{endloop}}}} without matching {{{{loop}}}}"
                    ));
                }
            }
            Tag::IfEnd => {
                if !matches!(stack.pop(), Some(Tag::IfStart(_))) {
                    return Err(format!("{part}: {{{{endif}}}} without matching {{{{if}}}}"));
                }
            }
            Tag::Else | Tag::Elif(_) => {
                if !matches!(stack.last(), Some(Tag::IfStart(_))) {
                    return Err(format!("{part}: {{{{else}}}} outside of an {{{{if}}}}"));
                }
            }
            Tag::Value(_) => {}
        }
    }
    if let Some(open) = stack.last() {
        return Err(format!("{part}: {open:?} is never closed"));
    }
    Ok(())
}

fn schema_set(target: &mut Map<String, Value>, path: &str, leaf: Value) -> Option<()> {
    let parts: Vec<&str> = path
        .trim_start_matches("this.")
        .split('.')
        .filter(|p| !p.is_empty() && *p != "this")
        .collect();
    let (last, init) = parts.split_last()?;
    let mut cur = target;
    for p in init {
        let entry = cur
            .entry(p.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        cur = entry.as_object_mut()?;
    }
    let slot = cur.entry(last.to_string()).or_insert(Value::Null);
    if slot.is_null() || (slot.is_string() && !leaf.is_string()) {
        *slot = leaf;
    }
    Some(())
}

/// The item object of the (possibly dotted) array `path` in `m`.
fn array_item<'m>(m: &'m mut Map<String, Value>, path: &str) -> &'m mut Map<String, Value> {
    schema_set(m, path, Value::Array(vec![Value::Object(Map::new())]));
    let mut cur = m;
    let parts: Vec<&str> = path.trim_start_matches("this.").split('.').collect();
    for (k, p) in parts.iter().enumerate() {
        let v = cur.get_mut(*p).expect("created by schema_set");
        if k + 1 < parts.len() {
            cur = v.as_object_mut().expect("object path");
            continue;
        }
        if !matches!(v, Value::Array(a) if a.first().is_some_and(Value::is_object)) {
            *v = Value::Array(vec![Value::Object(Map::new())]);
        }
        return v.as_array_mut().unwrap()[0].as_object_mut().unwrap();
    }
    unreachable!("empty loop path")
}

fn keys_from(tags: &[Tag], root: &mut Map<String, Value>) {
    // Keys inside a loop go into the loop's item object.
    let mut loops: Vec<String> = Vec::new();
    for t in tags {
        let mut target: &mut Map<String, Value> = root;
        for lp in &loops {
            target = array_item(target, lp);
        }
        match t {
            Tag::Value(p) if p != "this" && p != "." && !p.starts_with("loop_") => {
                schema_set(target, p, Value::String(String::new()));
            }
            Tag::LoopStart(p) => {
                array_item(target, p);
                loops.push(p.clone());
            }
            Tag::LoopEnd => {
                loops.pop();
            }
            Tag::IfStart(e) | Tag::Elif(e) => {
                for id in condition_idents(e) {
                    if !id.starts_with("loop_") {
                        schema_set(target, &id, Value::String(String::new()));
                    }
                }
            }
            _ => {}
        }
    }
}

// ============================================================================ package

const PARTS: &str = r"^word/(document|header\d*|footer\d*|footnotes|endnotes)\.xml$";

struct Package {
    files: BTreeMap<String, Vec<u8>>,
    order: Vec<String>,
}

fn read_package(docx: &[u8]) -> Res<Package> {
    let mut zip = zip::ZipArchive::new(Cursor::new(docx))
        .map_err(|e| format!("not a .docx (zip) file: {e}"))?;
    let mut files = BTreeMap::new();
    let mut order = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| format!("corrupt .docx: {e}"))?;
        let name = f
            .name()
            .map_err(|e| format!("corrupt .docx: {e}"))?
            .to_string();
        let mut data = Vec::new();
        f.read_to_end(&mut data)
            .map_err(|e| format!("corrupt .docx: {e}"))?;
        order.push(name.clone());
        files.insert(name, data);
    }
    if !files.contains_key("word/document.xml") {
        return Err("not a Word document: word/document.xml is missing".into());
    }
    Ok(Package { files, order })
}

fn write_package(p: &Package) -> Res<Vec<u8>> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut names = p.order.clone();
    for k in p.files.keys() {
        if !names.contains(k) {
            names.push(k.clone());
        }
    }
    for name in names {
        let data = &p.files[&name];
        zip.start_file(name, opts)
            .map_err(|e| format!("cannot write .docx: {e}"))?;
        zip.write_all(data)
            .map_err(|e| format!("cannot write .docx: {e}"))?;
    }
    Ok(zip
        .finish()
        .map_err(|e| format!("cannot write .docx: {e}"))?
        .into_inner())
}

fn template_parts(p: &Package) -> Vec<String> {
    let re = Regex::new(PARTS).unwrap();
    let mut parts: Vec<String> = p.files.keys().filter(|k| re.is_match(k)).cloned().collect();
    // document first, then headers/footers/notes
    parts.sort_by_key(|k| (k != "word/document.xml", k.clone()));
    parts
}

fn rels_path(part: &str) -> String {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    format!("{dir}/_rels/{file}.rels")
}

fn parsed_parts(p: &Package) -> Res<Vec<(String, XmlDoc)>> {
    template_parts(p)
        .into_iter()
        .map(|name| {
            let xml = String::from_utf8_lossy(&p.files[&name]).into_owned();
            let mut doc = parse_xml(&xml).map_err(|e| format!("{name}: {e}"))?;
            normalize_tree(&mut doc.root);
            Ok((name, doc))
        })
        .collect()
}

fn part_tags(doc: &XmlDoc) -> Vec<Tag> {
    let mut tags = Vec::new();
    all_tags(&Node::El(doc.root.clone()), &mut tags);
    tags
}

/// Validates the template; returns the JSON schema of the keys it expects.
pub fn template_keys(docx: &[u8]) -> Res<Value> {
    let pkg = read_package(docx)?;
    let mut root = Map::new();
    for (name, doc) in parsed_parts(&pkg)? {
        let tags = part_tags(&doc);
        validate(&tags, &name)?;
        keys_from(&tags, &mut root);
    }
    let mut v = Value::Object(root);
    scalar_loops(&mut v);
    Ok(v)
}

/// Loops whose items have no keys iterate over scalars ({{this}}): show them as [""].
fn scalar_loops(v: &mut Value) {
    match v {
        Value::Array(a) => {
            if a.len() == 1 && a[0].as_object().is_some_and(Map::is_empty) {
                a[0] = Value::String(String::new());
            }
            a.iter_mut().for_each(scalar_loops);
        }
        Value::Object(m) => m.values_mut().for_each(scalar_loops),
        _ => {}
    }
}

pub struct Filled {
    pub docx: Vec<u8>,
    pub warnings: Vec<String>,
}

/// Fills the template with `data`. Image values must carry "__image_b64" (resolved by the caller).
pub fn fill(docx: &[u8], data: &Value) -> Res<Filled> {
    let mut pkg = read_package(docx)?;
    let mut images = Images::default();
    let mut warnings = Vec::new();
    for (name, doc) in parsed_parts(&pkg)? {
        validate(&part_tags(&doc), &name)?;
        let mut rels = Vec::new();
        let mut ctx = Ctx {
            scopes: vec![data.clone()],
            images: &mut images,
            rels: &mut rels,
            warnings: &mut warnings,
        };
        let rendered = render_node(&Node::El(doc.root.clone()), &mut ctx)
            .map_err(|e| format!("{name}: {e}"))?;
        let Some(Node::El(root)) = rendered.into_iter().next() else {
            return Err(format!("{name}: rendering failed"));
        };
        let out = XmlDoc {
            prolog: doc.prolog,
            root,
        };
        pkg.files.insert(name.clone(), write_xml(&out).into_bytes());
        if !rels.is_empty() {
            let path = rels_path(&name);
            let mut xml = pkg.files.get(&path).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_else(|| {
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#.to_string()
            });
            let entries: String = rels
                .iter()
                .map(|(id, target)| format!(r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="{target}"/>"#))
                .collect();
            xml = xml.replacen("</Relationships>", &format!("{entries}</Relationships>"), 1);
            pkg.files.insert(path, xml.into_bytes());
        }
    }
    for (path, bytes) in images.media.drain(..) {
        pkg.files.insert(path, bytes);
    }
    if !images.extensions.is_empty() {
        let ct = pkg
            .files
            .get("[Content_Types].xml")
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let mut add = String::new();
        for ext in &images.extensions {
            if !ct.contains(&format!("Extension=\"{ext}\"")) {
                add.push_str(&format!(
                    r#"<Default Extension="{ext}" ContentType="image/{ext}"/>"#
                ));
            }
        }
        pkg.files.insert(
            "[Content_Types].xml".into(),
            ct.replacen("</Types>", &format!("{add}</Types>"), 1)
                .into_bytes(),
        );
    }
    Ok(Filled {
        docx: write_package(&pkg)?,
        warnings,
    })
}

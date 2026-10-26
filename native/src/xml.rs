//! Minimal XML DOM (quick-xml based) that round-trips WordprocessingML / XFDF faithfully:
//! attributes are kept raw, entity references are decoded into text.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::doc::Res;

#[derive(Clone, Debug)]
pub enum Node {
    El(El),
    Text(String),
    /// Declarations, comments, CDATA, processing instructions: written back verbatim.
    Raw(String),
}

#[derive(Clone, Debug)]
pub struct El {
    pub name: String,
    /// Raw attribute string, written back verbatim.
    pub attrs: String,
    pub children: Vec<Node>,
    pub empty: bool,
}

impl El {
    pub fn new(name: &str, attrs: &str, children: Vec<Node>) -> El {
        El {
            name: name.into(),
            attrs: attrs.into(),
            children,
            empty: false,
        }
    }

    pub fn is(&self, name: &str) -> bool {
        self.name == name
    }

    pub fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find_map(|c| match c {
            Node::El(e) if e.is(name) => Some(e),
            _ => None,
        })
    }
}

pub struct XmlDoc {
    pub prolog: Vec<Node>,
    pub root: El,
}

pub fn parse_xml(xml: &str) -> Res<XmlDoc> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<El> = Vec::new();
    let mut prolog = Vec::new();
    let mut root: Option<El> = None;
    let push = |stack: &mut Vec<El>, prolog: &mut Vec<Node>, n: Node| match stack.last_mut() {
        Some(top) => top.children.push(n),
        None => prolog.push(n),
    };
    loop {
        let start = reader.buffer_position() as usize;
        let ev = reader
            .read_event()
            .map_err(|e| format!("invalid XML in template: {e}"))?;
        let end = reader.buffer_position() as usize;
        match ev {
            Event::Start(e) => stack.push(El {
                name: e.name().as_ref().to_string(),
                attrs: e.attributes_raw().to_string(),
                children: vec![],
                empty: false,
            }),
            Event::Empty(e) => {
                let el = El {
                    name: e.name().as_ref().to_string(),
                    attrs: e.attributes_raw().to_string(),
                    children: vec![],
                    empty: true,
                };
                if stack.is_empty() {
                    root = Some(el);
                } else {
                    push(&mut stack, &mut prolog, Node::El(el));
                }
            }
            Event::End(_) => {
                let el = stack.pop().ok_or("unbalanced XML")?;
                if stack.is_empty() {
                    root = Some(el);
                } else {
                    push(&mut stack, &mut prolog, Node::El(el));
                }
            }
            Event::Text(t) => {
                let s = t.to_string();
                append_text(&mut stack, &mut prolog, s);
            }
            Event::GeneralRef(r) => {
                let s = match r.resolve_char_ref().ok().flatten() {
                    Some(c) => c.to_string(),
                    None => match &*r {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        other => format!("&{other};"),
                    },
                };
                append_text(&mut stack, &mut prolog, s);
            }
            Event::Eof => break,
            _ => push(
                &mut stack,
                &mut prolog,
                Node::Raw(xml[start..end].to_string()),
            ),
        }
    }
    Ok(XmlDoc {
        prolog,
        root: root.ok_or("empty XML document")?,
    })
}

fn append_text(stack: &mut [El], prolog: &mut Vec<Node>, s: String) {
    let list = match stack.last_mut() {
        Some(top) => &mut top.children,
        None => prolog,
    };
    if let Some(Node::Text(prev)) = list.last_mut() {
        prev.push_str(&s);
    } else {
        list.push(Node::Text(s));
    }
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn write_node(n: &Node, out: &mut String) {
    match n {
        Node::Text(t) => out.push_str(&escape(t)),
        Node::Raw(r) => out.push_str(r),
        Node::El(e) => write_el(e, out),
    }
}

fn write_el(e: &El, out: &mut String) {
    out.push('<');
    out.push_str(&e.name);
    if !e.attrs.is_empty() {
        if !e.attrs.starts_with(char::is_whitespace) {
            out.push(' ');
        }
        out.push_str(&e.attrs);
    }
    if e.empty && e.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for c in &e.children {
        write_node(c, out);
    }
    out.push_str("</");
    out.push_str(&e.name);
    out.push('>');
}

pub fn write_xml(doc: &XmlDoc) -> String {
    let mut out = String::new();
    for n in &doc.prolog {
        write_node(n, &mut out);
    }
    write_el(&doc.root, &mut out);
    out
}

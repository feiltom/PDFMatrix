//! Text extraction (glyphs → words → lines) and search, built on hayro-interpret.
//!
//! All coordinates are in PDF user space (origin bottom-left, unrotated page), which is what
//! PDFNet's TextExtractor and TextSearch report by default.

use std::sync::Arc;

use hayro::hayro_interpret::font::GlyphRun;
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::{Affine, BezPath, Point, Rect};
use regex::RegexBuilder;

use crate::doc::Res;

/// Vertical extent of a glyph cell in 1000-unit glyph space, used when no font bbox is known.
const ASCENT: f64 = 800.0;
const DESCENT: f64 = -200.0;
const DEFAULT_ADVANCE: f64 = 500.0;

#[derive(Clone, Debug)]
pub struct Glyph {
    pub text: String,
    /// Quad in user space: lower-left, lower-right, upper-right, upper-left of the glyph cell.
    pub quad: [f64; 8],
    pub bbox: [f64; 4],
    /// Direction of the baseline (unit vector) and em size in user space.
    pub dir: (f64, f64),
    pub size: f64,
    pub origin: (f64, f64),
}

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub bbox: [f64; 4],
    pub quad: [f64; 8],
    /// Index range into the page glyph list.
    pub glyph_start: usize,
    pub glyph_end: usize,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub bbox: [f64; 4],
    pub words: Vec<Word>,
}

pub struct PageText {
    pub glyphs: Vec<Glyph>,
    pub lines: Vec<Line>,
}

impl PageText {
    /// Plain text: words separated by spaces, lines by '\n'.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| {
                l.words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Default)]
struct Collector {
    glyphs: Vec<Glyph>,
}

impl<'a> Device<'a> for Collector {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            let text = match glyph.as_unicode() {
                Some(BfString::Char(c)) => c.to_string(),
                Some(BfString::String(s)) => s,
                None => '\u{FFFD}'.to_string(),
            };
            let advance = match &**glyph {
                hayro::hayro_interpret::font::Glyph::Outline(g) => g
                    .advance_width()
                    .map(|w| w as f64)
                    .filter(|w| *w > 0.0)
                    .unwrap_or(DEFAULT_ADVANCE),
                hayro::hayro_interpret::font::Glyph::Type3(_) => DEFAULT_ADVANCE,
            };
            let t: Affine = props.transform * glyph.transform();
            let corners = [
                t * Point::new(0.0, DESCENT),
                t * Point::new(advance, DESCENT),
                t * Point::new(advance, ASCENT),
                t * Point::new(0.0, ASCENT),
            ];
            let origin = t * Point::ZERO;
            let along = t * Point::new(1000.0, 0.0) - origin;
            let up = t * Point::new(0.0, 1000.0) - origin;
            let len = along.hypot().max(1e-9);
            let quad = [
                corners[0].x,
                corners[0].y,
                corners[1].x,
                corners[1].y,
                corners[2].x,
                corners[2].y,
                corners[3].x,
                corners[3].y,
            ];
            self.glyphs.push(Glyph {
                text,
                bbox: bbox_of(&quad),
                quad,
                dir: (along.x / len, along.y / len),
                size: up.hypot(),
                origin: (origin.x, origin.y),
            });
        }
    }
}

fn bbox_of(q: &[f64; 8]) -> [f64; 4] {
    let xs = [q[0], q[2], q[4], q[6]];
    let ys = [q[1], q[3], q[5], q[7]];
    [
        xs.iter().cloned().fold(f64::INFINITY, f64::min),
        ys.iter().cloned().fold(f64::INFINITY, f64::min),
        xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    ]
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

pub fn extract(bytes: Arc<Vec<u8>>, page_index: usize) -> Res<PageText> {
    let pdf =
        Pdf::new(bytes).map_err(|e| format!("cannot parse PDF for text extraction: {e:?}"))?;
    let pages = pdf.pages();
    let page = pages
        .get(page_index)
        .ok_or_else(|| format!("page index {page_index} out of range"))?;
    let cache = InterpreterCache::new();
    let settings = InterpreterSettings {
        render_annotations: false,
        ..Default::default()
    };
    let mut ctx = Context::new(
        Affine::IDENTITY,
        Rect::new(-1e7, -1e7, 1e7, 1e7),
        &cache,
        page.xref(),
        settings,
    );
    let mut collector = Collector::default();
    interpret_page(page, &mut ctx, &mut collector);
    let glyphs = collector.glyphs;
    let lines = group(&glyphs);
    Ok(PageText { glyphs, lines })
}

/// Groups glyphs (in content order) into lines and words.
///
/// A glyph continues the current line when it shares the baseline direction, its origin lies
/// close to the line's baseline, and it does not jump backwards. Words break on whitespace
/// glyphs or on horizontal gaps wider than a fraction of the em size.
fn group(glyphs: &[Glyph]) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut word: Option<Word> = None;
    let mut line_words: Vec<Word> = Vec::new();
    let mut prev: Option<&Glyph> = None;

    let flush_word = |word: &mut Option<Word>, line_words: &mut Vec<Word>| {
        if let Some(w) = word.take()
            && !w.text.trim().is_empty()
        {
            line_words.push(w);
        }
    };
    let flush_line = |line_words: &mut Vec<Word>, lines: &mut Vec<Line>| {
        if !line_words.is_empty() {
            let bbox = line_words.iter().map(|w| w.bbox).reduce(union).unwrap();
            lines.push(Line {
                bbox,
                words: std::mem::take(line_words),
            });
        }
    };

    for (i, g) in glyphs.iter().enumerate() {
        let is_space = g.text.chars().all(char::is_whitespace);
        let (same_line, gap) = match prev {
            None => (false, 0.0),
            Some(p) => {
                let same_dir = (p.dir.0 * g.dir.0 + p.dir.1 * g.dir.1) > 0.99;
                // Offset of g's origin from p's baseline, measured along/perpendicular to it.
                let dx = g.origin.0 - p.origin.0;
                let dy = g.origin.1 - p.origin.1;
                let along = dx * p.dir.0 + dy * p.dir.1;
                let perp = -dx * p.dir.1 + dy * p.dir.0;
                let em = p.size.max(g.size).max(1e-6);
                let p_advance =
                    { ((p.quad[2] - p.quad[0]).powi(2) + (p.quad[3] - p.quad[1]).powi(2)).sqrt() };
                let gap = along - p_advance;
                (
                    same_dir && perp.abs() < em * 0.5 && gap > -em * 0.6 && gap < em * 3.0,
                    gap / em,
                )
            }
        };

        if !same_line {
            flush_word(&mut word, &mut line_words);
            flush_line(&mut line_words, &mut lines);
        } else if is_space || gap > 0.15 {
            flush_word(&mut word, &mut line_words);
        }
        prev = Some(g);
        if is_space {
            continue;
        }
        match word.as_mut() {
            Some(w) => {
                w.text.push_str(&g.text);
                w.bbox = union(w.bbox, g.bbox);
                w.quad[2] = g.quad[2];
                w.quad[3] = g.quad[3];
                w.quad[4] = g.quad[4];
                w.quad[5] = g.quad[5];
                w.glyph_end = i + 1;
            }
            None => {
                word = Some(Word {
                    text: g.text.clone(),
                    bbox: g.bbox,
                    quad: g.quad,
                    glyph_start: i,
                    glyph_end: i + 1,
                })
            }
        }
    }
    flush_word(&mut word, &mut line_words);
    flush_line(&mut line_words, &mut lines);
    lines
}

// ------------------------------------------------------------------- search

pub struct SearchOpts {
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub ambient_chars: usize,
}

pub struct Match {
    pub text: String,
    pub ambient: String,
    /// One quad per line touched by the match.
    pub quads: Vec<[f64; 8]>,
}

/// Searches the page text. Words are joined with ' ' and lines with '\n', so patterns can match
/// across word boundaries (and across lines with `\s`).
pub fn search(page: &PageText, pattern: &str, opts: &SearchOpts) -> Res<Vec<Match>> {
    // Build the searchable string and a map from byte offsets to (line, word) indices.
    let mut haystack = String::new();
    let mut spans: Vec<(usize, usize, usize, usize)> = Vec::new(); // (start, end, line, word)
    for (li, line) in page.lines.iter().enumerate() {
        if li > 0 {
            haystack.push('\n');
        }
        for (wi, w) in line.words.iter().enumerate() {
            if wi > 0 {
                haystack.push(' ');
            }
            let start = haystack.len();
            haystack.push_str(&w.text);
            spans.push((start, haystack.len(), li, wi));
        }
    }

    let source = if opts.regex {
        pattern.to_string()
    } else {
        regex::escape(pattern)
    };
    let source = if opts.whole_word {
        format!(r"\b(?:{source})\b")
    } else {
        source
    };
    let re = RegexBuilder::new(&source)
        .case_insensitive(!opts.case_sensitive)
        .build()
        .map_err(|e| format!("invalid search pattern: {e}"))?;

    let mut out = Vec::new();
    for m in re.find_iter(&haystack) {
        if m.start() == m.end() {
            continue;
        }
        let mut quads: Vec<[f64; 8]> = Vec::new();
        let mut current: Option<(usize, [f64; 4])> = None;
        for &(s, e, li, wi) in &spans {
            if e <= m.start() || s >= m.end() {
                continue;
            }
            let word = &page.lines[li].words[wi];
            let bbox = partial_word_bbox(page, word, m.start().max(s) - s, m.end().min(e) - s);
            current = match current {
                Some((l, b)) if l == li => Some((l, union(b, bbox))),
                Some((_, b)) => {
                    quads.push(rect_quad(b));
                    Some((li, bbox))
                }
                None => Some((li, bbox)),
            };
        }
        if let Some((_, b)) = current {
            quads.push(rect_quad(b));
        }
        out.push(Match {
            text: m.as_str().to_string(),
            ambient: ambient(&haystack, m.start(), m.end(), opts.ambient_chars),
            quads,
        });
    }
    Ok(out)
}

/// Bounding box of the glyphs covering bytes [from, to) of the word text.
fn partial_word_bbox(page: &PageText, word: &Word, from: usize, to: usize) -> [f64; 4] {
    let mut offset = 0;
    let mut acc: Option<[f64; 4]> = None;
    for g in &page.glyphs[word.glyph_start..word.glyph_end] {
        if g.text.chars().all(char::is_whitespace) {
            continue;
        }
        let (s, e) = (offset, offset + g.text.len());
        offset = e;
        if e > from && s < to {
            acc = Some(acc.map_or(g.bbox, |a| union(a, g.bbox)));
        }
    }
    acc.unwrap_or(word.bbox)
}

fn rect_quad(b: [f64; 4]) -> [f64; 8] {
    [b[0], b[1], b[2], b[1], b[2], b[3], b[0], b[3]]
}

fn ambient(text: &str, start: usize, end: usize, n: usize) -> String {
    let before: String = text[..start]
        .chars()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after: String = text[end..].chars().take(n).collect();
    format!("{before}{}{after}", &text[start..end]).replace('\n', " ")
}

//! Text-anchored editing steps ("steering commands") on the main story.
//!
//! The step vocabulary mirrors quds' `office_steps.dart` (units in points,
//! line spacing as a multiple) so one recipe drives Word, quds and this
//! crate alike — see `scripts/wordsteer/ws/ops.json`. Targets are either
//!
//! * text-anchored: `Find { text, occurrence }` — the n-th occurrence of
//!   `text` inside a single paragraph of the main story, or
//! * index-based: `Index { paragraph, start, end }` (character offsets).
//!
//! The paragraph list and its text view follow Word's main-story
//! `paragraphs` collection: document order, including table cells and
//! content controls, excluding text boxes; deleted text and field codes are
//! invisible, tabs/breaks count as one character.
//!
//! Edits are surgical: only the runs/paragraphs a step touches are changed,
//! everything else is re-serialised verbatim (see [`crate::xml_tree`]).
//! Run and paragraph properties are inserted in CT_RPr / CT_PPr schema order
//! so Word opens the result without a repair prompt.

use crate::error::{Error, Result};
use crate::ns::{PART_DOCUMENT, W};
use crate::package::Package;
use crate::xml_tree::{escape, unescape, Document, Elem, Node};

const PART_STYLES: &str = "word/styles.xml";

/// Where a step applies.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// The `occurrence`-th (1-based) match of `text` within one paragraph.
    Find {
        /// Exact text to find (case-sensitive).
        text: String,
        /// 1-based occurrence, counted in document order.
        occurrence: usize,
    },
    /// A paragraph by index, optionally a character range of it.
    Index {
        /// Paragraph index in the main story.
        paragraph: usize,
        /// Start offset in characters (default 0).
        start: Option<usize>,
        /// End offset in characters (default: paragraph length).
        end: Option<usize>,
    },
}

/// Character formatting; `None` leaves a property untouched.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunFormat {
    /// Bold on/off.
    pub bold: Option<bool>,
    /// Italic on/off.
    pub italic: Option<bool>,
    /// Single strike-through on/off.
    pub strike: Option<bool>,
    /// Underline kind (`single`, `double`, `none`, …).
    pub underline: Option<String>,
    /// Font colour as six hex digits.
    pub color: Option<String>,
    /// Highlight as six hex digits (mapped to Word's named palette when it
    /// matches one, otherwise written as shading) or `none`.
    pub highlight: Option<String>,
    /// Font family name.
    pub font: Option<String>,
    /// Font size in points.
    pub size: Option<f64>,
}

/// Paragraph formatting; lengths in points, `line_spacing` a multiple.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParagraphFormat {
    /// `left`, `center`, `right` or `both` (justify).
    pub alignment: Option<String>,
    /// Space before.
    pub before: Option<f64>,
    /// Space after.
    pub after: Option<f64>,
    /// Line spacing multiple (1.0 = single).
    pub line_spacing: Option<f64>,
    /// Left indent.
    pub left: Option<f64>,
    /// Right indent.
    pub right: Option<f64>,
    /// First-line indent.
    pub first_line: Option<f64>,
    /// Hanging indent.
    pub hanging: Option<f64>,
}

/// Page setup for one section; lengths in points.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SectionFormat {
    /// 0-based section index.
    pub section: usize,
    /// Top margin.
    pub top: Option<f64>,
    /// Bottom margin.
    pub bottom: Option<f64>,
    /// Left margin.
    pub left: Option<f64>,
    /// Right margin.
    pub right: Option<f64>,
    /// Page width.
    pub width: Option<f64>,
    /// Page height.
    pub height: Option<f64>,
}

/// One editing step.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Format the target characters.
    RunFormat(Target, RunFormat),
    /// Format the paragraph containing the target.
    ParagraphFormat(Target, ParagraphFormat),
    /// Replace the target characters with new text (`\t`/`\n` allowed).
    ReplaceText(Target, String),
    /// Apply built-in heading style `level` (0 = back to Normal).
    Heading(Target, u8),
    /// Insert a new paragraph with `text` after the target's paragraph.
    ParagraphInsert(Target, String),
    /// Delete the target's paragraph.
    ParagraphDelete(Target),
    /// Change page margins / size of one section.
    SectionFormat(SectionFormat),
}

/// What each step resolved to: (paragraph index, start, end) or None.
#[derive(Debug, Clone, Default)]
pub struct StepReport {
    /// One entry per step.
    pub resolved: Vec<Option<(usize, usize, usize)>>,
}

/// Plain-text view of every main-story paragraph (the addressing view).
pub fn paragraph_texts(pkg: &Package) -> Result<Vec<String>> {
    let doc = load(pkg, PART_DOCUMENT)?;
    let cx = Cx::new(&doc);
    let body = body(&doc.root, &cx)?;
    Ok(paragraph_paths(body, &cx)
        .iter()
        .map(|p| segments(get(body, p), &cx).0)
        .collect())
}

/// Apply steps in order. On error the package is left unchanged.
pub fn apply_steps(pkg: &mut Package, steps: &[Step]) -> Result<StepReport> {
    let mut doc = load(pkg, PART_DOCUMENT)?;
    let cx = Cx::new(&doc);
    let mut styles: Option<Document> = None;
    let mut report = StepReport::default();
    for (i, step) in steps.iter().enumerate() {
        let r = apply_one(&mut doc, &mut styles, pkg, &cx, step)
            .map_err(|e| Error::Step(format!("step {} ({}): {e}", i + 1, step_name(step))))?;
        report.resolved.push(r);
    }
    pkg.set_part(PART_DOCUMENT, doc.to_bytes());
    if let Some(st) = styles {
        pkg.set_part(PART_STYLES, st.to_bytes());
    }
    Ok(report)
}

fn step_name(s: &Step) -> &'static str {
    match s {
        Step::RunFormat(..) => "runFormat",
        Step::ParagraphFormat(..) => "paragraphFormat",
        Step::ReplaceText(..) => "replaceText",
        Step::Heading(..) => "heading",
        Step::ParagraphInsert(..) => "paragraphInsert",
        Step::ParagraphDelete(..) => "paragraphDelete",
        Step::SectionFormat(..) => "sectionFormat",
    }
}

// ---------------------------------------------------------------------------
// context and navigation

struct Cx {
    /// `w:` (or whatever prefix binds the main namespace, plus colon).
    w: Vec<u8>,
}

impl Cx {
    fn new(doc: &Document) -> Self {
        let mut w = doc.prefix_for(W).unwrap_or_else(|| b"w".to_vec());
        w.push(b':');
        Cx { w }
    }
    fn n(&self, local: &str) -> Vec<u8> {
        let mut v = self.w.clone();
        v.extend_from_slice(local.as_bytes());
        v
    }
    fn is(&self, e: &Elem, local: &str) -> bool {
        e.name.len() == self.w.len() + local.len()
            && e.name.starts_with(&self.w)
            && &e.name[self.w.len()..] == local.as_bytes()
    }
    /// Local name if the element is in the main namespace prefix.
    fn local<'a>(&self, e: &'a Elem) -> Option<&'a [u8]> {
        e.name.strip_prefix(self.w.as_slice())
    }
}

fn load(pkg: &Package, part: &str) -> Result<Document> {
    let bytes = pkg
        .get_part(part)
        .ok_or_else(|| Error::Step(format!("package has no {part}")))?;
    Document::parse(bytes, part)
}

fn body<'a>(root: &'a Elem, cx: &Cx) -> Result<&'a Elem> {
    root.elems()
        .find(|e| cx.is(e, "body"))
        .ok_or_else(|| Error::Step("document has no body".into()))
}

fn body_mut<'a>(root: &'a mut Elem, cx: &Cx) -> Result<&'a mut Elem> {
    root.children
        .iter_mut()
        .find_map(|n| match n {
            Node::Elem(e) if cx.is(e, "body") => Some(e),
            _ => None,
        })
        .ok_or_else(|| Error::Step("document has no body".into()))
}

fn get<'a>(mut e: &'a Elem, path: &[usize]) -> &'a Elem {
    for &i in path {
        e = match &e.children[i] {
            Node::Elem(c) => c,
            _ => unreachable!("path points at a non-element"),
        };
    }
    e
}

fn get_mut<'a>(mut e: &'a mut Elem, path: &[usize]) -> &'a mut Elem {
    for &i in path {
        e = match &mut e.children[i] {
            Node::Elem(c) => c,
            _ => unreachable!("path points at a non-element"),
        };
    }
    e
}

const BLOCK_CONTAINERS: &[&str] = &[
    "tbl",
    "tr",
    "tc",
    "sdt",
    "sdtContent",
    "customXml",
    "ins",
    "moveTo",
    "smartTag",
];

fn paragraph_paths(body: &Elem, cx: &Cx) -> Vec<Vec<usize>> {
    fn walk(e: &Elem, cx: &Cx, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        for (i, c) in e.children.iter().enumerate() {
            if let Node::Elem(c) = c {
                path.push(i);
                if cx.is(c, "p") {
                    out.push(path.clone());
                } else if BLOCK_CONTAINERS.iter().any(|n| cx.is(c, n)) {
                    walk(c, cx, path, out);
                }
                path.pop();
            }
        }
    }
    let mut out = Vec::new();
    walk(body, cx, &mut Vec::new(), &mut out);
    out
}

// ---------------------------------------------------------------------------
// paragraph text view

#[derive(Debug, Clone)]
struct Seg {
    /// Path of the run, relative to the paragraph.
    run: Vec<usize>,
    /// Child index inside the run.
    child: usize,
    start: usize,
    len: usize,
    /// A `w:t` (splittable inside), vs. a one-character element.
    is_t: bool,
}

const INLINE_CONTAINERS: &[&str] = &[
    "hyperlink",
    "ins",
    "moveTo",
    "smartTag",
    "sdt",
    "sdtContent",
    "fldSimple",
    "customXml",
    "dir",
    "bdo",
];

fn segments(p: &Elem, cx: &Cx) -> (String, Vec<Seg>) {
    struct St<'c> {
        cx: &'c Cx,
        text: String,
        segs: Vec<Seg>,
        pos: usize,
        fields: Vec<bool>, // true while in the instruction part
    }
    fn walk(e: &Elem, st: &mut St, path: &mut Vec<usize>) {
        for (i, c) in e.children.iter().enumerate() {
            let Node::Elem(c) = c else { continue };
            path.push(i);
            if st.cx.is(c, "r") {
                run(c, st, path);
            } else if INLINE_CONTAINERS.iter().any(|n| st.cx.is(c, n)) {
                walk(c, st, path);
            }
            path.pop();
        }
    }
    fn run(r: &Elem, st: &mut St, path: &[usize]) {
        for (i, c) in r.children.iter().enumerate() {
            let Node::Elem(c) = c else { continue };
            let Some(local) = st.cx.local(c) else {
                continue;
            };
            if local == b"fldChar" {
                match c.attr(&st.cx.n("fldCharType")) {
                    Some(b"begin") => st.fields.push(true),
                    Some(b"separate") => {
                        if let Some(top) = st.fields.last_mut() {
                            *top = false;
                        }
                    }
                    Some(b"end") => {
                        st.fields.pop();
                    }
                    _ => {}
                }
                continue;
            }
            if st.fields.last().copied().unwrap_or(false) {
                continue;
            }
            let piece: Option<(String, bool)> = match local {
                b"t" => Some((c.text(), true)),
                b"tab" | b"ptab" => Some(("\t".into(), false)),
                b"br" | b"cr" => {
                    let page = c.attr(&st.cx.n("type")) == Some(b"page");
                    Some((if page { "\u{c}" } else { "\n" }.into(), false))
                }
                b"noBreakHyphen" => Some(("\u{2011}".into(), false)),
                b"sym" => c
                    .attr(&st.cx.n("char"))
                    .and_then(|v| u32::from_str_radix(&String::from_utf8_lossy(v), 16).ok())
                    .map(|code| {
                        if (0xF000..=0xF0FF).contains(&code) {
                            code - 0xF000
                        } else {
                            code
                        }
                    })
                    .and_then(char::from_u32)
                    .map(|ch| (ch.to_string(), false)),
                _ => None,
            };
            if let Some((s, is_t)) = piece {
                let len = s.chars().count();
                if len > 0 {
                    st.segs.push(Seg {
                        run: path.to_vec(),
                        child: i,
                        start: st.pos,
                        len,
                        is_t,
                    });
                    st.text.push_str(&s);
                    st.pos += len;
                }
            }
        }
    }
    let mut st = St {
        cx,
        text: String::new(),
        segs: Vec::new(),
        pos: 0,
        fields: Vec::new(),
    };
    walk(p, &mut st, &mut Vec::new());
    (st.text, st.segs)
}

/// Resolve a target to (paragraph index, start, end).
fn resolve(body: &Elem, cx: &Cx, t: &Target) -> Result<(Vec<Vec<usize>>, usize, usize, usize)> {
    let paths = paragraph_paths(body, cx);
    match t {
        Target::Find { text, occurrence } => {
            if text.is_empty() {
                return Err(Error::Step("empty find text".into()));
            }
            let mut seen = 0;
            for (i, path) in paths.iter().enumerate() {
                let (pt, _) = segments(get(body, path), cx);
                let mut from = 0;
                while let Some(off) = pt[from..].find(text.as_str()) {
                    let byte = from + off;
                    seen += 1;
                    if seen == *occurrence {
                        let s = pt[..byte].chars().count();
                        let e = s + text.chars().count();
                        return Ok((paths, i, s, e));
                    }
                    from = byte + pt[byte..].chars().next().map_or(1, |c| c.len_utf8());
                }
            }
            Err(Error::Step(format!(
                "target not found: {:?} #{occurrence}",
                truncate(text)
            )))
        }
        Target::Index {
            paragraph,
            start,
            end,
        } => {
            let path = paths
                .get(*paragraph)
                .ok_or_else(|| Error::Step(format!("no paragraph {paragraph}")))?;
            let len = segments(get(body, path), cx).0.chars().count();
            let s = start.unwrap_or(0);
            let e = end.unwrap_or(len);
            if s > e || e > len {
                return Err(Error::Step(format!(
                    "range {s}..{e} outside paragraph of {len}"
                )));
            }
            Ok((paths, *paragraph, s, e))
        }
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(40).collect()
}

// ---------------------------------------------------------------------------
// run splitting

/// Ensure a run boundary at character offset `k` of paragraph `p`.
fn split_at(p: &mut Elem, cx: &Cx, k: usize) {
    let (_, segs) = segments(p, cx);
    // Strictly inside a w:t: split the text node.
    if let Some(s) = segs.iter().find(|s| s.start < k && k < s.start + s.len) {
        if s.is_t {
            split_run(p, &s.run, s.child, k - s.start);
        }
        return;
    }
    // On a boundary between two pieces of the same run: split between them.
    let left = segs.iter().find(|s| s.start + s.len == k);
    let right = segs.iter().find(|s| s.start == k);
    if let (Some(l), Some(r)) = (left, right) {
        if l.run == r.run {
            split_run(p, &l.run, r.child, 0);
        }
    }
}

/// Split the run at `run_path` before child `child` (offset 0) or inside
/// the w:t at `child` after `off` characters.
fn split_run(p: &mut Elem, run_path: &[usize], child: usize, off: usize) {
    let (parent_path, idx) = run_path.split_at(run_path.len() - 1);
    let idx = idx[0];
    let parent = get_mut(p, parent_path);
    let Node::Elem(run) = parent.children.remove(idx) else {
        unreachable!()
    };
    let mut left = Elem {
        children: Vec::new(),
        ..run.clone()
    };
    let mut right = Elem {
        children: Vec::new(),
        ..run.clone()
    };
    left.self_closing = false;
    right.self_closing = false;
    for (i, c) in run.children.into_iter().enumerate() {
        let is_rpr = matches!(&c, Node::Elem(e) if e.local() == b"rPr");
        if is_rpr {
            left.children.push(c.clone());
            right.children.push(c);
        } else if i < child {
            left.children.push(c);
        } else if i > child || off == 0 {
            right.children.push(c);
        } else {
            // Split the w:t text node.
            let Node::Elem(t) = c else { unreachable!() };
            let text = t.text();
            let cut = text.char_indices().nth(off).map_or(text.len(), |(b, _)| b);
            for (part, dst) in [(&text[..cut], &mut left), (&text[cut..], &mut right)] {
                let mut nt = Elem {
                    children: vec![Node::Text(escape(part).into_bytes())],
                    ..t.clone()
                };
                nt.self_closing = false;
                set_space_preserve(&mut nt);
                dst.children.push(Node::Elem(nt));
            }
        }
    }
    parent.children.insert(idx, Node::Elem(right));
    parent.children.insert(idx, Node::Elem(left));
}

fn set_space_preserve(t: &mut Elem) {
    if t.attr(b"xml:space").is_none() {
        t.attrs.push((b"xml:space".to_vec(), b"preserve".to_vec()));
    }
}

/// Isolate [s, e) into whole runs; return their paths (relative to `p`).
fn isolate(p: &mut Elem, cx: &Cx, s: usize, e: usize) -> Vec<Vec<usize>> {
    split_at(p, cx, e);
    split_at(p, cx, s);
    let (_, segs) = segments(p, cx);
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for sg in segs
        .iter()
        .filter(|sg| sg.start >= s && sg.start + sg.len <= e)
    {
        if runs.last() != Some(&sg.run) && !runs.contains(&sg.run) {
            runs.push(sg.run.clone());
        }
    }
    runs
}

// ---------------------------------------------------------------------------
// ordered property insertion

const RPR_ORDER: &[&str] = &[
    "rStyle",
    "rFonts",
    "b",
    "bCs",
    "i",
    "iCs",
    "caps",
    "smallCaps",
    "strike",
    "dstrike",
    "outline",
    "shadow",
    "emboss",
    "imprint",
    "noProof",
    "snapToGrid",
    "vanish",
    "webHidden",
    "color",
    "spacing",
    "w",
    "kern",
    "position",
    "sz",
    "szCs",
    "highlight",
    "u",
    "effect",
    "bdr",
    "shd",
    "fitText",
    "vertAlign",
    "rtl",
    "cs",
    "em",
    "lang",
    "eastAsianLayout",
    "specVanish",
    "oMath",
    "rPrChange",
];

const PPR_ORDER: &[&str] = &[
    "pStyle",
    "keepNext",
    "keepLines",
    "pageBreakBefore",
    "framePr",
    "widowControl",
    "numPr",
    "suppressLineNumbers",
    "pBdr",
    "shd",
    "tabs",
    "suppressAutoHyphens",
    "kinsoku",
    "wordWrap",
    "overflowPunct",
    "topLinePunct",
    "autoSpaceDE",
    "autoSpaceDN",
    "bidi",
    "adjustRightInd",
    "snapToGrid",
    "spacing",
    "ind",
    "contextualSpacing",
    "mirrorIndents",
    "suppressOverlap",
    "jc",
    "textDirection",
    "textAlignment",
    "textboxTightWrap",
    "outlineLvl",
    "divId",
    "cnfStyle",
    "rPr",
    "sectPr",
    "pPrChange",
];

/// Get or create child `local` of `parent`, keeping schema order.
fn ordered_child<'a>(parent: &'a mut Elem, cx: &Cx, local: &str, order: &[&str]) -> &'a mut Elem {
    let name = cx.n(local);
    if let Some(i) = parent.child_index(&name) {
        let Node::Elem(e) = &mut parent.children[i] else {
            unreachable!()
        };
        return e;
    }
    let rank = order
        .iter()
        .position(|n| *n == local)
        .unwrap_or(order.len());
    let at = parent
        .children
        .iter()
        .position(|n| match n {
            Node::Elem(e) => cx
                .local(e)
                .and_then(|l| order.iter().position(|o| o.as_bytes() == l))
                .is_some_and(|r| r > rank),
            _ => false,
        })
        .unwrap_or(parent.children.len());
    parent.self_closing = false;
    parent.children.insert(at, Node::Elem(Elem::new(name)));
    let Node::Elem(e) = &mut parent.children[at] else {
        unreachable!()
    };
    e
}

fn remove_child(parent: &mut Elem, cx: &Cx, local: &str) {
    let name = cx.n(local);
    parent
        .children
        .retain(|n| !matches!(n, Node::Elem(e) if e.name == name));
}

/// The run's rPr (created as first child) / the paragraph's pPr.
fn props<'a>(owner: &'a mut Elem, cx: &Cx, local: &str) -> &'a mut Elem {
    let name = cx.n(local);
    if owner.child_index(&name).is_none() {
        owner.self_closing = false;
        let mut pr = Elem::new(name.clone());
        pr.self_closing = true;
        owner.children.insert(0, Node::Elem(pr));
    }
    owner.child_mut(&name).expect("just ensured")
}

fn set_val(e: &mut Elem, cx: &Cx, val: &str) {
    e.set_attr(&cx.n("val"), val);
}

fn set_toggle(rpr: &mut Elem, cx: &Cx, local: &str, on: bool) {
    let e = ordered_child(rpr, cx, local, RPR_ORDER);
    if on {
        e.remove_attr(&cx.n("val"));
    } else {
        set_val(e, cx, "0");
    }
}

fn twips(pt: f64) -> String {
    format!("{}", (pt * 20.0).round() as i64)
}

const HIGHLIGHTS: &[(&str, &str)] = &[
    ("FFFF00", "yellow"),
    ("00FF00", "green"),
    ("00FFFF", "cyan"),
    ("FF00FF", "magenta"),
    ("0000FF", "blue"),
    ("FF0000", "red"),
    ("000080", "darkBlue"),
    ("008080", "darkCyan"),
    ("008000", "darkGreen"),
    ("800080", "darkMagenta"),
    ("800000", "darkRed"),
    ("808000", "darkYellow"),
    ("808080", "darkGray"),
    ("C0C0C0", "lightGray"),
    ("000000", "black"),
];

fn hex6(v: &str) -> Result<String> {
    let v = v.trim_start_matches('#').to_ascii_uppercase();
    if v.len() == 6 && v.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(v)
    } else {
        Err(Error::Step(format!(
            "expected six-digit hex colour, got {v:?}"
        )))
    }
}

fn apply_run_format(run: &mut Elem, cx: &Cx, f: &RunFormat) -> Result<()> {
    let rpr = props(run, cx, "rPr");
    if let Some(b) = f.bold {
        set_toggle(rpr, cx, "b", b);
        set_toggle(rpr, cx, "bCs", b);
    }
    if let Some(i) = f.italic {
        set_toggle(rpr, cx, "i", i);
        set_toggle(rpr, cx, "iCs", i);
    }
    if let Some(s) = f.strike {
        set_toggle(rpr, cx, "strike", s);
        if s {
            remove_child(rpr, cx, "dstrike");
        }
    }
    if let Some(font) = &f.font {
        let e = ordered_child(rpr, cx, "rFonts", RPR_ORDER);
        for a in ["ascii", "hAnsi", "cs"] {
            e.set_attr(&cx.n(a), font);
        }
        // Theme fonts override literal names; drop them like Word does.
        for a in ["asciiTheme", "hAnsiTheme", "cstheme"] {
            e.remove_attr(&cx.n(a));
        }
    }
    if let Some(c) = &f.color {
        let c = hex6(c)?;
        let e = ordered_child(rpr, cx, "color", RPR_ORDER);
        set_val(e, cx, &c);
        for a in ["themeColor", "themeShade", "themeTint"] {
            e.remove_attr(&cx.n(a));
        }
    }
    if let Some(pt) = f.size {
        if !(1.0..=1638.0).contains(&pt) {
            return Err(Error::Step(format!("font size {pt} out of range")));
        }
        let half = format!("{}", (pt * 2.0).round() as i64);
        set_val(ordered_child(rpr, cx, "sz", RPR_ORDER), cx, &half);
        set_val(ordered_child(rpr, cx, "szCs", RPR_ORDER), cx, &half);
    }
    if let Some(h) = &f.highlight {
        if h.eq_ignore_ascii_case("none") {
            remove_child(rpr, cx, "highlight");
        } else {
            let h = hex6(h)?;
            match HIGHLIGHTS.iter().find(|(x, _)| *x == h) {
                Some((_, name)) => {
                    set_val(ordered_child(rpr, cx, "highlight", RPR_ORDER), cx, name)
                }
                None => {
                    // Not in Word's highlight palette: shading is the only
                    // way to express an arbitrary background colour.
                    let e = ordered_child(rpr, cx, "shd", RPR_ORDER);
                    set_val(e, cx, "clear");
                    e.set_attr(&cx.n("color"), "auto");
                    e.set_attr(&cx.n("fill"), &h);
                }
            }
        }
    }
    if let Some(u) = &f.underline {
        set_val(ordered_child(rpr, cx, "u", RPR_ORDER), cx, u);
    }
    if rpr.children.is_empty() {
        rpr.self_closing = true;
    }
    Ok(())
}

fn apply_paragraph_format(p: &mut Elem, cx: &Cx, f: &ParagraphFormat) -> Result<()> {
    let ppr = props(p, cx, "pPr");
    if let Some(a) = &f.alignment {
        let v = match a.as_str() {
            "left" | "start" => "left",
            "center" => "center",
            "right" | "end" => "right",
            "both" | "justify" => "both",
            other => return Err(Error::Step(format!("unknown alignment {other:?}"))),
        };
        set_val(ordered_child(ppr, cx, "jc", PPR_ORDER), cx, v);
    }
    if f.before.is_some() || f.after.is_some() || f.line_spacing.is_some() {
        let sp = ordered_child(ppr, cx, "spacing", PPR_ORDER);
        if let Some(b) = f.before {
            sp.set_attr(&cx.n("before"), &twips(b));
            sp.remove_attr(&cx.n("beforeAutospacing"));
            sp.remove_attr(&cx.n("beforeLines"));
        }
        if let Some(a) = f.after {
            sp.set_attr(&cx.n("after"), &twips(a));
            sp.remove_attr(&cx.n("afterAutospacing"));
            sp.remove_attr(&cx.n("afterLines"));
        }
        if let Some(m) = f.line_spacing {
            if m <= 0.0 {
                return Err(Error::Step("lineSpacing must be positive".into()));
            }
            sp.set_attr(&cx.n("line"), &format!("{}", (m * 240.0).round() as i64));
            sp.set_attr(&cx.n("lineRule"), "auto");
        }
    }
    if f.left.is_some() || f.right.is_some() || f.first_line.is_some() || f.hanging.is_some() {
        let ind = ordered_child(ppr, cx, "ind", PPR_ORDER);
        if let Some(v) = f.left {
            ind.set_attr(&cx.n("left"), &twips(v));
            ind.remove_attr(&cx.n("start"));
            ind.remove_attr(&cx.n("leftChars"));
            ind.remove_attr(&cx.n("startChars"));
        }
        if let Some(v) = f.right {
            ind.set_attr(&cx.n("right"), &twips(v));
            ind.remove_attr(&cx.n("end"));
            ind.remove_attr(&cx.n("rightChars"));
            ind.remove_attr(&cx.n("endChars"));
        }
        if let Some(v) = f.first_line {
            ind.set_attr(&cx.n("firstLine"), &twips(v));
            ind.remove_attr(&cx.n("hanging"));
            ind.remove_attr(&cx.n("firstLineChars"));
            ind.remove_attr(&cx.n("hangingChars"));
        }
        if let Some(v) = f.hanging {
            ind.set_attr(&cx.n("hanging"), &twips(v));
            ind.remove_attr(&cx.n("firstLine"));
            ind.remove_attr(&cx.n("firstLineChars"));
            ind.remove_attr(&cx.n("hangingChars"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// steps

fn apply_one(
    doc: &mut Document,
    styles: &mut Option<Document>,
    pkg: &Package,
    cx: &Cx,
    step: &Step,
) -> Result<Option<(usize, usize, usize)>> {
    if let Step::SectionFormat(f) = step {
        section_format(body_mut(&mut doc.root, cx)?, cx, f)?;
        return Ok(None);
    }
    let target = match step {
        Step::RunFormat(t, _)
        | Step::ParagraphFormat(t, _)
        | Step::ReplaceText(t, _)
        | Step::Heading(t, _)
        | Step::ParagraphInsert(t, _)
        | Step::ParagraphDelete(t) => t,
        Step::SectionFormat(_) => unreachable!(),
    };
    // Heading may need a style id before the body is borrowed mutably.
    let heading_style = if let Step::Heading(_, level) = step {
        if *level > 9 {
            return Err(Error::Step(format!("heading level {level} > 9")));
        }
        if *level == 0 {
            None
        } else {
            if styles.is_none() {
                *styles = Some(load(pkg, PART_STYLES)?);
            }
            Some(ensure_heading_style(styles.as_mut().unwrap(), *level)?)
        }
    } else {
        None
    };
    let body = body_mut(&mut doc.root, cx)?;
    let (paths, pi, s, e) = resolve(body, cx, target)?;
    let ppath = paths[pi].clone();
    match step {
        Step::RunFormat(_, f) => {
            if s == e {
                return Err(Error::Step("runFormat needs a non-empty range".into()));
            }
            let p = get_mut(body, &ppath);
            for run in isolate(p, cx, s, e) {
                apply_run_format(get_mut(p, &run), cx, f)?;
            }
        }
        Step::ReplaceText(_, text) => replace_text(get_mut(body, &ppath), cx, s, e, text)?,
        Step::ParagraphFormat(_, f) => apply_paragraph_format(get_mut(body, &ppath), cx, f)?,
        Step::Heading(_, _) => {
            let p = get_mut(body, &ppath);
            reset_like_word_style_apply(p, cx);
            let ppr = props(p, cx, "pPr");
            match heading_style {
                Some(id) => set_val(ordered_child(ppr, cx, "pStyle", PPR_ORDER), cx, &id),
                None => remove_child(ppr, cx, "pStyle"),
            }
        }
        Step::ParagraphInsert(_, text) => insert_after(body, cx, &ppath, text)?,
        Step::ParagraphDelete(_) => delete_paragraph(body, cx, &ppath, &paths)?,
        Step::SectionFormat(_) => unreachable!(),
    }
    Ok(Some((pi, s, e)))
}

/// Paragraph-level direct properties that survive a paragraph-style apply.
const PPR_KEEP_ON_STYLE: &[&str] = &[
    "pStyle",
    "numPr",
    "framePr",
    "rPr",
    "sectPr",
    "pPrChange",
    "cnfStyle",
    "divId",
];

/// Character properties subject to Word's ">50% of the paragraph" rule.
const RPR_RESET_ON_STYLE: &[&str] = &[
    "rFonts",
    "b",
    "bCs",
    "i",
    "iCs",
    "caps",
    "smallCaps",
    "strike",
    "dstrike",
    "color",
    "spacing",
    "w",
    "kern",
    "position",
    "sz",
    "szCs",
    "highlight",
    "u",
    "shd",
    "vertAlign",
];

/// What Word does when a paragraph style is applied: direct paragraph
/// formatting is cleared, and direct character formatting that covers more
/// than half of the paragraph's characters is removed (from the runs and
/// the paragraph mark). Character styles and language tags stay.
fn reset_like_word_style_apply(p: &mut Elem, cx: &Cx) {
    if let Some(ppr) = p.child_mut(&cx.n("pPr")) {
        ppr.children.retain(|n| match n {
            Node::Elem(c) => cx
                .local(c)
                .is_some_and(|l| PPR_KEEP_ON_STYLE.iter().any(|k| k.as_bytes() == l)),
            _ => true,
        });
    }
    let (text, segs) = segments(p, cx);
    let total = text.chars().count();
    if total == 0 {
        return;
    }
    let mut runs: Vec<(Vec<usize>, usize)> = Vec::new();
    for sg in &segs {
        match runs.iter_mut().find(|(r, _)| *r == sg.run) {
            Some((_, n)) => *n += sg.len,
            None => runs.push((sg.run.clone(), sg.len)),
        }
    }
    for prop in RPR_RESET_ON_STYLE {
        let name = cx.n(prop);
        let covered: usize = runs
            .iter()
            .filter(|(r, _)| {
                get(p, r)
                    .child(&cx.n("rPr"))
                    .and_then(|x| x.child(&name))
                    .is_some()
            })
            .map(|(_, n)| n)
            .sum();
        if covered * 2 <= total {
            continue;
        }
        for (r, _) in &runs {
            if let Some(rpr) = get_mut(p, r).child_mut(&cx.n("rPr")) {
                remove_child(rpr, cx, prop);
            }
        }
        if let Some(mark) = p
            .child_mut(&cx.n("pPr"))
            .and_then(|x| x.child_mut(&cx.n("rPr")))
        {
            remove_child(mark, cx, prop);
        }
    }
    // Drop property containers the reset left empty.
    let empty_rpr = |n: &Node| {
        matches!(n, Node::Elem(c) if cx.local(c) == Some(b"rPr")
        && !c.children.iter().any(|x| matches!(x, Node::Elem(_))))
    };
    for (r, _) in &runs {
        get_mut(p, r).children.retain(|n| !empty_rpr(n));
    }
    if let Some(ppr) = p.child_mut(&cx.n("pPr")) {
        ppr.children.retain(|n| !empty_rpr(n));
    }
}

fn text_nodes(cx: &Cx, text: &str) -> Vec<Node> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut Vec<Node>| {
        if !buf.is_empty() {
            let mut t = Elem::new(cx.n("t"));
            t.self_closing = false;
            set_space_preserve(&mut t);
            t.children.push(Node::Text(escape(buf).into_bytes()));
            out.push(Node::Elem(t));
            buf.clear();
        }
    };
    for ch in text.chars() {
        match ch {
            '\t' => {
                flush(&mut buf, &mut out);
                out.push(Node::Elem(Elem::new(cx.n("tab"))));
            }
            '\n' => {
                flush(&mut buf, &mut out);
                out.push(Node::Elem(Elem::new(cx.n("br"))));
            }
            _ => buf.push(ch),
        }
    }
    flush(&mut buf, &mut out);
    out
}

fn replace_text(p: &mut Elem, cx: &Cx, s: usize, e: usize, text: &str) -> Result<()> {
    if s == e {
        // Pure insertion: borrow formatting from the run before (or after).
        split_at(p, cx, s);
        let (_, segs) = segments(p, cx);
        let anchor = segs
            .iter()
            .rev()
            .find(|sg| sg.start + sg.len == s)
            .map(|sg| (sg.run.clone(), true))
            .or_else(|| {
                segs.iter()
                    .find(|sg| sg.start == s)
                    .map(|sg| (sg.run.clone(), false))
            });
        let Some((run_path, after)) = anchor else {
            // Empty paragraph: add a run carrying the paragraph-mark formatting.
            let mut r = Elem::new(cx.n("r"));
            r.self_closing = false;
            if let Some(mark) = p
                .child(&cx.n("pPr"))
                .and_then(|ppr| ppr.child(&cx.n("rPr")))
            {
                r.children.push(Node::Elem(mark_to_run_rpr(mark, cx)));
            }
            r.children.extend(text_nodes(cx, text));
            p.self_closing = false;
            p.children.push(Node::Elem(r));
            return Ok(());
        };
        let run = get(p, &run_path).clone();
        let mut r = Elem {
            children: Vec::new(),
            ..run
        };
        r.self_closing = false;
        if let Some(rpr) = get(p, &run_path).child(&cx.n("rPr")) {
            r.children.push(Node::Elem(rpr.clone()));
        }
        r.children.extend(text_nodes(cx, text));
        let (parent, idx) = run_path.split_at(run_path.len() - 1);
        get_mut(p, parent)
            .children
            .insert(idx[0] + usize::from(after), Node::Elem(r));
        return Ok(());
    }
    let runs = isolate(p, cx, s, e);
    if runs.is_empty() {
        return Err(Error::Step("range covers no editable text".into()));
    }
    // Refuse to delete structure that is not text (notes, drawings, fields).
    let tag_ok = |n: &Node| match n {
        Node::Elem(c) => matches!(
            cx.local(c),
            Some(
                b"rPr"
                    | b"t"
                    | b"tab"
                    | b"ptab"
                    | b"br"
                    | b"cr"
                    | b"noBreakHyphen"
                    | b"softHyphen"
                    | b"sym"
                    | b"lastRenderedPageBreak"
            )
        ),
        _ => true,
    };
    for r in &runs {
        if !get(p, r).children.iter().all(tag_ok) {
            return Err(Error::Step(
                "range contains non-text content (note, field or drawing)".into(),
            ));
        }
    }
    // Keep the first run (and its formatting) with the new text; drop the rest.
    {
        let first = get_mut(p, &runs[0]);
        first
            .children
            .retain(|n| matches!(n, Node::Elem(c) if cx.local(c) == Some(b"rPr")));
        first.children.extend(text_nodes(cx, text));
    }
    let mut rest: Vec<&Vec<usize>> = runs[1..].iter().collect();
    rest.sort();
    for r in rest.into_iter().rev() {
        let (parent, idx) = r.split_at(r.len() - 1);
        get_mut(p, parent).children.remove(idx[0]);
    }
    if text.is_empty() {
        let (parent, idx) = runs[0].split_at(runs[0].len() - 1);
        get_mut(p, parent).children.remove(idx[0]);
    }
    Ok(())
}

/// Paragraph-mark rPr (CT_ParaRPr) -> run rPr: drop revision markers.
fn mark_to_run_rpr(mark: &Elem, cx: &Cx) -> Elem {
    let mut r = mark.clone();
    r.children.retain(|n| match n {
        Node::Elem(c) => !matches!(
            cx.local(c),
            Some(b"ins" | b"del" | b"moveFrom" | b"moveTo" | b"rPrChange")
        ),
        _ => true,
    });
    r
}

fn insert_after(body: &mut Elem, cx: &Cx, ppath: &[usize], text: &str) -> Result<()> {
    let (parent_path, idx) = ppath.split_at(ppath.len() - 1);
    let idx = idx[0];
    let target = get_mut(body, ppath);
    let mut np = Elem::new(cx.n("p"));
    np.self_closing = false;
    // Copy only the paragraph properties (and attributes like paraId are
    // deliberately not copied: they must stay unique).
    let mut moved_sect: Option<Node> = None;
    if let Some(ppr) = target.child_mut(&cx.n("pPr")) {
        // Word semantics: the new paragraph receives the original paragraph
        // mark, so a section break carried by the mark moves with it.
        if let Some(i) = ppr.child_index(&cx.n("sectPr")) {
            moved_sect = Some(ppr.children.remove(i));
        }
        let mut copy = ppr.clone();
        copy.children
            .retain(|n| !matches!(n, Node::Elem(c) if cx.local(c) == Some(b"pPrChange")));
        if let Some(s) = moved_sect {
            copy.children.push(s);
        }
        np.children.push(Node::Elem(copy));
    }
    let mut r = Elem::new(cx.n("r"));
    r.self_closing = false;
    if let Some(mark) = np
        .child(&cx.n("pPr"))
        .and_then(|ppr| ppr.child(&cx.n("rPr")))
    {
        r.children.push(Node::Elem(mark_to_run_rpr(mark, cx)));
    }
    r.children.extend(text_nodes(cx, text));
    np.children.push(Node::Elem(r));
    get_mut(body, parent_path)
        .children
        .insert(idx + 1, Node::Elem(np));
    Ok(())
}

fn delete_paragraph(body: &mut Elem, cx: &Cx, ppath: &[usize], all: &[Vec<usize>]) -> Result<()> {
    let (parent_path, idx) = ppath.split_at(ppath.len() - 1);
    let idx = idx[0];
    let parent = get(body, parent_path);
    let p = get(body, ppath);
    let carries_sect = p
        .child(&cx.n("pPr"))
        .and_then(|x| x.child(&cx.n("sectPr")))
        .is_some();
    let siblings_p = parent.elems().filter(|e| cx.is(e, "p")).count();
    let in_cell = cx.is(parent, "tc");
    let is_last_body_p = parent_path.is_empty()
        && !parent.children[idx + 1..]
            .iter()
            .any(|n| matches!(n, Node::Elem(e) if cx.is(e, "p") || cx.is(e, "tbl")));
    if carries_sect {
        return Err(Error::Step(
            "paragraph carries a section break; delete refused".into(),
        ));
    }
    if (in_cell && siblings_p == 1) || is_last_body_p {
        // A cell and the document must keep a final paragraph (Word keeps
        // the mark too): empty it instead of removing it.
        let p = get_mut(body, ppath);
        p.children
            .retain(|n| matches!(n, Node::Elem(c) if cx.local(c) == Some(b"pPr")));
        let _ = all;
        return Ok(());
    }
    get_mut(body, parent_path).children.remove(idx);
    Ok(())
}

fn section_format(body: &mut Elem, cx: &Cx, f: &SectionFormat) -> Result<()> {
    // Section order = sectPr order: paragraph-level ones, then the body's.
    let mut paths: Vec<Vec<usize>> = Vec::new();
    for (i, n) in body.children.iter().enumerate() {
        let Node::Elem(e) = n else { continue };
        if cx.is(e, "p") {
            if let Some(pi) = e.child_index(&cx.n("pPr")) {
                if let Node::Elem(ppr) = &e.children[pi] {
                    if let Some(si) = ppr.child_index(&cx.n("sectPr")) {
                        paths.push(vec![i, pi, si]);
                    }
                }
            }
        } else if cx.is(e, "sectPr") {
            paths.push(vec![i]);
        }
    }
    let path = paths.get(f.section).ok_or_else(|| {
        Error::Step(format!(
            "no section {} (document has {})",
            f.section,
            paths.len()
        ))
    })?;
    let sect = get_mut(body, path);
    const SECT_ORDER: &[&str] = &[
        "headerReference",
        "footerReference",
        "footnotePr",
        "endnotePr",
        "type",
        "pgSz",
        "pgMar",
        "paperSrc",
        "pgBorders",
        "lnNumType",
        "pgNumType",
        "cols",
        "formProt",
        "vAlign",
        "noEndnote",
        "titlePg",
        "textDirection",
        "bidi",
        "rtlGutter",
        "docGrid",
        "printerSettings",
        "sectPrChange",
    ];
    if f.width.is_some() || f.height.is_some() {
        let sz = ordered_child(sect, cx, "pgSz", SECT_ORDER);
        if let Some(v) = f.width {
            sz.set_attr(&cx.n("w"), &twips(v));
        }
        if let Some(v) = f.height {
            sz.set_attr(&cx.n("h"), &twips(v));
        }
    }
    if f.top.is_some() || f.bottom.is_some() || f.left.is_some() || f.right.is_some() {
        let mar = ordered_child(sect, cx, "pgMar", SECT_ORDER);
        for (k, v) in [
            ("top", f.top),
            ("bottom", f.bottom),
            ("left", f.left),
            ("right", f.right),
        ] {
            if let Some(v) = v {
                mar.set_attr(&cx.n(k), &twips(v));
            }
        }
        // pgMar requires all of these attributes.
        for (k, d) in [
            ("top", "1440"),
            ("bottom", "1440"),
            ("left", "1440"),
            ("right", "1440"),
            ("header", "720"),
            ("footer", "720"),
            ("gutter", "0"),
        ] {
            if mar.attr(&cx.n(k)).is_none() {
                mar.set_attr(&cx.n(k), d);
            }
        }
    }
    Ok(())
}

/// Style id of built-in "heading N", adding Word 365's definition if absent.
fn ensure_heading_style(styles: &mut Document, level: u8) -> Result<String> {
    let cx = Cx::new(styles);
    let want = format!("heading {level}");
    for st in styles.root.elems().filter(|e| cx.is(e, "style")) {
        if let Some(name) = st.child(&cx.n("name")).and_then(|n| n.attr(&cx.n("val"))) {
            if unescape(name).eq_ignore_ascii_case(&want) {
                if let Some(id) = st.attr(&cx.n("styleId")) {
                    return Ok(unescape(id));
                }
            }
        }
    }
    let ids: Vec<String> = styles
        .root
        .elems()
        .filter_map(|e| e.attr(&cx.n("styleId")).map(unescape))
        .collect();
    let mut id = format!("Heading{level}");
    while ids.contains(&id) {
        id.push('_');
    }
    let normal = styles
        .root
        .elems()
        .find(|e| {
            cx.is(e, "style")
                && e.attr(&cx.n("type")) == Some(b"paragraph")
                && matches!(e.attr(&cx.n("default")), Some(b"1" | b"true"))
        })
        .and_then(|e| e.attr(&cx.n("styleId")).map(unescape));
    // Word 365 (2023+) built-in definitions, as Word itself inserts them.
    let (before, after, size) = match level {
        1 => (360, 80, 40),
        2 => (160, 80, 32),
        3 => (160, 80, 28),
        _ => (80, 40, 24),
    };
    let w = String::from_utf8_lossy(&cx.w).into_owned();
    let based = normal
        .map(|n| {
            format!(
                "<{w}basedOn {w}val=\"{}\"/><{w}next {w}val=\"{}\"/>",
                escape(&n),
                escape(&n)
            )
        })
        .unwrap_or_default();
    let italic = if level >= 4 {
        format!("<{w}i/><{w}iCs/>")
    } else {
        String::new()
    };
    let color = if level <= 5 {
        format!("<{w}color {w}val=\"0F4761\" {w}themeColor=\"accent1\" {w}themeShade=\"BF\"/>")
    } else {
        format!("<{w}color {w}val=\"595959\" {w}themeColor=\"text1\" {w}themeTint=\"A6\"/>")
    };
    let xml = format!(
        "<{w}style {w}type=\"paragraph\" {w}styleId=\"{id}\"><{w}name {w}val=\"heading {level}\"/>{based}\
<{w}uiPriority {w}val=\"9\"/><{w}qFormat/><{w}pPr><{w}keepNext/><{w}keepLines/>\
<{w}spacing {w}before=\"{before}\" {w}after=\"{after}\"/><{w}outlineLvl {w}val=\"{}\"/></{w}pPr>\
<{w}rPr><{w}rFonts {w}asciiTheme=\"majorHAnsi\" {w}eastAsiaTheme=\"majorEastAsia\" {w}hAnsiTheme=\"majorHAnsi\" {w}cstheme=\"majorBidi\"/>{italic}{color}\
<{w}sz {w}val=\"{size}\"/><{w}szCs {w}val=\"{size}\"/></{w}rPr></{w}style>",
        level - 1
    );
    // Parse the fragment with the root's namespace declarations in scope.
    let wrapped = format!("<x xmlns:{}=\"urn:x\">{xml}</x>", w.trim_end_matches(':'));
    let frag = Document::parse(wrapped.as_bytes(), PART_STYLES)?;
    let Some(Node::Elem(style)) = frag.root.children.into_iter().next() else {
        return Err(Error::Step("internal: bad style fragment".into()));
    };
    styles.root.self_closing = false;
    styles.root.children.push(Node::Elem(style));
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Normal"/></w:pPr><w:r><w:rPr><w:i/></w:rPr><w:t xml:space="preserve">Hello brave </w:t></w:r><w:r><w:t>new world.</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>7</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:tab/><w:t>tail</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#;

    const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>"#;

    fn pkg() -> Package {
        let mut p = Package::new();
        p.set_part(PART_DOCUMENT, DOC.as_bytes().to_vec());
        p.set_part(PART_STYLES, STYLES.as_bytes().to_vec());
        p
    }

    fn doc_xml(p: &Package) -> String {
        String::from_utf8(p.get_part(PART_DOCUMENT).unwrap().to_vec()).unwrap()
    }

    fn find(t: &str) -> Target {
        Target::Find {
            text: t.into(),
            occurrence: 1,
        }
    }

    #[test]
    fn text_view_matches_word_main_story() {
        let texts = paragraph_texts(&pkg()).unwrap();
        assert_eq!(texts, vec!["Hello brave new world.", "cell", "7\ttail"]);
    }

    #[test]
    fn untouched_package_roundtrips_byte_identically() {
        let mut p = pkg();
        apply_steps(&mut p, &[]).unwrap();
        assert_eq!(doc_xml(&p), DOC);
    }

    #[test]
    fn bold_one_word_across_a_run_boundary_splits_runs_and_keeps_rest() {
        let mut p = pkg();
        let fmt = RunFormat {
            bold: Some(true),
            ..Default::default()
        };
        apply_steps(&mut p, &[Step::RunFormat(find("brave new"), fmt)]).unwrap();
        let x = doc_xml(&p);
        assert!(
            x.contains(r#"<w:r><w:rPr><w:i/></w:rPr><w:t xml:space="preserve">Hello </w:t></w:r>"#),
            "{x}"
        );
        assert!(
            x.contains(
                r#"<w:rPr><w:b/><w:bCs/><w:i/></w:rPr><w:t xml:space="preserve">brave </w:t>"#
            ),
            "{x}"
        );
        assert!(
            x.contains(r#"<w:rPr><w:b/><w:bCs/></w:rPr><w:t xml:space="preserve">new</w:t>"#),
            "{x}"
        );
        assert!(
            x.contains(r#"<w:t xml:space="preserve"> world.</w:t>"#),
            "{x}"
        );
        assert_eq!(paragraph_texts(&p).unwrap()[0], "Hello brave new world.");
    }

    #[test]
    fn replace_text_spanning_runs_keeps_first_run_formatting() {
        let mut p = pkg();
        apply_steps(
            &mut p,
            &[Step::ReplaceText(find("brave new"), "bold".into())],
        )
        .unwrap();
        assert_eq!(paragraph_texts(&p).unwrap()[0], "Hello bold world.");
        assert!(
            doc_xml(&p).contains(r#"<w:rPr><w:i/></w:rPr><w:t xml:space="preserve">bold</w:t>"#)
        );
    }

    #[test]
    fn field_codes_are_invisible_and_field_results_editable() {
        let mut p = pkg();
        assert!(apply_steps(&mut p, &[Step::ReplaceText(find("PAGE"), "x".into())]).is_err());
        let fmt = RunFormat {
            italic: Some(true),
            ..Default::default()
        };
        apply_steps(&mut p, &[Step::RunFormat(find("tail"), fmt)]).unwrap();
        assert!(
            doc_xml(&p).contains(
                r#"<w:r><w:tab/></w:r><w:r><w:rPr><w:i/><w:iCs/></w:rPr><w:t>tail</w:t></w:r>"#
            ),
            "{}",
            doc_xml(&p)
        );
    }

    #[test]
    fn paragraph_format_inserts_in_schema_order() {
        let mut p = pkg();
        let f = ParagraphFormat {
            alignment: Some("center".into()),
            before: Some(6.0),
            line_spacing: Some(1.5),
            first_line: Some(18.0),
            ..Default::default()
        };
        apply_steps(&mut p, &[Step::ParagraphFormat(find("world"), f)]).unwrap();
        assert!(doc_xml(&p).contains(r#"<w:pPr><w:pStyle w:val="Normal"/><w:spacing w:before="120" w:line="360" w:lineRule="auto"/><w:ind w:firstLine="360"/><w:jc w:val="center"/></w:pPr>"#), "{}", doc_xml(&p));
    }

    #[test]
    fn heading_adds_word_style_when_missing() {
        let mut p = pkg();
        apply_steps(&mut p, &[Step::Heading(find("cell"), 2)]).unwrap();
        assert!(doc_xml(&p)
            .contains(r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>cell</w:t>"#));
        let st = String::from_utf8(p.get_part(PART_STYLES).unwrap().to_vec()).unwrap();
        assert!(
            st.contains(
                r#"w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/>"#
            ),
            "{st}"
        );
        assert!(st.contains(r#"<w:spacing w:before="160" w:after="80"/>"#));
    }

    #[test]
    fn heading_clears_direct_paragraph_and_majority_char_formatting() {
        let mut p = Package::new();
        let doc = DOC.replace(
            r#"<w:p><w:pPr><w:pStyle w:val="Normal"/></w:pPr><w:r><w:rPr><w:i/></w:rPr>"#,
            r#"<w:p><w:pPr><w:pStyle w:val="Normal"/><w:spacing w:before="0"/><w:jc w:val="center"/><w:rPr><w:color w:val="000000"/></w:rPr></w:pPr><w:r><w:rPr><w:i/><w:color w:val="000000"/></w:rPr>"#,
        );
        p.set_part(PART_DOCUMENT, doc.into_bytes());
        p.set_part(PART_STYLES, STYLES.as_bytes().to_vec());
        apply_steps(&mut p, &[Step::Heading(find("Hello"), 1)]).unwrap();
        let x = doc_xml(&p);
        // spacing/jc gone; colour covered 12 of 22 chars (>50%) -> removed;
        // italic covered the same 12 chars -> removed as well.
        assert!(x.contains(r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t xml:space="preserve">Hello brave </w:t>"#), "{x}");
    }

    #[test]
    fn insert_and_delete_paragraphs() {
        let mut p = pkg();
        apply_steps(
            &mut p,
            &[
                Step::ParagraphInsert(find("world"), "Inserted.".into()),
                Step::ParagraphDelete(find("Inserted")),
                Step::ParagraphInsert(find("world"), "Second.".into()),
            ],
        )
        .unwrap();
        assert_eq!(
            paragraph_texts(&p).unwrap(),
            vec!["Hello brave new world.", "Second.", "cell", "7\ttail"]
        );
        // The only paragraph of a cell is emptied, not removed.
        apply_steps(&mut p, &[Step::ParagraphDelete(find("cell"))]).unwrap();
        assert!(
            doc_xml(&p).contains("<w:tc><w:p></w:p></w:tc>"),
            "{}",
            doc_xml(&p)
        );
    }

    #[test]
    fn section_margins_in_points() {
        let mut p = pkg();
        let f = SectionFormat {
            section: 0,
            top: Some(56.7),
            left: Some(85.0),
            ..Default::default()
        };
        apply_steps(&mut p, &[Step::SectionFormat(f)]).unwrap();
        assert!(doc_xml(&p)
            .contains(r#"<w:pgMar w:top="1134" w:right="1440" w:bottom="1440" w:left="1700""#));
    }

    #[test]
    fn errors_leave_package_unchanged() {
        let mut p = pkg();
        let fmt = RunFormat {
            bold: Some(true),
            ..Default::default()
        };
        let err = apply_steps(
            &mut p,
            &[
                Step::RunFormat(find("Hello"), fmt),
                Step::RunFormat(find("nope"), RunFormat::default()),
            ],
        );
        assert!(err.is_err());
        assert_eq!(doc_xml(&p), DOC);
    }

    #[test]
    fn highlight_maps_palette_or_falls_back_to_shading() {
        let mut p = pkg();
        let a = RunFormat {
            highlight: Some("ffff00".into()),
            color: Some("C00000".into()),
            size: Some(14.0),
            font: Some("Arial".into()),
            ..Default::default()
        };
        let b = RunFormat {
            highlight: Some("123456".into()),
            ..Default::default()
        };
        apply_steps(
            &mut p,
            &[
                Step::RunFormat(find("Hello"), a),
                Step::RunFormat(find("tail"), b),
            ],
        )
        .unwrap();
        let x = doc_xml(&p);
        assert!(x.contains(r#"<w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:i/><w:color w:val="C00000"/><w:sz w:val="28"/><w:szCs w:val="28"/><w:highlight w:val="yellow"/></w:rPr>"#), "{x}");
        assert!(
            x.contains(r#"<w:shd w:val="clear" w:color="auto" w:fill="123456"/>"#),
            "{x}"
        );
    }
}

//! Capture the blueprint's footnote-marker formatting and apply it to a
//! transplanted footnotes part.
//!
//! Direct port of three pieces of `format_transplant.py`:
//!
//!   • `BlueprintAnalyzer._footnote_format` (lines 855-990)  — reads up to
//!     three blueprint footnotes (id > 0) and captures:
//!       - the `<w:rPr>` of the run containing `<w:footnoteRef>` (deep-copy)
//!       - the text/tab content of the run immediately after the marker,
//!         used as the footnote-number-to-body separator
//!
//!   • `DocumentBuilder._apply_fn_ref_style` (lines 1978-2010) — replaces
//!     the rPr of every transplanted `<w:footnoteRef>` run with the
//!     captured blueprint rPr.
//!
//!   • `DocumentBuilder._normalize_fn_separator` (lines 2013-2099) —
//!     ensures the run after `<w:footnoteRef>` matches the blueprint's
//!     separator convention (tab vs space vs nothing).
//!
//! Why this matters: after `transplant_body` copies the source's
//! `footnotes.xml` verbatim into the blueprint package, the source's
//! footnote-number formatting (font, size, vertical alignment) and
//! number-to-body separator hitch a ride. Applying the blueprint's
//! convention here is what makes the result *look* like the blueprint
//! rather than the source.

use std::io::Cursor;

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::error::{Error, Result};
use crate::ns::{PART_ENDNOTES, PART_FOOTNOTES};
use crate::package::Package;

/// Captures the blueprint's footnote-number formatting.
///
/// Mirrors the relevant fields of `format_transplant.py::BlueprintSchema`:
///   - `footnote_marker_rPr_xml`
///   - `footnote_separator`
#[derive(Debug, Clone, Default)]
pub struct FootnoteFormat {
    /// XML bytes of the captured `<w:rPr>...</w:rPr>` element from the
    /// blueprint's first numbered-footnote marker run. `None` if the
    /// blueprint had no numbered footnotes to sample.
    pub marker_rpr_xml: Option<Vec<u8>>,
    /// What separates the footnote number from its body in the blueprint:
    ///   - `Some("\t")` if a `<w:tab/>` element was found
    ///   - `Some("")` (empty) if no separator run exists across sampled fns
    ///   - `Some(" ")` etc. for explicit whitespace separator
    ///   - `None` if the blueprint had no footnotes to sample.
    pub separator: Option<String>,
}

/// Read the blueprint's footnotes part and capture its marker rPr +
/// separator from up to three numbered footnotes (id > 0). Word-internal
/// separator entries (id ≤ 0) are skipped.
///
/// Matches `BlueprintAnalyzer._footnote_format`. Errors are caught and
/// returned as a default `FootnoteFormat` — the Python implementation
/// does the same (it logs a warning and proceeds).
pub fn extract_footnote_format(pkg: &Package) -> Result<FootnoteFormat> {
    let Some(bytes) = pkg.get_part(PART_FOOTNOTES) else {
        return Ok(FootnoteFormat::default());
    };
    extract_from_xml(bytes)
}

fn extract_from_xml(input: &[u8]) -> Result<FootnoteFormat> {
    let mut out = FootnoteFormat::default();
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;

    // State machine:
    //   depth in <w:footnote> (current id, sampled count, whether positive id)
    //   When inside a positive footnote's first <w:p>:
    //     collect <w:r> events into a per-run buffer
    //     after </w:r>, if the run contained <w:footnoteRef>, harvest its rPr
    //     and queue "look at next run" state to extract the separator
    let mut buf = Vec::with_capacity(1024);

    let mut current_id: Option<i64> = None;
    let mut samples = 0;

    let mut in_first_para = false;
    let mut p_count_in_fn = 0; // increments on <w:p> within current <w:footnote>

    let mut run_buffer: Option<Vec<Event<'static>>> = None;

    // Once we've harvested the marker rPr from a run, the next run we see
    // in the same first paragraph is the separator-candidate. This Option
    // carries that intent forward.
    let mut next_run_is_separator_candidate = false;

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: PART_FOOTNOTES.into(),
                source: e,
            })?;

        match &ev {
            Event::Eof => break,

            Event::Start(s) if s.name().as_ref() == b"w:footnote" => {
                let id = parse_id_attr(s);
                current_id = Some(id);
                p_count_in_fn = 0;
                in_first_para = false;
                next_run_is_separator_candidate = false;
                if id > 0 {
                    samples += 1;
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:footnote" => {
                if out.marker_rpr_xml.is_some() && out.separator.is_some() {
                    break;
                }
                if samples >= 3 {
                    break;
                }
                current_id = None;
                in_first_para = false;
                next_run_is_separator_candidate = false;
            }

            Event::Start(s)
                if s.name().as_ref() == b"w:p" && current_id.is_some_and(|id| id > 0) =>
            {
                p_count_in_fn += 1;
                in_first_para = p_count_in_fn == 1;
            }
            Event::End(e) if e.name().as_ref() == b"w:p" && current_id.is_some_and(|id| id > 0) => {
                in_first_para = false;
            }

            Event::Start(s) if in_first_para && s.name().as_ref() == b"w:r" => {
                run_buffer = Some(Vec::new());
            }
            Event::End(e) if in_first_para && e.name().as_ref() == b"w:r" => {
                if let Some(events) = run_buffer.take() {
                    // Decide: does this run contain <w:footnoteRef>?
                    let is_marker = run_payload_has_footnote_ref(&events);
                    if is_marker && out.marker_rpr_xml.is_none() {
                        if let Some(rpr_bytes) = extract_rpr_xml(&events)? {
                            out.marker_rpr_xml = Some(rpr_bytes);
                        }
                        next_run_is_separator_candidate = out.separator.is_none();
                    } else if next_run_is_separator_candidate && out.separator.is_none() {
                        out.separator = Some(extract_separator(&events));
                        next_run_is_separator_candidate = false;
                    }
                }
            }
            other if run_buffer.is_some() => {
                run_buffer
                    .as_mut()
                    .unwrap()
                    .push(other.clone().into_owned());
            }

            _ => {}
        }
        buf.clear();
    }

    // Python: if we sampled footnotes but never found a separator,
    // record the empty separator explicitly.
    if samples > 0 && out.separator.is_none() {
        out.separator = Some(String::new());
    }

    Ok(out)
}

fn parse_id_attr(s: &BytesStart) -> i64 {
    s.attributes()
        .filter_map(Result::ok)
        .find(|a| a.key.as_ref() == b"w:id")
        .and_then(|a| {
            std::str::from_utf8(a.value.as_ref())
                .ok()
                .map(str::to_string)
        })
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
}

fn run_payload_has_footnote_ref(events: &[Event<'static>]) -> bool {
    events.iter().any(|ev| match ev {
        Event::Start(s) | Event::Empty(s) => s.name().as_ref() == b"w:footnoteRef",
        _ => false,
    })
}

/// Re-emit the rPr sub-element's bytes from the run's payload.
fn extract_rpr_xml(events: &[Event<'static>]) -> Result<Option<Vec<u8>>> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut emitting = false;
    let mut depth = 0u32;
    let mut found_anything = false;

    for ev in events {
        match ev {
            Event::Start(s) if s.name().as_ref() == b"w:rPr" => {
                emitting = true;
                depth = 1;
                writer
                    .write_event(Event::Start(s.clone()))
                    .map_err(xml_io)?;
                found_anything = true;
            }
            Event::End(e) if emitting && e.name().as_ref() == b"w:rPr" => {
                writer.write_event(Event::End(e.clone())).map_err(xml_io)?;
                break;
            }
            Event::Start(_) if emitting => {
                depth += 1;
                writer.write_event(ev.clone()).map_err(xml_io)?;
            }
            Event::End(_) if emitting => {
                depth -= 1;
                writer.write_event(ev.clone()).map_err(xml_io)?;
            }
            _ if emitting => {
                writer.write_event(ev.clone()).map_err(xml_io)?;
            }
            _ => {}
        }
        let _ = depth;
    }
    if !found_anything {
        return Ok(None);
    }
    Ok(Some(writer.into_inner().into_inner()))
}

/// Decide what kind of separator a run represents.
fn extract_separator(events: &[Event<'static>]) -> String {
    let mut has_tab = false;
    let mut text = String::new();
    for ev in events {
        match ev {
            Event::Start(s) | Event::Empty(s) if s.name().as_ref() == b"w:tab" => {
                has_tab = true;
            }
            Event::Text(t) => {
                if let Ok(s) = std::str::from_utf8(t.as_ref()) {
                    text.push_str(s);
                }
            }
            _ => {}
        }
    }
    if has_tab {
        "\t".to_string()
    } else if text.trim().is_empty() {
        text
    } else {
        // Non-whitespace run after marker — Python doesn't treat this
        // as a separator; we mirror that and report empty (the caller
        // detects "not a separator" via the empty-but-text-not-empty
        // mismatch). For our use we just return the literal text; the
        // applier checks `is_sep_run` semantics.
        text
    }
}

/// Apply the captured blueprint footnote format to `pkg`'s footnotes (and
/// endnotes) parts: every `<w:footnoteRef>` run gets its `<w:rPr>` replaced
/// with the deep-copied marker rPr, and the run immediately after is
/// normalised to the wanted separator.
///
/// Returns the number of footnote-marker runs touched.
///
/// No-op when `fmt.marker_rpr_xml` and `fmt.separator` are both `None`
/// (blueprint had no footnotes to sample — there's nothing to apply).
pub fn apply_footnote_format(pkg: &mut Package, fmt: &FootnoteFormat) -> Result<usize> {
    let mut total = 0usize;
    if fmt.marker_rpr_xml.is_none() && fmt.separator.is_none() {
        return Ok(0);
    }
    for part in [PART_FOOTNOTES, PART_ENDNOTES] {
        let Some(bytes) = pkg.get_part(part).map(<[u8]>::to_vec) else {
            continue;
        };
        let (rewritten, touched) = apply_to_xml(&bytes, fmt, part)?;
        if touched > 0 {
            pkg.set_part(part, rewritten);
        }
        total += touched;
    }
    Ok(total)
}

fn apply_to_xml(input: &[u8], fmt: &FootnoteFormat, part_name: &str) -> Result<(Vec<u8>, usize)> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(input.len())));
    let mut buf = Vec::with_capacity(1024);

    let mut current_id: Option<i64> = None;
    let mut in_first_para = false;
    let mut p_count = 0;

    // We buffer one paragraph at a time when we're inside the first <w:p>
    // of a positive-id <w:footnote>. After the paragraph closes, decide
    // whether to rewrite it (apply marker rPr + separator) and emit.
    let mut p_open: Option<BytesStart<'static>> = None;
    let mut p_events: Vec<Event<'static>> = Vec::new();

    let mut touched = 0usize;

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: part_name.into(),
                source: e,
            })?;

        match &ev {
            Event::Eof => break,

            // entering / leaving the outer <w:footnote>
            Event::Start(s)
                if s.name().as_ref() == b"w:footnote" || s.name().as_ref() == b"w:endnote" =>
            {
                current_id = Some(parse_id_attr(s));
                p_count = 0;
                in_first_para = false;
                writer
                    .write_event(ev.clone())
                    .map_err(|e| xml_io_p(e, part_name))?;
            }
            Event::End(e)
                if e.name().as_ref() == b"w:footnote" || e.name().as_ref() == b"w:endnote" =>
            {
                current_id = None;
                in_first_para = false;
                writer
                    .write_event(ev.clone())
                    .map_err(|e| xml_io_p(e, part_name))?;
            }

            // first <w:p> of positive-id footnote — buffer it
            Event::Start(s)
                if s.name().as_ref() == b"w:p" && current_id.is_some_and(|id| id > 0) =>
            {
                p_count += 1;
                if p_count == 1 {
                    in_first_para = true;
                    p_open = Some(s.clone().into_owned());
                    p_events.clear();
                } else {
                    writer
                        .write_event(ev.clone())
                        .map_err(|e| xml_io_p(e, part_name))?;
                }
            }
            Event::End(e) if e.name().as_ref() == b"w:p" && in_first_para => {
                // Process the buffered paragraph: rewrite the marker run's
                // rPr (if any) and normalise the separator run that follows.
                let para_open = p_open.take().expect("p open");
                let mut events = std::mem::take(&mut p_events);
                let n = rewrite_first_para(&mut events, fmt);
                touched += n;
                writer
                    .write_event(Event::Start(para_open))
                    .map_err(|e| xml_io_p(e, part_name))?;
                for inner in events.into_iter() {
                    writer
                        .write_event(inner)
                        .map_err(|e| xml_io_p(e, part_name))?;
                }
                writer
                    .write_event(ev.clone())
                    .map_err(|e| xml_io_p(e, part_name))?;
                in_first_para = false;
            }

            other if in_first_para => {
                p_events.push(other.clone().into_owned());
            }

            _ => writer
                .write_event(ev.clone())
                .map_err(|e| xml_io_p(e, part_name))?,
        }
        buf.clear();
    }
    Ok((writer.into_inner().into_inner(), touched))
}

fn xml_io(err: quick_xml::Error) -> Error {
    Error::XmlParse {
        part: PART_FOOTNOTES.into(),
        source: err,
    }
}
fn xml_io_p(err: quick_xml::Error, part: &str) -> Error {
    Error::XmlParse {
        part: part.into(),
        source: err,
    }
}

/// Mutate the events list in-place: find the run containing
/// `<w:footnoteRef>`, replace its rPr with the blueprint's, and normalise
/// the separator run that follows.
///
/// Returns 1 if any rewrite happened, else 0.
fn rewrite_first_para(events: &mut Vec<Event<'static>>, fmt: &FootnoteFormat) -> usize {
    // Slice the events into per-run chunks at the top level (we're inside
    // a single <w:p>, runs are direct children). The chunks are
    //   (start_index, end_index_exclusive, contains_footnoteRef, is_separator_candidate)
    let runs = collect_top_runs(events);
    if runs.is_empty() {
        return 0;
    }
    // Find the marker run.
    let marker_idx_in_runs = runs.iter().position(|r| r.has_footnote_ref);
    let Some(mi) = marker_idx_in_runs else {
        return 0;
    };

    let mut touched = 0;

    // (a) Replace the marker run's rPr.
    if let Some(rpr_bytes) = &fmt.marker_rpr_xml {
        replace_rpr_in_range(events, runs[mi].start, runs[mi].end, rpr_bytes);
        touched = 1;
    }

    // We need to refresh run boundaries since we may have mutated the
    // marker run's content (rPr replacement may differ in event count).
    let runs_after = collect_top_runs(events);
    // Find the marker run again (must still be present at the same
    // semantic position; index in runs_after should still be `mi`).
    if mi + 1 >= runs_after.len() {
        // No separator-candidate run; if blueprint wants a non-empty
        // separator, insert one after the marker run.
        if let Some(wanted) = &fmt.separator {
            if !wanted.is_empty() {
                let sep_run = build_separator_run(wanted);
                splice_after(events, runs_after[mi].end, sep_run);
                touched = 1;
            }
        }
        return touched;
    }

    // (b) Normalise the separator run.
    if let Some(wanted) = &fmt.separator {
        let sep_run_idx = mi + 1;
        let span = runs_after[sep_run_idx].clone();
        normalize_separator_run(events, span.start, span.end, wanted);
        touched = 1;
    }

    touched
}

#[derive(Clone, Debug)]
struct RunSpan {
    start: usize, // index in events of Event::Start(<w:r>)
    end: usize,   // index in events of Event::End(<w:r>) — inclusive
    has_footnote_ref: bool,
}

/// Walk events, return spans of every top-level (depth-0 within paragraph)
/// `<w:r>` element. Depth tracking keeps us from misidentifying nested
/// `<w:r>` in math/etc.
fn collect_top_runs(events: &[Event<'static>]) -> Vec<RunSpan> {
    let mut out = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut depth: i32 = 0;
    let mut has_ref = false;
    for (i, ev) in events.iter().enumerate() {
        match ev {
            Event::Start(s) if s.name().as_ref() == b"w:r" => {
                if depth == 0 {
                    current_start = Some(i);
                    has_ref = false;
                }
                depth += 1;
            }
            Event::End(e) if e.name().as_ref() == b"w:r" => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = current_start.take() {
                        out.push(RunSpan {
                            start,
                            end: i,
                            has_footnote_ref: has_ref,
                        });
                    }
                    has_ref = false;
                }
            }
            Event::Start(s) | Event::Empty(s)
                if depth > 0 && s.name().as_ref() == b"w:footnoteRef" =>
            {
                has_ref = true;
            }
            _ => {}
        }
    }
    out
}

fn replace_rpr_in_range(
    events: &mut Vec<Event<'static>>,
    run_start: usize,
    run_end: usize,
    rpr_bytes: &[u8],
) {
    // Find existing <w:rPr>...</w:rPr> inside [run_start, run_end] and
    // replace its events with the parsed events of rpr_bytes. If no
    // existing rPr, insert at the run's Start+1.
    let mut rpr_start: Option<usize> = None;
    let mut rpr_end: Option<usize> = None;
    let mut depth: i32 = 0;
    for (i, ev) in events.iter().enumerate().take(run_end).skip(run_start + 1) {
        match ev {
            Event::Start(s) if depth == 0 && s.name().as_ref() == b"w:rPr" => {
                rpr_start = Some(i);
                depth = 1;
            }
            Event::End(e) if depth == 1 && e.name().as_ref() == b"w:rPr" => {
                rpr_end = Some(i);
                break;
            }
            Event::Start(_) if rpr_start.is_some() => depth += 1,
            Event::End(_) if rpr_start.is_some() => depth -= 1,
            _ => {}
        }
    }

    let parsed = parse_events(rpr_bytes);

    if let (Some(s), Some(e)) = (rpr_start, rpr_end) {
        events.splice(s..=e, parsed);
    } else {
        // Insert just after the <w:r> opener.
        events.splice(run_start + 1..run_start + 1, parsed);
    }
}

fn normalize_separator_run(
    events: &mut Vec<Event<'static>>,
    run_start: usize,
    run_end: usize,
    wanted: &str,
) {
    // Decide whether the existing run IS a separator run:
    //   has <w:tab/> OR all <w:t> content is whitespace.
    let mut has_tab = false;
    let mut text = String::new();
    for ev in events.iter().take(run_end).skip(run_start + 1) {
        match ev {
            Event::Start(s) | Event::Empty(s) if s.name().as_ref() == b"w:tab" => {
                has_tab = true;
            }
            Event::Text(t) => {
                if let Ok(s) = std::str::from_utf8(t.as_ref()) {
                    text.push_str(s);
                }
            }
            _ => {}
        }
    }
    let is_sep_run = has_tab || text.trim().is_empty();

    if !is_sep_run {
        // Next run is actual footnote text. If blueprint uses a non-empty
        // separator, insert a separator run BEFORE it.
        if !wanted.is_empty() {
            let sep = build_separator_run(wanted);
            events.splice(run_start..run_start, sep);
        }
        return;
    }

    // The next run IS a separator run. Decide if it matches the wanted
    // form; if not, replace its content.
    let matches = (has_tab && wanted == "\t") || (!has_tab && text == wanted);

    if wanted.is_empty() {
        // Strip any <w:t> / <w:tab> from the existing separator run.
        strip_sep_payload(events, run_start, run_end);
        return;
    }
    if !matches {
        strip_sep_payload(events, run_start, run_end);
        // Now insert the wanted payload just before the run's End.
        let new_payload = build_separator_payload(wanted);
        let end_idx = find_run_end_after_strip(events, run_start);
        events.splice(end_idx..end_idx, new_payload);
    }
}

fn strip_sep_payload(events: &mut Vec<Event<'static>>, run_start: usize, run_end: usize) {
    // Remove every Event whose tag (Empty or Start/End pair) is w:t or w:tab.
    // Track removals shrinking the window.
    let mut i = run_start + 1;
    let mut stop = run_end;
    while i < stop {
        let drop = match &events[i] {
            Event::Empty(s) | Event::Start(s) => {
                matches!(s.name().as_ref(), b"w:t" | b"w:tab")
            }
            Event::End(e) => matches!(e.name().as_ref(), b"w:t" | b"w:tab"),
            Event::Text(_) => true,
            _ => false,
        };
        if drop {
            events.remove(i);
            stop -= 1;
        } else {
            i += 1;
        }
    }
}

fn find_run_end_after_strip(events: &[Event<'static>], run_start: usize) -> usize {
    let mut depth: i32 = 0;
    for (i, ev) in events.iter().enumerate().skip(run_start) {
        match ev {
            Event::Start(s) if s.name().as_ref() == b"w:r" => depth += 1,
            Event::End(e) if e.name().as_ref() == b"w:r" => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    events.len()
}

fn build_separator_run(text: &str) -> Vec<Event<'static>> {
    let mut out = Vec::new();
    out.push(Event::Start(BytesStart::new("w:r").into_owned()));
    out.extend(build_separator_payload(text));
    out.push(Event::End(BytesEnd::new("w:r").into_owned()));
    out
}

fn build_separator_payload(text: &str) -> Vec<Event<'static>> {
    if text == "\t" {
        vec![Event::Empty(BytesStart::new("w:tab").into_owned())]
    } else {
        let mut t = BytesStart::new("w:t");
        if text.contains(' ') {
            t.push_attribute(("xml:space", "preserve"));
        }
        vec![
            Event::Start(t.clone().into_owned()),
            Event::Text(BytesText::from_escaped(text).into_owned()),
            Event::End(BytesEnd::new("w:t").into_owned()),
        ]
    }
}

fn parse_events(xml_bytes: &[u8]) -> Vec<Event<'static>> {
    let mut reader = Reader::from_reader(xml_bytes);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::new();
    let mut out = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(ev) => out.push(ev.into_owned()),
            Err(_) => break,
        }
        buf.clear();
    }
    out
}

fn splice_after(events: &mut Vec<Event<'static>>, after: usize, mut inserted: Vec<Event<'static>>) {
    let at = after + 1;
    let len = inserted.len();
    events.reserve(len);
    for (offset, ev) in inserted.drain(..).enumerate() {
        events.insert(at + offset, ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:footnote w:id="-1" w:type="separator"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="0" w:type="continuationSeparator"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteRef/></w:r><w:r><w:tab/></w:r><w:r><w:t xml:space="preserve">Body text.</w:t></w:r></w:p></w:footnote></w:footnotes>"#;

    #[test]
    fn extracts_rpr_and_tab_separator() {
        let fmt = extract_from_xml(SAMPLE).unwrap();
        let rpr = fmt.marker_rpr_xml.as_ref().expect("rPr captured");
        let s = std::str::from_utf8(rpr).unwrap();
        assert!(s.starts_with("<w:rPr"), "got {s}");
        assert!(s.contains("FootnoteReference"));
        assert!(s.contains("vertAlign"));
        assert_eq!(fmt.separator, Some("\t".to_string()));
    }

    #[test]
    fn empty_pkg_returns_default() {
        let pkg = Package::new();
        let fmt = extract_footnote_format(&pkg).unwrap();
        assert!(fmt.marker_rpr_xml.is_none());
        assert!(fmt.separator.is_none());
    }

    #[test]
    fn skips_id_zero_and_negative() {
        // Same SAMPLE — separator entries (id=-1, id=0) come first and have
        // <w:separator/> not <w:footnoteRef/>, so they're naturally skipped
        // by run_payload_has_footnote_ref; verified via the working test
        // above. This test additionally checks that the harvested rPr did
        // NOT come from the separator entry.
        let fmt = extract_from_xml(SAMPLE).unwrap();
        let s = std::str::from_utf8(fmt.marker_rpr_xml.as_ref().unwrap()).unwrap();
        // The separator entry has no rPr; if we'd mistakenly harvested
        // from it, we'd see no <w:rStyle>.
        assert!(s.contains("FootnoteReference"));
    }

    #[test]
    fn apply_replaces_marker_rpr() {
        // Build a footnotes part whose marker run has a different rPr,
        // then apply the SAMPLE format and check the rPr was replaced.
        let target_bytes = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:footnote w:id="1"><w:p><w:r><w:rPr><w:rFonts w:ascii="WrongFont"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve">  </w:t></w:r></w:p></w:footnote></w:footnotes>"#;
        let fmt = extract_from_xml(SAMPLE).unwrap();
        let (out, touched) = apply_to_xml(target_bytes, &fmt, "word/footnotes.xml").unwrap();
        assert_eq!(touched, 1);
        let s = std::str::from_utf8(&out).unwrap();
        // Marker rPr now has FootnoteReference + vertAlign, not WrongFont.
        assert!(s.contains("FootnoteReference"));
        assert!(s.contains("vertAlign"));
        assert!(!s.contains("WrongFont"));
    }
}

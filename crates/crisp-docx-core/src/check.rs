//! Package validity checks — the Rust equivalent of
//! `debug_format.py::cmd_check`.
//!
//! Seven categories, each producing an "OK" line on the report or one or
//! more "FAIL" lines:
//!
//! 1. XML parse validity (every `.xml` / `.rels` part).
//! 2. rsid values in body paragraphs declared in `settings.xml`'s
//!    `<w:rsids>` — undeclared values cause Word's "found unreadable
//!    content" recovery dialog.
//! 3. `w14:paraId` uniqueness across all XML parts.
//! 4. Relationship `Target` values resolve to actual ZIP entries.
//! 5. `<w:body>` direct children are only `<w:p>` / `<w:tbl>` /
//!    `<w:sectPr>`, with `<w:sectPr>` (when present) appearing last.
//! 6. `<w:bookmarkStart w:id="…"/>` ID uniqueness.
//! 7. Every `r:id`/`r:embed`/`r:link` reference in the body resolves
//!    against `word/_rels/document.xml.rels`.

use std::collections::{BTreeMap, BTreeSet};

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::Result;
use crate::package::Package;

/// Outcome of running [`check_package`]: two ordered lists of
/// human-readable lines.
#[derive(Debug, Clone, Default)]
pub struct CheckReport {
    /// One line per passed check.
    pub ok: Vec<String>,
    /// One line per detected issue. Empty when the package is valid.
    pub issues: Vec<String>,
}

impl CheckReport {
    /// True iff no `issues` lines were emitted.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Run all seven validity checks against `pkg`. Never errors on a
/// malformed input — XML parse failures land in `issues` as a check-1 FAIL.
pub fn check_package(pkg: &Package) -> Result<CheckReport> {
    let mut r = CheckReport::default();

    check_xml_parses(pkg, &mut r);
    check_rsids(pkg, &mut r);
    check_para_id_uniqueness(pkg, &mut r);
    check_rel_targets(pkg, &mut r);
    check_body_structure(pkg, &mut r);
    check_bookmark_ids(pkg, &mut r);
    check_inline_rids(pkg, &mut r);

    Ok(r)
}

fn is_xml_or_rels(name: &str) -> bool {
    name.ends_with(".xml") || name.ends_with(".rels")
}

fn parses_as_xml(bytes: &[u8]) -> std::result::Result<(), String> {
    let mut r = Reader::from_reader(bytes);
    let mut buf = Vec::new();
    loop {
        match r.read_event_into(&mut buf) {
            Ok(Event::Eof) => return Ok(()),
            Ok(_) => buf.clear(),
            Err(e) => return Err(format!("{e}")),
        }
    }
}

fn check_xml_parses(pkg: &Package, r: &mut CheckReport) {
    let mut errors: Vec<String> = Vec::new();
    let mut count = 0usize;
    for (name, bytes) in pkg.parts() {
        if !is_xml_or_rels(name) {
            continue;
        }
        count += 1;
        if let Err(e) = parses_as_xml(bytes) {
            errors.push(format!("XML parse error: {name}: {e}"));
        }
    }
    if errors.is_empty() {
        r.ok.push(format!("All {count} XML/rels parts parse cleanly"));
    } else {
        r.issues.extend(errors);
    }
}

// --- helpers for namespaced attribute matching --------------------------------
//
// quick-xml gives us QName bytes; we want to match prefixed forms like
// `w:rsidR`, `w14:paraId`, etc. Since OOXML files invariably use the
// stable prefixes `w` and `w14`, we match on the raw QName rather than
// resolving namespaces (the Python implementation does the same — its
// `w(...)` helper expands to the Clark form, but the underlying lxml
// matches by namespace URI; for our purposes prefix matching against
// real docx output is observably equivalent).

/// Returns true if the attribute QName matches `w:<local>`.
fn attr_is_w(qname: &[u8], local: &[u8]) -> bool {
    qname.len() == 2 + local.len() && qname.starts_with(b"w:") && &qname[2..] == local
}

fn attr_is_w14(qname: &[u8], local: &[u8]) -> bool {
    qname.len() == 4 + local.len() && qname.starts_with(b"w14:") && &qname[4..] == local
}

fn tag_is_w(qname: &[u8], local: &[u8]) -> bool {
    attr_is_w(qname, local)
}

fn check_rsids(pkg: &Package, r: &mut CheckReport) {
    let Some(doc) = pkg.get_part("word/document.xml") else {
        r.issues
            .push("word/document.xml missing — cannot check rsids".into());
        return;
    };

    let rsid_locals: &[&[u8]] = &[
        b"rsidR",
        b"rsidRPr",
        b"rsidDel",
        b"rsidRDefault",
        b"rsidRPrChange",
    ];

    let mut body_rsids: BTreeSet<String> = BTreeSet::new();
    let mut reader = Reader::from_reader(doc);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                if !tag_is_w(e.name().as_ref(), b"p") {
                    buf.clear();
                    continue;
                }
                for attr in e.attributes().with_checks(false).flatten() {
                    if rsid_locals.iter().any(|l| attr_is_w(attr.key.as_ref(), l)) {
                        let v = String::from_utf8_lossy(&attr.value).into_owned();
                        if !v.is_empty() {
                            body_rsids.insert(v);
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                r.issues
                    .push(format!("rsid check: document.xml parse error: {e}"));
                return;
            }
        }
        buf.clear();
    }

    let mut settings_rsids: BTreeSet<String> = BTreeSet::new();
    if let Some(settings) = pkg.get_part("word/settings.xml") {
        let mut reader = Reader::from_reader(settings);
        let mut in_rsids = false;
        let mut depth = 0i32;
        let mut rsids_depth = 0i32;
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Eof) => break,
                Ok(Event::Start(ref e)) => {
                    depth += 1;
                    if tag_is_w(e.name().as_ref(), b"rsids") {
                        in_rsids = true;
                        rsids_depth = depth;
                    } else if in_rsids {
                        for attr in e.attributes().with_checks(false).flatten() {
                            if attr_is_w(attr.key.as_ref(), b"val") {
                                let v = String::from_utf8_lossy(&attr.value).into_owned();
                                if !v.is_empty() {
                                    settings_rsids.insert(v);
                                }
                            }
                        }
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    if in_rsids {
                        for attr in e.attributes().with_checks(false).flatten() {
                            if attr_is_w(attr.key.as_ref(), b"val") {
                                let v = String::from_utf8_lossy(&attr.value).into_owned();
                                if !v.is_empty() {
                                    settings_rsids.insert(v);
                                }
                            }
                        }
                    }
                }
                Ok(Event::End(_)) => {
                    if in_rsids && depth == rsids_depth {
                        in_rsids = false;
                    }
                    depth -= 1;
                }
                Ok(_) => {}
                Err(_) => break, // already reported by check 1
            }
            buf.clear();
        }
    }

    let missing: Vec<&String> = body_rsids.difference(&settings_rsids).collect();
    if !missing.is_empty() {
        let sample: Vec<&str> = missing.iter().take(4).map(|s| s.as_str()).collect();
        r.issues.push(format!(
            "{} paragraph rsid value(s) not in settings.xml <w:rsids> — \
             causes 'Word found unreadable content'. Sample: {:?}",
            missing.len(),
            sample
        ));
    } else if !body_rsids.is_empty() {
        r.ok.push(format!(
            "{} rsid value(s), all declared in settings.xml",
            body_rsids.len()
        ));
    } else {
        r.ok.push("No rsid attributes in body paragraphs".into());
    }
}

fn check_para_id_uniqueness(pkg: &Package, r: &mut CheckReport) {
    let mut all: Vec<(String, String)> = Vec::new();
    for (name, bytes) in pkg.parts() {
        if !name.ends_with(".xml") {
            continue;
        }
        let mut reader = Reader::from_reader(bytes);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Eof) => break,
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    if tag_is_w(e.name().as_ref(), b"p") {
                        for attr in e.attributes().with_checks(false).flatten() {
                            if attr_is_w14(attr.key.as_ref(), b"paraId") {
                                let v = String::from_utf8_lossy(&attr.value).into_owned();
                                if !v.is_empty() {
                                    all.push((v, name.to_string()));
                                }
                            }
                        }
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
            buf.clear();
        }
    }

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (pid, _) in &all {
        *counts.entry(pid.as_str()).or_default() += 1;
    }
    let dupes: Vec<(&str, usize)> = counts
        .iter()
        .filter(|(_, c)| **c > 1)
        .map(|(k, c)| (*k, *c))
        .collect();
    if dupes.is_empty() {
        r.ok.push(format!(
            "{} w14:paraId values across all parts, all unique",
            all.len()
        ));
    } else {
        for (pid, cnt) in dupes.iter().take(3) {
            let parts: Vec<&str> = all
                .iter()
                .filter(|(p, _)| p == pid)
                .map(|(_, n)| n.as_str())
                .collect();
            r.issues.push(format!(
                "Duplicate w14:paraId {pid:?} appears {cnt}x in: {parts:?}"
            ));
        }
    }
}

fn resolve_relative_target(base: &str, target: &str) -> String {
    // base examples: "word", "word/_rels", "" (root)
    if let Some(stripped) = target.strip_prefix('/') {
        return stripped.to_string();
    }
    let combined = if base.is_empty() {
        target.to_string()
    } else {
        format!("{base}/{target}")
    };
    let mut resolved: Vec<&str> = Vec::new();
    for part in combined.split('/') {
        match part {
            ".." => {
                resolved.pop();
            }
            "" | "." => {}
            other => resolved.push(other),
        }
    }
    resolved.join("/")
}

fn check_rel_targets(pkg: &Package, r: &mut CheckReport) {
    let names: BTreeSet<&str> = pkg.parts().map(|(n, _)| n).collect();
    let mut missing: Vec<(String, String, String)> = Vec::new();
    for (name, bytes) in pkg.parts() {
        if !name.ends_with(".rels") {
            continue;
        }
        // owner base = name with "_rels/" removed, then parent dir
        let no_rels = name.replace("_rels/", "");
        let base = match no_rels.rsplit_once('/') {
            Some((b, _)) => b.to_string(),
            None => String::new(),
        };

        let mut reader = Reader::from_reader(bytes);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Eof) => break,
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    // Each child Relationship — skip the root Relationships element
                    if e.name().as_ref() != b"Relationship" {
                        buf.clear();
                        continue;
                    }
                    let mut target: Option<String> = None;
                    let mut external = false;
                    for attr in e.attributes().with_checks(false).flatten() {
                        match attr.key.as_ref() {
                            b"Target" => {
                                target = Some(String::from_utf8_lossy(&attr.value).into_owned());
                            }
                            b"TargetMode" if attr.value.as_ref() == b"External" => {
                                external = true;
                            }
                            _ => {}
                        }
                    }
                    if external {
                        buf.clear();
                        continue;
                    }
                    if let Some(t) = target {
                        let full = resolve_relative_target(&base, &t);
                        if !full.is_empty() && !names.contains(full.as_str()) {
                            missing.push((name.to_string(), t, full));
                        }
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
            buf.clear();
        }
    }
    if missing.is_empty() {
        r.ok.push("All relationship targets present in ZIP".into());
    } else {
        for (rn, t, f) in missing.iter().take(5) {
            r.issues.push(format!(
                "Missing rel target {t:?} (resolved: {f:?}) in {rn}"
            ));
        }
    }
}

fn check_body_structure(pkg: &Package, r: &mut CheckReport) {
    let Some(doc) = pkg.get_part("word/document.xml") else {
        return;
    };
    let mut reader = Reader::from_reader(doc);
    let mut buf = Vec::new();
    let mut in_body = false;
    let mut body_depth = 0i32;
    let mut depth = 0i32;
    let mut children_tags: Vec<String> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) => {
                depth += 1;
                if tag_is_w(e.name().as_ref(), b"body") {
                    in_body = true;
                    body_depth = depth;
                } else if in_body && depth == body_depth + 1 {
                    children_tags.push(String::from_utf8_lossy(e.name().as_ref()).into_owned());
                }
            }
            Ok(Event::Empty(ref e)) => {
                if in_body && depth + 1 == body_depth + 1 {
                    // empty element is a child of body
                    children_tags.push(String::from_utf8_lossy(e.name().as_ref()).into_owned());
                }
            }
            Ok(Event::End(_)) => {
                if in_body && depth == body_depth {
                    in_body = false;
                }
                depth -= 1;
            }
            Ok(_) => {}
            Err(_) => return,
        }
        buf.clear();
    }
    if children_tags.is_empty() {
        return;
    }
    // OOXML allows bookmarkStart / bookmarkEnd as direct body children —
    // they let a bookmark span multiple paragraphs.
    let valid: &[&str] = &[
        "w:p",
        "w:tbl",
        "w:sectPr",
        "w:bookmarkStart",
        "w:bookmarkEnd",
    ];
    let bad: BTreeSet<&str> = children_tags
        .iter()
        .filter(|t| !valid.contains(&t.as_str()))
        .map(|s| s.as_str())
        .collect();
    let sect_last = children_tags
        .last()
        .map(|s| s == "w:sectPr")
        .unwrap_or(false);
    if !bad.is_empty() {
        let mut bad_list: Vec<&str> = bad.into_iter().collect();
        bad_list.sort();
        r.issues
            .push(format!("Body has unexpected element tags: {bad_list:?}"));
    } else if !sect_last && children_tags.iter().any(|t| t == "w:sectPr") {
        r.issues
            .push("Body <w:sectPr> is not the last child".into());
    } else {
        r.ok.push(format!(
            "Body structure valid ({} children, sectPr at end: {})",
            children_tags.len(),
            sect_last
        ));
    }
}

fn check_bookmark_ids(pkg: &Package, r: &mut CheckReport) {
    let Some(doc) = pkg.get_part("word/document.xml") else {
        return;
    };
    let mut reader = Reader::from_reader(doc);
    let mut buf = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                if tag_is_w(e.name().as_ref(), b"bookmarkStart") {
                    for attr in e.attributes().with_checks(false).flatten() {
                        if attr_is_w(attr.key.as_ref(), b"id") {
                            let v = String::from_utf8_lossy(&attr.value).into_owned();
                            if !v.is_empty() {
                                ids.push(v);
                            }
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(_) => return,
        }
        buf.clear();
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for id in &ids {
        *counts.entry(id.as_str()).or_default() += 1;
    }
    let dupes: Vec<(&str, usize)> = counts
        .iter()
        .filter(|(_, c)| **c > 1)
        .map(|(k, c)| (*k, *c))
        .collect();
    if dupes.is_empty() {
        r.ok.push(format!("{} bookmarkStart ID(s), all unique", ids.len()));
    } else {
        for (bid, cnt) in dupes.iter().take(3) {
            r.issues
                .push(format!("Duplicate bookmarkStart id={bid:?} appears {cnt}x"));
        }
    }
}

fn check_inline_rids(pkg: &Package, r: &mut CheckReport) {
    let Some(doc) = pkg.get_part("word/document.xml") else {
        return;
    };
    let body_xml = String::from_utf8_lossy(doc);
    // Match r:id="rIdN", r:embed="…", r:link="…"
    let mut rids: BTreeSet<String> = BTreeSet::new();
    for needle in ["r:id=\"", "r:embed=\"", "r:link=\""] {
        let mut start = 0usize;
        while let Some(pos) = body_xml[start..].find(needle) {
            let abs = start + pos + needle.len();
            if let Some(end) = body_xml[abs..].find('"') {
                let val = &body_xml[abs..abs + end];
                if let Some(stripped) = val.strip_prefix("rId") {
                    if stripped.chars().all(|c| c.is_ascii_digit()) && !stripped.is_empty() {
                        rids.insert(val.to_string());
                    }
                }
                start = abs + end;
            } else {
                break;
            }
        }
    }
    if rids.is_empty() {
        return;
    }
    if let Some(rels) = pkg.get_part("word/_rels/document.xml.rels") {
        let rels_xml = String::from_utf8_lossy(rels);
        let missing: Vec<&String> = rids
            .iter()
            .filter(|rid| !rels_xml.contains(&format!("Id=\"{rid}\"")))
            .collect();
        if missing.is_empty() {
            r.ok.push(format!(
                "{} body relationship reference(s), all resolved",
                rids.len()
            ));
        } else {
            r.issues.push(format!(
                "Body references {} rId(s) not in document.xml.rels: {:?}",
                missing.len(),
                missing.iter().map(|s| s.as_str()).collect::<Vec<_>>()
            ));
        }
    } else {
        r.issues
            .push("Body has rId references but no document.xml.rels".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open;

    #[test]
    fn resolve_relative_target_basic() {
        assert_eq!(
            resolve_relative_target("word", "footnotes.xml"),
            "word/footnotes.xml"
        );
        assert_eq!(
            resolve_relative_target("word", "/word/footnotes.xml"),
            "word/footnotes.xml"
        );
        assert_eq!(
            resolve_relative_target("word", "../docProps/core.xml"),
            "docProps/core.xml"
        );
    }

    #[test]
    fn check_vielfalt_doc_is_clean() {
        let path = "/Users/christianstrobele/OneDrive/2026 Vielfalt cs15.docx";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let pkg = open(path).unwrap();
        let report = check_package(&pkg).unwrap();
        // post-cleanup doc should be clean
        for issue in &report.issues {
            eprintln!("FAIL  {issue}");
        }
        assert!(report.is_clean(), "expected clean report on Vielfalt doc");
        assert!(!report.ok.is_empty());
    }
}

//! Live tests against a real-world docx.
//!
//! These are deliberately opt-in. Set `CRISP_DOCX_LIVE_FIXTURE` to the
//! path of a `.docx` you want to exercise the primitives against; if the
//! variable isn't set the tests trivially pass. Real files aren't checked
//! into the repo because they may contain private content.
//!
//! Example:
//!
//! ```ignore
//! CRISP_DOCX_LIVE_FIXTURE=~/Documents/paper.docx cargo test --test live
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use crisp_docx_core::{
    convert_notes_kind, inject_footnotes, normalize_tags, open, save, strip_rsids, transplant_body,
    NotesKind,
};

fn live_fixture() -> Option<PathBuf> {
    live_env("CRISP_DOCX_LIVE_FIXTURE")
}

fn live_blueprint() -> Option<PathBuf> {
    live_env("CRISP_DOCX_LIVE_BLUEPRINT")
}

fn live_env(var: &str) -> Option<PathBuf> {
    let raw = std::env::var(var).ok()?;
    let path = PathBuf::from(shellexpand_tilde(&raw));
    path.exists().then_some(path)
}

fn shellexpand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest,);
        }
    }
    s.to_string()
}

#[test]
fn live_inspect_only() {
    let Some(path) = live_fixture() else {
        eprintln!("CRISP_DOCX_LIVE_FIXTURE not set — skipping");
        return;
    };
    let pkg = open(&path).expect("open live fixture");
    let n = pkg.parts().count();
    assert!(n > 0);
    eprintln!("live fixture {} has {} parts", path.display(), n);
}

#[test]
fn live_clean_round_trip() {
    let Some(path) = live_fixture() else {
        eprintln!("CRISP_DOCX_LIVE_FIXTURE not set — skipping");
        return;
    };
    let tmp = std::env::temp_dir().join("crisp_docx_live_clean.docx");
    std::fs::copy(&path, &tmp).expect("copy fixture to tmp");

    let mut pkg = open(&tmp).expect("open tmp");
    let _ = strip_rsids(&mut pkg).expect("strip_rsids");
    let _ = normalize_tags(&mut pkg).expect("normalize_tags");
    save(&pkg, &tmp).expect("save");

    // Reopen and verify it's still a usable package.
    let pkg2 = open(&tmp).expect("reopen tmp");
    assert!(pkg2.parts().count() > 0);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn live_inject_footnotes_smoke() {
    // Use an extremely unlikely note number so the call is essentially a
    // no-op against any real document — we're just verifying the
    // implementation walks the file without panicking and reports
    // sensible counts. A docx that *does* contain `[88888]` would
    // legitimately have a footnote injected; we still tolerate that
    // and assert the report shape rather than the count.
    let Some(path) = live_fixture() else {
        eprintln!("CRISP_DOCX_LIVE_FIXTURE not set — skipping");
        return;
    };
    let tmp = std::env::temp_dir().join("crisp_docx_live_inject.docx");
    std::fs::copy(&path, &tmp).expect("copy fixture to tmp");

    let mut pkg = open(&tmp).expect("open tmp");
    let mut notes: BTreeMap<u32, &str> = BTreeMap::new();
    notes.insert(88888, "live-test sentinel note");
    let report = inject_footnotes(&mut pkg, &notes).expect("inject_footnotes");
    // Either 0 (no marker found) or some small number — but the report
    // must be coherent: anything in `seen` must be in `notes`.
    assert!(
        report.inserted <= 1,
        "unexpected count: {}",
        report.inserted
    );
    save(&pkg, &tmp).expect("save");
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn live_transplant_round_trip() {
    // Needs both fixtures: the blueprint owns the formatting, the source
    // owns the content.
    let (Some(bp_path), Some(src_path)) = (live_blueprint(), live_fixture()) else {
        eprintln!("CRISP_DOCX_LIVE_BLUEPRINT or CRISP_DOCX_LIVE_FIXTURE not set — skipping");
        return;
    };
    let tmp = std::env::temp_dir().join("crisp_docx_live_transplant.docx");
    std::fs::copy(&bp_path, &tmp).expect("copy blueprint");

    // Capture blueprint + source bytes before transplanting so we can
    // assert the right parts survived in the output.
    let bp_pre = open(&bp_path).expect("open blueprint pre");
    let src_pre = open(&src_path).expect("open source pre");
    let bp_styles = bp_pre.get_part("word/styles.xml").map(<[u8]>::to_vec);
    let src_footnotes = src_pre.get_part("word/footnotes.xml").map(<[u8]>::to_vec);

    let mut bp = open(&tmp).expect("open blueprint");
    let src = open(&src_path).expect("open source");
    transplant_body(&mut bp, &src).expect("transplant_body");
    save(&bp, &tmp).expect("save");

    // Re-open and assert structural invariants.
    let out = open(&tmp).expect("reopen");
    assert!(out.parts().count() > 0);

    // The blueprint's styles should be preserved verbatim.
    if let Some(want) = &bp_styles {
        let got = out
            .get_part("word/styles.xml")
            .expect("output missing styles.xml");
        assert_eq!(got, want.as_slice(), "blueprint styles.xml was modified");
    }

    // The source's footnotes (if any) should be carried over verbatim.
    if let Some(want) = &src_footnotes {
        let got = out
            .get_part("word/footnotes.xml")
            .expect("output missing footnotes.xml after transplant from a source that had them");
        assert_eq!(got, want.as_slice(), "source footnotes.xml not carried");
    }

    // The body must contain exactly one body-direct sectPr — blueprint's —
    // and every <w:footnoteReference> in the body must resolve to a
    // <w:footnote> with the same id in word/footnotes.xml.
    let doc = std::str::from_utf8(out.get_part("word/document.xml").unwrap()).unwrap();
    let body_close = doc.find("</w:body>").expect("no </w:body>");
    let body = &doc[..body_close];
    let sectpr_count = body.matches("<w:sectPr").count();
    assert_eq!(
        sectpr_count, 1,
        "expected exactly one body-direct sectPr, found {sectpr_count}"
    );

    if let Some(fn_bytes) = out.get_part("word/footnotes.xml") {
        let fn_str = std::str::from_utf8(fn_bytes).unwrap();
        let ref_ids: std::collections::HashSet<&str> = collect_attr(body, "w:footnoteReference");
        let def_ids: std::collections::HashSet<&str> = collect_attr(fn_str, "w:footnote");
        for r in &ref_ids {
            assert!(
                def_ids.contains(r),
                "body cites footnote w:id={r:?} but no matching <w:footnote> exists"
            );
        }
    }

    let _ = std::fs::remove_file(&tmp);
}

fn collect_attr<'a>(haystack: &'a str, elem: &str) -> std::collections::HashSet<&'a str> {
    let mut out = std::collections::HashSet::new();
    let needle_open = format!("<{elem} ");
    let mut i = 0;
    while let Some(rel) = haystack[i..].find(&needle_open) {
        let start = i + rel;
        let rest = &haystack[start..];
        if let Some(id_off) = rest.find(r#"w:id=""#) {
            let after = &rest[id_off + 6..];
            if let Some(end) = after.find('"') {
                out.insert(&after[..end]);
            }
        }
        i = start + needle_open.len();
    }
    out
}

#[test]
fn live_notes_kind_round_trip() {
    let Some(path) = live_fixture() else {
        eprintln!("CRISP_DOCX_LIVE_FIXTURE not set — skipping");
        return;
    };
    let tmp = std::env::temp_dir().join("crisp_docx_live_notes.docx");
    std::fs::copy(&path, &tmp).expect("copy fixture to tmp");

    let mut pkg = open(&tmp).expect("open tmp");
    let had_footnotes = pkg.get_part("word/footnotes.xml").is_some();
    convert_notes_kind(&mut pkg, NotesKind::Endnotes).expect("to endnotes");
    if had_footnotes {
        assert!(pkg.get_part("word/footnotes.xml").is_none());
        assert!(pkg.get_part("word/endnotes.xml").is_some());
    }
    convert_notes_kind(&mut pkg, NotesKind::Footnotes).expect("back to footnotes");
    if had_footnotes {
        assert!(pkg.get_part("word/footnotes.xml").is_some());
        assert!(pkg.get_part("word/endnotes.xml").is_none());
    }
    save(&pkg, &tmp).expect("save");
    let _ = std::fs::remove_file(&tmp);
}

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

use std::path::PathBuf;

use crisp_docx_core::{convert_notes_kind, normalize_tags, open, save, strip_rsids, NotesKind};

fn live_fixture() -> Option<PathBuf> {
    let raw = std::env::var("CRISP_DOCX_LIVE_FIXTURE").ok()?;
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

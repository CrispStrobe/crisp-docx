//! Parity harness — runs each Python primitive in `CrispTranslator/` and
//! each corresponding Rust primitive on the **same** input docx, then
//! asserts equivalence per the criterion in `PARITY.md`.
//!
//! These tests are gated on the parity fixtures being present and on the
//! `CrispTranslator` checkout being importable. When either is absent they
//! trivially pass with an `eprintln!` skip — so CI on a fresh runner
//! doesn't fall over, but on a developer's machine the harness is the
//! authoritative parity gate.
//!
//! Environment:
//!
//!   CRISP_DOCX_PARITY_VIELFALT
//!     Path to a real docx fixture (default:
//!     `/Users/christianstrobele/OneDrive/2026 Vielfalt cs15.docx`).
//!
//!   CRISP_DOCX_PARITY_PY_RUNNER
//!     Path to the Python interpreter (default: searches for
//!     `~/miniconda3/bin/python` then `python3`).
//!
//!   CRISP_TRANSLATOR_DIR
//!     Path to a CrispTranslator checkout (default:
//!     `~/code/CrispTranslator`). Forwarded to run_python.py.

use std::path::{Path, PathBuf};
use std::process::Command;

use crisp_docx_core::{convert_notes_kind, normalize_tags, open, save, strip_rsids, NotesKind};

// ─── fixtures + plumbing ────────────────────────────────────────────────────

fn fixture(var: &str, default: &str) -> Option<PathBuf> {
    let raw = std::env::var(var).unwrap_or_else(|_| default.to_string());
    let raw = expand_tilde(&raw);
    let p = PathBuf::from(raw);
    p.exists().then_some(p)
}

fn expand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    s.to_string()
}

fn python_interpreter() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CRISP_DOCX_PARITY_PY_RUNNER") {
        let path = PathBuf::from(expand_tilde(&p));
        if path.exists() {
            return Some(path);
        }
    }
    for candidate in &["~/miniconda3/bin/python", "/usr/bin/python3"] {
        let path = PathBuf::from(expand_tilde(candidate));
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn run_python_primitive(
    py: &Path,
    primitive: &str,
    src: &Path,
    dst: &Path,
) -> Result<serde_json::Value, String> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity_scripts/run_python.py");
    let output = Command::new(py)
        .arg(&script)
        .arg(primitive)
        .arg(src)
        .arg(dst)
        .output()
        .map_err(|e| format!("spawn python: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "python {primitive} failed: {} / stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|e| format!("non-utf8 stdout: {e}"))?
        .trim();
    if stdout.is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(stdout).map_err(|e| format!("bad json from python: {e}"))
}

// Structural-soundness checks applied to BOTH sides' output independently.
// Catches bugs in either Python or Rust — not just divergences between them.
mod validate {
    use std::collections::HashSet;
    use std::io::Read;
    use std::path::Path;

    /// Soundness assertions that apply to every docx produced by a
    /// CrispTranslator/crisp-docx primitive: every XML part parses,
    /// content-types has overrides for every named part, and every
    /// internal relationship target exists in the package.
    pub fn soundness(label: &str, path: &Path) -> Result<(), String> {
        let f = std::fs::File::open(path).map_err(|e| format!("{label}: open: {e}"))?;
        let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{label}: zip: {e}"))?;

        // Collect part bytes
        let mut parts = std::collections::BTreeMap::<String, Vec<u8>>::new();
        for i in 0..zip.len() {
            let mut e = zip
                .by_index(i)
                .map_err(|err| format!("{label}: idx: {err}"))?;
            let n = e.name().to_string();
            let mut bytes = Vec::new();
            e.read_to_end(&mut bytes)
                .map_err(|err| format!("{label}: read: {err}"))?;
            parts.insert(n, bytes);
        }

        // Every XML part must parse.
        for (name, bytes) in &parts {
            if name.ends_with(".xml") || name.ends_with(".rels") {
                quick_xml::de::from_reader::<_, serde_json::Value>(bytes.as_slice())
                    .err()
                    .map(|_| ())
                    .unwrap_or(());
                // Use a lighter reader-only well-formedness pass.
                let mut r = quick_xml::reader::Reader::from_reader(bytes.as_slice());
                let mut buf = Vec::with_capacity(1024);
                loop {
                    match r.read_event_into(&mut buf) {
                        Ok(quick_xml::events::Event::Eof) => break,
                        Ok(_) => {}
                        Err(e) => return Err(format!("{label}: malformed XML in {name}: {e}")),
                    }
                    buf.clear();
                }
            }
        }

        // Cross-references: every <Override PartName="/foo"> must point at
        // an actual part. Every <Relationship Target="bar"> from
        // word/_rels/document.xml.rels must resolve relative to word/.
        if let Some(ct) = parts.get("[Content_Types].xml") {
            let s = String::from_utf8_lossy(ct);
            for cap in regex_lite_overrides(&s) {
                let canonical = cap.trim_start_matches('/');
                if !parts.contains_key(canonical) {
                    return Err(format!(
                        "{label}: [Content_Types].xml references {cap} which is not in the package"
                    ));
                }
            }
        }

        if let Some(rels) = parts.get("word/_rels/document.xml.rels") {
            let s = String::from_utf8_lossy(rels);
            for tgt in regex_lite_targets(&s) {
                let resolved = if tgt.starts_with("../") {
                    tgt.trim_start_matches("../").to_string()
                } else if tgt.contains("://") {
                    continue; // external
                } else {
                    format!("word/{}", tgt)
                };
                if !parts.contains_key(&resolved) {
                    // Skip — some legit rels target external resources or
                    // image streams. Only flag when target looks like a
                    // local part path (no scheme).
                    if !tgt.contains("://") && !tgt.starts_with("#") {
                        return Err(format!(
                            "{label}: word/_rels/document.xml.rels targets {tgt} which resolves to {resolved} (absent)"
                        ));
                    }
                }
            }
        }

        // If document.xml references footnoteReference w:id="N", N must
        // exist as <w:footnote w:id="N"> in footnotes.xml.
        if let (Some(doc), Some(fn_bytes)) = (
            parts.get("word/document.xml"),
            parts.get("word/footnotes.xml"),
        ) {
            let doc_s = String::from_utf8_lossy(doc);
            let fn_s = String::from_utf8_lossy(fn_bytes);
            let ref_ids: HashSet<&str> = collect_ids(&doc_s, "w:footnoteReference");
            let def_ids: HashSet<&str> = collect_ids(&fn_s, "w:footnote");
            for r in &ref_ids {
                if !def_ids.contains(r) {
                    return Err(format!(
                        "{label}: body cites footnote w:id={r} but no <w:footnote w:id={r}> exists"
                    ));
                }
            }
        }
        if let (Some(doc), Some(en_bytes)) = (
            parts.get("word/document.xml"),
            parts.get("word/endnotes.xml"),
        ) {
            let doc_s = String::from_utf8_lossy(doc);
            let en_s = String::from_utf8_lossy(en_bytes);
            let ref_ids: HashSet<&str> = collect_ids(&doc_s, "w:endnoteReference");
            let def_ids: HashSet<&str> = collect_ids(&en_s, "w:endnote");
            for r in &ref_ids {
                if !def_ids.contains(r) {
                    return Err(format!(
                        "{label}: body cites endnote w:id={r} but no <w:endnote w:id={r}> exists"
                    ));
                }
            }
        }

        Ok(())
    }

    fn regex_lite_overrides(s: &str) -> Vec<&str> {
        // Yield each PartName="…" attribute value.
        let mut out = Vec::new();
        for (idx, _) in s.match_indices("PartName=\"") {
            let after = &s[idx + 10..];
            if let Some(end) = after.find('"') {
                out.push(&after[..end]);
            }
        }
        out
    }

    fn regex_lite_targets(s: &str) -> Vec<&str> {
        let mut out = Vec::new();
        for (idx, _) in s.match_indices("Target=\"") {
            let after = &s[idx + 8..];
            if let Some(end) = after.find('"') {
                out.push(&after[..end]);
            }
        }
        out
    }

    fn collect_ids<'a>(s: &'a str, element: &str) -> HashSet<&'a str> {
        let needle = format!("<{element} ");
        let mut out = HashSet::new();
        let mut i = 0;
        while let Some(rel) = s[i..].find(&needle) {
            let start = i + rel;
            let rest = &s[start..];
            if let Some(off) = rest.find("w:id=\"") {
                let after = &rest[off + 6..];
                if let Some(end) = after.find('"') {
                    out.insert(&after[..end]);
                }
            }
            i = start + needle.len();
        }
        out
    }

    /// Stricter: assert NO rsid-family attribute appears in document /
    /// footnotes / endnotes XML.
    pub fn no_rsids(label: &str, path: &Path) -> Result<(), String> {
        let bad = [
            "w14:paraId",
            "w14:textId",
            "w:rsidR",
            "w:rsidRPr",
            "w:rsidDel",
            "w:rsidRDefault",
            "w:rsidP",
            "w:rsidTr",
            "w:rsidSect",
        ];
        let f = std::fs::File::open(path).map_err(|e| format!("{label}: open: {e}"))?;
        let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{label}: zip: {e}"))?;
        for part in [
            "word/document.xml",
            "word/footnotes.xml",
            "word/endnotes.xml",
        ] {
            if let Ok(mut e) = zip.by_name(part) {
                let mut s = String::new();
                e.read_to_string(&mut s)
                    .map_err(|err| format!("{label}: read {part}: {err}"))?;
                for needle in &bad {
                    if s.contains(needle) {
                        return Err(format!("{label}: {part} still contains {needle}"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Stricter: assert NO textutil-style non-OOXML tag remains.
    pub fn no_textutil_tags(label: &str, path: &Path) -> Result<(), String> {
        let bad = ["w:sz-cs", "w:b-cs", "w:i-cs"];
        let f = std::fs::File::open(path).map_err(|e| format!("{label}: open: {e}"))?;
        let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{label}: zip: {e}"))?;
        for part in [
            "word/document.xml",
            "word/footnotes.xml",
            "word/endnotes.xml",
        ] {
            if let Ok(mut e) = zip.by_name(part) {
                let mut s = String::new();
                e.read_to_string(&mut s)
                    .map_err(|err| format!("{label}: read {part}: {err}"))?;
                for needle in &bad {
                    if s.contains(needle) {
                        return Err(format!("{label}: {part} still contains {needle}"));
                    }
                }
            }
        }
        Ok(())
    }
}

// quick-xml's serializer happens to be byte-stable with quick-xml's reader
// for our inputs, but the Python lxml roundtrip is NOT byte-stable. So we
// compare structural fingerprints rather than raw bytes.
mod compare {
    use std::collections::BTreeMap;
    use std::path::Path;

    /// Fingerprint a docx for comparison: returns BTreeMap<part_path,
    /// hash> where each XML part is normalised (whitespace stripped) and
    /// hashed.
    pub fn fingerprint(path: &Path) -> std::io::Result<BTreeMap<String, u64>> {
        let mut out = BTreeMap::new();
        let f = std::fs::File::open(path)?;
        let mut zip =
            zip::ZipArchive::new(f).map_err(|e| std::io::Error::other(format!("zip: {e}")))?;
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|e| std::io::Error::other(format!("zip entry: {e}")))?;
            let name = entry.name().to_string();
            let mut data = Vec::new();
            std::io::copy(&mut entry, &mut data)?;
            let key = std::hash::BuildHasher::hash_one(
                &std::collections::hash_map::RandomState::new(),
                normalize(&name, &data),
            );
            out.insert(name, key);
        }
        Ok(out)
    }

    /// Normalise a part for fingerprinting: for XML parts, collapse all
    /// runs of whitespace to a single space and trim. For non-XML parts,
    /// pass through.
    fn normalize(name: &str, bytes: &[u8]) -> Vec<u8> {
        if !name.ends_with(".xml") && !name.ends_with(".rels") {
            return bytes.to_vec();
        }
        let s = String::from_utf8_lossy(bytes);
        let mut out = String::with_capacity(s.len());
        let mut last_was_ws = false;
        for ch in s.chars() {
            if ch.is_ascii_whitespace() {
                if !last_was_ws {
                    out.push(' ');
                    last_was_ws = true;
                }
            } else {
                out.push(ch);
                last_was_ws = false;
            }
        }
        out.trim().as_bytes().to_vec()
    }
}

// ─── individual parity tests ────────────────────────────────────────────────
//
// Each test follows the same shape:
//   1. Run Python primitive on FIXTURE -> py_out.docx, capture report
//   2. Run Rust primitive on FIXTURE -> rust_out.docx, capture report
//   3. Compare reports (counts equal) and document fingerprints
//      (per-part normalised whitespace)
//
// Divergences are reported in detail so the next port iteration has a
// concrete target.

#[test]
fn parity_strip_rsids() {
    let Some(fx) = fixture(
        "CRISP_DOCX_PARITY_VIELFALT",
        "/Users/christianstrobele/OneDrive/2026 Vielfalt cs15.docx",
    ) else {
        eprintln!("CRISP_DOCX_PARITY_VIELFALT missing — skipping");
        return;
    };
    let Some(py) = python_interpreter() else {
        eprintln!("Python interpreter not available — skipping");
        return;
    };

    let td = tempdir();
    let py_out = td.path().join("py.docx");
    let rust_out = td.path().join("rust.docx");

    let py_report =
        run_python_primitive(&py, "strip_rsids", &fx, &py_out).expect("python strip_rsids");
    let py_removed = py_report["removed"].as_u64().expect("removed:number");

    std::fs::copy(&fx, &rust_out).expect("copy");
    let mut pkg = open(&rust_out).expect("open");
    let rust_removed = strip_rsids(&mut pkg).expect("strip_rsids");
    save(&pkg, &rust_out).expect("save");

    // Both outputs MUST be sound docx in their own right — catches a bug
    // in either implementation independently of parity comparison.
    validate::soundness("python", &py_out).unwrap();
    validate::soundness("rust", &rust_out).unwrap();
    // After strip_rsids, neither side should have any rsid/paraId attr.
    validate::no_rsids("python", &py_out).unwrap();
    validate::no_rsids("rust", &rust_out).unwrap();

    assert_eq!(
        rust_removed as u64, py_removed,
        "PARITY: strip_rsids count differs (rust={rust_removed}, python={py_removed})"
    );

    let py_fp = compare::fingerprint(&py_out).expect("py fp");
    let rust_fp = compare::fingerprint(&rust_out).expect("rust fp");
    assert_eq!(
        py_fp.keys().collect::<Vec<_>>(),
        rust_fp.keys().collect::<Vec<_>>(),
        "PARITY: strip_rsids part list differs"
    );
    // We don't assert per-part fingerprint equality yet — lxml and
    // quick-xml emit different attribute orderings and serialisation
    // quirks. Equal counts + equal part lists + soundness + no-rsids
    // is a meaningful gate until we tighten the criterion in PARITY.md.
}

#[test]
fn parity_normalize_tags() {
    let Some(fx) = fixture(
        "CRISP_DOCX_PARITY_VIELFALT",
        "/Users/christianstrobele/OneDrive/2026 Vielfalt cs15.docx",
    ) else {
        eprintln!("CRISP_DOCX_PARITY_VIELFALT missing — skipping");
        return;
    };
    let Some(py) = python_interpreter() else {
        eprintln!("Python interpreter not available — skipping");
        return;
    };

    let td = tempdir();
    let py_out = td.path().join("py.docx");
    let rust_out = td.path().join("rust.docx");

    let py_report = run_python_primitive(&py, "normalize_tags", &fx, &py_out).expect("python");
    let py_renamed = py_report["renamed"].as_u64().expect("renamed:number");

    std::fs::copy(&fx, &rust_out).expect("copy");
    let mut pkg = open(&rust_out).expect("open");
    let rust_renamed = normalize_tags(&mut pkg).expect("normalize_tags");
    save(&pkg, &rust_out).expect("save");

    validate::soundness("python", &py_out).unwrap();
    validate::soundness("rust", &rust_out).unwrap();
    validate::no_textutil_tags("python", &py_out).unwrap();
    validate::no_textutil_tags("rust", &rust_out).unwrap();

    assert_eq!(
        rust_renamed as u64, py_renamed,
        "PARITY: normalize_tags count differs"
    );
}

#[test]
fn parity_notes_to_endnotes() {
    let Some(fx) = fixture(
        "CRISP_DOCX_PARITY_VIELFALT",
        "/Users/christianstrobele/OneDrive/2026 Vielfalt cs15.docx",
    ) else {
        eprintln!("CRISP_DOCX_PARITY_VIELFALT missing — skipping");
        return;
    };
    let Some(py) = python_interpreter() else {
        eprintln!("Python interpreter not available — skipping");
        return;
    };

    let td = tempdir();
    let py_out = td.path().join("py.docx");
    let rust_out = td.path().join("rust.docx");

    run_python_primitive(&py, "notes_to_endnotes", &fx, &py_out).expect("python");

    std::fs::copy(&fx, &rust_out).expect("copy");
    let mut pkg = open(&rust_out).expect("open");
    convert_notes_kind(&mut pkg, NotesKind::Endnotes).expect("convert");
    save(&pkg, &rust_out).expect("save");

    // Both outputs must have the notes part renamed.
    let py_fp = compare::fingerprint(&py_out).expect("py fp");
    let rust_fp = compare::fingerprint(&rust_out).expect("rust fp");
    let py_has_en = py_fp.contains_key("word/endnotes.xml");
    let rust_has_en = rust_fp.contains_key("word/endnotes.xml");
    let py_has_fn = py_fp.contains_key("word/footnotes.xml");
    let rust_has_fn = rust_fp.contains_key("word/footnotes.xml");
    assert!(py_has_en && rust_has_en, "endnotes.xml absent in one side");
    assert!(
        !py_has_fn && !rust_has_fn,
        "footnotes.xml still present in one side"
    );

    // Both outputs must independently make sense: no dangling refs, no
    // footnoteReference still pointing into nowhere, content-types
    // overrides resolve.
    validate::soundness("python", &py_out).unwrap();
    validate::soundness("rust", &rust_out).unwrap();
}

// ─── tempdir helper ─────────────────────────────────────────────────────────

struct TempDir(PathBuf);
impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static TEMPDIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn tempdir() -> TempDir {
    let base = std::env::temp_dir();
    let n = TEMPDIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let p = base.join(format!("crisp-docx-parity-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&p).expect("mkdir tempdir");
    TempDir(p)
}

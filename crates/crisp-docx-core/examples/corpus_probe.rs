//! Reproducible local corpus probe; no network, macros or field evaluation.
use crisp_docx_core::{
    check_package, convert_notes_kind, extract_paragraph_runs, extract_paragraph_texts,
    normalize_tags, replace_paragraph_runs, replace_paragraph_texts, strip_rsids, NotesKind,
    Package,
};
use serde_json::{json, Value};
use std::path::Path;

fn outcome<T: std::fmt::Debug>(result: crisp_docx_core::Result<T>) -> Value {
    match result {
        Ok(value) => json!({"ok":true,"value":format!("{value:?}")}),
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    }
}

fn probe(path: &Path, destination: &Path) -> anyhow::Result<Value> {
    let pkg = Package::open(path)?;
    let texts = extract_paragraph_texts(&pkg)?;
    let runs = extract_paragraph_runs(&pkg)?;
    std::fs::create_dir_all(destination)?;
    pkg.save(destination.join("roundtrip.docx"))?;
    let report = check_package(&pkg)?;

    let mut cleaned = pkg.clone();
    let strip = outcome(strip_rsids(&mut cleaned));
    let normalize = outcome(normalize_tags(&mut cleaned));
    cleaned.save(destination.join("cleaned.docx"))?;
    let cleaned_once = cleaned.clone();
    strip_rsids(&mut cleaned)?;
    normalize_tags(&mut cleaned)?;
    let clean_idempotent = cleaned.as_btree() == cleaned_once.as_btree();

    let mut text_copy = pkg.clone();
    let text_replace = outcome(replace_paragraph_texts(&mut text_copy, &texts));
    text_copy.save(destination.join("text-identity.docx"))?;
    let mut run_copy = pkg.clone();
    let run_replace = outcome(replace_paragraph_runs(&mut run_copy, &runs));
    run_copy.save(destination.join("runs-identity.docx"))?;

    let mut note_results = Vec::new();
    for (target, source, dest_name) in [
        (NotesKind::Endnotes, "word/footnotes.xml", "notes-endnotes.docx"),
        (NotesKind::Footnotes, "word/endnotes.xml", "notes-footnotes.docx"),
    ] {
        if pkg.get_part(source).is_none() {
            continue;
        }
        let mut copy = pkg.clone();
        let result = outcome(convert_notes_kind(&mut copy, target));
        let changed = copy.as_btree() != pkg.as_btree();
        if result["ok"] == true && changed {
            copy.save(destination.join(dest_name))?;
        }
        note_results.push(json!({"target":format!("{target:?}"),"result":result,"changed":changed}));
    }
    Ok(json!({
        "file":path.file_name().unwrap().to_string_lossy(),"ok":true,
        "parts":pkg.parts().count(),"texts":texts,
        "runTexts":runs.iter().map(|p|p.full_text()).collect::<Vec<_>>(),
        "checkIssues":report.issues,"checkOk":report.ok,
        "strip":strip,"normalize":normalize,"cleanIdempotent":clean_idempotent,
        "textIdentity":text_replace,"runsIdentity":run_replace,"notes":note_results,
    }))
}

fn main() -> anyhow::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    anyhow::ensure!(args.len() == 3, "usage: corpus_probe INPUT_DIRECTORY OUTPUT_DIRECTORY");
    let mut paths = std::fs::read_dir(&args[1])?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    for path in paths.into_iter().filter(|p| p.extension().is_some_and(|e| e == "docx")) {
        let destination = Path::new(&args[2]).join(path.file_stem().unwrap());
        let value = match probe(&path, &destination) {
            Ok(value) => value,
            Err(error) => json!({"file":path.file_name().unwrap().to_string_lossy(),"ok":false,"error":error.to_string()}),
        };
        println!("{value}");
    }
    Ok(())
}

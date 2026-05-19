//! Python bindings for `crisp-docx-core`.
//!
//! Exposes the primitives as free functions that take a path and mutate
//! the file in place (or to an `output=` path). This is the lowest-effort
//! shape that lets `docxtool clean` in CrispTranslator opt into the
//! Rust-fast implementation with minimal call-site churn.

// The `#[pyfunction]` macro expands to wrapper code that does an
// `Into<PyErr>` conversion on our already-`PyErr`-typed return — clippy
// (rightly) flags it useless, but the noise comes from generated code we
// don't control.
#![allow(clippy::useless_conversion)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crisp_docx_core::{
    convert_notes_kind as core_convert, inject_footnotes as core_inject,
    normalize_tags as core_normalize, open, save, strip_rsids as core_strip,
    transplant_body as core_transplant, NotesKind,
};

#[pyclass(eq, eq_int)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PyNotesKind {
    Footnotes,
    Endnotes,
}

impl From<PyNotesKind> for NotesKind {
    fn from(p: PyNotesKind) -> Self {
        match p {
            PyNotesKind::Footnotes => NotesKind::Footnotes,
            PyNotesKind::Endnotes => NotesKind::Endnotes,
        }
    }
}

fn map_err(e: crisp_docx_core::Error) -> PyErr {
    PyValueError::new_err(e.to_string())
}

/// Strip rsid/paraId tracking attributes from a docx.
///
/// Returns the number of attributes removed. If `output` is omitted, the
/// input file is edited in place.
#[pyfunction]
#[pyo3(signature = (path, output=None))]
fn strip_rsids(path: PathBuf, output: Option<PathBuf>) -> PyResult<usize> {
    let mut pkg = open(&path).map_err(map_err)?;
    let n = core_strip(&mut pkg).map_err(map_err)?;
    let dest = output.unwrap_or(path);
    save(&pkg, &dest).map_err(map_err)?;
    Ok(n)
}

/// Rewrite Apple textutil's non-standard OOXML tags (w:sz-cs -> w:szCs, …).
/// Returns the number of byte-level substitutions performed.
#[pyfunction]
#[pyo3(signature = (path, output=None))]
fn normalize_tags(path: PathBuf, output: Option<PathBuf>) -> PyResult<usize> {
    let mut pkg = open(&path).map_err(map_err)?;
    let n = core_normalize(&mut pkg).map_err(map_err)?;
    let dest = output.unwrap_or(path);
    save(&pkg, &dest).map_err(map_err)?;
    Ok(n)
}

/// Convert the docx between footnotes and endnotes.
#[pyfunction]
#[pyo3(signature = (path, target, output=None))]
fn convert_notes_kind(path: PathBuf, target: PyNotesKind, output: Option<PathBuf>) -> PyResult<()> {
    let mut pkg = open(&path).map_err(map_err)?;
    core_convert(&mut pkg, target.into()).map_err(map_err)?;
    let dest = output.unwrap_or(path);
    save(&pkg, &dest).map_err(map_err)?;
    Ok(())
}

/// Inject Word footnote references at every inline `[N]` marker in the
/// document body. `notes` is a `Dict[int, str]` mapping note number to
/// note body text. Returns a `(inserted, unknown_ids, unused_ids)` tuple.
#[pyfunction]
#[pyo3(signature = (path, notes, output=None))]
fn inject_footnotes(
    path: PathBuf,
    notes: BTreeMap<u32, String>,
    output: Option<PathBuf>,
) -> PyResult<(usize, Vec<u32>, Vec<u32>)> {
    let mut pkg = open(&path).map_err(map_err)?;
    let view: BTreeMap<u32, &str> = notes.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let report = core_inject(&mut pkg, &view).map_err(map_err)?;
    let dest = output.unwrap_or(path);
    save(&pkg, &dest).map_err(map_err)?;
    Ok((report.inserted, report.unknown_ids, report.unused_ids))
}

/// Replace the body of `blueprint_path` with the body of `source_path`,
/// preserving the blueprint's trailing `<w:sectPr>` (page layout) and
/// stripping rsid tracking attrs from grafted runs. Footnotes/endnotes
/// in the source are carried over.
#[pyfunction]
#[pyo3(signature = (blueprint_path, source_path, output_path))]
fn transplant_body(
    blueprint_path: PathBuf,
    source_path: PathBuf,
    output_path: PathBuf,
) -> PyResult<()> {
    let mut bp = open(&blueprint_path).map_err(map_err)?;
    let src = open(&source_path).map_err(map_err)?;
    core_transplant(&mut bp, &src).map_err(map_err)?;
    save(&bp, &output_path).map_err(map_err)?;
    Ok(())
}

/// `crisp_docx` Python module.
#[pymodule]
fn crisp_docx(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyNotesKind>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(strip_rsids, m)?)?;
    m.add_function(wrap_pyfunction!(normalize_tags, m)?)?;
    m.add_function(wrap_pyfunction!(convert_notes_kind, m)?)?;
    m.add_function(wrap_pyfunction!(inject_footnotes, m)?)?;
    m.add_function(wrap_pyfunction!(transplant_body, m)?)?;
    Ok(())
}

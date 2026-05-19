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
    analyze_blueprint as core_analyze, apply_heading_inferences as core_apply_inferences,
    apply_style_mapping as core_apply_styles, convert_notes_kind as core_convert,
    infer_heading_levels as core_infer, inject_footnotes as core_inject,
    normalize_tags as core_normalize, open, save, strip_paragraph_bold as core_unbold,
    strip_rsids as core_strip, transplant_body as core_transplant, NotesKind, StyleIndex,
    StyleMapper,
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

/// Strip cosmetic whole-paragraph bold from `<w:p>` elements. Returns
/// the number of paragraphs unbolded.
#[pyfunction]
#[pyo3(signature = (path, output=None))]
fn strip_paragraph_bold(path: PathBuf, output: Option<PathBuf>) -> PyResult<usize> {
    let mut pkg = open(&path).map_err(map_err)?;
    let n = core_unbold(&mut pkg).map_err(map_err)?;
    let dest = output.unwrap_or(path);
    save(&pkg, &dest).map_err(map_err)?;
    Ok(n)
}

/// Strip non-semantic `<w:rPr>` children from non-footnote-ref runs. The
/// KEEP set is the same KEEP_RPR_TAGS that
/// `format_transplant.py::DocumentBuilder._clean_runs` uses. Returns the
/// total number of rPr children removed.
#[pyfunction]
#[pyo3(signature = (path, output=None))]
fn clean_runs(path: PathBuf, output: Option<PathBuf>) -> PyResult<usize> {
    let mut pkg = open(&path).map_err(map_err)?;
    let n = crisp_docx_core::clean_runs(&mut pkg).map_err(map_err)?;
    let dest = output.unwrap_or(path);
    save(&pkg, &dest).map_err(map_err)?;
    Ok(n)
}

/// Read all of a blueprint's metadata (page geometry, default font/size,
/// styles, footnote-marker format) in one call. Returns a Python dict
/// shaped:
///
/// ```python
/// {
///     "default_font": str,
///     "default_font_size_pt": float,
///     "sections": [
///         {
///             "index": int, "page_width_pt": Optional[float],
///             "page_height_pt": Optional[float],
///             "left_margin_pt": Optional[float], ...
///         },
///         ...
///     ],
///     "styles": [
///         {"name": str, "style_id": str, "type_val": int,
///          "outline_level": Optional[int]},
///         ...
///     ],
///     "body_para_style_names": [str, ...],
///     "footnote_format": {
///         "has_marker_rpr": bool,
///         "separator": Optional[str],
///     },
/// }
/// ```
#[pyfunction]
fn analyze_blueprint(py: Python<'_>, path: PathBuf) -> PyResult<PyObject> {
    let pkg = open(&path).map_err(map_err)?;
    let s = core_analyze(&pkg).map_err(map_err)?;
    let dict = pyo3::types::PyDict::new_bound(py);
    dict.set_item("default_font", s.default_font)?;
    dict.set_item("default_font_size_pt", s.default_font_size_pt)?;

    let sections = pyo3::types::PyList::empty_bound(py);
    for sect in s.sections {
        let d = pyo3::types::PyDict::new_bound(py);
        d.set_item("index", sect.index)?;
        d.set_item("page_width_pt", sect.page_width_pt)?;
        d.set_item("page_height_pt", sect.page_height_pt)?;
        d.set_item("left_margin_pt", sect.left_margin_pt)?;
        d.set_item("right_margin_pt", sect.right_margin_pt)?;
        d.set_item("top_margin_pt", sect.top_margin_pt)?;
        d.set_item("bottom_margin_pt", sect.bottom_margin_pt)?;
        d.set_item("gutter_pt", sect.gutter_pt)?;
        d.set_item("header_distance_pt", sect.header_distance_pt)?;
        d.set_item("footer_distance_pt", sect.footer_distance_pt)?;
        d.set_item("orientation", sect.orientation)?;
        sections.append(d)?;
    }
    dict.set_item("sections", sections)?;

    let styles = pyo3::types::PyList::empty_bound(py);
    for (_name, info) in s.styles.styles {
        let d = pyo3::types::PyDict::new_bound(py);
        d.set_item("name", info.name)?;
        d.set_item("style_id", info.style_id)?;
        d.set_item("type_val", info.type_val)?;
        d.set_item("outline_level", info.outline_level)?;
        styles.append(d)?;
    }
    dict.set_item("styles", styles)?;
    dict.set_item(
        "body_para_style_names",
        s.styles
            .body_para_style_names
            .into_iter()
            .collect::<Vec<_>>(),
    )?;

    let fn_dict = pyo3::types::PyDict::new_bound(py);
    fn_dict.set_item("has_marker_rpr", s.footnote_format.marker_rpr_xml.is_some())?;
    fn_dict.set_item("separator", s.footnote_format.separator)?;
    dict.set_item("footnote_format", fn_dict)?;

    Ok(dict.into())
}

/// Remap source pStyle styleIds to blueprint equivalents. Reads styles
/// from both packages so the styleId↔name translation works.
///
/// Returns the number of paragraphs whose pStyle was rewritten.
#[pyfunction]
#[pyo3(signature = (path, blueprint_path, source_styles_path=None))]
fn apply_style_mapping(
    path: PathBuf,
    blueprint_path: PathBuf,
    source_styles_path: Option<PathBuf>,
) -> PyResult<usize> {
    let mut pkg = open(&path).map_err(map_err)?;
    let bp = open(&blueprint_path).map_err(map_err)?;
    let bp_idx = StyleIndex::from_package(&bp).map_err(map_err)?;
    let src_idx = match source_styles_path {
        Some(p) => {
            let src = open(&p).map_err(map_err)?;
            StyleIndex::from_package(&src).map_err(map_err)?
        }
        // Fall back to using `path`'s own styles as the source index.
        None => StyleIndex::from_package(&pkg).map_err(map_err)?,
    };
    let mapper = StyleMapper::new(&bp_idx, std::collections::HashMap::new());
    let n = core_apply_styles(&mut pkg, &mapper, &src_idx, &bp_idx).map_err(map_err)?;
    save(&pkg, &path).map_err(map_err)?;
    Ok(n)
}

/// Infer heading levels for body paragraphs from direct formatting AND
/// (optionally) rewrite each flagged paragraph's `<w:pStyle>` to the
/// blueprint's heading-style styleId.
///
/// Returns a list of `(paragraph_index, heading_level, effective_size_pt,
/// preview)` tuples — one per inferred heading. If `apply_to_blueprint`
/// is provided, the document is also updated in place; otherwise it's
/// left untouched and only the inferences are returned.
#[pyfunction]
#[pyo3(signature = (path, source_path=None, apply_to_blueprint=None))]
fn infer_heading_levels(
    path: PathBuf,
    source_path: Option<PathBuf>,
    apply_to_blueprint: Option<PathBuf>,
) -> PyResult<Vec<(usize, u8, f64, String)>> {
    let mut pkg = open(&path).map_err(map_err)?;
    let src_idx = match source_path {
        Some(p) => {
            let s = open(&p).map_err(map_err)?;
            Some(StyleIndex::from_package(&s).map_err(map_err)?)
        }
        None => None,
    };
    let inferences = core_infer(&pkg, src_idx.as_ref()).map_err(map_err)?;
    if let Some(bp_path) = apply_to_blueprint {
        let bp = open(&bp_path).map_err(map_err)?;
        let bp_idx = StyleIndex::from_package(&bp).map_err(map_err)?;
        core_apply_inferences(&mut pkg, &inferences, &bp_idx).map_err(map_err)?;
        save(&pkg, &path).map_err(map_err)?;
    }
    Ok(inferences
        .into_iter()
        .map(|i| {
            (
                i.paragraph_index,
                i.heading_level,
                i.effective_size_pt,
                i.preview,
            )
        })
        .collect())
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
    m.add_function(wrap_pyfunction!(strip_paragraph_bold, m)?)?;
    m.add_function(wrap_pyfunction!(clean_runs, m)?)?;
    m.add_function(wrap_pyfunction!(analyze_blueprint, m)?)?;
    m.add_function(wrap_pyfunction!(apply_style_mapping, m)?)?;
    m.add_function(wrap_pyfunction!(infer_heading_levels, m)?)?;
    Ok(())
}

//! `crisp-docx-core` — pure Rust primitives for surgical edits on `.docx`
//! (OOXML) packages.
//!
//! See [`PLAN.md`](https://github.com/CrispStrobe/crisp-docx/blob/main/PLAN.md)
//! for scope and the operations roadmap. The public API surface evolves
//! across phases; everything `pub use`'d from this module is considered
//! stable for the current minor version.
//!
//! # Example
//!
//! ```no_run
//! use crisp_docx_core::{open, save, strip_rsids};
//!
//! let mut pkg = open("paper.docx")?;
//! let removed = strip_rsids(&mut pkg)?;
//! eprintln!("stripped {removed} rsid/paraId attributes");
//! save(&pkg, "paper.clean.docx")?;
//! # Ok::<(), crisp_docx_core::Error>(())
//! ```

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod ns;
pub mod package;

mod blueprint;
mod clean_runs;
mod footnote_format;
mod heading_inference;
mod normalize_tags;
mod note_injection;
mod notes_kind;
mod rsid_strip;
mod strip_paragraph_bold;
mod style_classify;
mod style_mapper;
mod transplant;

pub use blueprint::{analyze_blueprint, BlueprintSchema, SectionInfo};
pub use clean_runs::clean_runs;
pub use error::{Error, Result};
pub use footnote_format::{apply_footnote_format, extract_footnote_format, FootnoteFormat};
pub use heading_inference::{apply_heading_inferences, infer_heading_levels, HeadingInference};
pub use normalize_tags::normalize_tags;
pub use note_injection::{inject_footnotes, InjectionReport};
pub use notes_kind::{convert_notes_kind, NotesKind};
pub use package::Package;
pub use rsid_strip::strip_rsids;
pub use strip_paragraph_bold::strip_paragraph_bold;
pub use style_classify::{classify_style, SemanticClass, StyleClassification};
pub use style_mapper::{apply_style_mapping, StyleIndex, StyleInfo, StyleMapper};
pub use transplant::transplant_body;

use std::path::Path;

/// Open a `.docx` package from disk into an in-memory [`Package`].
pub fn open(path: impl AsRef<Path>) -> Result<Package> {
    Package::open(path)
}

/// Save an in-memory [`Package`] back to disk as a `.docx` zip.
pub fn save(pkg: &Package, path: impl AsRef<Path>) -> Result<()> {
    pkg.save(path)
}

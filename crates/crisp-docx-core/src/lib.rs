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

mod normalize_tags;
mod notes_kind;
mod rsid_strip;

pub use error::{Error, Result};
pub use normalize_tags::normalize_tags;
pub use notes_kind::{convert_notes_kind, NotesKind};
pub use package::Package;
pub use rsid_strip::strip_rsids;

use std::path::Path;

/// Open a `.docx` package from disk into an in-memory [`Package`].
pub fn open(path: impl AsRef<Path>) -> Result<Package> {
    Package::open(path)
}

/// Save an in-memory [`Package`] back to disk as a `.docx` zip.
pub fn save(pkg: &Package, path: impl AsRef<Path>) -> Result<()> {
    pkg.save(path)
}

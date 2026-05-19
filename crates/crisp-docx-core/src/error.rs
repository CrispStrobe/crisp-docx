//! Error types for the core crate.

use std::path::PathBuf;

/// Convenience alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// All failures the core library can produce.
///
/// The variants are intentionally coarse — most consumers want to know
/// "did this docx survive the operation?" rather than the specific OOXML
/// element that tripped a parse.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// I/O error reading or writing the docx zip on disk.
    #[error("I/O at {path}: {source}")]
    Io {
        /// Path being read or written when the error occurred.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The file exists but isn't a usable docx (corrupt zip or missing
    /// required parts).
    #[error("not a usable docx package at {path}: {reason}")]
    InvalidPackage {
        /// Path that failed validation.
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },

    /// A zip-level error from the underlying crate.
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// An XML parse error while reading a part.
    #[error("XML parse error in {part}: {source}")]
    XmlParse {
        /// Which part of the zip the error came from (e.g. `word/document.xml`).
        part: String,
        /// Underlying parser error.
        #[source]
        source: quick_xml::Error,
    },

    /// A bare I/O error not tied to a known path (e.g. from a writer).
    #[error("I/O: {0}")]
    Plain(#[from] std::io::Error),
}

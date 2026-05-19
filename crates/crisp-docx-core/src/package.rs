//! In-memory representation of a `.docx` package as a name → bytes map.
//!
//! A `Package` is just an ordered collection of parts. Loading reads every
//! entry from the zip into memory (docx files are typically well under 5 MB
//! for textual content), saving writes them back out. All higher-level
//! operations in this crate mutate the parts in place.
//!
//! Order is preserved because some consumers (including Word in edge
//! cases) seem mildly sensitive to part order on first open. We use an
//! `IndexMap`-style backing via `Vec<(String, Vec<u8>)>` to keep things
//! deterministic.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// A `.docx` package: a map of part-path → raw bytes.
///
/// Cloning is cheap-ish (it clones each part's `Vec<u8>`); generally
/// callers mutate parts via [`Self::get_part_mut`] or replace them via
/// [`Self::set_part`].
#[derive(Debug, Clone, Default)]
pub struct Package {
    /// Insertion order matters; we preserve it on save.
    entries: Vec<Entry>,
    /// Where the package was loaded from, used in error messages.
    source: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    data: Vec<u8>,
}

impl Package {
    /// Construct an empty package.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a `.docx` from disk into memory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let f = File::open(path).map_err(|e| Error::Io {
            path: path.into(),
            source: e,
        })?;
        let mut zip = zip::ZipArchive::new(f)?;
        Self::from_archive(&mut zip, Some(path.into()))
    }

    /// Read a `.docx` from any in-memory buffer.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let cursor = Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(cursor)?;
        Self::from_archive(&mut zip, None)
    }

    fn from_archive<R: Read + Seek>(
        zip: &mut zip::ZipArchive<R>,
        source: Option<PathBuf>,
    ) -> Result<Self> {
        let mut entries = Vec::with_capacity(zip.len());
        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            let name = file.name().to_string();
            let mut data = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut data)?;
            entries.push(Entry { name, data });
        }
        if !entries.iter().any(|e| e.name == "[Content_Types].xml") {
            return Err(Error::InvalidPackage {
                path: source.unwrap_or_default(),
                reason: "missing [Content_Types].xml".into(),
            });
        }
        Ok(Self { entries, source })
    }

    /// Write the package back out to disk as a `.docx` zip.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        // Write to a sibling temp file then rename — avoids corrupting the
        // destination if something fails mid-write.
        let tmp = path.with_extension({
            let mut ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            ext.push_str(".tmp");
            ext
        });
        {
            let f = File::create(&tmp).map_err(|e| Error::Io {
                path: tmp.clone(),
                source: e,
            })?;
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for entry in &self.entries {
                zw.start_file(&entry.name, opts)?;
                zw.write_all(&entry.data).map_err(|e| Error::Io {
                    path: tmp.clone(),
                    source: e,
                })?;
            }
            zw.finish()?;
        }
        std::fs::rename(&tmp, path).map_err(|e| Error::Io {
            path: path.into(),
            source: e,
        })?;
        Ok(())
    }

    /// Borrow a part's bytes by path. `None` if the part isn't present.
    pub fn get_part(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.data.as_slice())
    }

    /// Mutably borrow a part's bytes by path. `None` if absent.
    pub fn get_part_mut(&mut self, name: &str) -> Option<&mut Vec<u8>> {
        self.entries
            .iter_mut()
            .find(|e| e.name == name)
            .map(|e| &mut e.data)
    }

    /// Replace (or insert) a part at the given path. The part is appended
    /// at the end if it didn't already exist.
    pub fn set_part(&mut self, name: impl Into<String>, data: Vec<u8>) {
        let name = name.into();
        if let Some(existing) = self.entries.iter_mut().find(|e| e.name == name) {
            existing.data = data;
        } else {
            self.entries.push(Entry { name, data });
        }
    }

    /// Rename a part from `from` to `to`. Returns whether the rename took place.
    pub fn rename_part(&mut self, from: &str, to: impl Into<String>) -> bool {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.name == from) {
            entry.name = to.into();
            true
        } else {
            false
        }
    }

    /// Remove a part. Returns whether anything was removed.
    pub fn remove_part(&mut self, name: &str) -> bool {
        let len_before = self.entries.len();
        self.entries.retain(|e| e.name != name);
        self.entries.len() != len_before
    }

    /// Iterate over (path, bytes) pairs in insertion order.
    pub fn parts(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries
            .iter()
            .map(|e| (e.name.as_str(), e.data.as_slice()))
    }

    /// Source path the package was loaded from (if any). Used by error
    /// messages and `Display` impls.
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    /// Snapshot view as a `BTreeMap` for deterministic ordering in tests.
    pub fn as_btree(&self) -> BTreeMap<&str, &[u8]> {
        self.parts().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn make_minimal_docx() -> Vec<u8> {
        let buf = Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(buf);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("[Content_Types].xml", opts).unwrap();
        zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#)
            .unwrap();
        zw.start_file("word/document.xml", opts).unwrap();
        zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>"#)
            .unwrap();
        zw.finish().unwrap().into_inner()
    }

    #[test]
    fn loads_and_round_trips() {
        let bytes = make_minimal_docx();
        let pkg = Package::from_bytes(&bytes).unwrap();
        assert!(pkg.get_part("word/document.xml").is_some());
        assert!(pkg.get_part("[Content_Types].xml").is_some());
        assert_eq!(pkg.parts().count(), 2);
    }

    #[test]
    fn rejects_non_opc_zip() {
        let buf = Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(buf);
        zw.start_file("hello.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"hi").unwrap();
        let bytes = zw.finish().unwrap().into_inner();
        assert!(matches!(
            Package::from_bytes(&bytes),
            Err(Error::InvalidPackage { .. })
        ));
    }

    #[test]
    fn set_and_rename_part() {
        let mut pkg = Package::from_bytes(&make_minimal_docx()).unwrap();
        pkg.set_part("word/extra.xml", b"<x/>".to_vec());
        assert_eq!(pkg.get_part("word/extra.xml"), Some(b"<x/>".as_slice()));
        assert!(pkg.rename_part("word/extra.xml", "word/renamed.xml"));
        assert!(pkg.get_part("word/extra.xml").is_none());
        assert_eq!(pkg.get_part("word/renamed.xml"), Some(b"<x/>".as_slice()));
    }
}

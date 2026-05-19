# crisp-docx

Cross-platform OOXML (`.docx`) surgery — Rust core + CLI + Python bindings.

Sister project to [`CrispStrobe/CrispTranslator`](https://github.com/CrispStrobe/CrispTranslator)
(Python toolkit for translation, format transplant, and footnote/endnote
work) and [`CrispStrobe/CrispSorter`](https://github.com/CrispStrobe/CrispSorter)
(Tauri 2 desktop app — the eventual UI host).

This repository owns the **language-agnostic core** that operates on
`.docx` packages at the OOXML XML level. It produces:

- **`crisp-docx`** — a `cargo install`-able CLI binary (~10 MB, instant
  startup, zero runtime dependencies).
- **`crisp_docx`** — a `pip install`-able Python wheel built with
  [maturin](https://github.com/PyO3/maturin) so the existing Python
  tooling can opt into the Rust-fast implementations.
- **`crisp-docx-core`** — a Rust library crate consumable from
  CrispSorter's Tauri workspace or any other Rust application.

**Status:** scaffolding. See [`PLAN.md`](./PLAN.md) for the phased
execution plan and current progress.

## Operations covered

1. **Clean** — strip `w:rsidR` / `w:rsidRPr` / `w14:paraId` / etc.
   tracking attributes. The most common cure for Word's
   "_Word found unreadable content_" recovery dialog.
2. **Normalize tags** — rewrite Apple `textutil`'s non-standard
   element names (`w:sz-cs` → `w:szCs`, `w:b-cs` → `w:bCs`, `w:i-cs` → `w:iCs`).
3. **Notes-kind conversion** — switch a document between Word footnotes
   and endnotes (renames the part, rewrites references, fixes
   content-types and rels).
4. **Footnote-reference injection** — given inline `[N]` markers in body
   text, split runs at marker sites and append matching
   `<w:footnote w:id="N">` entries.
5. **Transplant** — clone a blueprint docx's package, drop the body
   content, and graft in paragraphs from a source document while
   preserving the trailing `<w:sectPr>` and stripping tracking attrs.

Pre-existing Python implementations of all five operations live in
[`CrispStrobe/CrispTranslator`](https://github.com/CrispStrobe/CrispTranslator).
This crate ports them, one at a time, with byte-level fixture parity.

## Quickstart (Rust)

```bash
cargo install --git https://github.com/CrispStrobe/crisp-docx crisp-docx-cli
crisp-docx clean broken.docx
crisp-docx notes-kind paper.docx --to endnotes
```

## Quickstart (Python)

```bash
pip install crisp-docx          # once published
```

```python
from crisp_docx import strip_rsids, convert_notes_kind, NotesKind

strip_rsids("paper.docx")                                 # in place
convert_notes_kind("paper.docx", NotesKind.Endnotes)
```

## Build from source

```bash
git clone https://github.com/CrispStrobe/crisp-docx
cd crisp-docx
cargo build --workspace --release
./target/release/crisp-docx --help
```

For the Python wheel:

```bash
pip install maturin
maturin develop --release --manifest-path crates/crisp-docx-py/Cargo.toml
python -c "import crisp_docx; print(crisp_docx.__doc__)"
```

## License

GNU Affero General Public License v3.0 or later. See [`LICENSE`](./LICENSE).

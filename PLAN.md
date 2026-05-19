# PLAN.md — crisp-docx

A methodical execution plan for porting the OOXML core of
`CrispStrobe/CrispTranslator` to Rust so it can ship as cross-platform
binaries, slot into `CrispStrobe/CrispSorter` (Tauri 2 workspace), and stay
usable from the existing Python tooling via PyO3.

This document is **the** source of truth for what gets built, in what
order, and against which acceptance criteria. Tick boxes are updated as
phases land.

---

## 0. Why this exists

`CrispStrobe/CrispTranslator` already has three working tools in Python
(`translator.py`, `format_transplant.py`, `debug_format.py`) plus the
freshly added `rtf_to_docx_endnotes.py` + unified `docxtool.py`. PyInstaller
binaries exist but are ~100 MB and start in a few seconds.

We need:

1. **Zero-Python deployment** for end users who can't pip-install.
2. **Tauri integration** so the operations are callable from the
   `crisp-docx`/SvelteKit UI in CrispSorter without shelling out.
3. **No regression** in any feature the Python tools currently expose; the
   Python ecosystem must keep working (Gradio UI, LLM clients, etc.).

The bottleneck of the existing tool is **OOXML knowledge**, not Python's
speed. So the port is value-for-money only if the resulting Rust library
is reusable from both standalone binaries and the existing Python code.

---

## 1. Scope of this repo

**In scope** (the operations covered here):

- ZIP read / write of `.docx` packages with byte-level fidelity.
- XML manipulation of `word/document.xml`, `word/footnotes.xml`,
  `word/endnotes.xml`, `word/styles.xml`, `[Content_Types].xml`, and the
  relationship parts.
- The five primitive operations from the Python tools:
  1. `strip_rsids` — remove `w14:paraId`, `w14:textId`, `w:rsidR`,
     `w:rsidRPr`, `w:rsidDel`, `w:rsidRDefault`, `w:rsidP`, `w:rsidTr`,
     `w:rsidSect` from every `<w:p>` and `<w:r>`. Cure for Word's
     "found unreadable content" recovery dialog.
  2. `normalize_tags` — rewrite Apple `textutil`'s non-standard local
     names (`w:sz-cs` → `w:szCs`, `w:b-cs` → `w:bCs`, `w:i-cs` → `w:iCs`).
  3. `footnotes_to_endnotes` — rename the part, rewrite all
     `<w:footnoteReference>` to `<w:endnoteReference>` and friends in
     `document.xml`, fix `[Content_Types].xml` overrides, fix the rels.
  4. `inject_footnote_references` — given inline `[N]` markers in
     `document.xml`, split the containing run and insert a
     `<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference w:id="N"/></w:r>`,
     plus append a corresponding `<w:footnote w:id="N">` to
     `word/footnotes.xml`.
  5. `transplant_body` — clone blueprint docx, drop body children except
     the trailing `<w:sectPr>`, insert source paragraphs *before* that
     sectPr. Per the FormatTransplant playbook: `_strip_tracking_attrs`
     on every transferred node, `xml:space="preserve"` for whitespace,
     direct `_blob = ...` for the footnote part.

**Explicitly out of scope** for the first ship:

- The full Style Guide LLM pass (`format_transplant.py` lines ~2400–2700).
  This stays in Python for now (phase E below).
- The neural alignment / translator backends (separate problem domain).
- A native UI. CrispSorter is the UI host.
- The full FormatTransplant `BlueprintAnalyzer` / `StyleMapper` /
  `ContentExtractor` orchestration. This sits *above* the primitives and
  is a follow-up (phase D).

---

## 2. Architecture

```
crisp-docx/                                ← this repo
├── Cargo.toml                              workspace root
├── crates/
│   ├── crisp-docx-core/                    pure library
│   │   ├── src/
│   │   │   ├── lib.rs                       public API surface
│   │   │   ├── package.rs                   docx (zip) read/write
│   │   │   ├── ns.rs                        OOXML namespace constants
│   │   │   ├── rsid_strip.rs                primitive (1)
│   │   │   ├── normalize_tags.rs            primitive (2)
│   │   │   ├── notes_kind.rs                primitive (3) footnotes↔endnotes
│   │   │   ├── note_injection.rs            primitive (4) — phase B
│   │   │   ├── transplant.rs                primitive (5) — phase D
│   │   │   └── error.rs                     thiserror Error enum
│   │   └── tests/
│   │       ├── rsid_strip.rs                fixture-based unit tests
│   │       └── notes_kind.rs
│   ├── crisp-docx-cli/
│   │   ├── src/main.rs                      clap dispatcher
│   │   └── README.md                        usage examples
│   └── crisp-docx-py/
│       ├── Cargo.toml                       cdylib + pyo3
│       ├── pyproject.toml                   maturin
│       ├── src/lib.rs                       #[pymodule] crisp_docx
│       └── tests/test_bindings.py           pytest against the built wheel
├── PLAN.md                                  this file
├── README.md
├── LICENSE                                  AGPL-3.0-or-later, matching CrispTranslator
├── .gitignore
└── .github/workflows/ci.yml                 fmt + clippy + test on linux/macos/windows
```

`crisp-docx-core` is the source of truth. Every operation is a free
function or a struct method on top of an in-memory `Package` (`HashMap<String,
Vec<u8>>`) which is loaded from / written to a zip file at the boundaries.

---

## 3. Public API surface (target)

```rust
// crisp-docx-core/src/lib.rs

pub use error::{Error, Result};
pub use package::Package;

/// Strip rsid/paraId tracking attrs from every <w:p>/<w:r> in the document
/// body, footnotes, and endnotes parts. Returns the count of attributes
/// removed. Mutates `pkg` in place.
pub fn strip_rsids(pkg: &mut Package) -> Result<usize>;

/// Rewrite Apple textutil's non-OOXML element local names
/// (w:sz-cs → w:szCs, w:b-cs → w:bCs, w:i-cs → w:iCs) in document/notes
/// parts. Returns the count of rewrites.
pub fn normalize_tags(pkg: &mut Package) -> Result<usize>;

/// Convert all footnotes in `pkg` to endnotes, or the reverse.
pub fn convert_notes_kind(pkg: &mut Package, target: NotesKind) -> Result<()>;

#[non_exhaustive]
pub enum NotesKind {
    Footnotes,
    Endnotes,
}

/// Open a docx file from disk.
pub fn open(path: impl AsRef<Path>) -> Result<Package>;

/// Save a docx package back to disk.
pub fn save(pkg: &Package, path: impl AsRef<Path>) -> Result<()>;
```

CLI surface (clap):

```
crisp-docx clean <input.docx> [-o out.docx] [--also-normalize-tags] [--dry-run]
crisp-docx notes-kind <input.docx> --to footnotes|endnotes [-o out.docx]
crisp-docx inspect <input.docx>                  ← human-readable summary
```

Python binding surface (PyO3):

```python
from crisp_docx import strip_rsids, normalize_tags, convert_notes_kind, NotesKind

n = strip_rsids("paper.docx")                      # in place
n = strip_rsids("paper.docx", output="clean.docx") # to new path
convert_notes_kind("paper.docx", NotesKind.Endnotes)
```

---

## 4. Phases

Each phase is **independently shippable** (its own PR + tag).
Acceptance criteria are concrete; tick them off in this file as you go.

### ☐ Phase A — Repo scaffold (this PR)

- [x] Cargo workspace with three members.
- [ ] `package.rs` — `Package` struct + `open()` / `save()`.
- [ ] `ns.rs` — namespace constants.
- [ ] `error.rs` — `thiserror` `Error` enum with the basic variants.
- [ ] `lib.rs` exporting the above.
- [ ] CLI `main.rs` with `clap` subcommands wired but stubbed.
- [ ] PyO3 `lib.rs` with `#[pymodule]` + one trivial function so the wheel builds.
- [ ] `cargo check --workspace` clean.
- [ ] CI workflow runs `cargo fmt --check`, `cargo clippy -- -D warnings`,
      `cargo test --workspace` on linux/macos/windows.

Acceptance: `cargo build --workspace` succeeds on all three OSes; the
empty CLI prints help; `maturin build` produces a wheel that
`import crisp_docx` finds.

### ☐ Phase B — Primitives 1, 2, 3

- [ ] `strip_rsids` with Python-equivalent tests using identical fixtures
      (port `tests/test_rsid_strip.py` to `tests/rsid_strip.rs`).
- [ ] `normalize_tags`.
- [ ] `convert_notes_kind(NotesKind::Endnotes)` and the reverse,
      mirroring `footnotes_to_endnotes` in `rtf_to_docx_endnotes.py`.
- [ ] CLI subcommands `clean` and `notes-kind` go live.
- [ ] PyO3 binds the three primitives.

Acceptance: existing `docxtool clean --dry-run` semantics reproduce on
the Rust side; cross-checked against the Python implementation on a
fixture corpus (the `Vielfalt cs15` test file plus a half-dozen synthetic
docx zips covering edge cases).

### ☐ Phase C — Bind into Python and verify back-compat

- [ ] Replace the rsid-strip path in `docxtool clean` with a call into
      `crisp_docx` when the wheel is available, falling back to the pure
      Python implementation otherwise.
- [ ] All 36 existing unit tests in CrispTranslator still pass with the
      Rust-backed path enabled.
- [ ] Add a `BENCH.md` measuring rsid strip on a 200 KB docx — Python
      vs Rust, vs Rust through PyO3 — so we know the binding overhead.

Acceptance: zero behaviour change for existing users; PyO3 path is
opt-in via a feature flag.

### ☐ Phase D — Primitives 4 & 5 (footnote injection + transplant)

- [ ] `note_injection.rs` — split runs at `[N]` and append `<w:footnote>`
      entries. Includes a runs-can-be-fragmented-across-elements
      walker (port of the `_normalize_fn_separator` insight from
      `format_transplant.py:1962`).
- [ ] `transplant.rs` — clone-blueprint, replace-body, preserve the final
      `<w:sectPr>` (`format_transplant.py:1519`). Strip rsids on insert
      (reuse phase B primitive). Preserve `xml:space="preserve"` on every
      `<w:t>` with leading/trailing whitespace.
- [ ] CLI subcommand `inject-footnotes` and `transplant`.
- [ ] PyO3 binds both.

Acceptance: round-trip the four fixtures from `CrispStrobe/FormatTransplant`
HF Space and diff the resulting bytes against the Python output. Any
diff documented and either reduced to zero or recorded as known
divergence with rationale.

### ☐ Phase E — Optional: rewire CrispSorter

- [ ] Add `crisp-docx-core` as a workspace dep in CrispSorter.
- [ ] Wire a Tauri command `tauri_docx_clean(path: PathBuf)` and a
      SvelteKit `+page.svelte` that exposes it.
- [ ] No release of CrispSorter yet — feature-flagged behind
      `crispsorter --features experimental-docx`.

Acceptance: dev-mode CrispSorter loads a docx and round-trips it via the
Rust core.

### ☐ Phase F — Distribution

- [ ] CI builds CLI binaries for x86_64-unknown-linux-gnu,
      aarch64-apple-darwin, x86_64-apple-darwin, x86_64-pc-windows-msvc.
- [ ] Wheels built by `maturin` for the same OS/arch matrix and CPython
      3.10/3.11/3.12.
- [ ] First `v0.1.0` GitHub release with attached binaries.
- [ ] README has install instructions (`cargo install crisp-docx-cli`
      and `pip install crisp-docx`).

Acceptance: `curl -L .../crisp-docx-linux | sh` from a clean container
runs `crisp-docx clean foo.docx` to completion.

---

## 5. Test fixtures

A small **fixtures corpus** lives at `crates/crisp-docx-core/tests/fixtures/`.
Each is a real or synthesized docx, paired with a Python script in the
sibling CrispTranslator repo that produces a reference output for
byte-diffing.

Initial fixtures we need:

- `minimal.docx` — single empty paragraph, no notes
- `with_rsids.docx` — paragraphs carrying every rsid attribute we strip
- `textutil_sample.docx` — Apple textutil output with `w:sz-cs` and other
  quirks
- `footnotes_46.docx` — the `Vielfalt cs15` corpus, anonymised
- `blueprint_*.docx` / `source_*.docx` — pairs for the transplant
  acceptance test (phase D)

A `gen_fixtures.py` script under CrispTranslator pins exactly how each
fixture is produced, so a future contributor can regenerate them
without guessing.

---

## 6. Style & conventions

- **Errors**: `thiserror::Error` enum in `crisp-docx-core::error`. Public
  API returns `Result<T, Error>`. CLI converts to `anyhow::Result` at the
  boundary.
- **XML**: prefer `quick-xml` with the streaming reader for input; build
  output as `Vec<u8>` with the writer. Avoid DOM round-trips unless we're
  consciously rewriting the whole tree — that's what trips Word with
  namespace re-ordering (we hit this with Python's ET; PEP this into
  unit tests).
- **No allocations on the happy path** when not necessary; this code runs
  on big documents.
- **No `unwrap()` outside tests.**
- **`#[deny(missing_docs)]` on `crisp-docx-core::lib`** once Phase B lands.
- **MSRV** in `rust-version` = 1.75. CI checks this.

---

## 7. Open questions

These need decisions before they become blockers; they are intentionally
*not* in any phase yet.

- **Wheel distribution channel**: PyPI, or GitHub Releases only? PyPI
  costs nothing but means committing to a name; the wheel is opt-in.
  *Lean toward PyPI as `crisp-docx` once Phase C is green.*
- **Crate naming on crates.io**: `crisp-docx` is taken in some
  ecosystems' nomenclature. *Verify availability in Phase F.*
- **CrispSorter integration depth**: just Tauri commands, or a full
  SvelteKit panel? *Defer to Phase E; needs UX direction.*
- **AGPL boundary**: the wheel inherits AGPL; we should make sure
  PyO3-exposed APIs don't accidentally trigger AGPL obligations for
  Python users who only do CLI work. *Add a `LICENSE-EXCEPTIONS.md`
  during Phase F if needed.*

---

## 8. Done definition

The repo is "done for v0.1" when:

1. All phase A-D boxes ticked.
2. CLI binary on macOS arm64 is ≤ 12 MB stripped and starts in < 50 ms.
3. The Python bindings drop into CrispTranslator without behaviour change
   for any of its 36 existing unit tests.
4. Phase E is at least scaffolded in CrispSorter, behind a feature flag.

Beyond v0.1 the path widens to the LLM-driven editorial passes, the full
`StyleMapper` machinery, and the Gradio-equivalent SvelteKit UI in
CrispSorter. None of that is on the critical path for "ship cross-platform
binaries to non-Python users."

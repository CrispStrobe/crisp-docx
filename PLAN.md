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

### ✅ Phase A — Repo scaffold *(2026-05)*

- [x] Cargo workspace, now 6 members (core / cli / py / llm / align / translate-cli).
- [x] `package.rs` — `Package` with `open()` / `save()` round-trip + dual zip/in-memory init.
- [x] `ns.rs`, `error.rs`, `lib.rs`.
- [x] `crisp-docx` clap CLI binary with 9 subcommands (`clean`, `notes-kind`, `inject-footnotes`, `transplant`, `strip-paragraph-bold`, `analyze`, `infer-headings`, `inspect`, `check`).
- [x] PyO3 module surfaces 11 functions through `crisp_docx`.
- [x] CI matrix: fmt / clippy / test × ubuntu / macos / windows × py 3.10/3.11/3.12. Sibling CrispEmbed + CrispASR cloned by every job so cargo metadata can resolve optional path deps.

### ✅ Phase B — OOXML primitives 1–3 *(2026-05)*

- [x] `strip_rsids` — drops `w14:paraId`, `w14:textId`, `w:rsidR`, `w:rsidRPr`, `w:rsidDel`, `w:rsidRDefault`, `w:rsidP`, `w:rsidTr`, `w:rsidSect`.
- [x] `normalize_tags` — `w:sz-cs` → `w:szCs`, `w:b-cs` → `w:bCs`, `w:i-cs` → `w:iCs`.
- [x] `convert_notes_kind(pkg, NotesKind)` (both directions) — renames part, rewrites refs, updates content-types and rels.
- [x] CLI + PyO3 binds.

### ✅ Phase C — Python integration & back-compat *(2026-05)*

- [x] `crisp_docx` wheel built by maturin, smoke-tested through `analyze_blueprint`, `infer_heading_levels`, etc.
- [x] PARITY.md ledger pairs every Python primitive with its Rust port; CI parity harness exists.
- [x] Three Python bugs found and fixed upstream in CrispTranslator during the parity port (`strip_paragraph_bold` spurious `**ô**`, `cmd_check`'s bookmark allow-list / `_rels/.rels` base / optional `settings.xml`).

### ✅ Phase D — Primitives 4 & 5 + the rest of format_transplant *(2026-05)*

- [x] `inject_footnotes` — `[N]` marker splitter with footnote XML appended.
- [x] `transplant_body` — blueprint package + source body, with full pipeline: `clean_runs` + `strip_rsids` + `apply_footnote_format` + style mapping + heading inference.
- [x] Bonus ports beyond the original scope:
      - `classify_style` + `StyleMapper` + `StyleIndex` (multilingual heading classification, 9 languages)
      - `BlueprintAnalyzer` (sections + defaults + footnote format)
      - `infer_heading_levels` + `apply_heading_inferences` (bold/short-text + size clustering)
      - `paragraph_runs` (run-granularity IO with verbatim rPr bytes)
      - `paragraph_text` (text-only round-trip)
      - `check_package` (7-axis validity check; `cmd_check` port)

### ✅ Phase D′ — Transformer alignment & format preservation *(2026-05)*

Out of original scope but landed naturally:

- [x] CrispEmbed C API addition: `crispembed_encode_tokens` for per-token contextual embeddings from any encoder model (was previously ColBERT-only).
- [x] `crisp-docx-align` crate — pure-Rust SimAlign (argmax / intersection / itermax) over CrispEmbed embeddings.
- [x] `transfer_format_via_words` — generic format-id bridge that maps source-run rPr onto translated text via word alignment.
- [x] `translate_runs(model, src_runs, translated_text, strategy)` end-to-end convenience.

### ✅ Phase E — LLM translation pipeline *(2026-05)*

- [x] `crisp-docx-llm` crate with **12 LLM providers**: OpenAI, Anthropic, Ollama, Groq, OpenRouter, Together, Cerebras, Mistral, Nebius, Scaleway, Poe, Google (Gemini).
- [x] All OpenAI-compat providers share the same `OpenAiProvider::new(name, default_base)` impl.
- [x] `LlmTranslator` orchestrator with fallback chain.
- [x] Live-verified: Groq, OpenRouter, Together, Nebius, Scaleway all returned "Der Hund schläft." End-to-end Vielfalt cs15 translation: 61/61 paragraphs via Nebius.
- [x] **Offline NMT** via CrispASR (m2m100 / wmt21 / madlad / gemma4-e2b) under the `nmt` feature.
- [x] 12 wiremock unit tests + 12 env-gated live tests.

### ✅ Phase F — `crisp-translate-cli` end-to-end binary *(2026-05)*

- [x] `crisp-translate <input.docx> -o <output.docx> --target-lang DE --provider groq`.
- [x] `--dry-run`, `--concurrency N`, `--preserve-formatting` (under `--features align`), `--align-model <gguf>`.
- [x] Auto-pick scans env keys in cost/latency order; multiple `--provider` flags chain as fallbacks.

### ✅ Phase G — CrispSorter Tauri integration *(2026-05)*

- [x] Translate tab in the SvelteKit UI (`src/lib/components/Translate.svelte`).
- [x] Two Tauri commands (`translate_dry_run`, `translate_docx`) wrapping the workspace crates as path deps.
- [x] Streams `translate://progress` events to the UI; live throughput + ETA.
- [x] Provider key status pill, form-state persistence, 12-provider grouped dropdown.
- [x] **OS-keychain credential storage** for LLM API keys; one-time migration moves plain-text keys out of `settings.json`.

### 🟡 Phase H — Distribution

- [x] CI matrix (fmt / clippy / test / build-wheel) on linux × macos × windows × py 3.10/3.11/3.12.
- [x] PUBLISH.md checklist with the exact two-command crates.io publish flow.
- [x] Cargo.toml metadata audit — every crate has description / keywords / categories; sibling-dep crates explicitly `publish = false`.
- [ ] **Actually run `cargo publish -p crisp-docx-core` then `crisp-docx-cli`.** Needs the user's crates.io token (skipped from the agent session).
- [ ] **Tag `v0.1.0` and trigger the release.yml binaries job** so `curl -L | sh` installs work.
- [ ] **Upload Python wheel to PyPI** — extend `build-wheel` CI to push to PyPI on tag.

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

v0.1 is **shippable** — every phase A→G ticked. The remaining work
(Phase H — distribution) is operational: cargo publish, GitHub release
tags, PyPI upload. None of it requires new code.

The repo's actual scope grew well past the original v0.1 target. What
shipped includes:

1. ✅ 11 OOXML primitives (was 5).
2. ✅ CLI binary on macOS arm64 = 8.4 MB stripped (target was ≤ 12 MB), starts in < 30 ms.
3. ✅ Python bindings — 11 exposed functions, used by CrispTranslator's `docxtool clean --backend rust`.
4. ✅ CrispSorter Tauri integration with a full Translate tab — not behind a feature flag, shipped as a first-class feature.
5. ✅ Beyond-v0.1 work — 12 LLM providers + offline NMT + transformer alignment + format-preserving translation pipeline + OS-keychain credential storage.

## 9. What's pending and doable now

Doable from this repo's working tree, no external systems required:

- **Live-test the format-preservation v0.2 pipeline.** Run `crisp-translate --features align` against the Vielfalt cs15.docx with a real LLM + multilingual-MiniLM aligner, then open the output in Word and check bold/italic spans survived word-order reordering. This is the load-bearing test that confirms the alignment bridge actually works on real prose; we have unit tests but no end-to-end Word-renders-correctly check.
- **Tests for the `secrets` module in CrispSorter** — there's already a `keyring::mock::default_credential_builder` pattern used by `src/images/crisplens/secret.rs`. Mirror that for `src/secrets/mod.rs`.
- **Surface `secrets_list_accounts`** so the Settings UI can show "you have keys stored for: OpenAI, Groq, Nebius" and let users delete individual ones without going through Keychain Access directly.
- **CHANGELOG.md** for both repos — none exists yet.
- **Document the workspace.dependencies pattern in CONTRIBUTING.md** so the next person to add a crate doesn't have to reverse-engineer the path-dep + version trick.

Doable but needs an external nudge (you, the human, runs a command):

- **Publish `crisp-docx-core` + `crisp-docx-cli` to crates.io.** PUBLISH.md has the two-command recipe; you run `cargo login` then ask me to continue. Same for PyPI — needs a token.
- **Tag `v0.1.0`.** Triggers the existing release.yml binary builds.

Out of scope (needs a separate decision):

- **Publish CrispEmbed + CrispASR to crates.io** so `crisp-docx-llm`, `crisp-docx-align`, `crisp-translate-cli` can drop their `publish = false` and ship.
- **Refactor the Provider trait to not pass src/tgt langs as free-form strings.** Right now NMT does its own name→code lookup; LLM prompts use the strings verbatim. A typed lang param would be cleaner but no consumer is asking for it.

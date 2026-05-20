# crisp-docx

Cross-platform OOXML (`.docx`) surgery + a complete LLM/NMT document
translation pipeline — all Rust, all offline-capable.

```text
┌─ Word document ──────────────────────────────────────────────────┐
│                                                                  │
│  paragraphs (text + run-level rPr + footnote refs + bookmarks)   │
│                                                                  │
│      ┌──────────────────┐    ┌──────────────────┐                │
│      │ crisp-docx-core  │ ─→ │ crisp-docx-llm   │ ─→ translated  │
│      │  • clean         │    │  • OpenAI / etc. │   paragraphs   │
│      │  • notes-kind    │    │  • Anthropic     │                │
│      │  • transplant    │    │  • Ollama        │                │
│      │  • extract runs  │    │  • CrispASR NMT  │                │
│      │  • replace runs  │    └──────────────────┘                │
│      └──────────────────┘                                        │
│                                ┌──────────────────┐              │
│                                │ crisp-docx-align │ → re-map     │
│                                │  • SimAlign via  │   bold/italic│
│                                │    CrispEmbed    │   spans      │
│                                └──────────────────┘              │
│                                                                  │
└─ Translated Word document ───────────────────────────────────────┘
```

Sister project to:
- [`CrispStrobe/CrispTranslator`](https://github.com/CrispStrobe/CrispTranslator) — Python ancestor; this Rust port has feature parity with most of its OOXML primitives. See [`PARITY.md`](./PARITY.md).
- [`CrispStrobe/CrispSorter`](https://github.com/CrispStrobe/CrispSorter) — Tauri 2 desktop app; consumes the workspace crates directly and exposes a "Translate" UI tab.
- [`CrispStrobe/CrispEmbed`](https://github.com/CrispStrobe/CrispEmbed) — ggml-based multilingual encoder; provides token embeddings for `crisp-docx-align`.
- [`CrispStrobe/CrispASR`](https://github.com/CrispStrobe/CrispASR) — ggml-based ASR + NMT engine; provides the offline m2m100 / wmt21 / madlad / gemma4-e2b backends for `crisp-docx-llm`.

## Workspace layout

| Crate | Purpose | Default features compile time |
|---|---|---|
| `crates/crisp-docx-core` | Pure-Rust OOXML primitives. Zero deps beyond `zip` + `quick-xml`. | seconds |
| `crates/crisp-docx-cli` | `crisp-docx` binary — clap-driven CLI over `core`. | seconds |
| `crates/crisp-docx-py` | PyO3 bindings → `pip install`-able wheel. | seconds |
| `crates/crisp-docx-llm` | Async LLM HTTP clients (12 providers) + optional offline NMT (CrispASR). | seconds (default) / minutes (with `nmt`) |
| `crates/crisp-docx-align` | SimAlign transformer-grade word aligner (argmax / intersection / itermax). | seconds (default) / minutes (with `crispembed`) |
| `crates/crisp-translate-cli` | `crisp-translate` binary — full docx-to-docx translation pipeline. | seconds (default) / minutes (with `align`) |

## OOXML operations covered

| Operation | Module | Use |
|---|---|---|
| Strip rsid / paraId tracking attrs | `rsid_strip` | Cures Word's "found unreadable content" dialog after transplants. |
| Normalize Apple `textutil` quirks | `normalize_tags` | `w:sz-cs` → `w:szCs`, etc. |
| Notes-kind conversion | `notes_kind` | Switch footnotes ↔ endnotes (part, rels, content-types, body refs). |
| Footnote-reference injection | `note_injection` | Given inline `[N]` markers, split runs and append `<w:footnote>` entries. |
| Body transplant | `transplant` | Clone a blueprint package, swap in source paragraphs, preserve sectPr / formatting / footnotes. |
| Run-level paragraph IO | `paragraph_runs` | Extract each `<w:r>` with verbatim `<w:rPr>` bytes for downstream reformatting. |
| Text-only paragraph IO | `paragraph_text` | Light flavour for plain-text round-trips (collapses runs into one). |
| Style mapping | `style_mapper` | Multilingual heading classifier + blueprint-driven pStyle remapping. |
| Heading inference | `heading_inference` | Detect H1/H2/H3 from font-size clusters when explicit styles aren't set. |
| Strip cosmetic paragraph bold | `strip_paragraph_bold` | Remove whole-paragraph bold inherited from RTF→md conversion. |
| Validity check | `check` | 7-axis cargo-clean diagnostic (XML parse, rsid, paraId, rels, body shape, bookmarks, rIds). |

## LLM provider matrix (`crisp-docx-llm`)

Every provider implements the same `Provider::translate(text, src_lang, tgt_lang, opts)` trait. The `LlmTranslator` orchestrator falls back through the chain on failure, so you can mix offline NMT, local Ollama, and cloud APIs.

| Provider | Wire format | Default base URL | Env key |
|---|---|---|---|
| OpenAI | Chat Completions | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| Anthropic | Messages | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` |
| Ollama (local) | `/api/generate` | `http://localhost:11434/api` | — |
| Groq | OpenAI-compat | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` |
| OpenRouter | OpenAI-compat | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` |
| Together | OpenAI-compat | `https://api.together.xyz/v1` | `TOGETHER_API_KEY` |
| Cerebras | OpenAI-compat | `https://api.cerebras.ai/v1` | `CEREBRAS_API_KEY` |
| Mistral | OpenAI-compat | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` |
| Nebius | OpenAI-compat | `https://api.studio.nebius.ai/v1` | `NEBIUS_API_KEY` |
| Scaleway | OpenAI-compat | `https://api.scaleway.ai/v1` | `SCALEWAY_API_KEY` |
| Poe | OpenAI-compat | `https://api.poe.com/v1` | `POE_API_KEY` |
| Google (Gemini) | OpenAI-compat | `https://generativelanguage.googleapis.com/v1beta/openai` | `GOOGLEAI_API_KEY` |
| **CrispASR NMT** | offline (GGUF) | n/a — runs in-process | — |

Live-verified end-to-end translation pairs (May 2026):

- ✅ Groq, OpenRouter, Together, Nebius, Scaleway: "Der Hund schläft." from "The dog is sleeping."
- ✅ CrispASR m2m100-418m-q8_0: "Der Hund schläft.", round-trip EN↔DE via offline GGML

## Quickstart

### Pure CLI (no LLM keys needed, OOXML surgery only)

```bash
cargo install --git https://github.com/CrispStrobe/crisp-docx crisp-docx-cli
crisp-docx clean broken.docx
crisp-docx notes-kind paper.docx --to endnotes
crisp-docx check paper.docx        # 7-axis validity report
crisp-docx analyze paper.docx      # blueprint metadata
crisp-docx transplant blueprint.docx source.docx -o out.docx
```

### Document translation (cloud LLM)

```bash
cargo install --git https://github.com/CrispStrobe/crisp-docx crisp-translate-cli
export GROQ_API_KEY=…
crisp-translate input.docx -o out.docx \
    --source-lang English --target-lang German \
    --provider groq
```

Provider auto-pick scans `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
`GROQ_API_KEY`, etc. in cost / latency order if `--provider` is omitted.
`--concurrency 4` (default) translates four paragraphs in parallel.

### Document translation (offline NMT, no network)

```bash
# Requires checkouts of sibling repos:
#   ../CrispEmbed/   (only if --features align)
#   ../CrispASR/     (only if --features nmt — for offline translation)
git clone https://github.com/CrispStrobe/crisp-docx
cd crisp-docx
cargo build --release -p crisp-translate-cli --features align
./target/release/crisp-translate input.docx -o out.docx \
    --target-lang German \
    --provider nmt \
    --model /path/to/m2m100-418m-q8_0.gguf
```

### Format-preserving translation (v0.2)

```bash
cargo build --release -p crisp-translate-cli --features align
./target/release/crisp-translate input.docx -o out.docx \
    --target-lang German \
    --provider groq \
    --preserve-formatting \
    --align-model /path/to/paraphrase-multilingual-MiniLM-L12-v2.gguf
```

`--preserve-formatting` keeps intra-paragraph bold / italic spans across the translation by aligning source and target words via a multilingual encoder, then redistributing the source runs' `<w:rPr>` onto the matching target text.

### Python bindings

```bash
pip install crisp-docx          # once published
# — or from source:
pip install maturin
maturin develop --release --manifest-path crates/crisp-docx-py/Cargo.toml
```

```python
from crisp_docx import (
    strip_rsids, convert_notes_kind, NotesKind,
    extract_paragraph_runs, replace_paragraph_runs,
    transplant_body, check_package, analyze_blueprint,
)

strip_rsids("paper.docx")
convert_notes_kind("paper.docx", NotesKind.Endnotes)
runs = extract_paragraph_runs("paper.docx")
print(f"{len(runs)} paragraphs, first runs: {[r.text for r in runs[0].runs[:3]]}")
```

## Build matrix

```bash
# Default — fast, no C++ deps, OOXML-only
cargo build --workspace

# With offline NMT (compiles CrispASR ggml runtime — minutes)
cargo build --workspace --features nmt

# With transformer alignment (compiles CrispEmbed ggml runtime — minutes)
cargo build --workspace --features align

# Both — fully offline translation + format preservation
cargo build --workspace --features align,nmt
```

CrispEmbed and CrispASR are sibling-repo path deps; clone them next to this repo if you build with their respective features. See `.github/workflows/ci.yml` for the CI checkout pattern.

## Testing

```bash
cargo test --workspace --exclude crisp-docx-py
# 145 tests passing (March 2026 baseline)
```

Live-integration tests for the LLM providers are env-gated. Set
`CRISP_DOCX_LLM_LIVE_<PROVIDER>=1` + the corresponding API key to
exercise the real wire:

```bash
CRISP_DOCX_LLM_LIVE_GROQ=1 GROQ_API_KEY=… \
    cargo test -p crisp-docx-llm --test live live_groq -- --nocapture
```

The parity harness (`crates/crisp-docx-core/tests/parity.rs`) runs each
ported primitive side-by-side against the original Python implementation
when both `CrispTranslator/` and a Python interpreter are available; CI
auto-skips when they aren't.

## Status

| Phase | Goal | State |
|---|---|---|
| A | Pure-Rust OOXML primitives | ✅ |
| B | Python wheel (PyO3 / maturin) | ✅ |
| C | Cross-platform CLI | ✅ |
| D | Word-level transformer alignment | ✅ |
| E | LLM HTTP clients | ✅ (12 providers) |
| F | Offline NMT (CrispASR) | ✅ |
| G | End-to-end translate-cli | ✅ |
| H | CrispSorter Tauri integration | ✅ |

See [`PARITY.md`](./PARITY.md) for the Python ↔ Rust per-primitive
status ledger, [`PLAN.md`](./PLAN.md) for the phased execution
roadmap, and [`PUBLISH.md`](./PUBLISH.md) for the crates.io / PyPI
release checklist.

## License

GNU Affero General Public License v3.0 or later. See [`LICENSE`](./LICENSE).

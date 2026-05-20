# Changelog

All notable changes land here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning: [SemVer](https://semver.org/).

## [Unreleased]

Nothing pending in `main` that isn't already in `0.1.0`. The next bump
will likely be `0.1.1` (small) or `0.2.0` (new crate / breaking API).

## [0.1.0] — 2026-05-20

First shippable cut. Workspace went from "scaffold" to "complete LLM
document-translation pipeline" in May 2026.

### Crates published

- **`crisp-docx-core`** — pure-Rust OOXML primitives.
- **`crisp-docx-cli`** — `crisp-docx` binary wrapping core.

Three more crates ship in-tree but are gated `publish = false` until
their upstream ML deps land on crates.io (see [PUBLISH.md](./PUBLISH.md)):

- **`crisp-docx-llm`** — 12-provider LLM client + optional CrispASR NMT.
- **`crisp-docx-align`** — SimAlign-style alignment via CrispEmbed.
- **`crisp-translate-cli`** — `crisp-translate` binary, end-to-end docx translation.

### Added

OOXML core (`crisp-docx-core`):
- `strip_rsids` — drops `w14:paraId`, `w14:textId`, `w:rsidR`, `w:rsidRPr`, `w:rsidDel`, `w:rsidRDefault`, `w:rsidP`, `w:rsidTr`, `w:rsidSect`. Cures Word's "found unreadable content" dialog.
- `normalize_tags` — Apple textutil's `w:sz-cs` → `w:szCs`, etc.
- `convert_notes_kind(pkg, NotesKind)` — switch footnotes ↔ endnotes (part / refs / content-types / rels).
- `inject_footnotes(pkg, notes)` — `[N]` markers → `<w:footnoteReference>` + `<w:footnote>` entries.
- `transplant_body(blueprint, source)` — full pipeline: clean_runs + strip_rsids + footnote format + style mapping + heading inference.
- `extract_paragraph_runs` / `replace_paragraph_runs` — run-granularity IO carrying verbatim `<w:rPr>` bytes.
- `extract_paragraph_texts` / `replace_paragraph_texts` — text-only round-trip (collapse runs into one).
- `classify_style`, `StyleMapper`, `StyleIndex` — multilingual heading classification (English / German / French / Italian / Spanish / Russian / Dutch / Swedish / Polish).
- `analyze_blueprint` — sections + defaults + footnote format introspection.
- `infer_heading_levels` + `apply_heading_inferences` — bold + short-text + font-size clustering.
- `strip_paragraph_bold` — whole-paragraph bold scrub.
- `check_package` — 7-axis validity report (XML parse, rsid, paraId, rels, body shape, bookmark IDs, inline rIds).

CLI (`crisp-docx-cli`):
- Subcommands `clean`, `notes-kind`, `inject-footnotes`, `transplant`, `strip-paragraph-bold`, `analyze`, `infer-headings`, `inspect`, `check`.

PyO3 bindings (`crisp-docx-py`, wheel on PyPI):
- 11 exposed functions covering everything the CLI does.

LLM clients (`crisp-docx-llm`):
- 12 cloud providers: OpenAI, Anthropic, Ollama, Groq, OpenRouter, Together, Cerebras, Mistral, Nebius, Scaleway, Poe, Google (Gemini).
- Offline NMT via CrispASR (m2m100 / wmt21 / madlad / gemma4-e2b) under `--features nmt`.
- `LlmTranslator` orchestrator with fallback chain.
- 12 wiremock unit tests + 12 env-gated live tests.

Alignment (`crisp-docx-align`):
- Pure-Rust SimAlign (argmax / intersection / itermax) over multilingual encoder token embeddings.
- `transfer_format_via_words` — maps source-run formatting onto translated text via word edges. Generic over the format-id type so callers carry OOXML `<w:rPr>` through translation.
- `translate_runs(model, src_runs, translated_text, strategy)` end-to-end convenience.

Translate CLI (`crisp-translate-cli`):
- `crisp-translate <in.docx> -o <out.docx> --target-lang DE` end-to-end.
- `--provider {openai|anthropic|ollama|groq|openrouter|together|cerebras|mistral|nebius|scaleway|poe|google|nmt}`.
- `--preserve-formatting --align-model <gguf>` under `--features align`.
- Auto-picks providers from env keys in cost/latency order.

CI (`.github/workflows/ci.yml`):
- Sibling-clones CrispEmbed and CrispASR so cargo metadata can resolve optional path deps.
- fmt / clippy / test × ubuntu / macos / windows.
- Wheel build × py 3.10 / 3.11 / 3.12.

### Verified live

- Groq, OpenRouter, Together, Nebius, Scaleway all returned "Der Hund schläft." from "The dog is sleeping."
- CrispASR m2m100-418m-q8_0 — EN↔DE round-trips work entirely offline.
- End-to-end translation of the real Vielfalt cs15.docx via Nebius: 61/61 paragraphs translated.

### Fixed (upstream, in CrispTranslator, found via parity port)

- `strip_paragraph_bold` over-stripped spurious `**ô**` / `**ç**` patterns from pandoc's RTF reader.
- `cmd_check` rejected `<w:bookmarkStart>` / `<w:bookmarkEnd>` as direct body children (valid OOXML).
- `cmd_check`'s relationship-target resolver computed wrong base for `_rels/.rels`.
- `cmd_check` crashed with KeyError on docx without optional `word/settings.xml`.

[Unreleased]: https://github.com/CrispStrobe/crisp-docx/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/CrispStrobe/crisp-docx/releases/tag/v0.1.0

# PARITY.md — Python ↔ Rust parity map

The Python implementations in
[`CrispStrobe/CrispTranslator`](https://github.com/CrispStrobe/CrispTranslator)
are the **source of truth**. Every Rust primitive in this repo MUST behave
identically (within a precise, written parity criterion) on the same input.

This file is the ledger. It is updated in the same commit as any port-changing
work; it never lags. CI runs `cargo test --test parity` against the matrix
below and fails on any divergence.

> Note: "Identical" here means *observable behaviour* on real docx inputs.
> Trivial cosmetic differences (insignificant whitespace inside XML, attribute
> order on a single element) are normalised by the parity harness — the
> criterion column states what's normalised.

---

## Scope statement

In-scope for parity: every Python primitive that operates on OOXML / docx
bytes. Out-of-scope for now (deliberate, see [PLAN.md](PLAN.md) phase E+):

- LLM HTTP clients (`MultiProviderLLMClient` and provider-specific code in
  `format_transplant.py` and `translator-app.py`). These are network I/O,
  not OOXML. They stay in Python; the Rust core is library-agnostic and
  can be called from a Python-orchestrated pipeline.
- ML model loading and translation (`translator.py`'s NMT backends —
  ctranslate2, transformers, fast_align, simalign). These have huge mature
  Python ecosystems and porting them is out-of-mission for crisp-docx.
- Pandoc subprocess orchestration. Pandoc is an external tool; Rust ports
  call the same binary the same way (when needed).

### Reclassified ✅ (2026-05-20)

- **NMT backends** (m2m100, wmt21, madlad, gemma4-e2b) were listed
  under "huge mature Python ecosystems" — but the
  [CrispASR](https://github.com/CrispStrobe/CrispASR) sister project
  now ships them as ggml C++ implementations with a clean Rust safe
  wrapper. The `crisp-docx-llm` crate gains a `ProviderKind::Nmt`
  variant behind the `nmt` Cargo feature; it wraps
  `crispasr::Session::translate_text(text, src_lang, tgt_lang,
  max_tokens)` and runs entirely in-process, no network. The
  language-name → ISO-639-1 code conversion happens via
  `nmt::map_lang_to_code` for the 35 major European/Asian languages.
  Composes with the existing fallback chain (set NMT first for
  cost, OpenAI last for quality). Live-verified on `m2m100-418m-q8_0.gguf`:
  EN↔DE round-trips clean.

### Reclassified ✅ (2026-05-19)

- **SimAlign-style transformer alignment** was previously listed under
  "huge mature Python ecosystems" — but with the addition of
  `crispembed_encode_tokens` to [CrispEmbed](https://github.com/CrispStrobe/CrispEmbed)
  the Rust ecosystem now has a working encoder. The aligner algorithm
  itself (argmax + intersection + itermax) is pure linear algebra and
  is now ported in [`crisp-docx-align`](crates/crisp-docx-align/).
  Live-tested against `paraphrase-multilingual-MiniLM-L12-v2` on
  en↔de sentence pairs; produces transformer-grade word alignments
  with no Python runtime. See the row below.

If the user later wants those ported too, they each get their own phase
in PLAN.md.

---

## Status legend

- ✅ **Parity** — Rust port complete, the parity harness runs it against the
  Python implementation on N≥1 real fixtures and asserts equivalence per
  the criterion. CI gate green.
- 🟡 **Partial** — Some inputs parity-equivalent, others not. Specific
  divergences listed in the row notes.
- ⏳ **Pending** — Port not yet written.
- 🚫 **Out of scope** — Deliberately not porting (LLM/ML/pandoc subprocess).

---

## rtf_to_docx_endnotes.py

| Python primitive | Lines | Rust equivalent | Status | Parity criterion |
|---|---|---|---|---|
| `split_body_notes(md) -> (body, notes)` | 459:81 | n/a (Rust works on docx, not md) | 🚫 | Markdown-level; covered upstream by pandoc invocation. |
| `parse_notes(notes_md) -> dict[int, str]` | 459:97 | n/a (md-level) | 🚫 | Same. |
| `rewrite_body(body_md, valid_nums) -> str` | 459:120 | `inject_footnotes` (docx-level) | 🟡 | Different abstraction layer; not directly comparable. Need a "given the same RTF, Python full-pipeline output == Rust full-pipeline output" test. |
| `strip_paragraph_bold(body_md) -> (str, int)` | 459:134 | `strip_paragraph_bold` (docx-level) | 🟡 | Need to verify: Python md-level vs Rust docx-level produce same final docx after the full pipeline. **Current Rust impl unbolded only 1 paragraph in `/tmp/transplanted.docx`; user reports many remain bold. Bug — see Issue #1.** |
| `build_footnoted_markdown(body, notes) -> str` | 459:152 | n/a (md-level) | 🚫 | Same. |
| `generate_reference_docx(out, body_font, body_size, heading_font)` | 459:160 | ⏳ | ⏳ | Will need a Rust equivalent that produces an identical reference docx given the same input fonts. |
| `rtf_to_markdown(src, dst)` | 459:225 | n/a (pandoc subprocess) | 🚫 | Pandoc is invoked the same way in both. |
| `markdown_to_docx(md, docx, ref)` | 459:230 | n/a (pandoc subprocess) | 🚫 | Same. |
| `footnotes_to_endnotes(docx)` | 459:240 | `convert_notes_kind(pkg, NotesKind::Endnotes)` | ✅ | docx output zipfile manifests equal except whitespace-normalised. Live test: ✅ |
| `_strip_rsids_in_xml(xml_bytes) -> (bytes, n)` | 459:316 | `rsid_strip::_strip_rsids_in_xml` (private) | 🟡 | Need byte-equality harness on the same XML input. Python returns `(new_bytes, count)`, Rust returns `(Vec<u8>, count)`. |
| `strip_rsids_from_docx(docx) -> int` | 459:341 | `strip_rsids(pkg) -> usize` | 🟡 | Need to verify count is identical on a real fixture. **Verified on cs15.docx: both return 0 (file already clean) ✅ but that's not exercising the function.** Need a fixture that has rsids. |
| `convert(input, output, kind, ref_doc, …)` | 459:375 | n/a (orchestrator) | 🚫 | Not a primitive; composes the above. |

## docxtool.py

| Python primitive | Lines | Rust equivalent | Status | Parity criterion |
|---|---|---|---|---|
| `_delegate(script, args)` | 276:39 | n/a (CLI dispatcher) | 🚫 | Not OOXML. |
| `cmd_clean(args)` | 276:68 | CLI `crisp-docx clean` | 🟡 | End-to-end CLI behaviour parity. |
| `_resolve_backend(choice)` | 276:153 | n/a (Python-only) | 🚫 | Python-only routing. |
| `_normalize_nonstandard_tags(docx)` | 276:163 | `normalize_tags(pkg) -> usize` | 🟡 | Count + output bytes equality on fixtures with `w:sz-cs` / `w:b-cs` / `w:i-cs`. |

## debug_format.py

| Python subcommand | Lines | Rust equivalent | Status | Parity criterion |
|---|---|---|---|---|
| `cmd_inspect(args)` | 951:200 | CLI `crisp-docx inspect` | 🟡 | Output is human-readable; criterion is "same set of parts and sizes reported." Different formatting is allowed. |
| `cmd_check(args)` | 951:270 | `check_package(pkg) -> CheckReport` | ✅ | All 7 sub-checks ported (XML parse, rsid vs settings, paraId uniqueness, rel targets, body structure, bookmark IDs, inline rIds). Parity harness verifies clean-flag and issue count match on Vielfalt cs15.docx. Three Python bugs discovered + fixed upstream during port: (1) body-structure rejected `<w:bookmarkStart>`/`<w:bookmarkEnd>` as direct body children — valid OOXML for multi-paragraph bookmarks; (2) rel-target resolver computed `base=".rels"` for the package-root `_rels/.rels`, prefixing every target with `.rels/`; (3) `cmd_check` crashed with KeyError on docx without `word/settings.xml` (optional per OPC). |
| `cmd_headings(args)` | 951:430 | ⏳ | ⏳ | Heading outline + inference. Read-only. |
| `cmd_footnotes(args)` | 951:530 | ⏳ | ⏳ | Footnote run-level dump. Read-only. |
| `cmd_compare(args)` | 951:660 | ⏳ | ⏳ | Side-by-side style/structure comparison of two docx. |
| `cmd_styles(args)` | 951:760 | ⏳ | ⏳ | Full style dump. |
| `cmd_xml(args)` | 951:795 | ⏳ | ⏳ | Pretty-print a ZIP part. |

## translator.py (NEW row — was previously 🚫 across the board)

| Python primitive | Lines | Rust equivalent | Status | Parity criterion |
|---|---|---|---|---|
| `SimAlignAligner` (Python, depends on transformers + simalign) | ~30 | `crisp-docx-align` crate (`align_texts`, `Strategy::{Intersection, Itermax}`) | ✅ | Pure-Rust SimAlign argmax+intersection / itermax over per-token embeddings from CrispEmbed's `encode_tokens` API. Verified live against `paraphrase-multilingual-MiniLM-L12-v2`: `dog↔Hund` 0.99, `classical↔klassischen`, `final↔letzte`, `religious↔religiösen`, `matters↔Fragen`. 12 algorithm-layer unit tests + live smoke example. |
| `AwesomeAlignAligner` | ~70 | (same primitive — different model) | 🟡 | Same algorithm; just point at an mBERT GGUF in CrispEmbed's registry when those land. No new code needed. |
| `FastAlignAligner` (subprocess to fast_align C binary) | ~80 | n/a — superseded | 🚫 | Statistical IBM-2 aligner. Lower quality than the transformer aligner that's now available; no need to port. |
| `HeuristicAligner` (length-ratio fallback) | ~20 | not yet | ⏳ | Trivial to port. Defer until a real consumer asks. |
| `LindatAligner` (HTTP API call) | ~40 | n/a | 🚫 | Online aligner; orthogonal to the offline path. |
| `MultiAligner` (orchestrator) | ~80 | n/a (orchestrator) | 🚫 | Composes the others. Caller-level concern. |
| NMT backends (NLLB / OpusMT / Madlad400 / CTranslate2) | ~600 | `crisp-docx-llm::ProviderKind::Nmt` (via CrispASR) | ✅ | Reclassified 2026-05-20 — see [section above](#reclassified--2026-05-20). The Rust port goes through CrispASR's ggml C++ runtime: m2m100 (100 langs any-to-any), wmt21 (EN-paired, higher quality on the supported pairs), madlad (419 langs via prefix tag), gemma4-e2b (140+ langs, dual ASR+MT). Gated behind the `nmt` feature so default builds don't compile the C++. Surfaced as a Provider so it composes with the cloud fallback chain. |
| `LLMTranslator` (OpenAI / Anthropic / Ollama / Groq HTTP clients) | ~300 | `crisp-docx-llm` crate (`LlmTranslator` + 4 providers) | ✅ | Full port. reqwest + tokio + async-trait. Provider trait abstraction lets the fallback chain mix any subset. 16 unit tests (incl. wiremock-driven wire-format tests for all 4 providers and the fallback path) + 4 env-gated live tests against real APIs. |
| `UltimateDocumentTranslator` (orchestrator) | ~580 | `crisp-translate-cli` binary (`crisp-translate`) | ✅ | End-to-end docx-to-docx translation with `--provider`/`--model`/`--source-lang`/`--target-lang`/`--dry-run`/`--concurrency`. v0.2 format preservation under `--features align`: live-verified on Vielfalt cs15.docx (May 2026) — 61/61 paragraphs translated via Nebius, paragraph order preserved (after fixing a `buffer_unordered` ordering bug surfaced by this test), all 64 source italic runs and 44 bold runs carried through translation via SimAlign + CrispEmbed. Offline NMT via CrispASR under `--features nmt`. Features split: `align` (CrispEmbed only) / `nmt` (CrispASR only) / `full` (both) so users can pick the build cost they need. |
| `paragraph_runs::{extract,replace}_paragraphs` (run-level IO) | n/a | `crisp-docx-core::{extract,replace}_paragraph_runs` | ✅ | New core module. `ParagraphInfo` carries verbatim pPr bytes, an ordered Vec<Run> with each run's text + verbatim rPr bytes + captured footnote refs, plus leading bookmark starts and trailing bookmark ends. 8 unit tests including round-trip on bold/italic mix, footnote-ref preservation, multi-paragraph. |
| `transfer_format_via_words` (alignment → runs bridge) | n/a | `crisp-docx-align::transfer_format_via_words` + `translate_runs` (feature-gated) | ✅ | Generic `<F: Clone + PartialEq>` over format-ids — caller decides what F is (raw rPr bytes, an enum, anything). Algorithm: tag every source char with its run index, find majority-run per source word, map to target via `word_edges`, fill unaligned target words via left-then-right scan or `default_format`. Adjacent same-format target runs merge. 6 unit tests covering single-run passthrough, mid-paragraph bold, neighbour-inheritance, default-format fallback, run merging, whitespace attachment. End-to-end convenience `translate_runs(model, src_runs, translated_text, strategy)` calls `align_texts` then `transfer_format_via_words`. |

## format_transplant.py

| Python primitive | Lines | Rust equivalent | Status | Parity criterion |
|---|---|---|---|---|
| `classify_style(name) -> (sem, level)` | ~50 | `classify_style` | ✅ | Parity-tested on 25 style names across English/German/French/Italian/Spanish/Russian/Dutch/Swedish/Polish, including regex-fallback ("Heading_02", "Titre2") and substring forms. |
| `BlueprintAnalyzer.analyze(doc) -> BlueprintSchema` | ~600 | `analyze_blueprint(pkg) -> BlueprintSchema` | ✅ | Full port. Combines `StyleIndex::from_package` (styles + body_para_style_names), `extract_footnote_format`, `_sections` (page geometry from `<w:sectPr>` — handles both nested and self-closing forms), `_defaults` (font + size from `<w:docDefaults>` with `Normal`-style fallback). Verified on real Vielfalt cs15.docx: 1 section, 81 styles, 4 body styles in use, footnote marker rPr + separator captured. |
| `ContentExtractor._infer_headings(elements)` | ~100 | `infer_heading_levels(pkg, source_styles)` + `apply_heading_inferences(pkg, inferences, bp_index)` | ✅ | Bold + short-text + font-size clustering verbatim. 6 unit tests cover: no-candidates, single-level-1, multi-size clustering, skip-already-heading, skip-long-text, pPr-default-bold-propagates. |
| `ContentExtractor.extract / _para / _run / _body / _footnotes` | ~180 | ⏳ | ⏳ | The data-model layer above _infer_headings (ParagraphData / RunData). Heading inference now ported standalone — the rest is mostly Python's intermediate representation, of limited value in the docx-direct Rust pipeline. |
| `StyleMapper.map(src_name, sem_class, hl) -> str` | ~200 | `StyleMapper::map` | ✅ | All 6 resolution-order branches ported (user override, semantic-heading-before-name, exact, case-insensitive, semantic class, body fallback) with 9 unit tests covering each branch. |
| `DocumentBuilder.build(bp, out, elements, footnotes)` | ~600 | `transplant_body(bp, src)` | 🟡 | Now invokes `clean_runs` + `strip_rsids` + `apply_footnote_format` + `apply_style_mapping` (the four heavy passes). Still missing: heading inference is *available* (`infer_heading_levels` + `apply_heading_inferences`) but not auto-invoked by `transplant_body` — caller composes; see PyO3/CLI surface. |
| `MultiProviderLLMClient` | ~300 | n/a | 🚫 | Network I/O, not OOXML. |
| Helper: `_strip_tracking_attrs(elem)` | ~50 | `strip_rsids` | 🟡 | Python helper strips per-node; Rust strips package-wide. Functionally equivalent if applied to whole document; need fixture-based equivalence check. |
| Helper: `_clean_runs(p, keep_set)` | ~80 | `clean_runs(pkg)` | ✅ | Removal-count parity verified on cs15.docx via parity harness. KEEP_RPR_TAGS locked to Python's set (regression-tested). Wired into `transplant_body`. |
| Helper: `_apply_fn_ref_style(footnote, rpr_xml)` | ~30 | `apply_footnote_format` (rPr half) | ✅ | Verified on real cs15.docx + pandoc-built blueprint: transplant output's marker run rPr matches blueprint verbatim. |
| Helper: `_normalize_fn_separator(footnote)` | ~80 | `apply_footnote_format` (separator half) | ✅ | Three branches ported (matches → no-op; differs → replace; absent → insert). Captures tab/whitespace/empty per Python. |
| Helper: `_transplant_footnotes(blueprint, source_footnotes, schema)` | ~130 | partial in `transplant_body` | 🟡 | Python carries footnotes with rPr application; Rust just copies the bytes. **Diverges.** |
| Helper: `_clear_body(doc)` | ~30 | partial in `transplant_body` | 🟡 | Python preserves final `<w:sectPr>`; Rust does too via byte-level. Approximately equivalent. |

---

## Open issues (today)

### Issue #1 — `strip_paragraph_bold` unbolds too few paragraphs

User reported (2026-05-19) opening `/tmp/transplanted.docx` in Word:
several paragraphs ("The classical tradition's final word on religious …",
"synagôgç. The polemical strands …") render bold when they shouldn't.

Rust ran `strip_paragraph_bold` and reported "unbolded 1 paragraph".

**Diagnosis (2026-05-19, after running the full pipeline + paragraph-by-paragraph audit):**

The user's reported paragraph "The classical tradition's final word…" *is*
genuine whole-paragraph bold (paragraph #45, 7/7 runs bold). My Rust
detector correctly handled it.

The user's second reported paragraph "synagôgç. The polemical strands…"
is a SECTION of paragraph #49. Paragraph #49 is **28% bold by character
count** — the tail third is bold, the rest plain. It is not
whole-paragraph bold.

**Why the tail is bold (Python pipeline bug):**

`pandoc rtf → md` translated the source RTF into:

```
**\[S23\] It is worth pausing… *krisis* and *synag**ô**g**ç*. The polemical strands… humans have achieved.**
```

The outer `**…**` would have wrapped the entire paragraph. But pandoc's
RTF reader also injected nested `**ô**` and `**ç**` around individual
non-ASCII letters (the source RTF had per-character bold formatting for
those Unicode codepoints). The resulting markdown has **mismatched
nested `**` markers**.

Python's `strip_paragraph_bold` regex:

```python
_WHOLE_PARA_BOLD_RE = re.compile(
    r"\A(\*\*)(?P<inner>(?:(?!\*\*).)+)\*\*\Z", re.DOTALL,
)
```

The `(?:(?!\*\*).)+` clause **rejects** any inner content containing `**`.
The regex correctly declines to touch a paragraph with mismatched markers.
But that leaves the paragraph un-stripped, and pandoc's md-→-docx parser
then renders it as a confused mixture: the leading `**[S23]` becomes
literal text (not bold), then a section in the middle becomes bold, then
some non-bold, then more bold for the tail.

End result: the user sees a paragraph that's mostly plain text with the
last third in bold, with `**[S23]` rendered as literal characters at the
start.

**Fixed** in CrispTranslator `f6b5ff4` (2026-05-19). Added a preprocess to
`strip_paragraph_bold` that strips three patterns to a fixed point:

```python
re.compile(r"\*\*([^\sA-Za-z0-9\*])\*\*")  # **X**
re.compile(r"\*\*([^\sA-Za-z0-9\*])\*")    # **X*  (bold-open + char + italic-close)
re.compile(r"\*([^\sA-Za-z0-9\*])\*\*")    # *X**  (italic-open + char + bold-close)
```

Iterating to fixed point handles cases like `*synag**ô**g**ç*`:
strip `**ô**` → `*synagôg**ç*` → strip `**ç*` → `*synagôgç*`.

Result on the real Vielfalt cs15.rtf:
  before: 50 paragraphs unbolded; paragraph #49 still 28% bold; leading
          `**[S23]` rendered as literal text
  after:  54 paragraphs unbolded; paragraph #49 is 0% bold; clean `[S23]`
          start; 0 all-bold paragraphs and 0 mostly-bold paragraphs remain

Two new regression tests in CrispTranslator/tests/test_text_processing.py
(`test_strips_spurious_single_char_non_ascii_bold`,
`test_leaves_legitimate_intra_paragraph_emphasis_on_ascii_word`).
41/41 CrispTranslator tests pass.

The Rust `strip_paragraph_bold` works at the docx level (after pandoc),
not at the markdown level, so it isn't affected by the source-side bug.
Documents produced by the fixed Python pipeline are already clean when
they reach the Rust transplant.

### Issue #2 — `transplant_body` is structurally different from Python

The Python `DocumentBuilder.build()` runs `_clean_runs` over every transplanted
run, applying KEEP_RPR_TAGS. The Rust `transplant_body` does not. Result: any
non-semantic rPr in source runs (random font sizes, weird kerning, etc.) is
preserved in the output. Need to port `_clean_runs`.

### Issue #3 — No actual parity harness yet

This file is the ledger. The harness is `tests/parity.rs` (see next commit).
Until it lands, all "✅" claims should be read as "best-effort, not verified
by a side-by-side run."

---

## PyO3 / CLI surface (2026-05-19)

All currently-ported primitives are now reachable both from Python and from
the shell. Smoke-tested live against `2026 Vielfalt cs15.docx`:

| Operation | Python (`crisp_docx.*`) | CLI (`crisp-docx …`) |
|---|---|---|
| Strip rsid/paraId | `strip_rsids(path, output=)` | `clean` |
| Normalize textutil tags | `normalize_tags(path, output=)` | `clean --also-normalize-tags` |
| Convert notes kind | `convert_notes_kind(path, target)` | `notes-kind --to {footnotes,endnotes}` |
| Inject `[N]` footnotes | `inject_footnotes(path, notes)` | `inject-footnotes --notes notes.json` |
| Transplant body | `transplant_body(bp, src, out)` | `transplant <bp> <src> -o <out>` |
| Strip whole-paragraph bold | `strip_paragraph_bold(path)` | `strip-paragraph-bold` |
| Clean run rPr | `clean_runs(path)` | (composed inside transplant) |
| Analyze blueprint | `analyze_blueprint(path) -> dict` | `analyze` |
| Apply style mapping | `apply_style_mapping(path, bp, src=)` | (composed inside transplant) |
| Infer heading levels | `infer_heading_levels(path, source=, apply_to_blueprint=)` | `infer-headings [--apply-to-blueprint]` |
| Validate package | `check_package(path) -> (clean, oks, issues)` | `check` (exit 1 on failure, mirroring Python) |

---

## Process commitments going forward

1. **No new Rust primitive lands without an entry in this table and a row
   in `tests/parity.rs` that runs both Python and Rust on at least one
   real fixture.** CI gates on the harness.

2. **`partial / 🟡` rows are bugs.** They block release.

3. **Out-of-scope rows (🚫) are written with a one-line rationale here.**
   If the rationale changes, the row moves out of 🚫.

4. **One reviewer-readable summary commit per closed parity gap.** No "kinda
   ports it" commits.

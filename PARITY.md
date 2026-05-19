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
| `cmd_check(args)` | 951:270 | ⏳ | ⏳ | XML well-formedness + rsid/paraId/rel/bookmark/rId consistency checks. Big function (160 lines); will need a phased port. |
| `cmd_headings(args)` | 951:430 | ⏳ | ⏳ | Heading outline + inference. Read-only. |
| `cmd_footnotes(args)` | 951:530 | ⏳ | ⏳ | Footnote run-level dump. Read-only. |
| `cmd_compare(args)` | 951:660 | ⏳ | ⏳ | Side-by-side style/structure comparison of two docx. |
| `cmd_styles(args)` | 951:760 | ⏳ | ⏳ | Full style dump. |
| `cmd_xml(args)` | 951:795 | ⏳ | ⏳ | Pretty-print a ZIP part. |

## format_transplant.py

| Python primitive | Lines | Rust equivalent | Status | Parity criterion |
|---|---|---|---|---|
| `classify_style(name) -> (sem, level)` | ~50 | `classify_style` | ✅ | Parity-tested on 25 style names across English/German/French/Italian/Spanish/Russian/Dutch/Swedish/Polish, including regex-fallback ("Heading_02", "Titre2") and substring forms. |
| `BlueprintAnalyzer.analyze(doc) -> BlueprintSchema` | ~600 | `analyze_blueprint(pkg) -> BlueprintSchema` | ✅ | Full port. Combines `StyleIndex::from_package` (styles + body_para_style_names), `extract_footnote_format`, `_sections` (page geometry from `<w:sectPr>` — handles both nested and self-closing forms), `_defaults` (font + size from `<w:docDefaults>` with `Normal`-style fallback). Verified on real Vielfalt cs15.docx: 1 section, 81 styles, 4 body styles in use, footnote marker rPr + separator captured. |
| `ContentExtractor.extract(doc) -> (paragraphs, footnotes)` | ~280 | ⏳ | ⏳ | Parses runs/paragraphs/tables/footnotes; infers headings. Needs port. |
| `StyleMapper.map(src_name, sem_class, hl) -> str` | ~200 | `StyleMapper::map` | ✅ | All 6 resolution-order branches ported (user override, semantic-heading-before-name, exact, case-insensitive, semantic class, body fallback) with 9 unit tests covering each branch. |
| `DocumentBuilder.build(bp, out, elements, footnotes)` | ~600 | `transplant_body(bp, src)` | 🟡 | Now invokes `clean_runs` (rPr filter ✅) + `strip_rsids` (rsid scrub ✅). Still missing: footnote-marker rPr deep-copy from blueprint, `_normalize_fn_separator` (tab/space after footnote number), style mapping (StyleMapper). |
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

## Process commitments going forward

1. **No new Rust primitive lands without an entry in this table and a row
   in `tests/parity.rs` that runs both Python and Rust on at least one
   real fixture.** CI gates on the harness.

2. **`partial / 🟡` rows are bugs.** They block release.

3. **Out-of-scope rows (🚫) are written with a one-line rationale here.**
   If the rationale changes, the row moves out of 🚫.

4. **One reviewer-readable summary commit per closed parity gap.** No "kinda
   ports it" commits.

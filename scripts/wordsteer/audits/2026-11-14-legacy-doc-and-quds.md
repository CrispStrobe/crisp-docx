# Audit: CrispCloud's legacy .doc parser and the quds fork (2026-11-14)

Measured with `scripts/wordsteer` against Microsoft Word for Mac 16.111 on a
private local corpus (all `.doc`/`.docx` under a user's Documents and
Downloads). **Only aggregate numbers are recorded here; the documents and
per-file results never leave the machine.**

Engines: legacy_doc_parser `544a50e` (identical in CrispCloud and the quds
fork); quds engine as vendored in CrispCloud (`84b1183`) and as fork `main`
(`f951276`, 160 commits newer, includes the `codex/office-recovery-20261114`
branch); crisp-docx `apply-steps` (this branch).

| Run | Files (unique by content) |
|---|---|
| Word-free: legacy on native `.doc`, quds open/save on `.docx` | 693 (587 docx, 106 doc) — run continues over the full 2,179 |
| Word round-trip: docx → Word `.doc` → legacy vs Word `.docx` | 189 (157 docx, 32 doc) |
| Edit tasks (10 recipes × Word, quds, crisp-docx) | 6 documents, 57 task runs per engine |

> The fork's `main` and the vendored quds produced **identical results in
> every comparison** — the newer 160 commits change none of the behaviour
> measured below, so re-vendoring alone fixes none of it.

## 1. Legacy `.doc` parser

### Capabilities confirmed
* Fast: median 0.04–0.09 s per document.
* When it converts, the text is right: Word-written `.doc` → parser `.docx`
  equals Word's own `.docx` of the same `.doc` in **87%** of files on the
  full semantic model; mean word similarity 1.0; bold/italic/colour/
  highlight/size retention 1.0; tables, notes, comments, sections identical.
* Declared limitations are honest and frequent: `partialFormatting` 87,
  `embeddedObjects` 63, `fieldsLists` 16 (of 693).

### Gaps (by impact)
| # | Gap | Evidence | Suggested fix |
|---|---|---|---|
| L1 | **Rejects 16% of native `.doc` files** that Word opens (17/104): `Compound sector outside its container` 7, `Referenced Word style is missing` 2, picture dimensions over limit, invalid/duplicate list identity, missing/invalid textbox ranges, structural property in table style. Word-written `.doc` fail far less (3.4%). | legacy[doc] failRate 0.179 incl. 2 non-Word files | Degrade instead of throwing: clamp/skip the offending sector, fall back to Normal for missing styles, drop the bad list/picture with a limitation. CFB: accept sectors past EOF the way Word does (truncated FAT tails). |
| L2 | **External hyperlinks dropped** (text kept, link gone) in 34 of 189 round-tripped files; neither `w:hyperlink`, `HYPERLINK` field nor relationship is emitted. | legacyVsWord countDiffs hyperlinks lost 30/143 docx-origin, 4/26 native | Emit `HYPERLINK` fields (or `w:hyperlink` + external rel) for the `.doc` field type 88 with a URL target. |
| L3 | **Localized built-in style names kept** (`Standard`, `Titel`, `Überschrift 7` instead of `Normal`, `Title`, `heading 7`): style agreement 0.39 on native `.doc`. Formatting is equal, but Word then treats them as custom styles (no built-in Normal/heading semantics, duplicates when pasting). | legacyVsWord[doc] para.style 0.385 | Map built-in style ids (sti 0–9, 15, …) to canonical English names; keep the localized name as `w:aliases`. |
| L4 | Underline retention 95%; drawing shapes lost in 5 files, fields in 4, one text box. | keep.u 0.9545 | Underline styles beyond single/double; OfficeArt shapes listed as limitation already. |
| L5 | Files with a `.doc` extension that are RTF or PDF fail with "Not a compound document". | 2 files | Sniff `{\rtf` / `%PDF` and route (or report the real format). |

## 2. quds (engine as used by the CrispCloud editor)

### Capabilities confirmed
* Edits do what is asked: the engine-independent intent oracle passes 46/47
  task runs (word/sentence/paragraph formatting, replace, heading,
  insert/delete paragraph, section margins).
* Main text survives open/save: mean word similarity ≥ 0.998; tables 0.997.

### Gaps (by impact)
| # | Gap | Evidence | Suggested fix |
|---|---|---|---|
| Q1 | **Refuses to open 17.4% of `.docx`** (102/587). Top causes: bookmark endpoints directly in `w:body`/`w:tbl` between blocks — valid OOXML, written by Word — 55 files (9.4%); bookmark start without end in the same story 16; content control in a paragraph-only header story 11; section break inside a content control 5. | quds[docx] failRate 0.174 | Attach body/table-level bookmark endpoints to the adjacent paragraph's start/end on import; tolerate unmatched starts (close at story end); unwrap header content controls. |
| Q2 | **Exponential text-box duplication**: one file's 18 text boxes became 4,101 (document.xml 160 KB → 10.3 MB) after a single open/save; text boxes nested 12 levels deep (2¹² ≈ 4,096). Stable on further saves. | qudsResave textbox counts; nesting depth 12 vs 1 | Do not re-emit floating anchors inside the content of a text box they belong to; add a regression fixture with two linked/overlapping text boxes. |
| Q3 | Drawing **shapes lost** (21 files), **images lost** (7), **fields lost** (10); text-box text similarity 0.62 after open/save, 0.0 when re-saving legacy-parser output. | qudsResave/qudsOnLegacy countDiffs | Preserve unmodelled DrawingML/VML as opaque (as already done for `altChunk`). |
| Q4 | **Media duplicated** on save (54 files gained media parts) — file bloat. | mediaParts gained 54 | De-duplicate media by hash on export. |
| Q5 | Revision metadata: in 41 files `w:ins` grouping changes (inserted text identical in 10 of 11 checked), author/date changed in 2 of 11, one extra inserted character. | revisions countDiffs | Keep revision run boundaries and attributes as imported. |
| Q6 | Empty paragraphs added (43 files); header/footer parts multiplied per section (37); cached `PAGE` results dropped (fields themselves kept). | paragraphs/headerFooterParts gained | Low impact; only add the mandatory paragraph after a final cell table. |
| Q7 | Edits differ from Word: matches Word in 7/36 task runs (crisp-docx: 43/46). Main causes: open/save noise above; `heading` uses Word-2007-era built-in spacing (14/8 pt instead of 8/4 pt) and keeps direct formatting Word clears; inserted paragraphs don't inherit the paragraph-mark font/size. | tasks vsWord | Apply Word 365 built-in style definitions; reset direct formatting on style apply (>50% rule); new paragraph run props from `pPr/rPr`. |
| Q8 | Vocabulary: `alignment` uses `justify` (OOXML `both`); underline limited to none/single/double/dotted/wavy. | adapter | Accept `both`; extend `WmlUnderline`. |
| Q9 | Performance outlier: one 600 KB document takes 87 s per open/save (median 0.09 s). | step timings | Profile with that shape (many floating objects). |

## 3. Word itself (reference behaviour worth knowing)

* `.docx` → `.doc` is lossy by itself in 20% of files (text boxes, spacing
  "auto"/contextual rules, a few media parts) — the parser is compared with
  Word's own reading of the same `.doc`, not with the original, for that
  reason.
* Automation pitfalls are documented in `../README.md`; most important: never
  drive one Word instance from several processes — its AppleScript object
  model degrades until Word restarts.

## 4. crisp-docx additions in this branch

`crisp-docx apply-steps` implements the shared vocabulary (runFormat,
paragraphFormat, replaceText, heading with Word's style-apply reset,
paragraphInsert, paragraphDelete, sectionFormat): intent oracle 56/57,
matches Word 43/46. Known differences: Word keeps direct formatting on runs
that carry a character style when applying a heading; `list`/`listClear`
are not implemented yet (quds and Word have them).

## Reproduce

```bash
python3.13 wordsteer.py setup --crispcloud ~/code/local/CrispCloud --fork-ref main
python3.13 wordsteer.py corpus roundtrip --root ~/Documents --root ~/Downloads --out RUN [--no-word]
python3.13 wordsteer.py corpus tasks --root ~/Documents --out RUN2 --limit 40
python3.13 wordsteer.py report RUN
```

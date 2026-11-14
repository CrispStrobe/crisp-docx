# wordsteer

One steering vocabulary for four Word engines, and an audit harness that
measures them against each other:

| Engine | What it is | Used for |
|---|---|---|
| `word` | Microsoft Word for Mac, driven by AppleScript | reference: `.docx↔.doc` conversion and edits |
| `legacy` | CrispCloud's `legacy_doc_parser` (Dart) | `.doc → .docx` |
| `quds:<flavour>` | quds `tool/fidelity/office_steps.dart`, AOT-compiled unchanged | open/save and edits as the CrispCloud editor does them |
| `crisp` | this repo's `crisp-docx apply-steps` | the same edits in pure Rust |

The vocabulary lives in [`ws/ops.json`](ws/ops.json) (single source of
truth, like ExcelLLMAddin's `shared/providers.json`); names and units follow
quds' `office_steps.dart` (points, line spacing as a multiple, hex colours).
`tests/` fails if an engine adapter and the spec disagree.

## Setup

```bash
python3.13 wordsteer.py setup --crispcloud ~/code/local/CrispCloud --fork-ref main
python3.13 wordsteer.py check-env
```

`setup` builds into `~/.cache/wordsteer/bin`: `legacy_doc`, `quds_steps_vendored`
(the copy vendored in CrispCloud), `quds_steps_<fork-ref>` (a sparse clone of
the quds fork, so unmerged fork work can be audited before it is vendored)
and `crisp-docx`. Python is stdlib-only; any 3.9+ with a working `pyexpat`
(Homebrew 3.14/3.15 currently ship a broken one — the script says so).

## Recipes (saved steering commands)

```json
{"name": "emphasise",
 "steps": [
   {"op": "runFormat", "target": {"select": "word"}, "bold": true, "color": "C00000"},
   {"op": "paragraphFormat", "target": {"find": "Dear colleagues"}, "alignment": "center", "after": 6},
   {"op": "heading", "target": {"select": "shortParagraph"}, "level": 1},
   {"op": "sectionFormat", "section": 0, "top": 50, "left": 70}
 ]}
```

A target is exact text (`{"find": "…", "occurrence": n}`) or a selector
(`word`, `sentence`, `paragraph`, `shortParagraph`) resolved per document to
a text that occurs exactly once — so one recipe replays over a whole corpus,
and every engine addresses the same spot regardless of how it counts
paragraphs. Bundled: [`recipes/corpus-tasks.json`](recipes/corpus-tasks.json)
(ten tasks: a word, a sentence, a paragraph, replace, heading, insert,
delete, section margins).

```bash
python3.13 wordsteer.py apply --engine word  --recipe r.json in.docx out-word.docx
python3.13 wordsteer.py apply --engine quds:main --recipe r.json in.docx out-quds.docx
python3.13 wordsteer.py apply --engine crisp --recipe r.json in.docx out-crisp.docx
python3.13 wordsteer.py compare out-word.docx out-crisp.docx
```

## Comparing documents

`compare` reduces each package to what a reader notices (ws/model.py):
main-story paragraphs with final-view text (field codes and deletions
dropped), *effective* character formatting per character (docDefaults →
paragraph style chain → numbering level → character style → direct, theme
fonts resolved), effective paragraph properties, and structure (tables,
sections, notes, comments, headers/footers, text boxes, images, fields,
revisions). Scores include `keep.<prop>` — the share of non-default
formatting (bold, italic, colour, …) that survived — which is what exposes
loss; plain text otherwise dominates any agreement figure.

## Corpus audits

```bash
WORDSTEER_HIDE=1 python3.13 wordsteer.py corpus roundtrip --root ~/Documents --root ~/Downloads --out RUN
WORDSTEER_HIDE=1 python3.13 wordsteer.py corpus tasks --root ~/Documents --out RUN2 --limit 40
python3.13 wordsteer.py report RUN
```

* `roundtrip`: `.docx` → Word `.doc` → (a) Word's `.docx` of it, (b) the
  legacy parser's `.docx`; compares (b) with (a) (parser gap), (a) with the
  original (inherent `.doc` loss), plus quds open/save of the original and
  of the legacy output (CrispCloud's own `.doc` path). Native `.doc` files go
  straight to Word and the parser.
* `tasks`: each recipe in `corpus-tasks.json` through every engine; scored by
  an engine-independent **intent** oracle (did the requested change happen?),
  agreement **vs Word**, and **collateral** changes elsewhere (measured
  against the same engine's own open/save, so resave noise is not blamed on
  the edit).

Runs are resumable (`results.jsonl`, deduplicated by content hash), delete
intermediates per file, stop cleanly on a `STOP` file in the run directory
or when free disk drops below 2 GB. **Results contain file paths and text
excerpts — keep them local.** `report` emits aggregate numbers only.

## Word safety

Word is usually the user's live instance. The engine stages copies inside
Word's sandbox container (no access prompts), addresses documents only by
their unique staged name, closes them `saving no`, never quits or kills Word
(a timeout stops the run instead — a dialog may be open), force-disables
macros for each call and restores the previous setting, and gives a dummy
password so protected files fail instead of prompting.

`WORDSTEER_HIDE=1` re-hides Word after every open; Word never takes focus
either way, but hidden it runs noticeably slower (App Nap).

AppleScript pitfalls this code works around (each one silently did the wrong
thing first): a Word range is a re-evaluated reference, so `execute find`
does not narrow it and editing it rewrites the whole story; a window's
selection only works while that document is frontmost; font colours are
16-bit per channel and 8-bit values are accepted and ignored; `case`,
`offset`, `ip` and `left indent` collide with Word's dictionary.

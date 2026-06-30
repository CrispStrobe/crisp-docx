#!/usr/bin/env python3
"""md-footnotes-to-docx — convert markdown using *numeric-bracket* footnotes
into a .docx with real, page-bottom Word footnotes (not endnotes).

Many authoring/translation pipelines emit footnotes as bare `[N]` markers in
the body plus a trailing block of `[N] text` definitions (e.g. under an
"Endnoten" / "Notes" heading) — which is *not* pandoc's `[^N]` / `[^N]:`
syntax, so pandoc would render them as literal text. This tool rewrites that
convention into pandoc footnote syntax and then shells out to pandoc, so the
note text keeps its inline formatting (italics, links, …) — something
crisp-docx's `inject-footnotes` (plain-text notes on an existing .docx) does
not preserve.

  scripts/md-footnotes-to-docx.py INPUT.md [-o OUTPUT.docx] [--keep-md FILE]
                                  [--reference-doc TEMPLATE.docx]

Only purely-numeric `[N]` markers that have a matching definition are touched;
markers like `[S2]` (slides) or `[Author]` are left alone. Requires `pandoc`
on PATH. The produced .docx has a `word/footnotes.xml` part and zero endnotes.
"""
import argparse, re, shutil, subprocess, sys, tempfile
from pathlib import Path

NOTE_HEADINGS = {
    "endnoten", "endnotes", "anmerkungen", "notes",
    "fußnoten", "fussnoten", "footnotes",
}
HEADING_RE = re.compile(r'^#{1,6}\s+(.*?)\s*$')
REF_RE = re.compile(r'\[(\d+)\]')            # numeric bracket reference in body
DEF_RE = re.compile(r'^\[(\d+)\]\s+(.*)$')   # leading "[N] text" definition
HR_RE = re.compile(r'^\s*-{3,}\s*$')


def split_body_notes(lines):
    """(body, notes): notes begin at the first notes-heading; a `---` rule
    immediately above the heading is dropped with it."""
    for i, ln in enumerate(lines):
        m = HEADING_RE.match(ln)
        if m and m.group(1).strip().lower() in NOTE_HEADINGS:
            start = i
            j = i - 1
            while j >= 0 and lines[j].strip() == "":
                j -= 1
            if j >= 0 and HR_RE.match(lines[j]):
                start = j
            return lines[:start], lines[i + 1:]
    return lines, []


def to_pandoc_footnotes(text):
    lines = text.splitlines()
    body, notes = split_body_notes(lines)

    defs = {}
    for ln in notes:
        m = DEF_RE.match(ln.strip())
        if m:
            defs[int(m.group(1))] = m.group(2).strip()

    referenced = set()

    def repl(m):
        n = int(m.group(1))
        if n in defs:
            referenced.add(n)
            return f'[^{n}]'
        return m.group(0)

    out = [REF_RE.sub(repl, ln) for ln in body]
    if out and out[-1].strip() != "":
        out.append("")
    for n in sorted(defs):
        out.append(f'[^{n}]: {defs[n]}')
        out.append("")

    cited = {int(m.group(1)) for ln in body for m in REF_RE.finditer(ln)}
    stats = {
        "defs": len(defs),
        "rewritten": len(referenced),
        "defined_unreferenced": sorted(set(defs) - referenced),
        "cited_undefined": sorted(cited - set(defs)),
    }
    return "\n".join(out) + "\n", stats


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("input", type=Path)
    ap.add_argument("-o", "--output", type=Path,
                    help="output .docx (default: alongside input, .docx)")
    ap.add_argument("--reference-doc", type=Path,
                    help="pandoc --reference-doc style template")
    ap.add_argument("--keep-md", type=Path,
                    help="also write the intermediate pandoc-markdown here")
    args = ap.parse_args()

    if shutil.which("pandoc") is None:
        sys.exit("error: pandoc not found on PATH")

    out = args.output or args.input.with_suffix(".docx")
    pandoc_md, stats = to_pandoc_footnotes(args.input.read_text(encoding="utf-8"))

    print(f"footnote definitions: {stats['defs']}; references rewritten: "
          f"{stats['rewritten']}", file=sys.stderr)
    if stats["defined_unreferenced"]:
        print(f"WARNING: defined but never referenced: "
              f"{stats['defined_unreferenced']}", file=sys.stderr)
    if stats["cited_undefined"]:
        print(f"note: numeric brackets with no definition (left literal): "
              f"{stats['cited_undefined']}", file=sys.stderr)

    if args.keep_md:
        args.keep_md.write_text(pandoc_md, encoding="utf-8")

    with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False,
                                     encoding="utf-8") as tf:
        tf.write(pandoc_md)
        tmp = Path(tf.name)
    try:
        cmd = ["pandoc", str(tmp), "-f", "markdown", "-t", "docx", "-o", str(out)]
        if args.reference_doc:
            cmd += ["--reference-doc", str(args.reference_doc)]
        subprocess.run(cmd, check=True)
    finally:
        tmp.unlink(missing_ok=True)

    print(f"wrote {out}", file=sys.stderr)


if __name__ == "__main__":
    main()

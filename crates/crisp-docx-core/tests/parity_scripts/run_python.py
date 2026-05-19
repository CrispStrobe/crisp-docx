#!/usr/bin/env python3
"""Run a single CrispTranslator Python primitive on an input and print the
result(s) needed for parity comparison to stdout as JSON.

Invocation:
    run_python.py <primitive> <input.docx> <output.docx> [extra args...]

Primitives implemented (one-to-one with PARITY.md rows):

    strip_rsids          input docx -> output docx; print {"removed": N}
    normalize_tags       input docx -> output docx; print {"renamed": N}
    notes_to_endnotes    input docx -> output docx; print {}
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import zipfile
from pathlib import Path

# Make CrispTranslator's source code importable. The harness assumes the
# repo lives at the canonical path next to this one. Override via env var if
# the layout differs.
import os
CRISP_TRANSLATOR = Path(
    os.environ.get(
        "CRISP_TRANSLATOR_DIR",
        "/Users/christianstrobele/code/CrispTranslator",
    )
)
sys.path.insert(0, str(CRISP_TRANSLATOR))


def _emit(report: dict) -> None:
    """Write the report JSON.

    `format_transplant` prints a system-check banner to stdout on import,
    so we can't rely on stdout being clean. The harness expects to read
    the report from `<dst_path>.json` next to the output docx.
    """
    dst = Path(sys.argv[3])
    side = dst.with_suffix(dst.suffix + ".json")
    side.write_text(json.dumps(report))


def main() -> int:
    if len(sys.argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    primitive, src, dst, *rest = sys.argv[1:]
    src_path = Path(src)
    dst_path = Path(dst)

    if primitive == "strip_rsids":
        shutil.copyfile(src_path, dst_path)
        from rtf_to_docx_endnotes import strip_rsids_from_docx  # type: ignore
        n = strip_rsids_from_docx(dst_path)
        _emit({"removed": n})
        return 0

    if primitive == "normalize_tags":
        shutil.copyfile(src_path, dst_path)
        # docxtool's normalizer mutates in place and returns the count.
        from docxtool import _normalize_nonstandard_tags  # type: ignore
        n = _normalize_nonstandard_tags(dst_path)
        _emit({"renamed": n})
        return 0

    if primitive == "notes_to_endnotes":
        shutil.copyfile(src_path, dst_path)
        from rtf_to_docx_endnotes import footnotes_to_endnotes  # type: ignore
        footnotes_to_endnotes(dst_path)
        _emit({})
        return 0

    if primitive == "classify_style":
        # rest[0] holds the style name to classify
        from format_transplant import classify_style  # type: ignore

        name = rest[0] if rest else ""
        sem, level = classify_style(name)
        _emit({"class": sem, "level": int(level)})
        return 0

    if primitive == "clean_runs":
        # Mirror format_transplant.py::DocumentBuilder._clean_runs on the
        # docx in place. Walk every <w:r> in document.xml, footnotes.xml,
        # and endnotes.xml; for each non-footnote-ref run, remove rPr
        # children not in KEEP_RPR_TAGS.
        import lxml.etree as ET  # type: ignore
        from format_transplant import KEEP_RPR_TAGS  # type: ignore

        shutil.copyfile(src_path, dst_path)
        W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
        NS = {"w": W}
        removed_total = 0
        with zipfile.ZipFile(dst_path, "r") as zin:
            parts = {n: zin.read(n) for n in zin.namelist()}
        for part_name in ("word/document.xml", "word/footnotes.xml",
                          "word/endnotes.xml"):
            if part_name not in parts:
                continue
            tree = ET.fromstring(parts[part_name])
            removed = 0
            for r_elem in tree.iter(f"{{{W}}}r"):
                # If the run contains a footnote reference, leave it.
                if r_elem.find(f".//{{{W}}}footnoteReference") is not None:
                    continue
                if r_elem.find(f".//{{{W}}}footnoteRef") is not None:
                    continue
                rPr = r_elem.find(f"{{{W}}}rPr")
                if rPr is None:
                    continue
                to_remove = [c for c in rPr if c.tag not in KEEP_RPR_TAGS]
                for child in to_remove:
                    rPr.remove(child)
                    removed += 1
            parts[part_name] = ET.tostring(
                tree, xml_declaration=True, encoding="UTF-8", standalone=True
            )
            removed_total += removed
        with zipfile.ZipFile(dst_path, "w", zipfile.ZIP_DEFLATED) as zout:
            for n, b in parts.items():
                zout.writestr(n, b)
        _emit({"removed": removed_total})
        return 0

    if primitive == "check":
        # Run debug_format.cmd_check on the source, capture (issue_count,
        # ok_count) by running the function via a captured argparse-style
        # call. cmd_check writes to stdout/stderr; we want to count its
        # "FAIL" and "OK" lines without depending on internal data
        # structures, since they're not exposed as a return value.
        import io
        import contextlib

        # No copy needed — check is read-only.
        from debug_format import cmd_check  # type: ignore

        ns = argparse.Namespace(doc=str(src_path))
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = cmd_check(ns)
        text = buf.getvalue()
        issues = [
            ln.strip()
            for ln in text.splitlines()
            if ln.lstrip().startswith("FAIL")
        ]
        oks = [
            ln.strip()
            for ln in text.splitlines()
            if ln.lstrip().startswith("OK")
        ]
        _emit({
            "rc": int(rc),
            "issue_count": len(issues),
            "ok_count": len(oks),
            "issues": [i[5:].strip() if i.startswith("FAIL") else i for i in issues],
        })
        return 0

    print(f"unknown primitive: {primitive}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())

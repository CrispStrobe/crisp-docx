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
        print(json.dumps({"removed": n}))
        return 0

    if primitive == "normalize_tags":
        shutil.copyfile(src_path, dst_path)
        # docxtool's normalizer mutates in place and returns the count.
        from docxtool import _normalize_nonstandard_tags  # type: ignore
        n = _normalize_nonstandard_tags(dst_path)
        print(json.dumps({"renamed": n}))
        return 0

    if primitive == "notes_to_endnotes":
        shutil.copyfile(src_path, dst_path)
        from rtf_to_docx_endnotes import footnotes_to_endnotes  # type: ignore
        footnotes_to_endnotes(dst_path)
        print(json.dumps({}))
        return 0

    print(f"unknown primitive: {primitive}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())

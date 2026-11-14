"""The two Dart engines CrispCloud ships: the legacy .doc parser and quds.

Both run as AOT executables built by ``wordsteer.py setup``:

* ``legacy_doc``           this repo's dart/legacy_doc.dart, compiled against
                           a quds/CrispCloud package config
* ``quds_steps_<flavour>`` quds' own tool/fidelity/office_steps.dart, compiled
                           unchanged from a checkout. Flavours let one run
                           compare, e.g., the copy vendored in CrispCloud
                           ("vendored") against the fork's head ("main").
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
from typing import Dict, List, Optional, Tuple

BIN = os.environ.get("WORDSTEER_BIN", os.path.expanduser("~/.cache/wordsteer/bin"))


class EngineError(RuntimeError):
    pass


def _run(argv: List[str], timeout: float) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired as e:
        raise EngineError(f"timeout after {timeout}s") from e


def quds_flavours() -> List[str]:
    if not os.path.isdir(BIN):
        return []
    return sorted(n[len("quds_steps_"):] for n in os.listdir(BIN)
                  if n.startswith("quds_steps_"))


def legacy_convert(src: str, dst: str, timeout: float = 180) -> Dict[str, object]:
    """Returns the harness report: ok, ms, limitations, preserved | error."""
    res = _run([os.path.join(BIN, "legacy_doc"), "convert", src, dst], timeout)
    try:
        report = json.loads(res.stdout.strip().splitlines()[-1])
    except (IndexError, json.JSONDecodeError):
        report = {"ok": False, "errorType": "crash",
                  "error": (res.stderr or res.stdout).strip()[-300:]}
    return report


def quds_export(src: str, dst: str, flavour: str = "main", timeout: float = 180) -> None:
    """Open with quds and save again (the editor's open/save path)."""
    res = _run([os.path.join(BIN, f"quds_steps_{flavour}"), "export", src, dst], timeout)
    if res.returncode != 0:
        raise EngineError(_err(res))


def quds_inspect(src: str, flavour: str = "main", timeout: float = 180) -> Dict[str, object]:
    res = _run([os.path.join(BIN, f"quds_steps_{flavour}"), "inspect", src], timeout)
    if res.returncode != 0:
        raise EngineError(_err(res))
    return json.loads(res.stdout)


def quds_modify(src: str, steps: List[Dict[str, object]], dst: str,
                flavour: str = "main", timeout: float = 180) -> None:
    """Apply *quds-native* steps (paragraph indices already resolved)."""
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(steps, f)
        path = f.name
    try:
        res = _run([os.path.join(BIN, f"quds_steps_{flavour}"), "modify", src, path, dst],
                   timeout)
    finally:
        os.unlink(path)
    if res.returncode != 0:
        raise EngineError(_err(res))


def _err(res: subprocess.CompletedProcess) -> str:
    text = (res.stderr or res.stdout).strip()
    # quds prefixes failures with "Office scenario failed: "; keep the cause.
    return text.replace("Office scenario failed: ", "")[-400:]


# -- canonical -> quds step translation ---------------------------------------

def resolve_for_quds(steps: List[Dict[str, object]], paragraphs: List[str]) -> List[Dict[str, object]]:
    """Turn text-anchored canonical steps into quds' index-based steps.

    `paragraphs` is quds' own view (inspect -> paragraphs[].text), so the
    resolution reflects how quds sees the document. Steps run sequentially,
    so the paragraph list is updated for inserts/deletes/replacements.
    """
    paras = list(paragraphs)
    out: List[Dict[str, object]] = []
    for step in steps:
        step = dict(step)
        target = step.pop("target", None)
        op = step["op"]
        if step.get("alignment") in ("both", "justify"):
            step["alignment"] = "justify"  # quds' WmlJustification name
        if op == "sectionFormat":
            out.append(step)
            continue
        if not isinstance(target, dict):
            out.append(step)
            continue
        pi, start, end = find(paras, str(target["find"]), int(target.get("occurrence", 1)))
        if op == "runFormat":
            step.update(paragraph=pi, start=start, end=end)
        elif op == "replaceText":
            step.update(paragraph=pi, start=start, end=end)
            paras[pi] = paras[pi][:start] + str(step["text"]) + paras[pi][end:]
        elif op == "paragraphInsert":
            # quds inserts by block index within a section; express "after
            # paragraph pi" via split-at-end + replace, which needs no block
            # arithmetic: split creates an empty paragraph after pi.
            text = str(step["text"])
            out.append({"op": "paragraphSplit", "paragraph": pi, "offset": len(paras[pi])})
            paras.insert(pi + 1, "")
            step = {"op": "replaceText", "paragraph": pi + 1, "text": text}
            paras[pi + 1] = text
        elif op == "paragraphDelete":
            step.update(paragraph=pi)
            del paras[pi]
        else:
            step.update(paragraph=pi)
        out.append(step)
    return out


def find(paragraphs: List[str], needle: str, occurrence: int = 1) -> Tuple[int, int, int]:
    seen = 0
    for i, text in enumerate(paragraphs):
        pos = text.find(needle)
        while pos >= 0:
            seen += 1
            if seen == occurrence:
                return i, pos, pos + len(needle)
            pos = text.find(needle, pos + 1)
    raise EngineError(f"target not found: {needle[:40]!r} #{occurrence}")


def quds_apply(src: str, steps: List[Dict[str, object]], dst: str,
               flavour: str = "main", timeout: float = 180) -> List[Dict[str, object]]:
    """Canonical steps -> quds. Returns the resolved quds steps (for logs)."""
    view = quds_inspect(src, flavour, timeout)
    resolved = resolve_for_quds(steps, [p.get("text") or "" for p in view["paragraphs"]])
    quds_modify(src, resolved, dst, flavour, timeout)
    return resolved

"""Recipes: reusable, engine-neutral lists of steering steps.

A recipe is JSON::

    {"name": "bold-a-word",
     "description": "…",
     "steps": [{"op": "runFormat", "target": {"select": "word"}, "bold": true}]}

(or just the bare list of steps). Targets either name exact text
(``{"find": "…", "occurrence": 1}``) or a *selector* (``{"select": "word"}``)
that is resolved against the input document right before the engines run,
so one recipe can be replayed over a whole corpus. Resolution only picks
texts that occur exactly once in the main story, which keeps every engine's
addressing unambiguous.
"""

from __future__ import annotations

import copy
import json
import os
import re
from typing import Dict, List, Optional, Tuple

from .model import Model

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC_PATH = os.path.join(HERE, "ops.json")


def spec() -> Dict[str, object]:
    with open(SPEC_PATH) as f:
        return json.load(f)


def load_recipe(path: str) -> Dict[str, object]:
    with open(path) as f:
        data = json.load(f)
    if isinstance(data, list):
        data = {"name": os.path.splitext(os.path.basename(path))[0], "steps": data}
    validate(data["steps"])
    return data


def validate(steps: List[Dict[str, object]]) -> None:
    ops = spec()["ops"]
    for i, s in enumerate(steps, 1):
        op = s.get("op")
        if op not in ops:
            raise ValueError(f"step {i}: unknown op {op!r}")
        allowed = set(ops[op]["params"]) | {"op", "target"}
        extra = set(s) - allowed
        if extra:
            raise ValueError(f"step {i} ({op}): unknown fields {sorted(extra)}")
        if op != "sectionFormat" and "target" not in s:
            raise ValueError(f"step {i} ({op}): needs a target")


_WORD = re.compile(r"[^\W\d_]{6,}", re.UNICODE)
_SENT = re.compile(r"[^.!?]*[^\s.!?][^.!?]{18,200}?[.!?]", re.UNICODE)
_SAFE = re.compile(r"^[^\t\n\f\u0002\u0007]+$")


def _eligible(m: Model) -> List[int]:
    """Plain body paragraphs: no note refs, tabs or breaks, real text."""
    out = []
    for i, p in enumerate(m.paragraphs):
        t = p.text
        if p.note_refs or not t.strip() or not _SAFE.match(t) or len(t) > 2000:
            continue
        out.append(i)
    return out


def candidates(m: Model, kind: str) -> List[Tuple[int, str]]:
    """(paragraph index, text) candidates for a selector, each unique in
    the main story."""
    full = m.text
    seen = set()
    out: List[Tuple[int, str]] = []

    def unique(s: str) -> bool:
        return s not in seen and full.count(s) == 1 and 4 <= len(s) <= 250

    for i in _eligible(m):
        t = m.paragraphs[i].text
        picks: List[str] = []
        if kind == "word":
            picks = [w.group(0) for w in _WORD.finditer(t)]
        elif kind == "sentence":
            picks = [s.group(0).strip() for s in _SENT.finditer(t)]
        elif kind == "paragraph":
            # A paragraph is addressed by a unique anchor inside it.
            if len(t) >= 40:
                picks = [t[:60].strip(), t[-60:].strip()]
        elif kind == "shortParagraph":
            if 8 <= len(t.strip()) <= 80 and not m.paragraphs[i].in_table:
                picks = [t.strip()]
        for s in picks:
            if unique(s):
                seen.add(s)
                out.append((i, s))
                break
    return out


def resolve(steps: List[Dict[str, object]], m: Model) -> List[Dict[str, object]]:
    """Replace selector targets with concrete find targets for document m."""
    out = []
    used = set()
    for s in steps:
        s = copy.deepcopy(s)
        t = s.get("target")
        if isinstance(t, dict) and "select" in t:
            cands = [c for c in candidates(m, str(t["select"])) if c[1] not in used]
            if not cands:
                raise LookupError(f"no candidate for select={t['select']!r}")
            nth = t.get("nth", "middle")
            idx = len(cands) // 2 if nth == "middle" else min(int(nth), len(cands) - 1)
            para, text = cands[idx]
            used.add(text)
            s["target"] = {"find": text, "occurrence": 1}
        out.append(s)
    return out

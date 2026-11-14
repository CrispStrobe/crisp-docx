"""Engine-independent oracle: did a step's requested change happen?

Comparing an engine with Word says whether they *agree*; it cannot say who
is right when Word itself does something surprising. This module checks the
result against the step's own request, using the semantic model only.
"""

from __future__ import annotations

from typing import Dict, List, Optional, Tuple

from .compare import norm
from .model import CHAR_PROPS, Model

_IDX = {k: i for i, k in enumerate(CHAR_PROPS)}


def _find(m: Model, text: str) -> Optional[Tuple[int, int]]:
    for i, p in enumerate(m.paragraphs):
        j = p.text.find(text)
        if j >= 0:
            return i, j
    return None


def _find_para(m: Model, anchor: str) -> Optional[int]:
    hit = _find(m, anchor)
    if hit:
        return hit[0]
    a = norm(anchor)
    for i, p in enumerate(m.paragraphs):
        if a and a in norm(p.text):
            return i
    return None


def check(step: Dict[str, object], before: Model, after: Model) -> Dict[str, object]:
    """Return {"ok": bool, "why": str} for one resolved step."""
    op = step["op"]
    t = step.get("target") or {}
    anchor = str(t.get("find", ""))
    fails: List[str] = []
    if op == "runFormat":
        hit = _find(after, anchor)
        if not hit:
            return {"ok": False, "why": "target text missing after edit"}
        pi, j = hit
        chars = after.paragraphs[pi].chars[j:j + len(anchor)]
        txt = after.paragraphs[pi].text[j:j + len(anchor)]
        want = {}
        for k in ("bold", "italic", "strike"):
            if k in step:
                want[{"bold": "b", "italic": "i", "strike": "strike"}[k]] = bool(step[k])
        if "underline" in step:
            want["u"] = step["underline"] != "none"
        if "size" in step:
            want["sz"] = int(round(float(step["size"]) * 2))
        if "font" in step:
            want["font"] = str(step["font"]).lower()
        if "color" in step:
            want["color"] = str(step["color"]).upper()
        if "highlight" in step:
            want["highlight"] = "*"
        for k, v in want.items():
            vals = [c[_IDX[k]] for c, ch in zip(chars, txt) if not ch.isspace()]
            if k == "u":
                ok = all(bool(x) == v for x in vals)
            elif v == "*":
                ok = all(x is not None for x in vals)
            else:
                ok = all(x == v for x in vals)
            if not ok:
                fails.append(f"{k}≠{v}")
        # Shading-based highlights are invisible to the w:highlight probe.
        fails = [f for f in fails if not (f.startswith("highlight") and "highlight" not in want)]
    elif op == "paragraphFormat":
        pi = _find_para(after, anchor)
        if pi is None:
            return {"ok": False, "why": "target paragraph missing"}
        p = after.paragraphs[pi].props
        exp = {}
        if "alignment" in step:
            exp["jc"] = {"justify": "both"}.get(step["alignment"], step["alignment"])
        for k in ("before", "after", "left", "right", "firstLine"):
            if k in step:
                exp[k] = int(round(float(step[k]) * 20))
        if "lineSpacing" in step:
            exp["line"] = int(round(float(step["lineSpacing"]) * 240))
        for k, v in exp.items():
            got = p.get(k)
            if k == "jc":
                if (got or "left") != v:
                    fails.append(f"jc={got}")
            elif got is None or abs(got - v) > 2:
                fails.append(f"{k}={got}≠{v}")
    elif op == "replaceText":
        new = str(step["text"])
        if before.text.count(anchor) and after.text.count(anchor) >= before.text.count(anchor):
            fails.append("old text still present")
        if new and after.text.count(new) <= before.text.count(new):
            fails.append("new text not present")
    elif op == "heading":
        pi = _find_para(after, anchor)
        if pi is None:
            return {"ok": False, "why": "target paragraph missing"}
        lvl = int(step["level"])
        got = after.paragraphs[pi].props.get("outline")
        if (lvl == 0 and got is not None) or (lvl > 0 and got != lvl - 1):
            fails.append(f"outline={got}")
    elif op == "paragraphInsert":
        pi = _find_para(after, anchor)
        if pi is None or pi + 1 >= len(after.paragraphs):
            fails.append("anchor paragraph missing")
        elif norm(after.paragraphs[pi + 1].text) != norm(str(step["text"])):
            fails.append("next paragraph is not the inserted text")
        if len(after.paragraphs) != len(before.paragraphs) + 1:
            fails.append(f"paragraph count {len(before.paragraphs)}→{len(after.paragraphs)}")
    elif op == "paragraphDelete":
        if anchor in after.text:
            fails.append("paragraph text still present")
        if len(after.paragraphs) != len(before.paragraphs) - 1:
            fails.append(f"paragraph count {len(before.paragraphs)}→{len(after.paragraphs)}")
    elif op == "sectionFormat":
        sec = int(step.get("section", 0))
        if sec >= len(after.sections):
            return {"ok": False, "why": "section missing"}
        s = after.sections[sec]
        for k in ("top", "bottom", "left", "right"):
            if k in step:
                want = int(round(float(step[k]) * 20))
                if s.get(k) is None or abs(s[k] - want) > 2:
                    fails.append(f"{k}={s.get(k)}≠{want}")
    return {"ok": not fails, "why": "; ".join(fails)}


def collateral(base: Model, out: Model, touched: int) -> int:
    """Non-empty paragraphs of `base` whose text, character formatting or
    paragraph properties differ in `out`, beyond the `touched` ones the
    task was supposed to change."""
    import difflib
    a = [(norm(p.text), p) for p in base.paragraphs if norm(p.text)]
    b = [(norm(p.text), p) for p in out.paragraphs if norm(p.text)]
    sm = difflib.SequenceMatcher(None, [x for x, _ in a], [x for x, _ in b], autojunk=False)
    changed = 0
    for tag, a0, a1, b0, b1 in sm.get_opcodes():
        if tag != "equal":
            changed += max(a1 - a0, b1 - b0)
            continue
        for k in range(a1 - a0):
            pa, pb = a[a0 + k][1], b[b0 + k][1]
            if pa.props != pb.props or [c for c, ch in zip(pa.chars, pa.text) if not ch.isspace()] != \
                    [c for c, ch in zip(pb.chars, pb.text) if not ch.isspace()]:
                changed += 1
    return max(0, changed - touched)

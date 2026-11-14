"""Compare two semantic models (see model.py) as reference vs. candidate.

Scores are designed to be aggregated across a corpus:

* ``text.wordSim``      2*matched/total words over aligned paragraphs (0..1)
* ``text.paraMatch``    share of non-empty reference paragraphs found verbatim
                        (whitespace-normalised) in the candidate
* ``format.<prop>``     {agree, n, refSet, kept}: over characters of aligned
                        paragraphs; ``kept/refSet`` is the recall of
                        non-default formatting — the number that exposes loss,
                        since plain text dominates ``agree``
* ``para.<prop>``       agreement of effective paragraph properties
* ``counts``            structural counts, only keys that differ
* ``stories``           similarity of notes, comments, headers/footers, boxes
* ``examples``          a few concrete diffs (contain document text; keep local)
"""

from __future__ import annotations

import difflib
import re
from typing import Dict, List, Optional, Tuple

from .model import CHAR_PROPS, PARA_PROPS, Model

_WS = re.compile(r"[\s   ]+")


def norm(s: str) -> str:
    return _WS.sub(" ", s.replace("‑", "-").replace("­", "")).strip()


def _sim(a: str, b: str) -> float:
    if a == b:
        return 1.0
    if not a or not b:
        return 0.0
    aw, bw = a.split(), b.split()
    if len(aw) + len(bw) > 40000:  # keep pathological stories bounded
        aw, bw = aw[:20000], bw[:20000]
    return difflib.SequenceMatcher(None, aw, bw, autojunk=False).ratio()


def _align(ref: Model, cand: Model):
    """Yield (ref_idx, cand_idx) for paragraphs with equal normalised text,
    plus word-level match statistics over the whole main story."""
    ri = [i for i, p in enumerate(ref.paragraphs) if norm(p.text)]
    ci = [i for i, p in enumerate(cand.paragraphs) if norm(p.text)]
    rt = [norm(ref.paragraphs[i].text) for i in ri]
    ct = [norm(cand.paragraphs[i].text) for i in ci]
    sm = difflib.SequenceMatcher(None, rt, ct, autojunk=False)
    pairs: List[Tuple[int, int]] = []
    matched_words = 0
    for tag, a0, a1, b0, b1 in sm.get_opcodes():
        if tag == "equal":
            for k in range(a1 - a0):
                pairs.append((ri[a0 + k], ci[b0 + k]))
                matched_words += len(rt[a0 + k].split())
        elif tag == "replace":
            aw = " ".join(rt[a0:a1]).split()
            bw = " ".join(ct[b0:b1]).split()
            if aw and bw and len(aw) * len(bw) < 4e8:
                local = difflib.SequenceMatcher(None, aw, bw, autojunk=len(aw) > 5000)
                matched_words += sum(b.size for b in local.get_matching_blocks())
            # Near-equal single paragraphs still contribute formatting evidence.
            if a1 - a0 == b1 - b0:
                for k in range(a1 - a0):
                    if _sim(rt[a0 + k], ct[b0 + k]) >= 0.9:
                        pairs.append((ri[a0 + k], ci[b0 + k]))
    total = sum(len(t.split()) for t in rt) + sum(len(t.split()) for t in ct)
    word_sim = (2.0 * matched_words / total) if total else 1.0
    exact = sum(1 for tag, a0, a1, *_ in sm.get_opcodes() if tag == "equal" for _ in range(a1 - a0))
    return pairs, word_sim, (exact / len(rt)) if rt else 1.0, len(rt), len(ct)


def _char_pairs(a: str, b: str):
    """Index pairs of identical characters between two paragraph texts."""
    if a == b:
        return ((i, i) for i in range(len(a)))
    sm = difflib.SequenceMatcher(None, a, b, autojunk=False)
    return ((m.a + k, m.b + k) for m in sm.get_matching_blocks() for k in range(m.size))


_DEFAULTS = {"b": False, "i": False, "u": None, "strike": False, "color": None,
             "highlight": None, "vertAlign": None, "caps": False,
             "smallCaps": False, "vanish": False}


def compare(ref: Model, cand: Model, examples: int = 6) -> Dict[str, object]:
    out: Dict[str, object] = {"refErrors": ref.errors, "candErrors": cand.errors}
    if ref.errors and not ref.paragraphs:
        out["ok"] = False
        return out
    pairs, word_sim, para_match, nref, ncand = _align(ref, cand)
    out["text"] = {"wordSim": round(word_sim, 4), "paraMatch": round(para_match, 4),
                   "refParas": nref, "candParas": ncand,
                   "exactFull": ref.text == cand.text}
    fmt = {k: {"agree": 0, "n": 0, "refSet": 0, "kept": 0} for k in CHAR_PROPS}
    para = {k: [0, 0] for k in PARA_PROPS}
    ex_fmt: List[Dict[str, object]] = []
    ex_para: List[Dict[str, object]] = []
    for r_i, c_i in pairs:
        rp, cp = ref.paragraphs[r_i], cand.paragraphs[c_i]
        for k in PARA_PROPS:
            rv, cv = rp.props.get(k), cp.props.get(k)
            para[k][1] += 1
            if _para_eq(k, rv, cv):
                para[k][0] += 1
            elif len(ex_para) < examples:
                ex_para.append({"prop": k, "ref": rv, "cand": cv, "text": rp.text[:60]})
        for ia, ib in _char_pairs(rp.text, cp.text):
            rs, cs = rp.chars[ia], cp.chars[ib]
            if rp.text[ia].isspace():
                continue  # formatting on spaces is invisible and engine-specific
            for j, k in enumerate(CHAR_PROPS):
                f = fmt[k]
                f["n"] += 1
                eq = _char_eq(k, rs[j], cs[j])
                if eq:
                    f["agree"] += 1
                if k in _DEFAULTS and rs[j] != _DEFAULTS[k]:
                    f["refSet"] += 1
                    if eq:
                        f["kept"] += 1
                if not eq and len(ex_fmt) < examples and k != "font":
                    ex_fmt.append({"prop": k, "ref": rs[j], "cand": cs[j],
                                   "text": rp.text[max(0, ia - 15): ia + 15]})
    out["format"] = fmt
    out["para"] = {k: {"agree": v[0], "n": v[1]} for k, v in para.items()}
    keys = sorted(set(ref.counts) | set(cand.counts))
    out["counts"] = {k: [ref.counts.get(k, 0), cand.counts.get(k, 0)]
                     for k in keys if ref.counts.get(k, 0) != cand.counts.get(k, 0)}
    stories = {}
    for k in sorted(set(ref.stories) | set(cand.stories)):
        a = norm("\n".join(ref.stories.get(k, [])))
        b = norm("\n".join(cand.stories.get(k, [])))
        if a or b:
            stories[k] = round(_sim(a, b), 4)
    out["stories"] = stories
    out["tables"] = _tables(ref, cand)
    out["sections"] = _sections(ref, cand)
    if examples:
        out["examples"] = {"text": _text_examples(ref, cand, examples),
                           "format": ex_fmt, "para": ex_para}
    out["ok"] = True
    return out


def _char_eq(k: str, a, b) -> bool:
    if k == "sz" and a is not None and b is not None:
        return abs(a - b) <= 1  # half-point rounding between engines
    if k == "u":
        return bool(a) == bool(b)
    return a == b


def _para_eq(k: str, a, b) -> bool:
    if k in ("before", "after", "left", "right", "firstLine", "line"):
        return abs((a or 0) - (b or 0)) <= 20  # one twip-pt of tolerance
    if k == "jc":
        return (a or "left") == (b or "left")
    if k == "lineRule":
        return (a or "auto") == (b or "auto")
    return a == b


def _tables(ref: Model, cand: Model) -> Dict[str, object]:
    out: Dict[str, object] = {"ref": len(ref.tables), "cand": len(cand.tables)}
    if not ref.tables:
        return out
    shape_eq = 0
    cell_sim = 0.0
    for k, rt in enumerate(ref.tables):
        if k >= len(cand.tables):
            break
        ct = cand.tables[k]
        if [len(r) for r in rt] == [len(r) for r in ct]:
            shape_eq += 1
        a = norm(" | ".join(c for row in rt for c in row))
        b = norm(" | ".join(c for row in ct for c in row))
        cell_sim += _sim(a, b)
    out["shapeEq"] = round(shape_eq / len(ref.tables), 4)
    out["cellSim"] = round(cell_sim / len(ref.tables), 4)
    return out


def _sections(ref: Model, cand: Model) -> Dict[str, object]:
    out: Dict[str, object] = {"ref": len(ref.sections), "cand": len(cand.sections)}
    eq = 0
    diffs = []
    for a, b in zip(ref.sections, cand.sections):
        bad = [k for k in ("w", "h", "top", "bottom", "left", "right", "cols")
               if abs((a.get(k) or 0) - (b.get(k) or 0)) > 20]
        if not bad:
            eq += 1
        elif len(diffs) < 3:
            diffs.append({k: [a.get(k), b.get(k)] for k in bad})
    out["geometryEq"] = round(eq / len(ref.sections), 4) if ref.sections else 1.0
    if diffs:
        out["diffs"] = diffs
    return out


def _text_examples(ref: Model, cand: Model, n: int) -> List[Dict[str, str]]:
    rt = [norm(p.text) for p in ref.paragraphs if norm(p.text)]
    ct = [norm(p.text) for p in cand.paragraphs if norm(p.text)]
    out = []
    for tag, a0, a1, b0, b1 in difflib.SequenceMatcher(None, rt, ct, autojunk=False).get_opcodes():
        if tag == "equal":
            continue
        out.append({"op": tag, "ref": " / ".join(rt[a0:a1])[:160],
                    "cand": " / ".join(ct[b0:b1])[:160]})
        if len(out) >= n:
            break
    return out


def summary(result: Dict[str, object]) -> Dict[str, Optional[float]]:
    """Flatten a compare() result into a few headline numbers (0..1)."""
    if not result.get("ok"):
        return {"ok": 0}
    s: Dict[str, Optional[float]] = {"ok": 1,
                                     "wordSim": result["text"]["wordSim"],
                                     "paraMatch": result["text"]["paraMatch"]}
    for k, f in result["format"].items():
        if f["refSet"]:
            s[f"keep.{k}"] = round(f["kept"] / f["refSet"], 4)
        if f["n"]:
            s[f"agree.{k}"] = round(f["agree"] / f["n"], 4)
    for k, f in result["para"].items():
        if f["n"]:
            s[f"para.{k}"] = round(f["agree"] / f["n"], 4)
    for k, v in result["stories"].items():
        s[f"story.{k}"] = v
    if "cellSim" in result["tables"]:
        s["tables.cellSim"] = result["tables"]["cellSim"]
        s["tables.shapeEq"] = result["tables"]["shapeEq"]
    s["sections.geometryEq"] = result["sections"]["geometryEq"]
    return s

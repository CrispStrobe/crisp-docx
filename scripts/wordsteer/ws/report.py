"""Aggregate corpus results into numbers safe to share.

The report contains counts, rates and normalised error classes only — no
paths, file names or document text — so it can go into a public repo or an
issue. Per-file details stay in the local results.jsonl.
"""

from __future__ import annotations

import json
import re
from collections import Counter, defaultdict
from statistics import mean, median
from typing import Dict, List


def _err_class(msg: str) -> str:
    msg = re.sub(r"/[^\s'\"]+", "<path>", msg)
    msg = re.sub(r"\b[0-9a-f]{10,}\b", "<id>", msg)
    msg = re.sub(r"\d+", "N", msg)
    msg = msg.replace("Office scenario failed: ", "").replace("EngineError: ", "")
    return msg[:110]


def load(path: str) -> List[Dict[str, object]]:
    out = []
    with open(path) as f:
        for line in f:
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return out


def roundtrip(recs: List[Dict[str, object]]) -> Dict[str, object]:
    rep: Dict[str, object] = {"files": len(recs),
                              "byKind": dict(Counter(r.get("kind", "?") for r in recs)),
                              "skipped": sum(1 for r in recs if r.get("skipped")),
                              "crashed": sum(1 for r in recs if r.get("crash"))}
    steps: Dict[str, Dict[str, object]] = defaultdict(lambda: {"n": 0, "fail": 0, "errors": Counter(), "s": []})
    lim = Counter()
    for r in recs:
        for name, st in (r.get("steps") or {}).items():
            key = f"{name}[{r.get('kind')}]"
            s = steps[key]
            s["n"] += 1
            s["s"].append(st.get("s") or (st.get("ms", 0) / 1000))
            if not st.get("ok"):
                s["fail"] += 1
                s["errors"][_err_class(str(st.get("error", "?")))] += 1
            for l in st.get("limitations") or []:
                lim[l] += 1
    rep["steps"] = {k: {"n": v["n"], "failRate": round(v["fail"] / v["n"], 4),
                        "medianSeconds": round(median(v["s"]), 3) if v["s"] else None,
                        "topErrors": v["errors"].most_common(6)}
                    for k, v in sorted(steps.items())}
    rep["legacyDeclaredLimitations"] = dict(lim.most_common())
    cmps: Dict[str, Dict[str, object]] = {}
    groups: Dict[str, List[Dict[str, object]]] = defaultdict(list)
    for r in recs:
        for name, c in (r.get("cmp") or {}).items():
            groups[f"{name}[{r.get('kind')}]"].append(c)
    for key, cs in sorted(groups.items()):
        sums = [c.get("summary") or {} for c in cs]
        ok = [s for s in sums if s.get("ok")]
        metrics: Dict[str, List[float]] = defaultdict(list)
        for s in ok:
            for k, v in s.items():
                if k != "ok" and isinstance(v, (int, float)):
                    metrics[k].append(v)
        perfect = sum(1 for s in ok if all(v == 1 for k, v in s.items() if k != "ok"))
        countdiff = Counter()
        direction = defaultdict(lambda: [0, 0])  # [lost, gained]
        for c in cs:
            for k, (a, b) in (c.get("counts") or {}).items():
                countdiff[k] += 1
                direction[k][0 if b < a else 1] += 1
        cmps[key] = {
            "n": len(cs), "compared": len(ok),
            "identical": perfect, "identicalRate": round(perfect / len(ok), 4) if ok else None,
            "mean": {k: round(mean(v), 4) for k, v in sorted(metrics.items())},
            "below99": {k: sum(1 for x in v if x < 0.99) for k, v in sorted(metrics.items())
                        if any(x < 0.99 for x in v)},
            "countDiffs": {k: {"files": n, "lost": direction[k][0], "gained": direction[k][1]}
                           for k, n in countdiff.most_common()},
        }
    rep["comparisons"] = cmps
    return rep


def tasks(recs: List[Dict[str, object]]) -> Dict[str, object]:
    out: Dict[str, Dict[str, object]] = defaultdict(lambda: defaultdict(lambda: {
        "n": 0, "intent": 0, "error": 0, "collateral": 0, "vsWordPerfect": 0, "vsWordN": 0,
        "errors": Counter(), "diffKeys": Counter()}))
    docs = 0
    for r in recs:
        if not r.get("tasks"):
            continue
        docs += 1
        for tid, tr in r["tasks"].items():
            for name, e in (tr.get("engines") or {}).items():
                a = out[tid][name]
                a["n"] += 1
                if e.get("error"):
                    a["error"] += 1
                    a["errors"][_err_class(e["error"])] += 1
                    continue
                a["intent"] += bool(e.get("intent"))
                a["collateral"] += 1 if (e.get("collateral") or 0) > 0 else 0
                if "vsWord" in e:
                    a["vsWordN"] += 1
                    a["vsWordPerfect"] += 0 if e["vsWord"] else 1
                    for k in e["vsWord"]:
                        a["diffKeys"][k] += 1
    rep: Dict[str, object] = {"documents": docs, "tasks": {}}
    for tid, engines in out.items():
        rep["tasks"][tid] = {name: {"n": a["n"], "intentOk": a["intent"], "errors": a["error"],
                                    "collateralDocs": a["collateral"],
                                    "matchesWord": f"{a['vsWordPerfect']}/{a['vsWordN']}" if a["vsWordN"] else None,
                                    "topDiffs": a["diffKeys"].most_common(4),
                                    "topErrors": a["errors"].most_common(3)}
                             for name, a in engines.items()}
    return rep

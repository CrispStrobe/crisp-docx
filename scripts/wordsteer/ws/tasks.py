"""c)/d): the same editing task through Word, quds and crisp-docx.

For each document and each task recipe:

1. resolve the recipe's selectors against the original (Python model),
2. apply the resolved steps with every engine,
3. score each result three ways:
   * ``intent``     — did the requested change happen? (ws.intent, no Word)
   * ``vsWord``     — agreement with Word's result (ws.compare)
   * ``collateral`` — paragraphs changed besides the target, measured
                      against the same engine's own open/save of the original,
                      so an engine's resave noise is not blamed on the task.
"""

from __future__ import annotations

import json
import os
from typing import Dict, List

from . import compare as cmp
from . import intent, recipe
from .engines import crisp, dart, word
from .model import load

HERE = os.path.dirname(os.path.abspath(__file__))
TASKS_PATH = os.path.join(os.path.dirname(HERE), "recipes", "corpus-tasks.json")

# How many paragraphs each op is expected to change (for collateral).
TOUCHES = {"runFormat": 1, "paragraphFormat": 1, "replaceText": 1, "heading": 1,
           "paragraphInsert": 1, "paragraphDelete": 1, "sectionFormat": 0,
           "list": 1, "listClear": 1}


def engines(flavours: List[str], use_word: bool) -> Dict[str, Dict[str, object]]:
    out: Dict[str, Dict[str, object]] = {}
    if use_word:
        out["word"] = {"apply": word.apply,
                       "resave": lambda s, d: word.convert(s, d)}
    for fl in flavours:
        out[f"quds_{fl}"] = {"apply": (lambda s, st, d, fl=fl: dart.quds_apply(s, st, d, fl)),
                             "resave": (lambda s, d, fl=fl: dart.quds_export(s, d, fl))}
    out["crisp"] = {"apply": crisp.apply, "resave": crisp.resave}
    return out


def load_tasks(path: str = TASKS_PATH) -> List[Dict[str, object]]:
    with open(path) as f:
        tasks = json.load(f)["tasks"]
    for t in tasks:
        recipe.validate(t["steps"])
    return tasks


def run_one(src: str, work: str, tasks: List[Dict[str, object]], flavours: List[str],
            use_word: bool = True) -> Dict[str, object]:
    if not src.lower().endswith(".docx"):
        return {"skipped": "tasks run on .docx only"}
    orig = load(src)
    if orig.errors and not orig.paragraphs:
        return {"skipped": f"unreadable: {orig.errors[:1]}"}
    rec: Dict[str, object] = {"paras": len(orig.paragraphs), "tasks": {}}
    eng = engines(flavours, use_word)
    # Baselines: each engine's own open/save of the untouched original.
    base = {}
    for name, e in eng.items():
        out = os.path.join(work, f"base_{name}.docx")
        try:
            e["resave"](src, out)
            base[name] = load(out)
        except word.WordHung:
            raise
        except Exception as ex:
            base[name] = None
            rec.setdefault("baseErrors", {})[name] = f"{type(ex).__name__}: {str(ex)[:200]}"
    for task in tasks:
        tid = task["id"]
        try:
            steps = recipe.resolve(task["steps"], orig)
        except LookupError as ex:
            rec["tasks"][tid] = {"skipped": str(ex)}
            continue
        trec: Dict[str, object] = {"steps": steps, "engines": {}}
        results = {}
        for name, e in eng.items():
            out = os.path.join(work, f"{tid}_{name}.docx")
            er: Dict[str, object] = {}
            try:
                e["apply"](src, steps, out)
                results[name] = load(out)
                checks = [intent.check(s, orig, results[name]) for s in steps]
                er["intent"] = all(c["ok"] for c in checks)
                if not er["intent"]:
                    er["why"] = "; ".join(c["why"] for c in checks if not c["ok"])[:200]
                touched = sum(TOUCHES.get(s["op"], 1) for s in steps)
                if base.get(name) is not None:
                    er["collateral"] = intent.collateral(base[name], results[name], touched)
            except word.WordHung:
                raise
            except Exception as ex:
                er["error"] = f"{type(ex).__name__}: {str(ex)[:240]}"
            trec["engines"][name] = er
        if "word" in results:
            for name, m in results.items():
                if name != "word":
                    s = cmp.summary(cmp.compare(results["word"], m, examples=0))
                    trec["engines"][name]["vsWord"] = {k: v for k, v in s.items() if v != 1}
        rec["tasks"][tid] = trec
    return rec

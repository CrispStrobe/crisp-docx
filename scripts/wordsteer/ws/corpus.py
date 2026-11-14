"""Corpus audits: round-trips and edit tasks over a folder tree of Word files.

Results are JSON lines, one per unique source (deduplicated by content hash),
appended as each file finishes, so an interrupted run resumes where it
stopped. Intermediate packages are deleted per file unless --keep is given;
only the comparison results remain.

Privacy: results include source paths and short text excerpts of diffs. They
are written to a local run directory and must not be committed to a public
repository. `report` produces aggregate numbers only (no paths, no text).
"""

from __future__ import annotations

import hashlib
import json
import os
import random
import shutil
import sys
import time
import traceback
from typing import Callable, Dict, Iterable, List, Optional

from . import compare as cmp
from .model import load
from .engines import dart, word

EXTS = (".doc", ".docx")


def scan(roots: Iterable[str]) -> List[str]:
    out = []
    for root in roots:
        for dirpath, dirnames, files in os.walk(os.path.expanduser(root)):
            dirnames[:] = [d for d in dirnames if not d.startswith(".")
                           and not d.endswith((".app", ".photoslibrary", ".bundle"))]
            for f in files:
                if f.startswith(("~$", "._", ".~")):
                    continue
                if f.lower().endswith(EXTS):
                    out.append(os.path.join(dirpath, f))
    return sorted(out)


def digest(path: str) -> str:
    h = hashlib.sha1()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _done(results: str) -> set:
    seen = set()
    if os.path.exists(results):
        with open(results) as f:
            for line in f:
                try:
                    seen.add(json.loads(line)["id"])
                except (json.JSONDecodeError, KeyError):
                    pass
    return seen


def _cmp(ref_path: str, cand_path: str, examples: int = 3) -> Dict[str, object]:
    try:
        r = cmp.compare(load(ref_path), load(cand_path), examples=examples)
        return {"summary": cmp.summary(r), "counts": r.get("counts"),
                "sections": r.get("sections"), "tables": r.get("tables"),
                "text": r.get("text"), "examples": r.get("examples"),
                "errors": (r.get("refErrors") or []) + (r.get("candErrors") or [])}
    except Exception as e:  # comparator bugs must not stop a corpus run
        return {"summary": {"ok": 0}, "compareError": f"{type(e).__name__}: {e}"}


def _timed(fn: Callable, *a, **kw):
    t = time.time()
    try:
        out = fn(*a, **kw)
        return {"ok": True, "s": round(time.time() - t, 2), "out": out}
    except word.WordHung:
        raise
    except Exception as e:
        return {"ok": False, "s": round(time.time() - t, 2),
                "error": f"{type(e).__name__}: {str(e)[:300]}"}


def roundtrip_one(src: str, work: str, flavours: List[str], use_word: bool) -> Dict[str, object]:
    """a) docx -> (Word) doc; b) doc -> (legacy) docx, compared with Word's
    reading of the same .doc and with the original; c) quds open/save."""
    ext = os.path.splitext(src)[1].lower()
    rec: Dict[str, object] = {"kind": ext[1:], "bytes": os.path.getsize(src), "steps": {},
                              "cmp": {}}
    steps, cmps = rec["steps"], rec["cmp"]
    w_doc = os.path.join(work, "w.doc")
    w_docx = os.path.join(work, "w.docx")
    l_docx = os.path.join(work, "l.docx")
    if ext == ".docx":
        if use_word:
            steps["word"] = _timed(word.convert_chain, src, [w_doc, w_docx])
            if steps["word"]["ok"] and steps["word"]["out"]:
                steps["word"]["ok"] = False
                steps["word"]["error"] = json.dumps(steps["word"]["out"])[:300]
            steps["word"].pop("out", None)
        doc_for_legacy = w_doc
    else:
        if use_word:
            steps["word"] = _timed(word.convert, src, w_docx)
            steps["word"].pop("out", None)
        doc_for_legacy = src
    have_wdocx = os.path.exists(w_docx)
    if os.path.exists(doc_for_legacy):
        rep = _timed(dart.legacy_convert, doc_for_legacy, l_docx)
        if rep["ok"]:
            rep.update(rep.pop("out"))  # harness report: ok/ms/limitations/...
        steps["legacy"] = rep
    have_l = os.path.exists(l_docx) and steps.get("legacy", {}).get("ok")
    if ext == ".docx" and have_wdocx:
        cmps["wordDocLoss"] = _cmp(src, w_docx)          # inherent .doc loss
    if have_wdocx and have_l:
        cmps["legacyVsWord"] = _cmp(w_docx, l_docx)      # parser gap (main metric)
    if ext == ".docx" and have_l:
        cmps["legacyVsOrig"] = _cmp(src, l_docx)
    # quds open -> save, on the original docx (or Word's docx of a .doc).
    base = src if ext == ".docx" else (w_docx if have_wdocx else None)
    for fl in flavours:
        if base:
            out = os.path.join(work, f"q_{fl}.docx")
            steps[f"quds_{fl}"] = _timed(dart.quds_export, base, out, fl)
            steps[f"quds_{fl}"].pop("out", None)
            if steps[f"quds_{fl}"]["ok"]:
                cmps[f"qudsResave_{fl}"] = _cmp(base, out)
        if have_l:
            # CrispCloud's own .doc path: legacy conversion, then the editor's save.
            out = os.path.join(work, f"ql_{fl}.docx")
            steps[f"qudsOnLegacy_{fl}"] = _timed(dart.quds_export, l_docx, out, fl)
            steps[f"qudsOnLegacy_{fl}"].pop("out", None)
            if steps[f"qudsOnLegacy_{fl}"]["ok"]:
                cmps[f"qudsOnLegacy_{fl}"] = _cmp(l_docx, out)
    return rec


def run(mode: str, roots: List[str], out_dir: str, limit: Optional[int] = None,
        flavours: Optional[List[str]] = None, use_word: bool = True,
        keep: bool = False, seed: int = 7, kinds=("doc", "docx"),
        max_bytes: int = 60 << 20, task_fn=None, log=print,
        min_free: int = 2 << 30) -> None:
    os.makedirs(out_dir, exist_ok=True)
    results = os.path.join(out_dir, "results.jsonl")
    done = _done(results)
    flavours = flavours if flavours is not None else dart.quds_flavours()
    files = [f for f in scan(roots) if os.path.splitext(f)[1].lower()[1:] in kinds]
    random.Random(seed).shuffle(files)  # partial runs stay representative
    seen_hashes = set()
    processed = 0
    t0 = time.time()
    if use_word:
        word.cleanup_stage()
    log(f"{len(files)} candidate files, {len(done)} already done -> {results}")
    for path in files:
        if limit is not None and processed >= limit:
            break
        free = shutil.disk_usage(out_dir).free
        if free < min_free:
            log(f"STOP: only {free >> 20} MB free on disk (< {min_free >> 20} MB); "
                f"stopping before the machine wedges. Free space, then rerun.")
            break
        if os.path.exists(os.path.join(out_dir, "STOP")):
            log("STOP file found; stopping cleanly (rerun resumes).")
            break
        try:
            size = os.path.getsize(path)
            h = digest(path)
        except OSError as e:
            continue
        rid = f"{mode}:{h}"
        if h in seen_hashes or rid in done:
            seen_hashes.add(h)
            continue
        seen_hashes.add(h)
        rec: Dict[str, object] = {"id": rid, "path": path, "sha1": h}
        if size > max_bytes:
            rec.update(skipped=f"larger than {max_bytes} bytes")
        else:
            work = os.path.join(out_dir, "work", h[:12])
            os.makedirs(work, exist_ok=True)
            try:
                if mode == "roundtrip":
                    rec.update(roundtrip_one(path, work, flavours, use_word))
                else:
                    rec.update(task_fn(path, work))
            except word.WordHung as e:
                log(f"STOP: Word did not respond ({e}). Not killing Word: it may "
                    f"hold unsaved user documents. Check for a dialog, then rerun "
                    f"to resume. Stuck file: {path}")
                rec.update(hung=True)
                _append(results, rec)
                if not keep:
                    shutil.rmtree(work, ignore_errors=True)
                return
            except Exception as e:
                rec.update(crash=f"{type(e).__name__}: {e}", tb=traceback.format_exc()[-800:])
            if not keep:
                shutil.rmtree(work, ignore_errors=True)
        _append(results, rec)
        processed += 1
        if processed % 10 == 0:
            rate = (time.time() - t0) / processed
            log(f"{processed} done, {rate:.1f}s/file")
    log(f"finished: {processed} processed in {time.time() - t0:.0f}s")


def _append(path: str, rec: Dict[str, object]) -> None:
    with open(path, "a") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")

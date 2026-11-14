"""crisp-docx (this repository's Rust CLI) as a steering engine."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
from typing import Dict, List

from .dart import BIN, EngineError


def binary() -> str:
    for cand in (os.path.join(BIN, "crisp-docx"), shutil.which("crisp-docx") or ""):
        if cand and os.access(cand, os.X_OK):
            return cand
    raise EngineError("crisp-docx binary not found (run `wordsteer.py setup`)")


def apply(src: str, steps: List[Dict[str, object]], dst: str, timeout: float = 120) -> Dict[str, object]:
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(steps, f)
        path = f.name
    try:
        res = subprocess.run([binary(), "apply-steps", src, path, dst],
                             capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired as e:
        raise EngineError(f"timeout after {timeout}s") from e
    finally:
        os.unlink(path)
    if res.returncode != 0:
        raise EngineError((res.stderr or res.stdout).strip().splitlines()[-1][-400:]
                          if (res.stderr or res.stdout).strip() else "crisp-docx failed")
    return json.loads(res.stdout.strip().splitlines()[-1])


def resave(src: str, dst: str) -> None:
    """crisp-docx's open/save path: an empty step list."""
    apply(src, [], dst)


def paragraphs(src: str) -> List[str]:
    res = subprocess.run([binary(), "paragraphs", src], capture_output=True, text=True, timeout=120)
    if res.returncode != 0:
        raise EngineError(res.stderr.strip()[-400:])
    return json.loads(res.stdout)

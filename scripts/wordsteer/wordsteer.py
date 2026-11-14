#!/usr/bin/env python3
"""wordsteer — steer Word, quds, crisp-docx and the legacy .doc parser with
one recipe vocabulary, and audit them against each other.

  wordsteer.py check-env
  wordsteer.py setup --crispcloud ~/code/local/CrispCloud [--fork-ref main]
  wordsteer.py ops
  wordsteer.py convert  --engine word|legacy|quds[:flavour]|crisp IN OUT
  wordsteer.py apply    --engine word|quds[:flavour]|crisp (--recipe R | --steps JSON) IN OUT
  wordsteer.py compare  REF CAND [--json]
  wordsteer.py corpus roundtrip|tasks --root DIR... --out RUN [--limit N] [--no-word]
  wordsteer.py report RUN [--json]

Engines: word = Microsoft Word for Mac via AppleScript (reference; set
WORDSTEER_HIDE=1 to keep it hidden), legacy = CrispCloud's .doc parser,
quds:<flavour> = quds office_steps (flavours e.g. "vendored" = the copy in
CrispCloud, "main" = the fork's head), crisp = this repo's crisp-docx CLI.
See README.md in this directory.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)


def _expat_ok() -> bool:
    try:
        from xml.etree import ElementTree as ET
        ET.fromstring("<a/>")
        return True
    except ImportError:
        return False


if not _expat_ok():
    sys.exit("This Python has a broken pyexpat (seen with Homebrew python@3.14/3.15 "
             "against an older system libexpat). Run with another interpreter, "
             "e.g. python3.13 or /usr/bin/python3.")

from ws import compare as cmp  # noqa: E402
from ws import corpus, recipe, report  # noqa: E402
from ws.engines import crisp, dart, word  # noqa: E402
from ws.model import load  # noqa: E402


def _engine(spec: str):
    name, _, flavour = spec.partition(":")
    if name == "quds":
        flavour = flavour or "main"
        if flavour not in dart.quds_flavours():
            sys.exit(f"quds flavour {flavour!r} not built; have {dart.quds_flavours()}")
    return name, flavour


def cmd_convert(a):
    name, fl = _engine(a.engine)
    if name == "word":
        word.convert(a.input, a.output)
    elif name == "legacy":
        print(json.dumps(dart.legacy_convert(a.input, a.output)))
    elif name == "quds":
        dart.quds_export(a.input, a.output, fl)
    elif name == "crisp":
        crisp.resave(a.input, a.output)
    else:
        sys.exit(f"unknown engine {name}")


def cmd_apply(a):
    if a.recipe:
        steps = recipe.load_recipe(a.recipe)["steps"]
    else:
        steps = json.loads(a.steps)
        recipe.validate(steps)
    steps = recipe.resolve(steps, load(a.input))
    name, fl = _engine(a.engine)
    if name == "word":
        word.apply(a.input, steps, a.output)
    elif name == "quds":
        dart.quds_apply(a.input, steps, a.output, fl)
    elif name == "crisp":
        crisp.apply(a.input, steps, a.output)
    else:
        sys.exit(f"engine {name} cannot apply steps")
    print(json.dumps({"engine": a.engine, "steps": steps}, ensure_ascii=False))


def cmd_compare(a):
    r = cmp.compare(load(a.ref), load(a.cand))
    if a.json:
        print(json.dumps(r, ensure_ascii=False, indent=1))
        return
    s = cmp.summary(r)
    bad = {k: v for k, v in s.items() if v != 1}
    print("identical (semantic model)" if not bad and not r.get("counts") else json.dumps(
        {"belowOne": bad, "counts": r.get("counts"), "examples": r.get("examples")},
        ensure_ascii=False, indent=1))


def cmd_ops(a):
    sp = recipe.spec()
    for op, d in sp["ops"].items():
        print(f"{op:16} engines={','.join(d['engines']):18} params={','.join(d['params'])}")
    print("\nconversions:", json.dumps(sp["conversions"]))


def cmd_corpus(a):
    from ws import tasks
    flavours = a.flavours.split(",") if a.flavours else None
    if a.mode == "roundtrip":
        corpus.run("roundtrip", a.root, a.out, limit=a.limit, flavours=flavours,
                   use_word=not a.no_word, keep=a.keep)
    else:
        T = tasks.load_tasks(a.tasks or tasks.TASKS_PATH)
        fl = flavours if flavours is not None else dart.quds_flavours()
        corpus.run("tasks", a.root, a.out, limit=a.limit, flavours=fl,
                   use_word=not a.no_word, keep=a.keep, kinds=("docx",),
                   task_fn=lambda p, w: tasks.run_one(p, w, T, fl, not a.no_word))


def cmd_report(a):
    path = os.path.join(a.run, "results.jsonl")
    recs = report.load(path)
    rep = report.tasks(recs) if any("tasks" in r for r in recs) else report.roundtrip(recs)
    print(json.dumps(rep, ensure_ascii=False, indent=1))


def cmd_setup(a):
    """Build the engine binaries into ~/.cache/wordsteer/bin."""
    os.makedirs(dart.BIN, exist_ok=True)
    cc = os.path.expanduser(a.crispcloud)
    vend = os.path.join(cc, "packages", "quds_office_engine")
    run = lambda *c, **k: subprocess.run(c, check=True, **k)  # noqa: E731
    run("dart", "pub", "get", cwd=vend)
    run("dart", "compile", "exe", "tool/fidelity/office_steps.dart",
        "-o", os.path.join(dart.BIN, "quds_steps_vendored"), cwd=vend)
    run("dart", "compile", "exe",
        f"--packages={vend}/.dart_tool/package_config.json",
        os.path.join(HERE, "dart", "legacy_doc.dart"), "-o", os.path.join(dart.BIN, "legacy_doc"))
    if a.fork_ref:
        clone = os.path.join(os.path.dirname(dart.BIN), "quds-fork")
        shutil.rmtree(clone, ignore_errors=True)
        run("git", "clone", "-q", "--depth", "1", "--branch", a.fork_ref, "--filter=blob:none",
            "--sparse", "https://github.com/CrispStrobe/quds_office_suite.git", clone)
        run("git", "sparse-checkout", "set", "packages/quds_office_engine",
            "packages/legacy_doc_parser", cwd=clone)
        # The fork is a pub workspace; build the engine standalone.
        for p in ("packages/quds_office_engine/pubspec.yaml", "packages/legacy_doc_parser/pubspec.yaml"):
            fp = os.path.join(clone, p)
            with open(fp) as f:
                txt = f.read().replace("resolution: workspace\n", "")
            with open(fp, "w") as f:
                f.write(txt)
        os.rename(os.path.join(clone, "pubspec.yaml"), os.path.join(clone, "pubspec.workspace.yaml"))
        eng = os.path.join(clone, "packages", "quds_office_engine")
        run("dart", "pub", "get", cwd=eng)
        run("dart", "compile", "exe", "tool/fidelity/office_steps.dart",
            "-o", os.path.join(dart.BIN, f"quds_steps_{a.fork_ref.replace('/', '_')}"), cwd=eng)
    repo = os.path.dirname(os.path.dirname(HERE))
    env = dict(os.environ, CARGO_TARGET_DIR=os.environ.get(
        "CARGO_TARGET_DIR", os.path.expanduser("~/.cache/shared-cargo-target")))
    run("cargo", "build", "--release", "-p", "crisp-docx-cli", cwd=repo, env=env)
    shutil.copy(os.path.join(env["CARGO_TARGET_DIR"], "release", "crisp-docx"), dart.BIN)
    print("built:", sorted(os.listdir(dart.BIN)))


def cmd_check_env(a):
    print("python", sys.version.split()[0], "expat ok")
    print("word", word.available())
    print("bin", dart.BIN, sorted(os.listdir(dart.BIN)) if os.path.isdir(dart.BIN) else "missing")
    print("quds flavours", dart.quds_flavours())


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("check-env").set_defaults(fn=cmd_check_env)
    p = sub.add_parser("setup")
    p.add_argument("--crispcloud", default="~/code/local/CrispCloud")
    p.add_argument("--fork-ref", default="main", help="quds fork branch to build as a flavour ('' to skip)")
    p.set_defaults(fn=cmd_setup)
    sub.add_parser("ops").set_defaults(fn=cmd_ops)
    p = sub.add_parser("convert")
    p.add_argument("--engine", required=True)
    p.add_argument("input")
    p.add_argument("output")
    p.set_defaults(fn=cmd_convert)
    p = sub.add_parser("apply")
    p.add_argument("--engine", required=True)
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--recipe")
    g.add_argument("--steps", help="inline JSON list of steps")
    p.add_argument("input")
    p.add_argument("output")
    p.set_defaults(fn=cmd_apply)
    p = sub.add_parser("compare")
    p.add_argument("ref")
    p.add_argument("cand")
    p.add_argument("--json", action="store_true")
    p.set_defaults(fn=cmd_compare)
    p = sub.add_parser("corpus")
    p.add_argument("mode", choices=["roundtrip", "tasks"])
    p.add_argument("--root", action="append", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--limit", type=int)
    p.add_argument("--flavours", help="comma list of quds flavours (default: all built)")
    p.add_argument("--tasks", help="task recipe file (default recipes/corpus-tasks.json)")
    p.add_argument("--no-word", action="store_true")
    p.add_argument("--keep", action="store_true", help="keep intermediate packages")
    p.set_defaults(fn=cmd_corpus)
    p = sub.add_parser("report")
    p.add_argument("run")
    p.set_defaults(fn=cmd_report)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()

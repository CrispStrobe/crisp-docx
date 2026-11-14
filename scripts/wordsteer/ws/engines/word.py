"""Microsoft Word for Mac, steered through AppleScript (osascript).

Word is the reference engine: conversions (docx<->doc) and edits made here are
what the other engines are measured against.

Safety rules (Word is usually the user's live instance with unsaved work):

* Files are staged inside Word's sandbox container, so no "Grant access"
  prompts appear; each job uses a unique file name and every document is
  addressed *by that name*, never as "active document" — a click on another
  window mid-run cannot redirect a save.
* Only documents this module opened are closed, always ``saving no``.
  Word is never quit and never killed. A timeout raises WordHung; callers must
  stop rather than retry blindly, because a modal dialog may be up.
* Macros are force-disabled for the duration of every call (automation
  security), then the user's previous setting is restored. Password-protected
  files get a dummy password so they fail fast instead of prompting.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import uuid
from typing import Dict, List, Optional

STAGE = os.path.expanduser(
    "~/Library/Containers/com.microsoft.Word/Data/tmp/wordsteer")

FORMATS = {"doc": "format document97", "docx": "format document",
           "rtf": "format rtf", "pdf": "format PDF", "odt": "format ODT",
           "txt": "format Unicode text", "dot": "format template97"}

ALIGN = {"left": "align paragraph left", "center": "align paragraph center",
         "right": "align paragraph right", "both": "align paragraph justify",
         "justify": "align paragraph justify"}
HIGHLIGHT = {"FFFF00": "yellow", "00FF00": "bright green", "00FFFF": "turquoise",
             "FF00FF": "pink", "0000FF": "blue", "FF0000": "red",
             "000080": "dark blue", "008080": "teal", "008000": "green",
             "800080": "violet", "800000": "dark red", "808000": "dark yellow",
             "808080": "gray50", "C0C0C0": "gray25", "000000": "black",
             "yellow": "yellow", "green": "bright green", "none": "no highlight"}
UNDERLINE = {"single": "underline single", "double": "underline double",
             "none": "underline none", "words": "underline words",
             "dotted": "underline dotted", "thick": "underline thick",
             "dash": "underline dash", "wave": "underline wavy"}

# Ops this engine implements (canonical names from ops.json).
SUPPORTED = {"runFormat", "paragraphFormat", "replaceText", "heading",
             "paragraphInsert", "paragraphDelete", "sectionFormat", "list",
             "listClear"}


class WordError(RuntimeError):
    pass


class WordHung(WordError):
    """osascript timed out; a modal dialog may be open in Word."""


def _q(s: str) -> str:
    """AppleScript string literal."""
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _highlight(v) -> str:
    v = str(v)
    for key in (v, v.lower(), v.upper().lstrip("#")):
        if key in HIGHLIGHT:
            return HIGHLIGHT[key]
    raise WordError(f"no Word highlight colour for {v!r}")


def _find_text(s: str) -> str:
    # Word's Find treats ^ as an escape; it also caps the pattern at 255.
    return s.replace("^", "^^")


_PRELUDE = """
on hfsOf(p)
  return (POSIX file p) as string
end hfsOf

on wsOffset(needle, hay)
  considering case
    return offset of needle in hay
  end considering
end wsOffset

on wsSame(a, b)
  considering case
    return a is b
  end considering
end wsSame
"""

_OPEN = """
  set oldSec to automation security
  set automation security to msoAutomationSecurityForceDisable
  try
    open file name my hfsOf({src}) without confirm conversions and add to recent files password document "ws-no-password" write password "ws-no-password"
    -- Under load Word can return from `open` before the document is
    -- addressable by name; wait for it rather than failing.
    set d to missing value
    repeat 100 times
      try
        set d to document {name}
        exit repeat
      end try
      delay 0.2
    end repeat
    if d is missing value then error "wordsteer: document did not open" number 9003
{hide}"""

_CLOSE = """
    close d saving no
    set automation security to oldSec
  on error errMsg number errNum
    try
      close document {name} saving no
    end try
    set automation security to oldSec
    error errMsg number errNum
  end try
"""


_PERMANENT = ("(9001)", "(9002)", "number 9001", "number 9002", "target not found",
              "find/range mismatch", "does not implement")


def _retrying(fn):
    """Retry once on transient Word errors (busy app, late file). Never on
    a hang, and never on deterministic outcomes such as target-not-found."""
    import functools
    import time

    @functools.wraps(fn)
    def wrapper(*a, **kw):
        try:
            return fn(*a, **kw)
        except WordHung:
            raise
        except WordError as e:
            if any(m in str(e) for m in _PERMANENT):
                raise
            time.sleep(2)
            return fn(*a, **kw)
    return wrapper


LOCK = os.path.expanduser("~/.cache/wordsteer/word.lock")


def run(script: str, timeout: float = 180) -> str:
    """One osascript call, serialised across processes: Word's AppleScript
    bridge degrades (documents resolve to `missing value` until Word is
    restarted) when several clients drive it concurrently."""
    import fcntl
    os.makedirs(os.path.dirname(LOCK), exist_ok=True)
    with open(LOCK, "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        return _run_unlocked(script, timeout)


def healthy() -> bool:
    """False when Word's object model has degraded (names come back as
    `missing value` although documents are open). Only a Word restart fixes
    that; never attempt it automatically — the user may have unsaved work."""
    out = run('tell application "Microsoft Word" to get {count documents, name of every document}', 30)
    return "missing value" not in out


def _run_unlocked(script: str, timeout: float) -> str:
    try:
        res = subprocess.run(["osascript", "-"], input=_PRELUDE + script,
                             capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired as e:
        raise WordHung(f"osascript timed out after {timeout}s") from e
    if res.returncode != 0:
        raise WordError(res.stderr.strip()[-400:])
    return res.stdout.strip()


class Job:
    """A staging directory in Word's container, removed on exit."""

    def __init__(self, keep: bool = False):
        self.id = "ws" + uuid.uuid4().hex[:10]
        self.dir = os.path.join(STAGE, self.id)
        self.keep = keep

    def __enter__(self):
        os.makedirs(self.dir, exist_ok=True)
        return self

    def __exit__(self, *exc):
        # The AppleScript error path cannot always close its document (e.g.
        # after a failed command); close anything carrying this job's prefix.
        try:
            run(f'''tell application "Microsoft Word"
  repeat with k from (count documents) to 1 by -1
    if name of document k starts with "{self.id}" then close document k saving no
  end repeat
end tell''', 30)
        except WordError:
            pass
        if not self.keep:
            shutil.rmtree(self.dir, ignore_errors=True)

    def stage(self, src: str, tag: str = "in") -> str:
        ext = os.path.splitext(src)[1].lower()
        dst = os.path.join(self.dir, f"{self.id}-{tag}{ext}")
        shutil.copyfile(src, dst)
        return dst

    def path(self, tag: str, ext: str) -> str:
        return os.path.join(self.dir, f"{self.id}-{tag}.{ext}")


# WORDSTEER_HIDE=1: re-hide Word right after every open. Word un-hides itself
# when it opens a window but does not take focus, so with this a window shows
# for a fraction of a second at most. It also hides the user's own Word
# windows for the duration (click Word in the Dock to bring them back).
_HIDE = ('    tell application "System Events" to set visible of process '
         '"Microsoft Word" to false\n')


def _open_block(src: str) -> str:
    hide = _HIDE if os.environ.get("WORDSTEER_HIDE") == "1" else ""
    return _OPEN.format(src=_q(src), name=_q(os.path.basename(src)), hide=hide)


def _close_block(src: str) -> str:
    return _CLOSE.format(name=_q(os.path.basename(src)))


def _save_as(dst: str, fmt: str) -> str:
    return (f"    save as d file name my hfsOf({_q(dst)}) "
            f"file format {FORMATS[fmt]}\n")


@_retrying
def convert(src: str, dst: str, timeout: float = 180) -> None:
    """Open src in Word and save a copy as dst (format from dst's extension)."""
    fmt = os.path.splitext(dst)[1].lstrip(".").lower()
    with Job() as job:
        staged = job.stage(src)
        out = job.path("out", fmt)
        script = ('tell application "Microsoft Word"\n' + _open_block(staged)
                  + _save_as(out, fmt) + _close_block(staged) + "end tell\n")
        # After save-as, the open document is named after the *new* file.
        script = script.replace(f"close d saving no",
                                f"close document {_q(os.path.basename(out))} saving no")
        run(script, timeout)
        _wait_for(out)
        shutil.copyfile(out, dst)


def convert_chain(src: str, outputs: List[str], timeout: float = 240) -> Dict[str, str]:
    """src -> outputs[0] -> outputs[1] ... each step reopening the previous
    result, all inside one osascript call. Returns {output: error} for
    failed steps (empty on success)."""
    errors: Dict[str, str] = {}
    with Job() as job:
        cur = job.stage(src)
        for i, dst in enumerate(outputs):
            fmt = os.path.splitext(dst)[1].lstrip(".").lower()
            out = job.path(f"o{i}", fmt)
            script = ('tell application "Microsoft Word"\n' + _open_block(cur)
                      + _save_as(out, fmt) + _close_block(cur) + "end tell\n")
            script = script.replace("close d saving no",
                                    f"close document {_q(os.path.basename(out))} saving no")
            try:
                try:
                    run(script, timeout)
                    _wait_for(out)
                except WordHung:
                    raise
                except WordError as e:  # one retry for transient errors
                    if any(m in str(e) for m in _PERMANENT):
                        raise
                    import time
                    time.sleep(2)
                    run(script, timeout)
                    _wait_for(out)
                shutil.copyfile(out, dst)
                cur = out
            except WordHung:
                raise
            except WordError as e:
                errors[dst] = str(e)
                break
    return errors


def _wait_for(path: str, tries: int = 300):
    import time
    for _ in range(tries):
        if os.path.exists(path) and os.path.getsize(path) > 0:
            return
        time.sleep(0.1)
    raise WordError(f"Word reported success but wrote no file: {os.path.basename(path)}")


# -- edits -------------------------------------------------------------------

def _target(t: Dict[str, object], var: str = "r") -> str:
    """AppleScript that sets `var` to the range of target.find (occurrence n)
    in the main story, or errors with a recognisable message."""
    text = str(t["find"])
    if len(text) > 255:
        raise WordError("target.find longer than Word's 255-character find limit")
    n = int(t.get("occurrence", 1))
    # Word ranges in AppleScript are references or fixed `text range id`s:
    # `execute find` narrows neither, and a window's selection only works
    # while that document is frontmost. So the n-th occurrence is located
    # paragraph by paragraph (case-sensitive), turned into absolute offsets
    # from the paragraph start, and verified. Field codes inside the
    # paragraph shift positions; a short forward scan corrects for that.
    return f"""
    set wsNeedle to {_q(text)}
    set wsLen to (count wsNeedle)
    set hits to 0
    set wsHit to 0
    set wsOff to 0
      repeat with k from 1 to (count paragraphs of d)
        set ptxt to content of text object of paragraph k of d
        if ptxt is missing value then set ptxt to ""
        set wsFrom to 1
        repeat while wsFrom <= (count ptxt)
          set o to my wsOffset(wsNeedle, (text wsFrom thru -1 of ptxt))
          if o = 0 then exit repeat
          set hits to hits + 1
          if hits = {n} then
            set wsHit to k
            set wsOff to wsFrom + o - 2
            exit repeat
          end if
          set wsFrom to wsFrom + o
        end repeat
        if wsHit > 0 then exit repeat
      end repeat
      if wsHit = 0 then error "wordsteer: target not found" number 9001
      set wsBase to (start of content of text object of paragraph wsHit of d) + wsOff
      set wsOk to false
      repeat with delta from 0 to 400
        set {var} to create range d start (wsBase + delta) end (wsBase + delta + wsLen)
        if my wsSame((content of {var}), wsNeedle) then
          set wsOk to true
          exit repeat
        end if
      end repeat
    if not wsOk then error "wordsteer: find/range mismatch" number 9002
"""


def _para_of(var: str = "r", out: str = "rp") -> str:
    return f"    set {out} to text object of paragraph 1 of {var}\n"


def _step(step: Dict[str, object]) -> str:
    op = step["op"]
    t = step.get("target")
    if op not in SUPPORTED:
        raise WordError(f"word engine does not implement {op}")
    if op == "sectionFormat":
        sec = int(step.get("section", 0)) + 1
        s = f"    set ps to page setup of section {sec} of d\n"
        for key, prop in (("top", "top margin"), ("bottom", "bottom margin"),
                          ("left", "left margin"), ("right", "right margin"),
                          ("width", "page width"), ("height", "page height")):
            if key in step:
                s += f"    set {prop} of ps to {float(step[key])}\n"
        return s
    if not isinstance(t, dict) or "find" not in t:
        raise WordError(f"{op} needs a target with 'find'")
    s = _target(t)
    if op == "runFormat":
        s += "    set f to font object of r\n"
        if "bold" in step:
            s += f"    set bold of f to {str(bool(step['bold'])).lower()}\n"
        if "italic" in step:
            s += f"    set italic of f to {str(bool(step['italic'])).lower()}\n"
        if "strike" in step:
            s += f"    set strike through of f to {str(bool(step['strike'])).lower()}\n"
        if "underline" in step:
            s += f"    set underline of f to {UNDERLINE[str(step['underline'])]}\n"
        if "font" in step:
            s += f"    set name of f to {_q(str(step['font']))}\n"
        if "size" in step:
            s += f"    set font size of f to {float(step['size'])}\n"
        if "color" in step:
            hexv = str(step["color"]).lstrip("#")
            # Word's AppleScript colours are 16-bit per channel; 8-bit
            # values are accepted *and silently ignored*.
            rgb = [int(hexv[i:i + 2], 16) * 257 for i in (0, 2, 4)]
            s += f"    set color of f to {{{rgb[0]}, {rgb[1]}, {rgb[2]}}}\n"
        if "highlight" in step:
            s += f"    set highlight color index of r to {_highlight(step['highlight'])}\n"
        return s
    if op == "replaceText":
        return s + f"    set content of r to {_q(str(step['text']))}\n"
    s += _para_of()
    if op == "paragraphFormat":
        s += "    set pf to paragraph format of rp\n"
        if "alignment" in step:
            s += f"    set alignment of pf to {ALIGN[str(step['alignment'])]}\n"
        if "before" in step:
            s += f"    set space before of pf to {float(step['before'])}\n"
        if "after" in step:
            s += f"    set space after of pf to {float(step['after'])}\n"
        if "lineSpacing" in step:
            mult = float(step["lineSpacing"])
            s += ("    set line spacing rule of pf to line space multiple\n"
                  f"    set line spacing of pf to {12.0 * mult}\n")
        for key, prop in (("left", "paragraph format left indent"), ("right", "paragraph format right indent"),
                          ("firstLine", "first line indent")):
            if key in step:
                s += f"    set {prop} of pf to {float(step[key])}\n"
        if "hanging" in step:
            s += f"    set first line indent of pf to {-float(step['hanging'])}\n"
        return s
    if op == "heading":
        level = int(step["level"])
        style = f"style heading{level}" if level > 0 else "style normal"
        return s + f"    set style of rp to {style}\n"
    if op == "paragraphDelete":
        return s + "    delete rp\n"
    if op == "paragraphInsert":
        # Insert a new paragraph after the target paragraph; it inherits the
        # target's paragraph formatting exactly as pressing Return would.
        return s + (f"    set wsAt to create range d start ((end of content of rp) - 1) end ((end of content of rp) - 1)\n"
                    f"    insert paragraph at wsAt\n"
                    f"    set wsNew to create range d start (end of content of rp) end (end of content of rp)\n"
                    f"    insert text {_q(str(step['text']))} at wsNew\n")
    if op == "list":
        kind = "numbering" if step.get("numbered") else "bullets"
        return s + (f"    apply list format template (list format of rp) list template "
                    f"(list template 1 of list gallery {'numbered gallery' if kind == 'numbering' else 'bullet gallery'} of application)\n")
    if op == "listClear":
        return s + "    remove numbers (list format of rp)\n"
    raise WordError(f"unhandled {op}")


@_retrying
def apply(src: str, steps: List[Dict[str, object]], dst: str, timeout: float = 240) -> None:
    """Apply canonical steps in Word and save dst (docx/doc by extension)."""
    fmt = os.path.splitext(dst)[1].lstrip(".").lower()
    with Job() as job:
        staged = job.stage(src)
        out = job.path("edit", fmt)
        body = "".join(_step(s) for s in steps)
        script = ('tell application "Microsoft Word"\n' + _open_block(staged) + body
                  + _save_as(out, fmt) + _close_block(staged) + "end tell\n")
        script = script.replace("close d saving no",
                                f"close document {_q(os.path.basename(out))} saving no")
        run(script, timeout)
        _wait_for(out)
        shutil.copyfile(out, dst)


def available() -> bool:
    return os.path.isdir("/Applications/Microsoft Word.app")


def open_documents() -> List[str]:
    out = run('tell application "Microsoft Word" to get name of every document', 30)
    return [s.strip() for s in out.split(",") if s.strip()]


def cleanup_stage() -> None:
    """Remove leftover job dirs (e.g. after an interrupted run). Never touches
    a job whose file is still open in Word."""
    if not os.path.isdir(STAGE):
        return
    try:
        open_names = " ".join(open_documents())
    except WordError:
        return
    for name in os.listdir(STAGE):
        if name not in open_names:
            shutil.rmtree(os.path.join(STAGE, name), ignore_errors=True)


if __name__ == "__main__":  # tiny manual probe
    import sys
    if sys.argv[1] == "convert":
        convert(sys.argv[2], sys.argv[3])
    elif sys.argv[1] == "apply":
        apply(sys.argv[2], json.load(open(sys.argv[3])), sys.argv[4])

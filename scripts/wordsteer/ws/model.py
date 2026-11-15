"""Engine-neutral semantic model of a .docx package.

Every engine (Word, quds, crisp-docx, the legacy .doc parser, LibreOffice)
serialises the same document differently: direct formatting vs. styles,
theme fonts vs. literal names, split vs. merged runs, field codes, rsids.
Byte or XML diffs are therefore useless for an audit. This module reduces a
package to what a reader would notice:

* main-story paragraphs in document order (table cells and content controls
  included, text boxes kept apart — this matches Word's ``paragraphs``
  collection and quds' ``WmlDocument.paragraphs``), each with
  - final-view text (insertions kept, deletions and field codes dropped),
  - *effective* character formatting per character, resolved through
    docDefaults -> paragraph style chain -> character style chain -> direct,
  - effective paragraph properties (style name, alignment, spacing, indents,
    numbering, heading level);
* structure: tables, sections/page setup, headers/footers, notes, comments,
  images, text boxes, bookmarks, hyperlinks, fields, revisions, equations.

Only the stdlib is used so the tool runs anywhere Python 3.9+ does.
"""

from __future__ import annotations

import re
import zipfile
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple
from xml.etree import ElementTree as ET

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
A = "http://schemas.openxmlformats.org/drawingml/2006/main"
V = "urn:schemas-microsoft-com:vml"
M = "http://schemas.openxmlformats.org/officeDocument/2006/math"
PR = "http://schemas.openxmlformats.org/package/2006/relationships"


def w(tag: str) -> str:
    return f"{{{W}}}{tag}"


def _wattr(el: Optional[ET.Element], name: str, default=None):
    if el is None:
        return default
    return el.get(w(name), default)


# Character properties tracked by the comparator. Each maps to a function
# returning a normalised, hashable value (None == "not set at this level").
TOGGLES = ("b", "i", "strike", "caps", "smallCaps", "vanish")
CHAR_PROPS = ("b", "i", "u", "strike", "sz", "font", "color", "highlight",
              "vertAlign", "caps", "smallCaps", "vanish")
PARA_PROPS = ("style", "jc", "before", "after", "line", "lineRule", "left",
              "right", "firstLine", "list", "outline")

_ON = {None, "1", "true", "on"}


def _toggle(el: ET.Element) -> bool:
    return _wattr(el, "val") in _ON


def read_rpr(rpr: Optional[ET.Element], theme: Dict[str, str]) -> Dict[str, object]:
    """Direct values set in one <w:rPr>; absent keys are inherited."""
    out: Dict[str, object] = {}
    if rpr is None:
        return out
    for child in rpr:
        tag = child.tag.rsplit("}", 1)[-1]
        if tag in TOGGLES:
            out[tag] = _toggle(child)
        elif tag == "dstrike":
            out["strike"] = _toggle(child) or out.get("strike", False)
        elif tag == "u":
            val = _wattr(child, "val", "single")
            out["u"] = None if val == "none" else val
        elif tag == "sz":
            try:
                out["sz"] = int(_wattr(child, "val"))
            except (TypeError, ValueError):
                pass
        elif tag == "rFonts":
            name = (_wattr(child, "ascii") or _wattr(child, "hAnsi"))
            if name is None:
                th = _wattr(child, "asciiTheme") or _wattr(child, "hAnsiTheme")
                if th:
                    name = theme.get("major" if th.startswith("major") else "minor")
            if name:
                out["font"] = name
        elif tag == "color":
            val = (_wattr(child, "val") or "auto").upper()
            out["color"] = None if val in ("AUTO", "000000") else val
        elif tag == "highlight":
            val = _wattr(child, "val")
            out["highlight"] = None if val in (None, "none") else val
        elif tag == "vertAlign":
            val = _wattr(child, "val")
            out["vertAlign"] = None if val == "baseline" else val
        elif tag == "rStyle":
            out["_rStyle"] = _wattr(child, "val")
    return out


def read_ppr(ppr: Optional[ET.Element]) -> Dict[str, object]:
    out: Dict[str, object] = {}
    if ppr is None:
        return out
    for child in ppr:
        tag = child.tag.rsplit("}", 1)[-1]
        if tag == "jc":
            val = _wattr(child, "val")
            out["jc"] = {"start": "left", "end": "right", "justify": "both",
                         "distribute": "both"}.get(val, val)
        elif tag == "spacing":
            for k in ("before", "after", "line", "lineRule"):
                v = _wattr(child, k)
                if v is not None:
                    out[k] = v if k == "lineRule" else _int(v)
        elif tag == "ind":
            for k, alias in (("left", "start"), ("right", "end")):
                v = _wattr(child, k) or _wattr(child, alias)
                if v is not None:
                    out[k] = _int(v)
            fl, hg = _wattr(child, "firstLine"), _wattr(child, "hanging")
            if hg is not None:
                out["firstLine"] = -_int(hg)
            elif fl is not None:
                out["firstLine"] = _int(fl)
        elif tag == "numPr":
            num = child.find(w("numId"))
            lvl = child.find(w("ilvl"))
            nid = _wattr(num, "val")
            out["list"] = None if nid in (None, "0") else int(_wattr(lvl, "val", "0") or 0)
            out["_numId"] = nid
        elif tag == "outlineLvl":
            out["outline"] = _int(_wattr(child, "val"))
        elif tag == "pStyle":
            out["_pStyle"] = _wattr(child, "val")
    return out


def _int(v) -> Optional[int]:
    try:
        return int(round(float(v)))
    except (TypeError, ValueError):
        return None


@dataclass
class Style:
    sid: str
    name: str
    kind: str
    based_on: Optional[str]
    rpr: Dict[str, object]
    ppr: Dict[str, object]


class Styles:
    def __init__(self, root: Optional[ET.Element], theme: Dict[str, str]):
        self.by_id: Dict[str, Style] = {}
        self.default_r: Dict[str, object] = {"sz": 20}
        self.default_p: Dict[str, object] = {}
        self.default_para_style: Optional[str] = None
        self.default_char_style: Optional[str] = None
        self._cache_r: Dict[str, Dict[str, object]] = {}
        self._cache_p: Dict[str, Dict[str, object]] = {}
        if root is None:  # a package without styles.xml
            return
        dd = root.find(w("docDefaults"))
        if dd is not None:
            rpr = dd.find(f"{w('rPrDefault')}/{w('rPr')}")
            self.default_r.update(read_rpr(rpr, theme))
            self.default_p.update(read_ppr(dd.find(f"{w('pPrDefault')}/{w('pPr')}")))
        for st in root.findall(w("style")):
            sid = _wattr(st, "styleId") or ""
            name_el = st.find(w("name"))
            based = st.find(w("basedOn"))
            s = Style(sid, (_wattr(name_el, "val") or sid).lower(),
                      _wattr(st, "type", "paragraph"), _wattr(based, "val"),
                      read_rpr(st.find(w("rPr")), theme), read_ppr(st.find(w("pPr"))))
            self.by_id[sid] = s
            if _wattr(st, "default") in ("1", "true"):
                if s.kind == "paragraph":
                    self.default_para_style = sid
                elif s.kind == "character":
                    self.default_char_style = sid

    def chain(self, sid: Optional[str]) -> List[Style]:
        out: List[Style] = []
        seen = set()
        while sid and sid in self.by_id and sid not in seen:
            seen.add(sid)
            out.append(self.by_id[sid])
            sid = self.by_id[sid].based_on
        return list(reversed(out))

    def resolved_r(self, sid: Optional[str]) -> Dict[str, object]:
        key = sid or ""
        if key not in self._cache_r:
            acc: Dict[str, object] = {}
            for s in self.chain(sid):
                acc.update({k: v for k, v in s.rpr.items() if not k.startswith("_")})
            self._cache_r[key] = acc
        return self._cache_r[key]

    def resolved_p(self, sid: Optional[str]) -> Dict[str, object]:
        key = sid or ""
        if key not in self._cache_p:
            acc: Dict[str, object] = {}
            for s in self.chain(sid):
                acc.update({k: v for k, v in s.ppr.items() if not k.startswith("_")})
            self._cache_p[key] = acc
        return self._cache_p[key]

    def name(self, sid: Optional[str]) -> str:
        if sid and sid in self.by_id:
            return self.by_id[sid].name
        if sid:
            return sid.lower()
        return "normal"


class Numbering:
    """numId/ilvl -> paragraph properties of that list level (indents mostly).
    Word places list indents here; other writers put them on the paragraph,
    so effective props must layer style -> numbering level -> direct."""

    def __init__(self, root: Optional[ET.Element]):
        self.levels: Dict[Tuple[str, int], Dict[str, object]] = {}
        if root is None:
            return
        abstract: Dict[str, Dict[int, Dict[str, object]]] = {}
        for an in root.findall(w("abstractNum")):
            lv = {}
            for lvl in an.findall(w("lvl")):
                lv[int(_wattr(lvl, "ilvl", "0") or 0)] = read_ppr(lvl.find(w("pPr")))
            abstract[_wattr(an, "abstractNumId")] = lv
        for num in root.findall(w("num")):
            nid = _wattr(num, "numId")
            base = abstract.get(_wattr(num.find(w("abstractNumId")), "val"), {})
            merged = {k: dict(v) for k, v in base.items()}
            for ov in num.findall(w("lvlOverride")):
                lvl = ov.find(w("lvl"))
                if lvl is not None:
                    i = int(_wattr(ov, "ilvl", "0") or 0)
                    merged.setdefault(i, {}).update(read_ppr(lvl.find(w("pPr"))))
            for i, v in merged.items():
                self.levels[(nid, i)] = {k: x for k, x in v.items() if not k.startswith("_")}

    def ppr(self, num_id: Optional[str], ilvl: int) -> Dict[str, object]:
        return self.levels.get((num_id, ilvl), {}) if num_id else {}


@dataclass
class Para:
    text: str
    chars: List[Tuple]          # one CHAR_PROPS tuple per character of text
    props: Dict[str, object]    # effective PARA_PROPS
    in_table: bool = False
    note_refs: int = 0


@dataclass
class Model:
    paragraphs: List[Para] = field(default_factory=list)
    counts: Dict[str, int] = field(default_factory=dict)
    stories: Dict[str, List[str]] = field(default_factory=dict)   # story -> texts
    tables: List[List[List[str]]] = field(default_factory=list)   # rows x cells
    sections: List[Dict[str, object]] = field(default_factory=list)
    errors: List[str] = field(default_factory=list)

    @property
    def text(self) -> str:
        return "\n".join(p.text for p in self.paragraphs)


class _Walker:
    """Collects final-view text and effective formatting from block content."""

    def __init__(self, styles: Styles, theme: Dict[str, str], counts: Dict[str, int],
                 numbering: Optional["Numbering"] = None):
        self.numbering = numbering or Numbering(None)
        self.styles = styles
        self.theme = theme
        self.counts = counts

    # -- paragraph ---------------------------------------------------------
    def paragraph(self, p: ET.Element, in_table: bool) -> Para:
        ppr_el = p.find(w("pPr"))
        direct_p = read_ppr(ppr_el)
        sid = direct_p.get("_pStyle") or self.styles.default_para_style
        props = dict(self.styles.default_p)
        props.update(self.styles.resolved_p(sid))
        num_id = direct_p.get("_numId")
        if num_id is None and "list" not in direct_p:
            for st in reversed(self.styles.chain(sid)):
                if "_numId" in st.ppr:
                    num_id = st.ppr["_numId"]
                    break
        if num_id not in (None, "0"):
            level = direct_p.get("list") if direct_p.get("list") is not None else props.get("list") or 0
            props.update(self.numbering.ppr(num_id, int(level)))
        props.update({k: v for k, v in direct_p.items() if not k.startswith("_")})
        props["style"] = self.styles.name(sid)
        m = re.match(r"^(heading|überschrift|titre|titolo|título) ?(\d)$", props["style"])
        if props.get("outline") is None and m:
            props["outline"] = int(m.group(2)) - 1
        if props.get("outline") == 9:
            props["outline"] = None
        para_r = dict(self.styles.default_r)
        para_r.update(self.styles.resolved_r(sid))
        self._text: List[str] = []
        self._chars: List[Tuple] = []
        self._notes = 0
        self._field_depth: List[bool] = []  # True while inside an instr part
        self._instr: List[str] = []         # instruction text per open field
        self._inline(p, para_r, deleted=False)
        return Para("".join(self._text), self._chars,
                    {k: props.get(k) for k in PARA_PROPS}, in_table, self._notes)

    def _emit(self, s: str, eff: Dict[str, object]):
        if not s:
            return
        # Symbol-font characters live in U+F000–F0FF whether written as
        # w:sym or as w:t in a symbol font; compare them as one form.
        if any("\uf000" <= ch <= "\uf0ff" for ch in s):
            s = "".join(chr(ord(ch) - 0xF000) if "\uf000" <= ch <= "\uf0ff" else ch for ch in s)
        sig = tuple(_norm_char(k, eff.get(k)) for k in CHAR_PROPS)
        self._text.append(s)
        self._chars.extend([sig] * len(s))

    def _in_instr(self) -> bool:
        return bool(self._field_depth) and self._field_depth[-1]

    def _inline(self, el: ET.Element, para_r: Dict[str, object], deleted: bool):
        for child in el:
            tag = child.tag
            if tag == w("r"):
                self._run(child, para_r, deleted)
            elif tag == w("del") or tag == w("moveFrom"):
                self.counts["revisions"] = self.counts.get("revisions", 0) + 1
                self._inline(child, para_r, True)
            elif tag in (w("ins"), w("moveTo")):
                self.counts["revisions"] = self.counts.get("revisions", 0) + 1
                self._inline(child, para_r, deleted)
            elif tag == w("hyperlink"):
                self.counts["hyperlinks"] = self.counts.get("hyperlinks", 0) + 1
                self._inline(child, para_r, deleted)
            elif tag == w("fldSimple"):
                self.counts["fields"] = self.counts.get("fields", 0) + 1
                if (_wattr(child, "instr") or "").strip().upper().startswith("HYPERLINK"):
                    self.counts["hyperlinks"] = self.counts.get("hyperlinks", 0) + 1
                self._inline(child, para_r, deleted)
            elif tag in (w("smartTag"), w("customXml"), w("sdt"), w("sdtContent"),
                         w("dir"), w("bdo")):
                self._inline(child, para_r, deleted)
            elif tag == w("bookmarkStart"):
                name = _wattr(child, "name", "")
                if not name.startswith("_"):
                    self.counts["bookmarks"] = self.counts.get("bookmarks", 0) + 1
            elif tag == f"{{{M}}}oMath" or tag == f"{{{M}}}oMathPara":
                self.counts["equations"] = self.counts.get("equations", 0) + 1

    def _run(self, r: ET.Element, para_r: Dict[str, object], deleted: bool):
        direct = read_rpr(r.find(w("rPr")), self.theme)
        eff = dict(para_r)
        rs = direct.get("_rStyle")
        if rs:
            eff.update(self.styles.resolved_r(rs))
        eff.update({k: v for k, v in direct.items() if not k.startswith("_")})
        for child in r:
            tag = child.tag
            if tag == w("fldChar"):
                kind = _wattr(child, "fldCharType")
                if kind == "begin":
                    self.counts["fields"] = self.counts.get("fields", 0) + 1
                    self._field_depth.append(True)
                    self._instr.append("")
                elif kind in ("separate", "end") and self._field_depth:
                    if self._field_depth[-1] and self._instr:
                        # A HYPERLINK complex field is a working link too.
                        if self._instr[-1].strip().upper().startswith("HYPERLINK"):
                            self.counts["hyperlinks"] = self.counts.get("hyperlinks", 0) + 1
                    if kind == "separate":
                        self._field_depth[-1] = False
                    else:
                        self._field_depth.pop()
                        if self._instr:
                            self._instr.pop()
                continue
            if tag == w("instrText") and self._in_instr() and self._instr:
                self._instr[-1] += child.text or ""
                continue
            if deleted or self._in_instr():
                continue
            if tag == w("t"):
                self._emit(child.text or "", eff)
            elif tag == w("tab") or tag == w("ptab"):
                self._emit("\t", eff)
            elif tag in (w("br"), w("cr")):
                kind = _wattr(child, "type")
                self._emit("\f" if kind == "page" else "\n", eff)
            elif tag == w("noBreakHyphen"):
                self._emit("‑", eff)
            elif tag == w("sym"):
                ch = _wattr(child, "char", "")
                try:
                    code = int(ch, 16)
                    if 0xF000 <= code <= 0xF0FF:
                        code -= 0xF000
                    self._emit(chr(code), eff)
                except ValueError:
                    pass
            elif tag in (w("footnoteReference"), w("endnoteReference")):
                self._notes += 1
            elif tag in (w("drawing"), w("pict"), w("object")):
                self._drawing(child)
            elif tag == f"{{http://schemas.openxmlformats.org/markup-compatibility/2006}}AlternateContent":
                choice = child.find("{http://schemas.openxmlformats.org/markup-compatibility/2006}Choice")
                if choice is not None:
                    for c in choice:
                        if c.tag in (w("drawing"), w("pict")):
                            self._drawing(c)

    def _drawing(self, el: ET.Element):
        blips = len(el.findall(f".//{{{A}}}blip")) + len(el.findall(f".//{{{V}}}imagedata"))
        if blips:
            self.counts["images"] = self.counts.get("images", 0) + blips
        boxes = el.findall(f".//{w('txbxContent')}")
        if boxes:
            self.counts["textboxes"] = self.counts.get("textboxes", 0) + len(boxes)
        if not blips and not boxes:
            self.counts["shapes"] = self.counts.get("shapes", 0) + 1


def _norm_char(k: str, v):
    if k in TOGGLES:
        return bool(v)
    if k == "font" and isinstance(v, str):
        return v.strip().lower()
    return v


def _theme_fonts(z: zipfile.ZipFile, names: List[str]) -> Dict[str, str]:
    out: Dict[str, str] = {}
    theme = next((n for n in names if re.match(r"word/theme/theme\d*\.xml$", n)), None)
    if not theme:
        return out
    try:
        root = ET.fromstring(z.read(theme))
    except ET.ParseError:
        return out
    for kind in ("major", "minor"):
        latin = root.find(f".//{{{A}}}{kind}Font/{{{A}}}latin")
        if latin is not None and latin.get("typeface"):
            out[kind] = latin.get("typeface")
    return out


def _main_part(z: zipfile.ZipFile) -> str:
    try:
        rels = ET.fromstring(z.read("_rels/.rels"))
        for rel in rels.findall(f"{{{PR}}}Relationship"):
            if rel.get("Type", "").endswith("/officeDocument"):
                return rel.get("Target").lstrip("/")
    except (KeyError, ET.ParseError):
        pass
    return "word/document.xml"


def _story_texts(walker: _Walker, root: ET.Element, item_tag: str) -> List[str]:
    texts = []
    items = root.findall(w(item_tag)) if item_tag else [root]
    for item in items:
        if item_tag in ("footnote", "endnote") and _wattr(item, "type") in (
                "separator", "continuationSeparator", "continuationNotice"):
            continue
        paras = [walker.paragraph(p, False).text for p in item.iter(w("p"))]
        texts.append("\n".join(paras))
    return texts


def load(path: str) -> Model:
    """Parse a .docx into a Model. Damaged packages produce errors, not raises."""
    m = Model()
    try:
        z = zipfile.ZipFile(path)
    except (zipfile.BadZipFile, OSError) as e:
        m.errors.append(f"zip: {e}")
        return m
    with z:
        names = z.namelist()
        theme = _theme_fonts(z, names)
        main = _main_part(z)
        base = main.rsplit("/", 1)[0] + "/"

        def read(name: str) -> Optional[ET.Element]:
            if name not in names:
                return None
            try:
                return ET.fromstring(z.read(name))
            except ET.ParseError as e:
                m.errors.append(f"xml {name}: {e}")
                return None

        styles = Styles(read(base + "styles.xml"), theme)
        counts: Dict[str, int] = {}
        numbering = Numbering(read(base + "numbering.xml"))
        walker = _Walker(styles, theme, counts, numbering)
        doc = read(main)
        if doc is None:
            m.errors.append("no main document part")
            return m
        body = doc.find(w("body"))
        if body is None:
            m.errors.append("no body")
            return m
        _blocks(walker, body, m, in_table=False)
        # Sections: every sectPr in body order (paragraph-level ones + final).
        for sp in body.iter(w("sectPr")):
            pg, mar = sp.find(w("pgSz")), sp.find(w("pgMar"))
            cols = sp.find(w("cols"))
            m.sections.append({
                "w": _int(_wattr(pg, "w")), "h": _int(_wattr(pg, "h")),
                "orient": _wattr(pg, "orient", "portrait"),
                **{k: _int(_wattr(mar, k)) for k in ("top", "bottom", "left", "right")},
                "cols": _int(_wattr(cols, "num", "1")) or 1,
            })
        counts["sections"] = len(m.sections)
        counts["paragraphs"] = len(m.paragraphs)
        counts["tables"] = len(m.tables)
        counts["listParagraphs"] = sum(1 for p in m.paragraphs if p.props.get("list") is not None)
        counts["headings"] = sum(1 for p in m.paragraphs if p.props.get("outline") is not None)
        counts["noteRefs"] = sum(p.note_refs for p in m.paragraphs)
        # Other stories. Parsed with their own walker so their fields/images
        # do not inflate the main-story counts.
        side: Dict[str, int] = {}
        sw = _Walker(styles, theme, side, numbering)
        for story, tag in (("footnotes", "footnote"), ("endnotes", "endnote"),
                           ("comments", "comment")):
            root = read(f"{base}{story}.xml")
            if root is not None:
                m.stories[story] = _story_texts(sw, root, tag)
        hf: List[str] = []
        for n in sorted(names):
            if re.match(re.escape(base) + r"(header|footer)\d*\.xml$", n):
                root = read(n)
                if root is not None:
                    hf.append(("H:" if "header" in n else "F:") + "\n".join(_story_texts(sw, root, "")))
        m.stories["headersFooters"] = sorted(set(hf))
        tb = []
        for box in body.iter(w("txbxContent")):
            tb.append("\n".join(sw.paragraph(p, False).text for p in box.iter(w("p"))))
        m.stories["textboxes"] = tb
        for k in ("footnotes", "endnotes", "comments"):
            counts[k] = len(m.stories.get(k, []))
        counts["headerFooterParts"] = len(hf)
        counts["mediaParts"] = sum(1 for n in names if n.startswith(base + "media/"))
        counts["embeddings"] = sum(1 for n in names if n.startswith(base + "embeddings/"))
        m.counts = counts
    return m


def _blocks(walker: _Walker, parent: ET.Element, m: Model, in_table: bool):
    for child in parent:
        tag = child.tag
        if tag == w("p"):
            m.paragraphs.append(walker.paragraph(child, in_table))
        elif tag == w("tbl"):
            rows = []
            for tr in child.findall(w("tr")):
                cells = []
                for tc in tr.findall(w("tc")):
                    before = len(m.paragraphs)
                    _blocks(walker, tc, m, True)
                    cells.append("\n".join(p.text for p in m.paragraphs[before:]))
                rows.append(cells)
            if not in_table:
                m.tables.append(rows)
            else:
                walker.counts["nestedTables"] = walker.counts.get("nestedTables", 0) + 1
        elif tag in (w("sdt"), w("sdtContent"), w("customXml"), w("ins"),
                     w("moveTo"), w("smartTag")):
            _blocks(walker, child, m, in_table)
        elif tag == w("altChunk"):
            walker.counts["altChunks"] = walker.counts.get("altChunks", 0) + 1

"""Offline tests: synthetic packages only (no Word, no corpus documents).

Run:  python3.13 -m unittest discover -s scripts/wordsteer/tests
Engine-binary tests are skipped when ~/.cache/wordsteer/bin is not built.
"""

import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))

from ws import compare as cmp  # noqa: E402
from ws import intent, recipe  # noqa: E402
from ws.engines import crisp, dart, word  # noqa: E402
from ws.model import load  # noqa: E402

W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'

STYLES = f"""<?xml version="1.0"?><w:styles {W}>
<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="H1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/>
<w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>
<w:style w:type="character" w:styleId="Strong"><w:name w:val="Strong"/><w:rPr><w:b/></w:rPr></w:style>
</w:styles>"""

NUMBERING = f"""<?xml version="1.0"?><w:numbering {W}>
<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"""


def docx(body: str, numbering: bool = False) -> str:
    """Write a minimal package and return its path."""
    fd, path = tempfile.mkstemp(suffix=".docx")
    os.close(fd)
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("[Content_Types].xml",
                   '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
                   '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
                   '<Default Extension="xml" ContentType="application/xml"/>'
                   '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
                   '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
                   '</Types>')
        z.writestr("_rels/.rels",
                   '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                   '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
                   '</Relationships>')
        z.writestr("word/document.xml", f'<?xml version="1.0"?><w:document {W}><w:body>{body}'
                   '<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" '
                   'w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>'
                   '</w:body></w:document>')
        z.writestr("word/styles.xml", STYLES)
        if numbering:
            z.writestr("word/numbering.xml", NUMBERING)
    return path


BASE = ('<w:p><w:r><w:t xml:space="preserve">Plain and </w:t></w:r><w:r><w:rPr><w:b/></w:rPr>'
        '<w:t>bold</w:t></w:r><w:r><w:t xml:space="preserve"> words here.</w:t></w:r></w:p>'
        '<w:p><w:pPr><w:pStyle w:val="H1"/></w:pPr><w:r><w:t>A heading</w:t></w:r></w:p>'
        '<w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell one</w:t></w:r></w:p></w:tc>'
        '<w:tc><w:p><w:r><w:t>cell two</w:t></w:r></w:p></w:tc></w:tr></w:tbl>'
        '<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r>'
        '<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>3</w:t></w:r>'
        '<w:r><w:fldChar w:fldCharType="end"/></w:r><w:del><w:r><w:delText>gone</w:delText></w:r></w:del></w:p>')


class ModelTests(unittest.TestCase):
    def test_text_view_and_effective_formatting(self):
        m = load(docx(BASE))
        self.assertEqual([p.text for p in m.paragraphs],
                         ["Plain and bold words here.", "A heading", "cell one", "cell two", "3"])
        p = m.paragraphs[0]
        self.assertFalse(p.chars[0][0])                 # "P" not bold
        self.assertTrue(p.chars[p.text.index("bold")][0])
        self.assertEqual(p.chars[0][4], 22)             # docDefaults size
        h = m.paragraphs[1]
        self.assertEqual(h.props["outline"], 0)
        self.assertTrue(h.chars[0][0])                  # bold via style
        self.assertEqual(m.counts["tables"], 1)
        self.assertEqual(m.counts["fields"], 1)
        self.assertEqual(m.counts["revisions"], 1)

    def test_list_indent_from_numbering_equals_direct_indent(self):
        via_num = load(docx('<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr>'
                            '<w:r><w:t>item</w:t></w:r></w:p>', numbering=True))
        direct = load(docx('<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>'
                           '<w:ind w:left="720" w:hanging="360"/></w:pPr><w:r><w:t>item</w:t></w:r></w:p>',
                           numbering=True))
        s = cmp.summary(cmp.compare(via_num, direct))
        self.assertEqual({k: v for k, v in s.items() if v != 1}, {})


class CompareNegativeControls(unittest.TestCase):
    """The comparator must notice losses; a silently agreeing one is useless."""

    def setUp(self):
        self.ref = load(docx(BASE))

    def test_identity(self):
        s = cmp.summary(cmp.compare(self.ref, self.ref))
        self.assertEqual({k: v for k, v in s.items() if v != 1}, {})

    def test_lost_bold_is_reported(self):
        cand = load(docx(BASE.replace("<w:rPr><w:b/></w:rPr>", "")))
        s = cmp.summary(cmp.compare(self.ref, cand))
        # 4 direct-bold chars lost, 8 style-bold heading chars kept.
        self.assertAlmostEqual(s["keep.b"], 8 / 12, places=3)

    def test_text_change_is_reported(self):
        cand = load(docx(BASE.replace("words here", "terms there")))
        r = cmp.compare(self.ref, cand)
        self.assertLess(r["text"]["wordSim"], 1)
        self.assertTrue(r["examples"]["text"])

    def test_lost_table_is_reported(self):
        cand = load(docx(BASE.split("<w:tbl>")[0] + BASE.split("</w:tbl>")[1]))
        r = cmp.compare(self.ref, cand)
        self.assertEqual(r["counts"]["tables"], [1, 0])

    def test_lost_heading_style_is_reported(self):
        cand = load(docx(BASE.replace('<w:pStyle w:val="H1"/>', "")))
        s = cmp.summary(cmp.compare(self.ref, cand))
        self.assertLess(s["para.outline"], 1)
        self.assertLess(s["para.style"], 1)


class RecipeTests(unittest.TestCase):
    def test_selectors_pick_unique_text(self):
        m = load(docx(BASE))
        steps = recipe.resolve([{"op": "runFormat", "target": {"select": "word"}, "bold": True}], m)
        text = steps[0]["target"]["find"]
        self.assertEqual(m.text.count(text), 1)

    def test_validate_rejects_unknown_fields(self):
        with self.assertRaises(ValueError):
            recipe.validate([{"op": "runFormat", "target": {"find": "x"}, "bolt": True}])
        with self.assertRaises(ValueError):
            recipe.validate([{"op": "teleport", "target": {"find": "x"}}])

    def test_bundled_recipes_validate(self):
        root = os.path.join(os.path.dirname(HERE), "recipes")
        for name in os.listdir(root):
            with open(os.path.join(root, name)) as f:
                data = json.load(f)
            for t in data.get("tasks", [data] if "steps" in data else []):
                recipe.validate(t["steps"])

    def test_intent_oracle_detects_missing_change(self):
        before = load(docx(BASE))
        step = {"op": "runFormat", "target": {"find": "Plain"}, "italic": True}
        self.assertFalse(intent.check(step, before, before)["ok"])


class SpecConsistency(unittest.TestCase):
    """Like ExcelLLMAddin's provider guard: adapters must match ops.json."""

    def test_word_adapter_matches_spec(self):
        sp = recipe.spec()["ops"]
        declared = {op for op, d in sp.items() if "word" in d["engines"]}
        self.assertEqual(declared, set(word.SUPPORTED))

    @unittest.skipUnless(os.path.exists(os.path.join(dart.BIN, "crisp-docx")), "crisp-docx not built")
    def test_crisp_cli_implements_declared_ops(self):
        sp = recipe.spec()["ops"]
        src = docx(BASE)
        for op, d in sp.items():
            step = {"op": op, "target": {"find": "words"}}
            if op == "sectionFormat":
                step = {"op": op, "section": 0, "top": 50}
            elif op in ("replaceText", "paragraphInsert"):
                step["text"] = "x"
            elif op == "heading":
                step["level"] = 1
            elif op == "runFormat":
                step["bold"] = True
            elif op == "paragraphFormat":
                step["alignment"] = "center"
            out = src + f".{op}.docx"
            try:
                crisp.apply(src, [step], out)
                ok = True
            except dart.EngineError:
                ok = False
            self.assertEqual(ok, "crisp" in d["engines"], op)

    @unittest.skipUnless(dart.quds_flavours(), "quds not built")
    def test_quds_declares_every_spec_op(self):
        fl = dart.quds_flavours()[0]
        out = subprocess.run([os.path.join(dart.BIN, f"quds_steps_{fl}"), "operations"],
                             capture_output=True, text=True, check=True).stdout
        quds_ops = set(json.loads(out)["docx"])
        for op, d in recipe.spec()["ops"].items():
            if "quds" in d["engines"]:
                self.assertIn(op, quds_ops)


if __name__ == "__main__":
    unittest.main()

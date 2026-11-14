//! Independent review controls, retained as permanent regressions.
#[cfg(test)]
mod tests {
    use crisp_docx_core::{
        convert_notes_kind, extract_paragraph_runs, extract_paragraph_texts,
        replace_paragraph_runs, replace_paragraph_texts, NotesKind, Package,
    };
    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    const STRICT_W: &str = "http://purl.oclc.org/ooxml/wordprocessingml/main";
    const R: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

    fn package(body: &str) -> Package {
        let mut p = Package::new();
        p.set_part(
            "word/document.xml",
            format!(r#"<w:document xmlns:w="{W}"><w:body>{body}</w:body></w:document>"#)
                .into_bytes(),
        );
        p.set_part("untouched.bin", vec![0, 1, 2, 3]);
        p
    }

    fn xml<'a>(p: &'a Package, part: &str) -> &'a str {
        std::str::from_utf8(p.get_part(part).unwrap()).unwrap()
    }

    fn notes_package(word_ns: &str, rel_ns: &str) -> Package {
        let mut p = Package::new();
        p.set_part("word/document.xml", format!(r#"<w:document xmlns:w="{word_ns}"><w:body><w:p><w:r><w:t>text</w:t><w:footnoteReference w:id="1"/></w:r></w:p></w:body></w:document>"#).into_bytes());
        p.set_part("word/footnotes.xml", format!(r#"<w:footnotes xmlns:w="{word_ns}"><w:footnote w:id="1"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:t>note</w:t></w:r></w:p></w:footnote></w:footnotes>"#).into_bytes());
        p.set_part("word/_rels/document.xml.rels", format!(r#"<Relationships xmlns="{R}"><Relationship Id="note" Type="{rel_ns}/footnotes" Target="footnotes.xml"/><Relationship Id="styles" Type="{rel_ns}/styles" Target="styles.xml"/></Relationships>"#).into_bytes());
        p.set_part("[Content_Types].xml", br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/footnotes.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#.to_vec());
        p.set_part("word/styles.xml", format!(r#"<w:styles xmlns:w="{word_ns}"><w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:rPr><w:b/><w:color w:val="112233"/></w:rPr></w:style></w:styles>"#).into_bytes());
        p
    }

    #[test]
    fn mid_run_note_anchor_cannot_move_on_text_edit() {
        let mut p = package(
            r#"<w:p><w:r><w:t>A</w:t><w:footnoteReference w:id="1"/><w:t>B</w:t></w:r></w:p>"#,
        );
        let before = p.clone();
        assert!(
            replace_paragraph_texts(&mut p, &["changed".into()]).is_err(),
            "{}",
            xml(&p, "word/document.xml")
        );
        assert_eq!(p.as_btree(), before.as_btree());
    }

    #[test]
    fn mid_run_note_anchor_cannot_move_on_run_edit() {
        let mut p = package(
            r#"<w:p><w:r><w:t>A</w:t><w:endnoteReference w:id="1"/><w:t>B</w:t></w:r></w:p>"#,
        );
        let before = p.clone();
        let mut runs = extract_paragraph_runs(&p).unwrap();
        runs[0].runs[0].text = "changed".into();
        assert!(
            replace_paragraph_runs(&mut p, &runs).is_err(),
            "{}",
            xml(&p, "word/document.xml")
        );
        assert_eq!(p.as_btree(), before.as_btree());
    }

    #[test]
    fn mid_paragraph_note_anchor_cannot_move_on_text_edit() {
        let mut p = package(
            r#"<w:p><w:r><w:t>A</w:t><w:footnoteReference w:id="1"/></w:r><w:r><w:t>B</w:t></w:r></w:p>"#,
        );
        let before = p.clone();
        assert!(
            replace_paragraph_texts(&mut p, &["changed".into()]).is_err(),
            "{}",
            xml(&p, "word/document.xml")
        );
        assert_eq!(p.as_btree(), before.as_btree());
    }

    #[test]
    fn strict_notes_convert_coherently_or_refuse_atomically() {
        let mut p = notes_package(
            STRICT_W,
            "http://purl.oclc.org/ooxml/officeDocument/relationships",
        );
        let before = p.clone();
        match convert_notes_kind(&mut p, NotesKind::Endnotes) {
            Err(_) => assert_eq!(p.as_btree(), before.as_btree()),
            Ok(()) => {
                assert!(
                    xml(&p, "word/endnotes.xml").contains("<w:endnotes"),
                    "{}",
                    xml(&p, "word/endnotes.xml")
                );
                assert!(xml(&p, "word/document.xml").contains("<w:endnoteReference"));
                assert!(xml(&p, "word/_rels/document.xml.rels").contains("/endnotes\""));
            }
        }
    }

    #[test]
    fn note_conversion_preserves_style_identity_or_copies_its_definition() {
        let mut p = notes_package(
            W,
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        );
        convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
        let notes = xml(&p, "word/endnotes.xml");
        let styles = xml(&p, "word/styles.xml");
        assert!(
            notes.contains("w:val=\"FootnoteText\"")
                || styles.contains("w:styleId=\"EndnoteText\""),
            "notes={notes}\nstyles={styles}"
        );
    }

    #[test]
    fn foreign_t_names_are_ignored_and_rich_edit_refuses() {
        let mut p = package(
            r#"<w:p><w:r><x:t xmlns:x="urn:foreign">foreign</x:t><w:t>visible</w:t></w:r></w:p>"#,
        );
        assert_eq!(extract_paragraph_texts(&p).unwrap(), ["visible"]);
        let before = p.clone();
        let texts = extract_paragraph_texts(&p).unwrap();
        replace_paragraph_texts(&mut p, &texts).unwrap();
        assert_eq!(p.as_btree(), before.as_btree());
        assert!(replace_paragraph_texts(&mut p, &["changed".into()]).is_err());
        assert_eq!(p.as_btree(), before.as_btree());
    }

    #[test]
    fn scoped_foreign_w_prefix_is_not_word_text() {
        let mut p = package(
            r#"<w:p><w:r><w:t xmlns:w="urn:foreign">foreign</w:t><w:t>visible</w:t></w:r></w:p>"#,
        );
        assert_eq!(extract_paragraph_texts(&p).unwrap(), ["visible"]);
        let before = p.clone();
        let runs = extract_paragraph_runs(&p).unwrap();
        replace_paragraph_runs(&mut p, &runs).unwrap();
        assert_eq!(p.as_btree(), before.as_btree());
    }

    #[test]
    fn default_namespace_entity_cdata_and_empty_paragraph_controls() {
        let mut p = Package::new();
        p.set_part("word/document.xml", format!(r#"<document xmlns="{W}"><body><p/><p><r><t>A&amp;&#x1F600;<![CDATA[<B>]]>&quot;&apos;</t></r></p><p></p></body></document>"#).into_bytes());
        let texts = extract_paragraph_texts(&p).unwrap();
        assert_eq!(texts, ["", "A&😀<B>\"'", ""]);
        let before = p.clone();
        replace_paragraph_texts(&mut p, &texts).unwrap();
        assert_eq!(p.as_btree(), before.as_btree());
        replace_paragraph_texts(
            &mut p,
            &["first".into(), "A<&\"'\tB\nC\rD😀".into(), "last".into()],
        )
        .unwrap();
        assert_eq!(
            extract_paragraph_texts(&p).unwrap(),
            ["first", "A<&\"'\tB\nC\rD😀", "last"]
        );
    }

    #[test]
    fn failed_last_paragraph_refusal_does_not_apply_earlier_edits() {
        let mut p = package(
            r#"<w:p/><w:p><w:r><w:t>simple</w:t></w:r></w:p><w:p><w:hyperlink w:anchor="target"><w:r><w:t>link</w:t></w:r></w:hyperlink></w:p>"#,
        );
        let before = p.clone();
        assert!(replace_paragraph_texts(
            &mut p,
            &["first".into(), "second".into(), "third".into()]
        )
        .is_err());
        assert_eq!(p.as_btree(), before.as_btree());
    }

    #[test]
    fn positioned_references_still_allow_byte_exact_identity() {
        for tail in ["<w:t>B</w:t>", "<w:tab/>", "<w:br/>", "<w:cr/>"] {
            for kind in ["footnote", "endnote"] {
                let mut p = package(&format!(
                    r#"<w:p><w:r><w:t>A</w:t><w:{kind}Reference w:id="1"/>{tail}</w:r></w:p>"#
                ));
                let before = p.clone();
                let texts = extract_paragraph_texts(&p).unwrap();
                let runs = extract_paragraph_runs(&p).unwrap();
                replace_paragraph_texts(&mut p, &texts).unwrap();
                replace_paragraph_runs(&mut p, &runs).unwrap();
                assert_eq!(p.as_btree(), before.as_btree());
                assert!(replace_paragraph_texts(&mut p, &["changed".into()])
                    .unwrap_err()
                    .to_string()
                    .contains("cannot preserve"));
                assert_eq!(p.as_btree(), before.as_btree());
            }
        }
    }

    #[test]
    fn run_edit_preserves_representable_inter_run_reference_boundary() {
        let mut p = package(
            r#"<w:p><w:r><w:t>A</w:t><w:footnoteReference w:id="1"/></w:r><w:r><w:t>B</w:t></w:r></w:p>"#,
        );
        let mut runs = extract_paragraph_runs(&p).unwrap();
        runs[0].runs[0].text = "first".into();
        runs[0].runs[1].text = "second".into();
        replace_paragraph_runs(&mut p, &runs).unwrap();
        assert_eq!(extract_paragraph_runs(&p).unwrap(), runs);
        let output = xml(&p, "word/document.xml");
        assert!(output.find("first").unwrap() < output.find("footnoteReference").unwrap());
        assert!(output.find("footnoteReference").unwrap() < output.find("second").unwrap());
    }

    #[test]
    fn terminal_note_references_allow_text_edits() {
        for kind in ["footnote", "endnote"] {
            let mut p = package(&format!(
                r#"<w:p><w:r><w:t>A</w:t></w:r><w:r><w:t>B</w:t><w:{kind}Reference w:id="1"/></w:r></w:p>"#
            ));
            replace_paragraph_texts(&mut p, &["changed".into()]).unwrap();
            let output = xml(&p, "word/document.xml");
            assert!(
                output.find("changed").unwrap() < output.find(&format!("{kind}Reference")).unwrap()
            );
            assert_eq!(extract_paragraph_texts(&p).unwrap(), ["changed"]);
        }
    }

    #[test]
    fn late_positioned_anchor_rejection_is_atomic() {
        let mut p = package(
            r#"<w:p><w:r><w:t>simple</w:t></w:r></w:p><w:p><w:r><w:footnoteReference w:id="1"/></w:r><w:r><w:t>after</w:t></w:r></w:p>"#,
        );
        let before = p.clone();
        assert!(replace_paragraph_texts(
            &mut p,
            &["changed first".into(), "changed second".into()]
        )
        .is_err());
        assert_eq!(p.as_btree(), before.as_btree());
    }
    #[test]
    fn strict_notes_convert_both_directions_and_preserve_namespace_family() {
        let relation_base = "http://purl.oclc.org/ooxml/officeDocument/relationships";
        let mut p = notes_package(STRICT_W, relation_base);
        let styles = p.get_part("word/styles.xml").unwrap().to_vec();
        convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
        assert!(p.get_part("word/footnotes.xml").is_none());
        assert!(xml(&p, "word/endnotes.xml").contains("<w:endnotes"));
        assert!(xml(&p, "word/endnotes.xml").contains(STRICT_W));
        assert!(xml(&p, "word/document.xml").contains("<w:endnoteReference"));
        assert!(
            xml(&p, "word/_rels/document.xml.rels").contains(&format!("{relation_base}/endnotes"))
        );
        convert_notes_kind(&mut p, NotesKind::Footnotes).unwrap();
        assert!(p.get_part("word/endnotes.xml").is_none());
        assert!(xml(&p, "word/footnotes.xml").contains("<w:footnotes"));
        assert!(
            xml(&p, "word/_rels/document.xml.rels").contains(&format!("{relation_base}/footnotes"))
        );
        assert_eq!(p.get_part("word/styles.xml").unwrap(), styles);
    }

    #[test]
    fn opposite_style_definition_cannot_override_source_formatting() {
        let mut p = notes_package(
            W,
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        );
        let styles = xml(&p, "word/styles.xml").replace("</w:styles>", r#"<w:style w:type="paragraph" w:styleId="EndnoteText"><w:name w:val="unrelated endnote style"/><w:rPr><w:color w:val="ABCDEF"/></w:rPr></w:style></w:styles>"#).into_bytes();
        p.set_part("word/styles.xml", styles.clone());
        convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
        assert!(xml(&p, "word/endnotes.xml").contains(r#"w:val="FootnoteText""#));
        assert_eq!(p.get_part("word/styles.xml").unwrap(), styles);
    }
    #[test]
    fn customized_destination_separator_is_not_disposable() {
        for kind in ["separator", "continuationNotice"] {
            let mut p = notes_package(
                W,
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
            );
            p.set_part("word/endnotes.xml", format!(r#"<w:endnotes xmlns:w="{W}"><w:endnote w:type="{kind}" w:id="0"><w:p><w:r><w:t>Retain this custom separator text</w:t></w:r></w:p></w:endnote></w:endnotes>"#).into_bytes());
            let ct = xml(&p, "[Content_Types].xml").replace("</Types>", r#"<Override PartName="/word/endnotes.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml"/></Types>"#);
            p.set_part("[Content_Types].xml", ct.into_bytes());
            let rels = xml(&p, "word/_rels/document.xml.rels").replace("</Relationships>", r#"<Relationship Id="targetNotes" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes" Target="endnotes.xml"/></Relationships>"#);
            p.set_part("word/_rels/document.xml.rels", rels.into_bytes());
            let before = p.clone();
            match convert_notes_kind(&mut p, NotesKind::Endnotes) {
                Err(_) => assert_eq!(p.as_btree(), before.as_btree()),
                Ok(()) => assert!(
                    xml(&p, "word/endnotes.xml").contains("Retain this custom separator text"),
                    "{kind}: {}",
                    xml(&p, "word/endnotes.xml")
                ),
            }
        }
    }

    #[test]
    fn single_quoted_attributes_keep_embedded_literal_quotes() {
        let mut p = notes_package(
            W,
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        );
        let doc = xml(&p, "word/document.xml").replace("<w:body>", r#"<w:body><w:p><w:ins w:id="2" w:author='Pat "Lee" &amp; Co'><w:r><w:t>tracked</w:t></w:r></w:ins></w:p>"#);
        p.set_part("word/document.xml", doc.into_bytes());
        assert!(extract_paragraph_texts(&p).is_ok());
        let before = p.clone();
        match convert_notes_kind(&mut p, NotesKind::Endnotes) {
            Err(_) => assert_eq!(p.as_btree(), before.as_btree()),
            Ok(()) => assert!(
                extract_paragraph_texts(&p).is_ok(),
                "{}",
                xml(&p, "word/document.xml")
            ),
        }
    }
    #[test]
    fn strict_default_separator_placeholder_still_converts() {
        let relation_base = "http://purl.oclc.org/ooxml/officeDocument/relationships";
        let mut p = notes_package(STRICT_W, relation_base);
        p.set_part("word/endnotes.xml", format!(r#"<w:endnotes xmlns:w="{STRICT_W}"><w:endnote w:type="separator" w:id="-1"><w:p w:rsidR="00000000"><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:endnote></w:endnotes>"#).into_bytes());
        let ct = xml(&p, "[Content_Types].xml").replace("</Types>", r#"<Override PartName="/word/endnotes.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml"/></Types>"#);
        p.set_part("[Content_Types].xml", ct.into_bytes());
        let rels = xml(&p, "word/_rels/document.xml.rels").replace("</Relationships>", &format!(r#"<Relationship Id="targetNotes" Type="{relation_base}/endnotes" Target="endnotes.xml"/></Relationships>"#));
        p.set_part("word/_rels/document.xml.rels", rels.into_bytes());
        convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
        assert!(p.get_part("word/footnotes.xml").is_none());
        assert!(xml(&p, "word/endnotes.xml").contains("<w:t>note</w:t>"));
        assert_eq!(
            xml(&p, "word/_rels/document.xml.rels")
                .matches(&format!("{relation_base}/endnotes"))
                .count(),
            1
        );
        assert!(!xml(&p, "word/_rels/document.xml.rels").contains("targetNotes"));
    }

    #[test]
    fn customized_separator_formatting_and_shapes_are_not_disposable() {
        for body in [
            r#"<w:p><w:pPr><w:spacing w:after="120"/></w:pPr><w:r><w:separator/></w:r></w:p>"#,
            r#"<w:p><w:r><w:rPr><w:color w:val="112233"/></w:rPr><w:separator/></w:r></w:p>"#,
            r#"<w:p><w:r><w:drawing/></w:r></w:p>"#,
            r#"<w:p/><w:p/>"#,
            r#"<w:tbl/>"#,
        ] {
            let mut p = notes_package(
                W,
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
            );
            p.set_part("word/endnotes.xml", format!(r#"<w:endnotes xmlns:w="{W}"><w:endnote w:type="separator" w:id="0">{body}</w:endnote></w:endnotes>"#).into_bytes());
            let before = p.clone();
            assert!(convert_notes_kind(&mut p, NotesKind::Endnotes)
                .unwrap_err()
                .to_string()
                .contains("customized"));
            assert_eq!(p.as_btree(), before.as_btree());
        }
    }

    #[test]
    fn changed_relationship_keeps_decoded_extension_attribute_value() {
        let mut p = notes_package(
            W,
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        );
        let rels = xml(&p, "word/_rels/document.xml.rels").replace(
            r#"Id="note""#,
            r#"Id="note" xmlns:x="urn:metadata" x:label='Pat "Lee" &amp; Co &lt;日本語&gt;'"#,
        );
        p.set_part("word/_rels/document.xml.rels", rels.into_bytes());
        convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
        let mut reader =
            quick_xml::Reader::from_reader(p.get_part("word/_rels/document.xml.rels").unwrap());
        let mut found = false;
        loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Eof => break,
                quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e) => {
                    for attribute in e.attributes() {
                        let attribute = attribute.unwrap();
                        let value = attribute.unescape_value().unwrap();
                        if attribute.key.as_ref() == b"x:label" {
                            assert_eq!(value, "Pat \"Lee\" & Co <日本語>");
                            found = true;
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(found);
    }
}

# Parity fixture corpus

Real .docx files used by the parity harness. None are checked into git —
each entry below is a path on the developer's machine. CI cannot run the
parity harness without these fixtures present; CI runs the unit/integration
suite only.

| Fixture | Path (env var override) | Purpose |
|---|---|---|
| `vielfalt_cs15` | `$CRISP_DOCX_PARITY_VIELFALT` (default: `/Users/christianstrobele/OneDrive/2026 Vielfalt cs15.docx`) | Large real document with 46 footnotes; pandoc-built via the Python pipeline. |
| `blueprint_pandoc` | `$CRISP_DOCX_PARITY_BLUEPRINT` (default: `/tmp/blueprint.docx`) | Minimal pandoc-built docx; used as the blueprint side of transplant tests. |

To regenerate the blueprint deterministically:

```bash
echo '# Blueprint Heading

Body of the blueprint document; not part of the transplanted result.' > /tmp/bp.md
pandoc /tmp/bp.md -o /tmp/blueprint.docx
```

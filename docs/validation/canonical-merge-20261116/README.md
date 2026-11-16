# Local qualification before canonical merge

Qualified source: `99a6b147d75dd1f4e793a1fa87acd13203025d69`.

- `python3 -m unittest discover -s scripts/wordsteer/tests -v`: 15 passed.
- `cargo test -p crisp-docx-core -p crisp-docx-cli`: 143 passed, no failures.
- No live Microsoft Word or private corpus was exercised in this pass.

The isolated harness resolved the unchanged workspace manifests. Its lockfile
is preserved here: four optional sibling package version records reflect the
installed CrispASR/CrispEmbed sources; those optional features were not built.
The repository production lockfile remains unchanged. Tests used Rust 1.100.0.
No GitHub Actions status was queried or awaited.

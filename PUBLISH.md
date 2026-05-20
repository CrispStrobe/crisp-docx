# Publishing checklist

Two release channels:

- **crates.io** — for the OOXML primitives. `cargo install crisp-docx-cli` ↔ the `crisp-docx` binary.
- **PyPI** — for the Python wheel. `pip install crisp-docx` ↔ `import crisp_docx`.

The translator pipeline crates (`crisp-docx-llm`, `crisp-docx-align`, `crisp-translate-cli`) are **not** published to crates.io: they path-dep the sibling [CrispEmbed](https://github.com/CrispStrobe/CrispEmbed) and [CrispASR](https://github.com/CrispStrobe/CrispASR) repos, neither of which is on crates.io yet. Consumers wanting them keep using `cargo install --git`. See [Unlocking the rest](#unlocking-the-rest) below for what to do upstream first.

## crates.io

### Prereqs

1. `cargo login <crates.io-token>` (one-time per machine).
2. Bump the workspace version in `Cargo.toml::[workspace.package].version` if this isn't the first publish of `0.1.0`.

### Steps

```bash
# 1. crisp-docx-core — leaf crate, nothing depends on it on crates.io.
cargo publish -p crisp-docx-core --allow-dirty

# Wait ~30 s for the registry to index it (or check with
# `cargo search crisp-docx-core`).

# 2. crisp-docx-cli — depends on crisp-docx-core, must publish after.
cargo publish -p crisp-docx-cli --allow-dirty
```

Both should be live within a minute each. Verify:

```bash
cargo install crisp-docx-cli
crisp-docx --help
```

### Update README

Once the `cargo install crisp-docx-cli` install path is live, remove the `--git` flag from the Quickstart in `README.md`:

```diff
-cargo install --git https://github.com/CrispStrobe/crisp-docx crisp-docx-cli
+cargo install crisp-docx-cli
```

## PyPI (Python wheel)

Maturin builds platform wheels via the existing CI matrix
(`build-wheel` job in `.github/workflows/ci.yml`). Two paths:

### A. Automated via GitHub Releases (preferred)

1. Tag the repo: `git tag v0.1.0 && git push --tags`.
2. CI builds wheels for Linux × macOS × Windows × Python 3.10 / 3.11 / 3.12 and uploads them as workflow artifacts.
3. Download the `dist/` artifacts and `twine upload dist/*` against PyPI.

(Or extend `.github/workflows/release.yml` to publish to PyPI directly via `maturin publish` with the [PyPI trusted-publisher](https://docs.pypi.org/trusted-publishers/) flow.)

### B. Manual (one platform)

```bash
cd crates/crisp-docx-py
pip install maturin twine
maturin build --release --out dist
twine upload dist/*
```

Note: a single `maturin build` on macOS arm64 only produces a macOS arm64 wheel. Linux users would `pip install` and hit "no matching distribution". For full coverage, use path A.

## Verification matrix (post-publish)

| Channel | Install command | Verify |
|---|---|---|
| crates.io | `cargo install crisp-docx-cli` | `crisp-docx --version` |
| PyPI | `pip install crisp-docx` | `python -c "import crisp_docx; print(crisp_docx.__version__)"` |
| Git (translator) | `cargo install --git https://github.com/CrispStrobe/crisp-docx crisp-translate-cli` (needs sibling `../CrispEmbed`, `../CrispASR`) | `crisp-translate --help` |

## Reverting

`cargo yank` doesn't delete; it marks the version as "do not auto-resolve to this". Use it if a publish slipped out with a bug:

```bash
cargo yank --version 0.1.0 crisp-docx-core
```

Existing `Cargo.lock` files that already pinned to the yanked version keep working. New `cargo install` skips it.

## Unlocking the rest

To get `crisp-docx-llm`, `crisp-docx-align`, and `crisp-translate-cli` on crates.io, their upstream path-deps need to be public first. Both sibling repos have full Cargo.toml metadata (description / keywords / categories / homepage); only the actual `cargo publish` runs remain.

### CrispEmbed (powers `crisp-docx-align`)

Two crates, in order:

```bash
cd ~/code/CrispEmbed
cargo publish -p crispembed-sys    # FFI layer
# wait ~30s for the registry index
cargo publish -p crispembed         # safe wrapper
```

Notes:

- `crispembed-sys` ships a `build.rs` that compiles the C++ ggml runtime. Publishing-time `cargo publish --dry-run` does its own verification build, which can take 10+ minutes for the first run. Use `--no-verify` if you've already built locally and trust it (or set `CRISPEMBED_LIB_DIR` to skip the C++ rebuild during verify — see the build.rs header).
- Both crates list `keywords` / `categories` matching crates.io's allowlist (`text-processing`, `science`, `api-bindings` for the safe wrapper; `external-ffi-bindings` + `text-processing` for `-sys`).

### CrispASR (powers `crisp-docx-llm`'s `nmt` feature)

```bash
cd ~/code/CrispASR
cargo publish -p crispasr-sys
# wait ~30s
cargo publish -p crispasr
```

Same `-sys` caveat as CrispEmbed: compiles a C++ ggml runtime; the verify step is slow. CrispASR's metadata is already complete (license = MIT, full keywords + categories).

### After both upstreams ship

Drop `publish = false` from these three Cargo.toml files in crisp-docx:

```text
crates/crisp-docx-llm/Cargo.toml
crates/crisp-docx-align/Cargo.toml
crates/crisp-translate-cli/Cargo.toml
```

Then publish in dep order: `crisp-docx-llm` → `crisp-docx-align` → `crisp-translate-cli`. Once that lands, `cargo install crisp-translate-cli` gives anyone the full document-translation pipeline.

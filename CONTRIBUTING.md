# Contributing to crisp-docx

Thanks for the interest. This file documents the conventions a new
contributor needs to know — most of which are non-obvious because they
emerge from the workspace topology rather than any single crate.

---

## 1. Workspace topology

Six crates under `crates/`, four of which can ship to crates.io once
their upstream deps are public, and two of which are explicitly
`publish = false`:

```text
                   crisp-docx-core          (publishable)
                          │
            ┌─────────────┴──────────────┬───────────────┐
            │                            │               │
   crisp-docx-cli                crisp-docx-py    crisp-docx-llm
    (publishable)              (publish = false:    (publish = false:
                              cdylib for PyPI)    optional dep on CrispASR)
                                                          │
                                              crisp-docx-align
                                            (publish = false:
                                            optional dep on CrispEmbed)
                                                          │
                                              crisp-translate-cli
                                            (publish = false:
                                            transitive on both)
```

The arrows are workspace deps (`path = "../crisp-docx-core"`). External
optional deps (CrispEmbed for `crisp-docx-align`, CrispASR for
`crisp-docx-llm`) live in **sibling repositories**, not in the workspace.

---

## 2. Path-dep + version pattern

Every workspace dep declaration looks like this:

```toml
crisp-docx-core = { path = "crates/crisp-docx-core", version = "0.1.0" }
```

Both `path` and `version` are required:

- **During local development**, cargo resolves the path. The version is ignored.
- **When `cargo publish` runs**, the path is stripped from the manifest cargo uploads. Downstream consumers see only the version, which cargo resolves from crates.io.

For optional dependencies on sibling repositories (CrispEmbed, CrispASR), the same pattern applies — but the version *must* exist on crates.io or the crate becomes unpublishable. Until those upstream crates land on crates.io, the crates that depend on them must be marked `publish = false` in their `[package]` section.

When adding a new dependency, decide the publish posture up front:

| dep shape | publishable? | example |
|---|---|---|
| crates.io crate | ✅ | `serde = { workspace = true }` |
| workspace sibling, no external paths | ✅ | `crisp-docx-core = { workspace = true }` |
| optional path-dep on a sibling repo on crates.io | ✅ (with explicit version) | (future) `crispembed = { path = "...", version = "0.4", optional = true }` |
| optional path-dep on a sibling repo *not yet* on crates.io | ❌ — mark `publish = false` | current `crispasr` path dep |

---

## 3. Sibling repos referenced by path

Two external repos are pulled in by relative `../../` path:

- `../CrispEmbed/` — ggml-based multilingual encoder. Required if you build with `--features crispembed` (in `crisp-docx-align`) or `--features align` (in `crisp-translate-cli`).
- `../CrispASR/` — ggml-based ASR + NMT engine. Required if you build with `--features nmt` (in `crisp-docx-llm`).

For local development clone them next to crisp-docx:

```
~/code/
├── crisp-docx/
├── CrispEmbed/
└── CrispASR/
```

CI does the same thing in `.github/workflows/ci.yml` via parallel
`actions/checkout@v4` steps with `path: CrispEmbed` and
`path: CrispASR`.

---

## 4. Cargo features matrix

| Crate | Feature | What it pulls in | When to use |
|---|---|---|---|
| `crisp-docx-align` | `crispembed` | CrispEmbed Rust crate (compiles C++ ggml) | Anything that needs SimAlign alignment |
| `crisp-docx-llm` | `nmt` | CrispASR Rust crate | The `Nmt` provider variant (offline m2m100 / wmt21 / madlad) |
| `crisp-translate-cli` | `align` | crisp-docx-align with `crispembed` | The `--preserve-formatting` CLI flag |

`cargo build --workspace` (default features) skips all C++ compilation
and finishes in seconds. The feature flags above add minutes to the
first build because the ggml runtime is compiled in.

---

## 5. Adding a new LLM provider

If the provider speaks OpenAI's Chat Completions wire format:

1. Add a new variant to `ProviderKind` in `crates/crisp-docx-llm/src/providers/mod.rs`.
2. Extend the match in `ProviderConfig::into_provider` to call
   `openai::OpenAiProvider::new(self, "name", "https://default.base.url/v1")`.
3. Extend `crisp-translate-cli`'s `ProviderKindArg` + `auto_pick_providers` + `build_provider_config` with the new env var.
4. Extend `CrispSorter/src-tauri/src/translate/tauri_commands.rs::provider_kind` so the Tauri command accepts it.
5. Extend `CrispSorter/src/lib/components/Translate.svelte`'s `ProviderKind` union + `defaultModelFor` + the `<select>` options.

If the provider uses a different wire format (Anthropic-style, Ollama-style, anything genuinely new), copy the pattern from `providers/anthropic.rs` — separate struct, implement `Provider::translate` directly.

For a non-LLM backend (e.g. another offline NMT engine), see how `nmt.rs` does it: feature-gated `Provider` impl wrapping an external GGML library, with a `map_lang_to_code` helper that converts free-form language names to whatever the model expects.

---

## 6. Running tests

```bash
# Default — no C++, ~145 tests
cargo test --workspace --exclude crisp-docx-py

# With NMT (compiles CrispASR's C++)
DYLD_LIBRARY_PATH=$(pwd)/../CrispASR/build/src \
    cargo test -p crisp-docx-llm --features nmt

# Live LLM tests (provider-specific; gated on env flags)
GROQ_API_KEY=… CRISP_DOCX_LLM_LIVE_GROQ=1 \
    cargo test -p crisp-docx-llm --test live live_groq

# Python wheel
PYO3_PYTHON=$(which python) cargo test -p crisp-docx-py
```

CI runs the default config plus a feature matrix; see `.github/workflows/ci.yml`.

---

## 7. Style

- `#![deny(unsafe_code)]` on every library crate. No exceptions.
- `#[warn(missing_docs)]` on `crisp-docx-core`'s lib.
- `cargo fmt --all` and `cargo clippy --workspace --exclude crisp-docx-py --all-targets -- -D warnings` must pass.
- Error types: each crate has a `thiserror` enum in `error.rs` or `errors.rs`. CLI/Tauri boundaries convert to `anyhow::Result<()>` or `Result<_, String>` respectively.
- No `unwrap()` outside tests.

---

## 8. Adding a crate to the workspace

1. Create `crates/crisp-docx-newthing/` with a `Cargo.toml`.
2. List it in the root `Cargo.toml` `members` array.
3. Use workspace inheritance for metadata:
   ```toml
   [package]
   name        = "crisp-docx-newthing"
   description = "…what it does, one sentence…"
   version.workspace      = true
   edition.workspace      = true
   rust-version.workspace = true
   authors.workspace      = true
   license.workspace      = true
   repository.workspace   = true
   homepage.workspace     = true
   readme.workspace       = true
   keywords    = ["…"]
   categories  = ["…"]
   ```
4. If the crate has any optional dep that path-deps an unpublished sibling repo, add `publish = false` and document why.
5. Add a `README.md` in the crate (cargo publish warns if missing). Pointing at the workspace README with a one-paragraph context note is fine.

---

## 9. Releasing

See `PUBLISH.md` for the cargo / PyPI publish flow.
See `CHANGELOG.md` for the format-of-record (Keep a Changelog).

---

## 10. License

By contributing, you agree your contribution is licensed under
AGPL-3.0-or-later, matching the project's `LICENSE` file.

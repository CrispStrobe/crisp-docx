//! `crisp-translate` — translate every paragraph of a .docx with an LLM,
//! preserving paragraph styles, sections, footnote references, and the
//! blueprint's run-formatting where it can.
//!
//! Architecture:
//!
//!     input.docx
//!       │
//!       ├─ crisp-docx-core::extract_paragraphs       (read XML, grab w:t text)
//!       │
//!       ├─ crisp-docx-llm::LlmTranslator             (translate each text)
//!       │     • OpenAI / Anthropic / Ollama / Groq
//!       │     • fallback chain
//!       │
//!       └─ crisp-docx-core::replace_paragraph_text   (write XML back)
//!     output.docx
//!
//! Current scope: text-only paragraph translation with paragraph-style
//! preservation. Intra-paragraph run formatting (bold/italic span
//! boundaries) is preserved structurally but not realigned to the new
//! word order — that's the next phase, where `crisp-docx-align` slots in
//! as a mapper from source-token positions to target-token positions.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};

use crisp_docx_llm::{LlmTranslator, ProviderConfig, ProviderKind};

#[derive(Parser)]
#[command(name = "crisp-translate", about, version)]
struct Cli {
    /// Input .docx path.
    input: PathBuf,

    /// Output .docx path.
    #[arg(short, long)]
    output: PathBuf,

    /// Source language name (free-form; passed into the LLM prompt).
    #[arg(long, default_value = "English")]
    source_lang: String,

    /// Target language name.
    #[arg(long)]
    target_lang: String,

    /// LLM provider. Multiple `--provider` flags chain as fallbacks
    /// (first one tried first). When omitted, the binary picks the
    /// first provider whose API key env var is set, in the order
    /// openai → anthropic → groq → ollama.
    #[arg(long, value_enum)]
    provider: Vec<ProviderKindArg>,

    /// Model name. Used with the first provider in the chain. Subsequent
    /// providers fall back to their default models.
    #[arg(long)]
    model: Option<String>,

    /// Concurrent translation workers. Defaults to 4.
    #[arg(long, default_value_t = 4)]
    concurrency: usize,

    /// Don't actually call the LLM — just print extracted paragraph
    /// texts to stdout and exit. Useful for dry-running large docs.
    #[arg(long)]
    dry_run: bool,

    /// Suppress all our own status output. Translation result still
    /// goes to disk; only the per-paragraph progress lines (and the
    /// init / chain / summary lines) on stderr are gated.
    ///
    /// Note: this doesn't silence chatter from CrispASR / CrispEmbed
    /// C++ runtimes when those features are on — they print to stderr
    /// directly. If you need a fully quiet run, pipe `2>/dev/null`
    /// at the shell level.
    #[arg(short, long)]
    quiet: bool,

    /// Preserve intra-paragraph run formatting (bold / italic / rStyle)
    /// across the translation by aligning source ↔ target words via a
    /// multilingual encoder. Requires building with `--features align`
    /// and pointing `--align-model` at a multilingual encoder GGUF.
    #[cfg(feature = "align")]
    #[arg(long)]
    preserve_formatting: bool,

    /// Path to a multilingual encoder GGUF (e.g.
    /// paraphrase-multilingual-MiniLM-L12-v2.gguf). Used only when
    /// `--preserve-formatting` is set.
    #[cfg(feature = "align")]
    #[arg(long)]
    align_model: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ProviderKindArg {
    Openai,
    Anthropic,
    Ollama,
    Groq,
    Openrouter,
    Together,
    Cerebras,
    Mistral,
    Nebius,
    Scaleway,
    Poe,
    Google,
    /// Offline NMT via CrispASR (m2m100 / wmt21 / madlad / gemma4-e2b).
    /// `--model <path-to.gguf>` is required; `--align-model` and `--api-key`
    /// are unused. The crate must be built with `--features nmt` for this
    /// to work at runtime; otherwise the runtime returns a config error.
    Nmt,
}

impl From<ProviderKindArg> for ProviderKind {
    fn from(p: ProviderKindArg) -> Self {
        match p {
            ProviderKindArg::Openai => ProviderKind::OpenAi,
            ProviderKindArg::Anthropic => ProviderKind::Anthropic,
            ProviderKindArg::Ollama => ProviderKind::Ollama,
            ProviderKindArg::Groq => ProviderKind::Groq,
            ProviderKindArg::Openrouter => ProviderKind::OpenRouter,
            ProviderKindArg::Together => ProviderKind::Together,
            ProviderKindArg::Cerebras => ProviderKind::Cerebras,
            ProviderKindArg::Mistral => ProviderKind::Mistral,
            ProviderKindArg::Nebius => ProviderKind::Nebius,
            ProviderKindArg::Scaleway => ProviderKind::Scaleway,
            ProviderKindArg::Poe => ProviderKind::Poe,
            ProviderKindArg::Google => ProviderKind::Google,
            ProviderKindArg::Nmt => ProviderKind::Nmt,
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    macro_rules! info {
        ($($arg:tt)*) => {
            if !cli.quiet { eprintln!($($arg)*); }
        };
    }

    // ── Step 1: open the package and extract paragraphs ───────────────
    let mut pkg = crisp_docx_core::open(&cli.input)
        .with_context(|| format!("opening {}", cli.input.display()))?;
    let paragraphs = crisp_docx_core::extract_paragraph_texts(&pkg)
        .context("extracting paragraphs from word/document.xml")?;
    info!(
        "extracted {} paragraph(s) from {}",
        paragraphs.len(),
        cli.input.display()
    );

    if cli.dry_run {
        for (i, t) in paragraphs.iter().enumerate() {
            println!("[{i:03}] {t}");
        }
        return Ok(());
    }

    // ── Step 2: build the LLM translator ──────────────────────────────
    let mut chosen = cli.provider.clone();
    if chosen.is_empty() {
        chosen = auto_pick_providers();
    }
    if chosen.is_empty() {
        anyhow::bail!(
            "no providers — set OPENAI_API_KEY / ANTHROPIC_API_KEY / GROQ_API_KEY \
             or run a local Ollama, or pass --provider ..."
        );
    }

    let mut translator = LlmTranslator::new();
    for (i, p) in chosen.iter().enumerate() {
        let cfg = build_provider_config(*p, if i == 0 { cli.model.clone() } else { None })?;
        translator = translator
            .add_provider(cfg)
            .context("instantiating provider")?;
    }
    info!("translator chain: {:?}", translator.provider_names());

    // ── Step 3: translate ─────────────────────────────────────────────
    let translations = translate_with_concurrency(
        &translator,
        &paragraphs,
        &cli.source_lang,
        &cli.target_lang,
        cli.concurrency,
        cli.quiet,
    )
    .await?;

    let mut succeeded = 0;
    for r in &translations {
        if r.is_ok() {
            succeeded += 1;
        }
    }
    info!(
        "{}/{} paragraph(s) translated successfully",
        succeeded,
        translations.len()
    );

    // ── Step 4: write back ────────────────────────────────────────────
    //
    // Two paths:
    //
    //   - v0.2 path (`--preserve-formatting`): switch to run-level
    //     extract+replace and use the alignment-driven format-transfer
    //     to redistribute the original runs' rPr onto the translated
    //     text. Preserves bold / italic / rStyle spans across the
    //     translation.
    //
    //   - v0.1 path (default): paragraph-level text-only round-trip —
    //     loses intra-paragraph formatting (collapsed to one run) but
    //     keeps pStyle, sections, bookmarks, footnote refs.

    #[cfg(feature = "align")]
    if cli.preserve_formatting {
        write_back_preserving_formatting(
            &mut pkg,
            &cli,
            &paragraphs,
            &translations,
        )
        .context("write-back with format preservation")?;
        crisp_docx_core::save(&pkg, &cli.output)
            .with_context(|| format!("saving to {}", cli.output.display()))?;
        info!("wrote {}", cli.output.display());
        return Ok(());
    }

    let mut new_texts: Vec<String> = Vec::with_capacity(translations.len());
    for (orig, t) in paragraphs.iter().zip(translations.iter()) {
        match t {
            Ok(v) => new_texts.push(v.clone()),
            Err(_) => new_texts.push(orig.clone()),
        }
    }
    crisp_docx_core::replace_paragraph_texts(&mut pkg, &new_texts)
        .context("rewriting paragraph texts")?;
    crisp_docx_core::save(&pkg, &cli.output)
        .with_context(|| format!("saving to {}", cli.output.display()))?;
    info!("wrote {}", cli.output.display());

    Ok(())
}

#[cfg(feature = "align")]
fn write_back_preserving_formatting(
    pkg: &mut crisp_docx_core::Package,
    cli: &Cli,
    src_texts: &[String],
    translations: &[Result<String, crisp_docx_llm::Error>],
) -> Result<()> {
    use crisp_docx_align::{transfer_format_via_words, SourceRun, Strategy};
    use crisp_docx_align::align_texts;
    use crisp_docx_core::{ParagraphInfo, Run as CoreRun};
    use crispembed::CrispEmbed;

    let model_path = cli
        .align_model
        .as_deref()
        .context("--preserve-formatting requires --align-model <path-to-gguf>")?;
    let mut model = CrispEmbed::new(
        model_path.to_str().context("non-UTF-8 align-model path")?,
        4,
    )
    .map_err(anyhow::Error::msg)
    .with_context(|| format!("loading align model {}", model_path.display()))?;

    // Re-extract the runs so we have the source rPr per run.
    let src_paragraphs = crisp_docx_core::extract_paragraph_runs(pkg)
        .context("extracting paragraph runs from word/document.xml")?;
    if src_paragraphs.len() != src_texts.len() {
        anyhow::bail!(
            "paragraph-count mismatch: text-extract found {} but run-extract found {}",
            src_texts.len(),
            src_paragraphs.len()
        );
    }

    let mut new_paragraphs: Vec<ParagraphInfo> = Vec::with_capacity(src_paragraphs.len());
    for (i, info) in src_paragraphs.iter().enumerate() {
        let translation = match translations.get(i) {
            Some(Ok(t)) => t.as_str(),
            _ => {
                // Translation failed — keep the paragraph as-is.
                new_paragraphs.push(info.clone());
                continue;
            }
        };
        let src_text = info.full_text();
        if src_text.trim().is_empty() {
            new_paragraphs.push(info.clone());
            continue;
        }

        // Build SourceRun<Option<Vec<u8>>> from the OOXML runs. We treat
        // each run's `rpr_xml` as the opaque format identifier.
        let source_runs: Vec<SourceRun<Option<Vec<u8>>>> = info
            .runs
            .iter()
            .map(|r| SourceRun {
                text: r.text.clone(),
                format_id: r.rpr_xml.clone(),
            })
            .collect();

        let alignment = align_texts(&mut model, &src_text, translation, Strategy::Itermax {
            min_sim: 0.3,
        })
        .with_context(|| format!("aligning paragraph {i}"))?;
        let target_runs = transfer_format_via_words(
            &source_runs,
            translation,
            &alignment.word_edges,
            None,
        );

        // Convert TargetRun<Option<Vec<u8>>> into core Run + carry
        // footnote refs across by appending all of the source paragraph's
        // refs to the FINAL target run (a coarse but deterministic
        // placement; finer-grained anchor migration is a future
        // improvement once we surface character offsets out of the
        // aligner).
        let mut footnote_refs_all: Vec<Vec<u8>> = info
            .runs
            .iter()
            .flat_map(|r| r.footnote_refs.clone())
            .collect();

        let mut runs: Vec<CoreRun> = target_runs
            .into_iter()
            .map(|tr| CoreRun {
                text: tr.text,
                rpr_xml: tr.format_id,
                footnote_refs: Vec::new(),
            })
            .collect();
        if !footnote_refs_all.is_empty() {
            if let Some(last) = runs.last_mut() {
                last.footnote_refs.append(&mut footnote_refs_all);
            } else {
                runs.push(CoreRun {
                    text: String::new(),
                    rpr_xml: None,
                    footnote_refs: footnote_refs_all,
                });
            }
        }

        new_paragraphs.push(ParagraphInfo {
            ppr_xml: info.ppr_xml.clone(),
            runs,
            leading_bookmark_starts: info.leading_bookmark_starts.clone(),
            trailing_bookmark_ends: info.trailing_bookmark_ends.clone(),
        });
    }

    crisp_docx_core::replace_paragraph_runs(pkg, &new_paragraphs)
        .context("rewriting paragraph runs")?;
    Ok(())
}

async fn translate_with_concurrency(
    translator: &LlmTranslator,
    texts: &[String],
    src: &str,
    tgt: &str,
    concurrency: usize,
    quiet: bool,
) -> Result<Vec<Result<String, crisp_docx_llm::Error>>> {
    use futures::stream::{self, StreamExt};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    // `buffered(N)` preserves input order — head-of-line blocking is
    // fine here because all paragraphs take similar time. Using
    // `buffer_unordered` here is a **bug**: the output vec ends up in
    // completion order rather than paragraph order, and the caller
    // zips it against the original paragraphs by index, scrambling
    // the document.
    let done = AtomicUsize::new(0);
    let total = texts.len();
    let started = Instant::now();
    let outs: Vec<_> = stream::iter(texts.iter())
        .map(|t| {
            let done = &done;
            async move {
                let r = translator.translate_text(t, src, tgt).await;
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if !quiet && (n % 5 == 0 || n == total) {
                    let elapsed = started.elapsed().as_secs_f64().max(1e-3);
                    let rate_per_s = n as f64 / elapsed;
                    let rate_per_min = rate_per_s * 60.0;
                    let remaining = total.saturating_sub(n);
                    let eta = if rate_per_s > 1e-3 {
                        (remaining as f64 / rate_per_s) as u64
                    } else {
                        0
                    };
                    eprintln!(
                        "  …{n}/{total}  {rate_per_min:.1} par/min  ETA {eta}s",
                    );
                }
                r
            }
        })
        .buffered(concurrency.max(1))
        .collect()
        .await;
    Ok(outs)
}

fn auto_pick_providers() -> Vec<ProviderKindArg> {
    // Preference order: a) cheapest / fastest (Groq, Cerebras) first
    // because they're rate-limited free tiers good for batch work;
    // b) cloud frontier (OpenAI, Anthropic) next; c) Together /
    // OpenRouter / Nebius / Scaleway / Mistral / Poe last. Ollama is
    // local and intentionally not auto-picked.
    let mut out = Vec::new();
    let candidates = [
        (ProviderKindArg::Groq, "GROQ_API_KEY"),
        (ProviderKindArg::Cerebras, "CEREBRAS_API_KEY"),
        (ProviderKindArg::Openai, "OPENAI_API_KEY"),
        (ProviderKindArg::Anthropic, "ANTHROPIC_API_KEY"),
        (ProviderKindArg::Together, "TOGETHER_API_KEY"),
        (ProviderKindArg::Openrouter, "OPENROUTER_API_KEY"),
        (ProviderKindArg::Nebius, "NEBIUS_API_KEY"),
        (ProviderKindArg::Scaleway, "SCALEWAY_API_KEY"),
        (ProviderKindArg::Mistral, "MISTRAL_API_KEY"),
        (ProviderKindArg::Poe, "POE_API_KEY"),
        (ProviderKindArg::Google, "GOOGLEAI_API_KEY"),
    ];
    for (kind, var) in candidates {
        if std::env::var(var).is_ok() {
            out.push(kind);
        }
    }
    out
}

fn build_provider_config(kind: ProviderKindArg, model: Option<String>) -> Result<ProviderConfig> {
    let (api_key_env, default_model): (Option<&str>, &str) = match kind {
        ProviderKindArg::Openai => (Some("OPENAI_API_KEY"), "gpt-4o-mini"),
        ProviderKindArg::Anthropic => (Some("ANTHROPIC_API_KEY"), "claude-3-5-sonnet-20241022"),
        ProviderKindArg::Groq => (Some("GROQ_API_KEY"), "llama-3.3-70b-versatile"),
        ProviderKindArg::Ollama => (None, "llama3.2"),
        ProviderKindArg::Openrouter => (
            Some("OPENROUTER_API_KEY"),
            "meta-llama/llama-3.3-70b-instruct",
        ),
        ProviderKindArg::Together => (
            Some("TOGETHER_API_KEY"),
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        ),
        ProviderKindArg::Cerebras => (Some("CEREBRAS_API_KEY"), "llama-3.3-70b"),
        ProviderKindArg::Mistral => (Some("MISTRAL_API_KEY"), "mistral-large-latest"),
        ProviderKindArg::Nebius => (
            Some("NEBIUS_API_KEY"),
            "meta-llama/Llama-3.3-70B-Instruct",
        ),
        ProviderKindArg::Scaleway => (Some("SCALEWAY_API_KEY"), "llama-3.3-70b-instruct"),
        ProviderKindArg::Poe => (Some("POE_API_KEY"), "GPT-4o-mini"),
        ProviderKindArg::Google => (Some("GOOGLEAI_API_KEY"), "gemini-2.0-flash"),
        // NMT loads a GGUF file from disk — no env-driven API key,
        // and the `model` arg carries the path verbatim. The caller
        // MUST pass `--model /path/to/m2m100-…gguf` for this to work.
        ProviderKindArg::Nmt => (None, ""),
    };
    let api_key = match api_key_env {
        Some(var) => Some(std::env::var(var).map_err(|_| {
            anyhow::anyhow!(
                "{} provider: env var {} not set",
                ProviderKind::from(kind).name(),
                var
            )
        })?),
        None => None,
    };
    Ok(ProviderConfig {
        kind: kind.into(),
        api_key,
        model: model.unwrap_or_else(|| default_model.to_string()),
        base_url: None,
    })
}

// Re-export the provider name helper for error messages.
trait ProviderKindName {
    fn name(&self) -> &'static str;
}

impl ProviderKindName for ProviderKind {
    fn name(&self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Ollama => "ollama",
            ProviderKind::Groq => "groq",
            ProviderKind::OpenRouter => "openrouter",
            ProviderKind::Together => "together",
            ProviderKind::Cerebras => "cerebras",
            ProviderKind::Mistral => "mistral",
            ProviderKind::Nebius => "nebius",
            ProviderKind::Scaleway => "scaleway",
            ProviderKind::Poe => "poe",
            ProviderKind::Google => "google",
            ProviderKind::Nmt => "nmt",
        }
    }
}

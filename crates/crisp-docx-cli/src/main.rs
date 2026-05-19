//! `crisp-docx` — cross-platform CLI for OOXML (.docx) surgery.
//!
//! Subcommands:
//!
//! - `clean` — strip rsid/paraId tracking attrs (Word "unreadable content"
//!   cure); optional textutil tag normalization.
//! - `notes-kind` — convert footnotes to endnotes or back.
//! - `inspect` — human-readable summary of a package.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use crisp_docx_core::{convert_notes_kind, normalize_tags, open, save, strip_rsids, NotesKind};

#[derive(Parser)]
#[command(name = "crisp-docx", about, version, long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Strip rsid/paraId tracking attrs from a docx.
    ///
    /// The most common cure for Word's "found unreadable content" recovery
    /// dialog. Tracking attrs reference revision sessions registered in
    /// settings.xml's <w:rsids>; when a body fragment lands in another
    /// document those refs go dangling and Word balks.
    Clean(CleanArgs),

    /// Convert a docx between footnotes and endnotes.
    NotesKind(NotesKindArgs),

    /// Print a human-readable summary of the package's parts.
    Inspect(InspectArgs),
}

#[derive(clap::Args)]
struct CleanArgs {
    /// Path to the input .docx file.
    input: PathBuf,
    /// Output path. Defaults to editing the input in place.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Also rewrite Apple textutil's non-standard tags (w:sz-cs -> w:szCs).
    #[arg(long)]
    also_normalize_tags: bool,
    /// Don't write the result; just report what would change.
    #[arg(long)]
    dry_run: bool,
}

#[derive(clap::Args)]
struct NotesKindArgs {
    /// Path to the input .docx file.
    input: PathBuf,
    /// Output path. Defaults to editing the input in place.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Convert to footnotes or endnotes.
    #[arg(long, value_enum)]
    to: NotesKindCli,
}

#[derive(clap::Args)]
struct InspectArgs {
    /// Path to the input .docx file.
    input: PathBuf,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum NotesKindCli {
    Footnotes,
    Endnotes,
}

impl From<NotesKindCli> for NotesKind {
    fn from(k: NotesKindCli) -> Self {
        match k {
            NotesKindCli::Footnotes => NotesKind::Footnotes,
            NotesKindCli::Endnotes => NotesKind::Endnotes,
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Clean(args) => cmd_clean(args),
        Cmd::NotesKind(args) => cmd_notes_kind(args),
        Cmd::Inspect(args) => cmd_inspect(args),
    }
}

fn cmd_clean(args: CleanArgs) -> Result<()> {
    let mut pkg = open(&args.input).with_context(|| format!("opening {}", args.input.display()))?;
    let rsid_removed = strip_rsids(&mut pkg)?;
    let renamed = if args.also_normalize_tags {
        normalize_tags(&mut pkg)?
    } else {
        0
    };
    if args.dry_run {
        println!(
            "would strip {rsid_removed} rsid/paraId attrs{}; not writing (--dry-run)",
            if args.also_normalize_tags {
                format!(", normalize {renamed} non-standard tags")
            } else {
                String::new()
            },
        );
        return Ok(());
    }
    let out = args.output.as_deref().unwrap_or(args.input.as_path());
    save(&pkg, out)?;
    println!(
        "stripped {rsid_removed} rsid/paraId attrs{} -> {}",
        if args.also_normalize_tags {
            format!(", normalized {renamed} non-standard tags")
        } else {
            String::new()
        },
        out.display(),
    );
    Ok(())
}

fn cmd_notes_kind(args: NotesKindArgs) -> Result<()> {
    let mut pkg = open(&args.input).with_context(|| format!("opening {}", args.input.display()))?;
    convert_notes_kind(&mut pkg, args.to.into())?;
    let out = args.output.as_deref().unwrap_or(args.input.as_path());
    save(&pkg, out)?;
    println!("converted notes to {:?} -> {}", args.to, out.display());
    Ok(())
}

fn cmd_inspect(args: InspectArgs) -> Result<()> {
    let pkg = open(&args.input).with_context(|| format!("opening {}", args.input.display()))?;
    println!("{} -- {} parts", args.input.display(), pkg.parts().count());
    for (name, data) in pkg.parts() {
        println!("  {name:55}  {} bytes", data.len());
    }
    Ok(())
}

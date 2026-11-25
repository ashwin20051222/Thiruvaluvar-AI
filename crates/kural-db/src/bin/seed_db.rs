use anyhow::{Context, Result};
use clap::Parser;
use kural_core::load_and_audit_canonical;
use kural_db::{compile_database, load_enriched_dir, load_knowledge_base};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "seed_db",
    about = "Compile canonical Layer 1 + enriched Layer 2 + knowledge_base into data/thirukkural.db"
)]
struct Cli {
    /// Directory containing verified Layer 1 canonical JSON files
    #[arg(long, default_value = "seed_data/canonical")]
    canonical_dir: PathBuf,

    /// Directory containing Layer 2 adhikaram_XXX.json checkpoint files
    #[arg(long, default_value = "seed_data/enriched")]
    enriched_dir: PathBuf,

    /// Path to bilingual knowledge_base.json
    #[arg(long, default_value = "seed_data/knowledge_base.json")]
    knowledge: PathBuf,

    /// Output SQLite database file path
    #[arg(long, default_value = "data/thirukkural.db")]
    output: PathBuf,

    /// Require all 133 enriched Adhikaram files (default strict production mode)
    #[arg(long, conflicts_with = "partial_ok")]
    strict: bool,

    /// Allow partial Layer 2 enrichment during development (default is strict 1,330/1,330 production mode)
    #[arg(long)]
    partial_ok: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let strict = cli.strict || !cli.partial_ok;

    println!(
        "Loading and auditing Layer 1 canonical data from {}...",
        cli.canonical_dir.display()
    );
    let canonical = load_and_audit_canonical(&cli.canonical_dir)
        .context("failed loading/auditing canonical dataset")?;

    println!(
        "Loading Layer 2 enriched chapters from {}...",
        cli.enriched_dir.display()
    );
    let enriched = load_enriched_dir(&cli.enriched_dir, &canonical)
        .context("failed loading/validating enriched chapters")?;

    println!("Loading knowledge base from {}...", cli.knowledge.display());
    let knowledge =
        load_knowledge_base(&cli.knowledge).context("failed loading knowledge_base.json")?;

    println!(
        "Compiling {} (mode: {})...",
        cli.output.display(),
        if strict {
            "STRICT 1330/1330"
        } else {
            "PARTIAL-OK DEV"
        }
    );
    let stats = compile_database(&canonical, &enriched, &knowledge, &cli.output, strict)
        .context("failed compiling SQLite database")?;

    let size_mb = stats.file_size_bytes as f64 / (1024.0 * 1024.0);
    println!("Database compiled successfully:");
    println!("  Path:            {}", cli.output.display());
    println!("  Build mode:      {}", stats.build_mode);
    println!("  Integrity check: {}", stats.integrity_check);
    println!(
        "  File size:       {:.2} MB ({} bytes)",
        size_mb, stats.file_size_bytes
    );
    println!(
        "  Counts:          divisions={}, iyals={}, adhikarams={}, kurals={}, child={}, youth={}, adult={}, tags={}, kural_tags={}, kb={}",
        stats.divisions,
        stats.iyals,
        stats.adhikarams,
        stats.kurals,
        stats.kural_child,
        stats.kural_youth,
        stats.kural_adult,
        stats.tags,
        stats.kural_tags,
        stats.knowledge_base
    );

    Ok(())
}

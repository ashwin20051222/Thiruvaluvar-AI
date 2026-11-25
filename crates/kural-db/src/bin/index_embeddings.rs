use anyhow::{Context, Result, bail};
use clap::Parser;
use kural_db::{Model2VecEmbedder, index_database_embeddings, resolve_db_path};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "index_embeddings",
    about = "Index 1,330 Kurals and 45 Knowledge Base entries using local model2vec INT8 weights"
)]
struct Cli {
    /// Path to compiled SQLite database (defaults via resolve_db_path)
    #[arg(long)]
    db: Option<PathBuf>,

    /// Path to local `potion-multilingual-128M-int8` directory
    #[arg(long, default_value = "models/embeddings")]
    models_dir: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = resolve_db_path(cli.db.as_deref())?;

    if !cli.models_dir.join("model.safetensors").is_file() {
        bail!(
            "Offline embeddings model missing at {}. Run scripts/download_fast_models.sh (Phase 4) first.",
            cli.models_dir.display()
        );
    }

    let load_start = Instant::now();
    let (embedder, rss_delta_mib) = Model2VecEmbedder::load_with_rss_check(&cli.models_dir)
        .context("failed loading local model2vec weights")?;
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;

    if let Some(mib) = rss_delta_mib {
        println!("Loaded Model2VecEmbedder in {load_ms:.1} ms (RSS delta: {mib} MiB)");
        if mib > 96 {
            eprintln!(
                "WARNING: model2vec-rs RSS delta ({mib} MiB) exceeded 96 MiB Pi compact threshold."
            );
        }
    } else {
        println!("Loaded Model2VecEmbedder in {load_ms:.1} ms");
    }

    let idx_start = Instant::now();
    let (k_count, kb_count) = index_database_embeddings(&db_path, &embedder, "model2vec")
        .with_context(|| format!("failed indexing embeddings into {}", db_path.display()))?;
    let idx_ms = idx_start.elapsed().as_secs_f64() * 1000.0;

    println!(
        "Indexed {k_count} Kurals + {kb_count} Knowledge Base entries into {} in {idx_ms:.1} ms",
        db_path.display()
    );
    Ok(())
}

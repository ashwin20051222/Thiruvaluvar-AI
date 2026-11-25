use anyhow::{Context, Result, bail};
use clap::Parser;
use kural_core::{
    CanonicalDataset, EnrichedChapterFile, KuralCore, load_and_audit_canonical,
    normalize_enriched_chapter, validate_enriched_chapter,
};
use kural_enricher::synthesize_chapter;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "kural-enricher",
    about = "100% offline pure-Rust synthesizer generating all 1,330 Child, Youth, Adult, and Tag records"
)]
struct Cli {
    /// Directory containing verified Layer 1 canonical JSON files
    #[arg(long, default_value = "seed_data/canonical")]
    canonical_dir: PathBuf,

    /// Directory for checkpointed adhikaram_XXX.json files
    #[arg(long, default_value = "seed_data/enriched")]
    output_dir: PathBuf,

    /// Synthesize only sample chapters #1, #49, and #109 and print a readable preview
    #[arg(long)]
    sample_first: bool,

    /// Optional comma-separated chapter IDs (e.g. "1,49,109")
    #[arg(long)]
    chapters: Option<String>,

    /// Validate existing checkpoint JSON files offline without rewriting them
    #[arg(long)]
    verify_only: bool,
}

fn parse_target_chapters(sample_first: bool, chapters_arg: Option<&str>) -> Result<Vec<u8>> {
    if sample_first {
        return Ok(vec![1, 49, 109]);
    }
    if let Some(raw) = chapters_arg {
        let mut out = Vec::new();
        for part in raw.split(',') {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            let id: u8 = trimmed
                .parse()
                .with_context(|| format!("invalid chapter id '{trimmed}'"))?;
            if !(1..=133).contains(&id) {
                bail!("chapter id {id} out of range 1..=133");
            }
            if !out.contains(&id) {
                out.push(id);
            }
        }
        if out.is_empty() {
            bail!("--chapters yielded an empty chapter list");
        }
        return Ok(out);
    }
    Ok((1u8..=133u8).collect())
}

fn checkpoint_path(output_dir: &Path, adhikaram_id: u8) -> PathBuf {
    output_dir.join(format!("adhikaram_{adhikaram_id:03}.json"))
}

fn verify_chapters_offline(
    canonical: &CanonicalDataset,
    output_dir: &Path,
    targets: &[u8],
) -> Result<()> {
    let mut by_chapter: BTreeMap<u8, Vec<&KuralCore>> = BTreeMap::new();
    for k in &canonical.kurals {
        by_chapter.entry(k.adhikaram_id).or_default().push(k);
    }

    let mut passed = 0usize;
    let mut failures = Vec::new();

    for &id in targets {
        let path = checkpoint_path(output_dir, id);
        if !path.exists() {
            failures.push(format!("adhikaram #{id}: missing {}", path.display()));
            continue;
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed reading {}", path.display()))?;
        let mut parsed: EnrichedChapterFile = match serde_json::from_str(&raw) {
            Ok(p) => p,
            Err(err) => {
                failures.push(format!("adhikaram #{id}: JSON parse error: {err}"));
                continue;
            }
        };
        normalize_enriched_chapter(&mut parsed);
        let refs = by_chapter.get(&id).map(Vec::as_slice).unwrap_or(&[]);
        match validate_enriched_chapter(&parsed, refs) {
            Ok(()) => passed += 1,
            Err(errs) => failures.push(format!("adhikaram #{id}: {}", errs.join("; "))),
        }
    }

    if failures.is_empty() {
        println!(
            "Offline verification PASSED for all {passed}/{} target adhikarams.",
            targets.len()
        );
        Ok(())
    } else {
        for f in &failures {
            eprintln!("FAIL: {f}");
        }
        bail!(
            "offline verification failed ({} ok, {} failed)",
            passed,
            failures.len()
        );
    }
}

fn print_sample_preview(chapter: &EnrichedChapterFile, canonical: &CanonicalDataset) {
    let first_rec = &chapter.kurals[0];
    let first_canon = canonical
        .kurals
        .iter()
        .find(|k| k.number == first_rec.kural_number)
        .expect("canonical kural exists");
    println!("\n======================================================================");
    println!(
        "SAMPLE PREVIEW — Adhikaram #{:03} | Kural #{}",
        chapter.adhikaram_id, first_canon.number
    );
    println!("  Tamil:   {} / {}", first_canon.line1_ta, first_canon.line2_ta);
    println!("  English: {}", first_canon.translation_en);
    println!("  Tags:    {:?}", first_rec.tags);
    println!("  [Child] {}", first_rec.child.simple_meaning_ta);
    println!("  [Child EN] {}", first_rec.child.simple_meaning_en);
    println!(
        "  [Kutty Story] {} — {}",
        first_rec.child.story_title_ta, first_rec.child.kutty_story_ta
    );
    println!(
        "  [Word Game] Blank='{}' | Options TA={:?}",
        first_rec.child.game_blank_word_ta, first_rec.child.game_options_ta
    );
    println!("  [Youth TA] {}", first_rec.youth.analysis_ta);
    println!("  [Youth EN] {}", first_rec.youth.analysis_en);
    println!(
        "  [Adult ({})] {}",
        first_rec.adult.research_theme, first_rec.adult.reflection_en
    );
}

fn main() -> Result<()> {
    let started = Instant::now();
    let cli = Cli::parse();
    let targets = parse_target_chapters(cli.sample_first, cli.chapters.as_deref())?;

    let canonical = load_and_audit_canonical(&cli.canonical_dir)
        .context("failed loading/auditing canonical dataset")?;

    if cli.verify_only {
        return verify_chapters_offline(&canonical, &cli.output_dir, &targets);
    }

    fs::create_dir_all(&cli.output_dir)
        .with_context(|| format!("failed creating {}", cli.output_dir.display()))?;

    let mut total_kurals = 0usize;
    let mut total_tags = 0usize;

    for &id in &targets {
        let chapter = synthesize_chapter(&canonical, id)?;
        total_kurals += chapter.kurals.len();
        total_tags += chapter.kurals.iter().map(|r| r.tags.len()).sum::<usize>();

        let dest = checkpoint_path(&cli.output_dir, id);
        let tmp = dest.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(&chapter)?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &dest)?;

        if cli.sample_first {
            print_sample_preview(&chapter, &canonical);
        }
    }

    println!(
        "Synthesized and validated {}/{} Adhikarams ({total_kurals} Kurals, {total_tags} tags) in {:.2}ms -> {}",
        targets.len(),
        targets.len(),
        started.elapsed().as_secs_f64() * 1000.0,
        cli.output_dir.display()
    );
    Ok(())
}

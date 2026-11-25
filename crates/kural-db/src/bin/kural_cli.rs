use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use kural_core::AgeGroup;
use kural_db::{KuralDb, resolve_db_path};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "kural_cli",
    about = "Terminal inspection and verification CLI for data/thirukkural.db"
)]
struct Cli {
    /// Path to compiled SQLite database (default: $KURAL_DB_PATH, then ./data/thirukkural.db,
    /// ../data/thirukkural.db, ../../data/thirukkural.db, or the workspace root)
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show complete Kural details (Layer 1 + Layer 2)
    Get { number: u16 },
    /// Show Child Mode (6-12) simple meaning, Kutty Story, moral, and word game
    Story { number: u16 },
    /// Show Youth Mode (13-17) modern analysis, action tip, and quiz
    Youth { number: u16 },
    /// Run hybrid 4-tier search across all Kurals
    Search {
        query: String,
        #[arg(long)]
        division: Option<u8>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Search curated Thiruvalluvar biography & FAQ knowledge base
    Knowledge {
        query: String,
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    /// Pick deterministic daily Kural for an age group (kids | youth | adult)
    Daily {
        age: String,
        #[arg(long, default_value_t = 1)]
        day: u32,
    },
    /// Show database row counts, build mode, file size, and integrity check
    Stats,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = resolve_db_path(cli.db.as_deref())?;
    let db = KuralDb::open(&db_path)
        .with_context(|| format!("failed opening database {}", db_path.display()))?;

    match cli.command {
        Commands::Get { number } => {
            let full = db.get_kural_full(number)?;
            println!(
                "=== குறள் {} | {} ({}) • {} ({}) ===",
                full.kural.number,
                full.division.name_ta,
                full.division.name_en,
                full.adhikaram.name_ta,
                full.adhikaram.name_en
            );
            println!("Opening Letter: {}", full.kural.opening_letter_ta);
            println!("  {}", full.kural.line1_ta);
            println!("  {}", full.kural.line2_ta);
            println!("Transliteration:");
            println!("  {}", full.kural.transliteration1);
            println!("  {}", full.kural.transliteration2);
            println!("\nEnglish Translation: {}", full.kural.translation_en);
            println!("English Explanation: {}", full.kural.explanation_en);
            println!("\nமு.வ. உரை: {}", full.kural.meaning_mu_va);
            println!("சாலமன் பாப்பையா: {}", full.kural.meaning_solomon);
            println!("கலைஞர் உரை: {}", full.kural.meaning_kalaignar);
            if let Some(parimel) = &full.kural.meaning_parimel {
                println!("பரிமேலழகர்: {parimel}");
            }
            if !full.tags.is_empty() {
                let tag_list: Vec<String> = full
                    .tags
                    .iter()
                    .map(|t| format!("{} ({})", t.slug, t.name_ta))
                    .collect();
                println!("\nTags: {}", tag_list.join(", "));
            }
            if let Some(child) = &full.child {
                println!("\nChild Mode [kids_featured={}]:", child.kids_featured);
                println!("  Simple TA: {}", child.simple_meaning_ta);
                println!("  Simple EN: {}", child.simple_meaning_en);
                println!(
                    "  Story:     {} / {} (blank='{}')",
                    child.story_title_ta, child.story_title_en, child.game_blank_word_ta
                );
            }
            if let Some(youth) = &full.youth {
                println!("\nYouth Mode:");
                println!("  Analysis TA: {}", youth.analysis_ta);
                println!("  Analysis EN: {}", youth.analysis_en);
                println!("  Action Tip:  {}", youth.action_tip_en);
            }
            if let Some(adult) = &full.adult {
                println!("\nAdult Reflection [{}]:", adult.research_theme);
                println!("  TA: {}", adult.reflection_ta);
                println!("  EN: {}", adult.reflection_en);
            }
        }
        Commands::Story { number } => {
            let full = db.get_kural_full(number)?;
            println!(
                "=== Kutty Story for Kural #{} ({}) ===",
                full.kural.number, full.adhikaram.name_en
            );
            println!("  {}\n  {}", full.kural.line1_ta, full.kural.line2_ta);
            match full.child {
                Some(child) => {
                    println!("Kids Featured: {}", child.kids_featured);
                    println!("\nSimple Meaning (TA): {}", child.simple_meaning_ta);
                    println!("Simple Meaning (EN): {}", child.simple_meaning_en);
                    println!("\n[{}] {}", child.story_title_ta, child.kutty_story_ta);
                    println!("\n[{}] {}", child.story_title_en, child.kutty_story_en);
                    println!("\nMoral (TA): {}", child.moral_ta);
                    println!("Moral (EN): {}", child.moral_en);
                    println!(
                        "\nWord Game Blank: '{}' | Options TA: {:?} | Options EN: {:?}",
                        child.game_blank_word_ta, child.game_options_ta, child.game_options_en
                    );
                }
                None => {
                    println!(
                        "(Child enrichment not yet generated for Kural #{number}; showing canonical meanings)"
                    );
                    println!("\nமு.வ. உரை: {}", full.kural.meaning_mu_va);
                    println!("English: {}", full.kural.explanation_en);
                }
            }
        }
        Commands::Youth { number } => {
            let full = db.get_kural_full(number)?;
            println!(
                "=== Youth Mode for Kural #{} ({}) ===",
                full.kural.number, full.adhikaram.name_en
            );
            println!("  {}\n  {}", full.kural.line1_ta, full.kural.line2_ta);
            match full.youth {
                Some(youth) => {
                    println!("\nAnalysis (TA): {}", youth.analysis_ta);
                    println!("Analysis (EN): {}", youth.analysis_en);
                    println!("\nAction Tip (TA): {}", youth.action_tip_ta);
                    println!("Action Tip (EN): {}", youth.action_tip_en);
                    println!("\nQuiz (TA): {}", youth.quiz_question_ta);
                    for (idx, opt) in youth.quiz_options_ta.iter().enumerate() {
                        let mark = if idx == usize::from(youth.quiz_correct_idx) {
                            "*"
                        } else {
                            " "
                        };
                        println!("  [{mark}] {idx}: {opt}");
                    }
                    println!("Quiz (EN): {}", youth.quiz_question_en);
                    for (idx, opt) in youth.quiz_options_en.iter().enumerate() {
                        let mark = if idx == usize::from(youth.quiz_correct_idx) {
                            "*"
                        } else {
                            " "
                        };
                        println!("  [{mark}] {idx}: {opt}");
                    }
                }
                None => {
                    println!(
                        "(Youth enrichment not yet generated for Kural #{number}; showing canonical meanings)"
                    );
                    println!("\nசாலமன் பாப்பையா: {}", full.kural.meaning_solomon);
                    println!("English: {}", full.kural.explanation_en);
                }
            }
        }
        Commands::Search {
            query,
            division,
            limit,
        } => {
            let outcome = db.search_hybrid(&query, division, limit)?;
            println!("Query: '{}'", outcome.query);
            if let Some(dym) = &outcome.did_you_mean {
                println!("Did you mean: {dym}");
            }
            println!("Hits ({}):", outcome.hits.len());
            for hit in &outcome.hits {
                println!(
                    "  #{} [{:?} score={:.2}] {} — {} | {}",
                    hit.kural.number,
                    hit.match_kind,
                    hit.score,
                    hit.kural.adhikaram_ta,
                    hit.kural.line1_ta,
                    hit.kural.translation_en
                );
            }
        }
        Commands::Knowledge { query, limit } => {
            let entries = db.search_knowledge(&query, limit)?;
            println!("Knowledge hits ({}):", entries.len());
            for e in entries {
                println!("\n[#{}, {}] {}", e.id, e.category, e.question_en);
                println!("  TA: {}", e.question_ta);
                println!("  Answer EN: {}", e.answer_en);
                println!("  Answer TA: {}", e.answer_ta);
            }
        }
        Commands::Daily { age, day } => {
            let age_group: AgeGroup = age.parse()?;
            let full = db.get_daily_kural(age_group, day)?;
            println!(
                "Daily Kural (age={}, day={}): #{} — {} / {}",
                age_group, day, full.kural.number, full.kural.line1_ta, full.kural.line2_ta
            );
            println!("Translation: {}", full.kural.translation_en);
        }
        Commands::Stats => {
            let s = db.stats()?;
            let mb = s.file_size_bytes as f64 / (1024.0 * 1024.0);
            println!("Database Stats ({}):", db_path.display());
            println!("  build_mode:      {}", s.build_mode);
            println!("  integrity_check: {}", s.integrity_check);
            println!(
                "  file_size:       {mb:.2} MB ({} bytes)",
                s.file_size_bytes
            );
            println!("  divisions:       {}", s.divisions);
            println!("  iyals:           {}", s.iyals);
            println!("  adhikarams:      {}", s.adhikarams);
            println!("  kurals:          {}", s.kurals);
            println!("  kural_child:     {}", s.kural_child);
            println!("  kural_youth:     {}", s.kural_youth);
            println!("  kural_adult:     {}", s.kural_adult);
            println!("  tags:            {}", s.tags);
            println!("  kural_tags:      {}", s.kural_tags);
            println!("  knowledge_base:  {}", s.knowledge_base);
            println!("  kural_vectors:   {}", s.kural_embeddings);
            println!("  kb_vectors:      {}", s.knowledge_embeddings);
        }
    }

    Ok(())
}

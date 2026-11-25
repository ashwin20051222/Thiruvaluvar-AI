//! `ai_cli` — Phase 4 benchmark & utility CLI for `kural-ai`.
//!
//! # Commands
//! - `download` — print offline model asset locations & `scripts/download_fast_models.sh` helper
//! - `benchmark` — run latency benchmarks (`AgeClassifier`, `SemanticExplainer`, and optional STT)
//! - `transcribe <wav>` — transcribe a 16-kHz mono PCM WAV file via `SpeechEngine`
//! - `synthesize <text>` — synthesize speech to WAV via `SpeechEngine` (`Piper` or `espeak-ng`)
#![forbid(unsafe_code)]

use std::f32::consts::PI;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use kural_ai::{
    AgeClassifier, COUPLET_PAUSE_MS, SemanticExplainer, SpeechEngine, extract_vocal_tract_features,
    parse_wav_pcm_mono_i16,
};
use kural_audio::{CapturedAudio, PcmChunk};
use kural_core::{AgeAiConfig, AgeGroup, AppConfig, Language, Scholar};
use kural_db::KuralDb;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "ai_cli",
    about = "Thiruvalluvar AI — Phase 4 vocal-tract, semantic explainer & speech CLI",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print expected offline model layout under `models/` and download helper command.
    Download,
    /// Run Phase 4 latency benchmarks for AgeClassifier, SemanticExplainer, and optional STT.
    Benchmark {
        /// Path to workspace config.toml (default: auto-detect).
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Transcribe a 16-kHz mono WAV file using `SpeechEngine`.
    Transcribe {
        /// Path to a 16-kHz mono PCM WAV file.
        wav: PathBuf,
        /// Language: `en` or `ta` (default: en).
        #[arg(long, default_value = "en")]
        lang: String,
        /// Models directory (default: from config.toml or `models`).
        #[arg(long)]
        model_dir: Option<PathBuf>,
    },
    /// Synthesize text to a WAV file using `SpeechEngine` (Piper VITS or `espeak-ng` fallback).
    Synthesize {
        /// Text to speak (UTF-8, English or Tamil; split lines on newline for couplet pause).
        text: String,
        /// Language: `en` or `ta` (default: en).
        #[arg(long, default_value = "en")]
        lang: String,
        /// Output WAV file path (omit to write WAV bytes to stdout).
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Speech rate factor (default: 1.0).
        #[arg(long, default_value_t = 1.0)]
        speed: f32,
        /// Models directory (default: from config.toml or `models`).
        #[arg(long)]
        model_dir: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Download => cmd_download(),
        Cmd::Benchmark { config } => cmd_benchmark(config),
        Cmd::Transcribe {
            wav,
            lang,
            model_dir,
        } => cmd_transcribe(&wav, &lang, model_dir),
        Cmd::Synthesize {
            text,
            lang,
            output,
            speed,
            model_dir,
        } => cmd_synthesize(&text, &lang, output.as_deref(), speed, model_dir),
    }
}

fn cmd_download() -> Result<()> {
    println!("Offline Phase 4 model layout (< 350 MB total under models/):");
    println!(
        "  models/embeddings/  -> potion-multilingual-128M-int8 (model.safetensors, tokenizer.json, config.json)"
    );
    println!("  models/vad/         -> silero_vad.onnx");
    println!(
        "  models/stt_en/      -> sherpa-onnx-streaming-zipformer-en-20M (encoder/decoder/joiner + tokens.txt)"
    );
    println!("  models/stt_ta/      -> indicconformer-ta int8 (model.int8.onnx + tokens.txt)");
    println!("  models/tts/en/      -> en_US-lessac-low-int8 (.onnx + .onnx.json)");
    println!("  models/tts/ta/      -> ta_IN-ValluvarNeural-medium (.onnx + .onnx.json)");
    println!();
    println!("Run: bash scripts/download_fast_models.sh");
    Ok(())
}

fn cmd_benchmark(config_path: Option<PathBuf>) -> Result<()> {
    let (cfg, workspace_root) = load_config(config_path)?;
    println!("=== Phase 4 Offline Latency Benchmarks ===\n");

    benchmark_age_classifier(&cfg.age_ai);
    benchmark_semantic_explainer(&cfg, &workspace_root)?;
    benchmark_stt_if_present(&cfg, &workspace_root);

    Ok(())
}

fn benchmark_age_classifier(age_cfg: &AgeAiConfig) {
    let clf = AgeClassifier::new(age_cfg.clone());
    let speech = synthesize_glottal_vowel_signal(115.0, [500.0, 1500.0, 2500.0], 2000);
    let cap = CapturedAudio {
        samples_16k: speech.clone(),
        speech_only_16k: speech.clone(),
        pre_roll_noise_16k: Vec::new(),
        voiced_ms: 2000,
        snr_db: 25.0,
        clipping_ratio: 0.0,
        rms_level: 0.08,
    };

    let _ = clf.classify(&cap);
    const ITERS: u32 = 30;
    let t0 = Instant::now();
    for _ in 0..ITERS {
        let _ = clf.classify(&cap);
    }
    let mean_ms = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(ITERS);

    let t1 = Instant::now();
    for _ in 0..ITERS {
        let _ = extract_vocal_tract_features(&speech, &[], age_cfg);
    }
    let feat_ms = t1.elapsed().as_secs_f64() * 1000.0 / f64::from(ITERS);

    println!("AgeClassifier (2 s adult-male synthetic vowel):");
    println!("  Mean classify() latency : {mean_ms:.2} ms ({ITERS} iters)");
    println!("  Mean features() latency : {feat_ms:.2} ms");
    println!("  Release target          : < 30 ms (<= 50 ms gate)\n");
}

fn benchmark_semantic_explainer(cfg: &AppConfig, workspace_root: &Path) -> Result<()> {
    let db_path = resolve_from_root(workspace_root, &cfg.paths.kural_db);
    if !db_path.is_file() {
        println!(
            "SemanticExplainer benchmark skipped: database not found at {}\n",
            db_path.display()
        );
        return Ok(());
    }

    let models_dir = resolve_from_root(workspace_root, &cfg.paths.models_dir);
    let db = KuralDb::open_with_models_dir(&db_path, &models_dir)
        .with_context(|| format!("opening KuralDb at {}", db_path.display()))?;
    let emb_loaded = db.embeddings_loaded();
    let explainer = SemanticExplainer::new(db, cfg.ai.clone());

    let explain_queries = [
        (1_u16, AgeGroup::Kids, Language::Tamil, Scholar::MuVa),
        (
            395_u16,
            AgeGroup::Adult,
            Language::English,
            Scholar::Parimel,
        ),
    ];
    let t0 = Instant::now();
    for (num, age, lang, scholar) in explain_queries {
        let ans = explainer.explain_kural(num, age, lang, scholar)?;
        assert!(!ans.out_of_scope);
    }
    let explain_mean = t0.elapsed().as_secs_f64() * 1000.0 / (explain_queries.len() as f64);

    let qa_queries: [(&str, Option<u16>); 4] = [
        ("What does Kural 1 mean?", None),
        ("Explain friendship in Thirukkural", None),
        ("What is 42 * 10?", Some(1)),
        ("Who is Thiruvalluvar?", None),
    ];
    let mut ask_latencies_ms = Vec::with_capacity(qa_queries.len() * 5);
    for _ in 0..5 {
        for &(q, seed) in &qa_queries {
            let t = Instant::now();
            let _ = explainer.ask_question(q, AgeGroup::Youth, Language::English, seed)?;
            ask_latencies_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    ask_latencies_ms.sort_by(f64::total_cmp);
    let p99_idx = ((ask_latencies_ms.len() as f64 * 0.99).ceil() as usize)
        .saturating_sub(1)
        .min(ask_latencies_ms.len().saturating_sub(1));
    let mean_ask: f64 = ask_latencies_ms.iter().sum::<f64>() / (ask_latencies_ms.len() as f64);

    println!("SemanticExplainer (embeddings_loaded={emb_loaded}):");
    println!("  explain_kural() mean : {explain_mean:.2} ms");
    println!("  ask_question() mean  : {mean_ask:.2} ms");
    println!(
        "  ask_question() p99   : {:.2} ms",
        ask_latencies_ms[p99_idx]
    );
    println!("  Release target       : < 15 ms (<= 20 ms p99 gate)\n");
    Ok(())
}

fn benchmark_stt_if_present(cfg: &AppConfig, workspace_root: &Path) {
    let models_dir = resolve_from_root(workspace_root, &cfg.paths.models_dir);
    let Ok(mut engine) = SpeechEngine::new(&models_dir, Language::English, &cfg.ai) else {
        println!("STT benchmark skipped: SpeechEngine failed to initialize\n");
        return;
    };

    let sample = vec![0.0_f32; 16_000];
    for lang in [Language::English, Language::Tamil] {
        if engine.ensure_language(lang).is_err() || !engine.stt_loaded() {
            println!(
                "STT benchmark ({lang}): model absent under {}/stt_*",
                models_dir.display()
            );
            continue;
        }
        let t0 = Instant::now();
        let _ = engine.transcribe(&sample, lang);
        println!(
            "STT benchmark ({lang}) 1 s frame : {:.1} ms (target <= 300 ms)",
            t0.elapsed().as_secs_f64() * 1000.0
        );
    }
}

fn cmd_transcribe(wav: &Path, lang_str: &str, model_dir: Option<PathBuf>) -> Result<()> {
    let lang = parse_language(lang_str);
    let models_dir = resolve_models_dir(model_dir);
    let wav_bytes =
        std::fs::read(wav).with_context(|| format!("reading WAV file: {}", wav.display()))?;
    let (pcm_i16, sr) = parse_wav_pcm_mono_i16(&wav_bytes)?;
    anyhow::ensure!(
        sr == 16_000,
        "expected 16000 Hz WAV for STT input, got {sr} Hz"
    );
    let samples_f32: Vec<f32> = pcm_i16.iter().map(|&s| f32::from(s) / 32768.0).collect();

    let mut engine = SpeechEngine::new(&models_dir, lang, &kural_core::AiConfig::default())?;
    let t0 = Instant::now();
    let transcript = engine.transcribe(&samples_f32, lang)?;
    println!("Transcript : {transcript}");
    println!("Latency    : {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}

fn cmd_synthesize(
    text: &str,
    lang_str: &str,
    output: Option<&Path>,
    speed: f32,
    model_dir: Option<PathBuf>,
) -> Result<()> {
    let lang = parse_language(lang_str);
    let models_dir = resolve_models_dir(model_dir);
    let mut engine = SpeechEngine::new(&models_dir, lang, &kural_core::AiConfig::default())?;

    let chunks_owned: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let (tx, mut rx) = mpsc::channel::<PcmChunk>(32);
    let cancel = CancellationToken::new();

    let t0 = Instant::now();
    engine.synthesize_streaming(&chunks_owned, lang, speed, &cancel, &tx)?;
    drop(tx);

    let mut all_samples: Vec<i16> = Vec::new();
    let mut sample_rate_hz = 22_050_u32;
    while let Ok(chunk) = rx.try_recv() {
        sample_rate_hz = chunk.sample_rate_hz;
        all_samples.extend_from_slice(&chunk.samples);
    }

    let wav_bytes = encode_wav_mono_i16(&all_samples, sample_rate_hz);
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    match output {
        Some(path) => {
            std::fs::write(path, &wav_bytes)
                .with_context(|| format!("writing WAV file: {}", path.display()))?;
            println!(
                "Synthesized {} samples ({} Hz, gap={} ms) in {:.1} ms -> {}",
                all_samples.len(),
                sample_rate_hz,
                COUPLET_PAUSE_MS,
                elapsed_ms,
                path.display()
            );
        }
        None => {
            std::io::stdout()
                .write_all(&wav_bytes)
                .context("writing WAV bytes to stdout")?;
        }
    }
    Ok(())
}

fn encode_wav_mono_i16(samples: &[i16], sample_rate_hz: u32) -> Vec<u8> {
    let data_bytes = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let riff_size = data_bytes.saturating_add(36);
    let byte_rate = sample_rate_hz.saturating_mul(2);

    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn synthesize_glottal_vowel_signal(f0_hz: f32, formants: [f32; 3], duration_ms: usize) -> Vec<f32> {
    let len = (16_000 * duration_ms) / 1000;
    let bws = [75.0_f32, 110.0, 150.0];
    let mut y1 = [0.0_f32; 3];
    let mut y2 = [0.0_f32; 3];
    let mut a1 = [0.0_f32; 3];
    let mut a2 = [0.0_f32; 3];
    for i in 0..3 {
        let r = (-PI * bws[i] / 16_000.0).exp();
        let theta = 2.0 * PI * formants[i] / 16_000.0;
        a1[i] = -2.0 * r * theta.cos();
        a2[i] = r * r;
    }

    let mut phase = 0.0_f32;
    let inc = f0_hz / 16_000.0;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        phase += inc;
        let pulse = if phase >= 1.0 {
            phase -= 1.0;
            1.0
        } else {
            0.0
        };
        let mut x = pulse;
        for i in 0..3 {
            let y = x - a1[i] * y1[i] - a2[i] * y2[i];
            y2[i] = y1[i];
            y1[i] = y;
            x = y;
        }
        out.push((x * 0.04).clamp(-0.8, 0.8));
    }
    out
}

fn parse_language(raw: &str) -> Language {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ta" | "tamil" => Language::Tamil,
        _ => Language::English,
    }
}

fn find_workspace_root() -> PathBuf {
    let mut d = std::env::current_dir().unwrap_or_default();
    for _ in 0..6 {
        if d.join("config.toml").is_file() {
            return d;
        }
        if !d.pop() {
            break;
        }
    }
    PathBuf::from(".")
}

fn load_config(path: Option<PathBuf>) -> Result<(AppConfig, PathBuf)> {
    let cfg_path = path.unwrap_or_else(|| find_workspace_root().join("config.toml"));
    let root = cfg_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let cfg = AppConfig::load(Some(&cfg_path))
        .with_context(|| format!("loading config: {}", cfg_path.display()))?;
    Ok((cfg, root))
}

fn resolve_from_root(root: &Path, rel: &Path) -> PathBuf {
    if rel.is_absolute() {
        rel.to_path_buf()
    } else {
        root.join(rel)
    }
}

fn resolve_models_dir(override_dir: Option<PathBuf>) -> PathBuf {
    override_dir.unwrap_or_else(|| find_workspace_root().join("models"))
}

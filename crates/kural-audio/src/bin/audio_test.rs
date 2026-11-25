use clap::Parser;
use kural_audio::{AudioDeviceInfo, AudioError, AudioHandle, EarconKind, list_audio_devices};
use kural_core::{AppConfig, AudioState, CaptureMode};
use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::process::ExitCode;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "audio_test",
    about = "ALSA / CPAL / Rodio hardware diagnostic CLI for kural-audio"
)]
struct Cli {
    /// List all detected input/output audio devices and exit
    #[arg(long)]
    list: bool,

    /// Path to config.toml
    #[arg(long)]
    config: Option<PathBuf>,

    /// Override configured input device name pattern (or "default")
    #[arg(long)]
    input: Option<String>,

    /// Override configured output device name pattern (or "default")
    #[arg(long)]
    output: Option<String>,

    /// Speech capture duration in seconds
    #[arg(long, default_value_t = 3)]
    duration: u32,

    /// Skip earcons and captured clip speaker echo playback
    #[arg(long)]
    no_playback: bool,
}

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            Poll::Pending => thread::park(),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.list {
        return run_list();
    }

    match run_diagnostics(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(DiagnosticFailure::DeviceUnavailable(msg)) => {
            print_hardware_hint(&msg);
            ExitCode::from(1)
        }
        Err(DiagnosticFailure::Other(msg)) => {
            eprintln!("audio_test error: {msg}");
            ExitCode::from(1)
        }
    }
}

enum DiagnosticFailure {
    DeviceUnavailable(String),
    Other(String),
}

impl From<AudioError> for DiagnosticFailure {
    fn from(err: AudioError) -> Self {
        match err {
            AudioError::DeviceUnavailable(msg) => Self::DeviceUnavailable(msg),
            other => Self::Other(other.to_string()),
        }
    }
}

fn run_list() -> ExitCode {
    match list_audio_devices() {
        Ok(devices) => {
            print_device_list(&devices);
            ExitCode::SUCCESS
        }
        Err(AudioError::DeviceUnavailable(msg)) => {
            print_hardware_hint(&msg);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("failed to list audio devices: {err}");
            ExitCode::SUCCESS
        }
    }
}

fn print_device_list(devices: &[AudioDeviceInfo]) {
    if devices.is_empty() {
        println!("No ALSA/host audio devices detected.");
        println!("Hint: check /dev/snd permissions or connect a USB audio interface.");
        return;
    }

    println!("Detected audio devices ({}):", devices.len());
    for (idx, dev) in devices.iter().enumerate() {
        let dir = match (dev.is_input, dev.is_output) {
            (true, true) => "duplex",
            (true, false) => "input",
            (false, true) => "output",
            (false, false) => "none",
        };
        let default_tag = if dev.is_default { " [default]" } else { "" };
        let driver = dev.driver.as_deref().unwrap_or("unknown");
        let rate = dev
            .default_sample_rate_hz
            .map_or_else(|| "-".to_string(), |r| format!("{r} Hz"));
        let ch = dev
            .default_channels
            .map_or_else(|| "-".to_string(), |c| format!("{c} ch"));
        println!(
            "  [{idx}] {name}{default_tag} | dir={dir} | driver={driver} | rate={rate} | {ch}",
            name = dev.name
        );
    }
}

fn print_hardware_hint(reason: &str) {
    eprintln!("Audio hardware unavailable: {reason}");
    eprintln!("Hint: ensure an ALSA capture/playback device is attached, check `/dev/snd` group");
    eprintln!(
        "      membership (`audio`), inspect `cargo run -p kural-audio --bin audio_test -- --list`,"
    );
    eprintln!("      or set `[audio].input_device` / `[audio].output_device` in `config.toml`.");
}

fn run_diagnostics(cli: &Cli) -> Result<(), DiagnosticFailure> {
    let mut cfg = AppConfig::load(cli.config.as_deref())
        .map_err(|e| DiagnosticFailure::Other(format!("config load failed: {e}")))?;

    if let Some(ref input) = cli.input {
        cfg.audio.input_device = input.clone();
    }
    if let Some(ref output) = cli.output {
        cfg.audio.output_device = output.clone();
    }

    println!(
        "Booting AudioHandle (input='{}', output='{}')...",
        cfg.audio.input_device, cfg.audio.output_device
    );
    let handle = AudioHandle::spawn(&cfg.audio, None, cfg.ai.tts_cache_max_bytes);
    let health = handle.health();
    println!(
        "AudioHealthStatus: input_ok={}, output_ok={}, sample_rate_hz={}",
        health.input_ok, health.output_ok, health.sample_rate_hz
    );

    if !health.input_ok {
        handle.shutdown();
        return Err(DiagnosticFailure::DeviceUnavailable(
            "no working audio input device opened".to_string(),
        ));
    }
    if !cli.no_playback && !health.output_ok {
        handle.shutdown();
        return Err(DiagnosticFailure::DeviceUnavailable(
            "no working audio output device opened (pass --no-playback to test mic only)"
                .to_string(),
        ));
    }
    if cli.no_playback {
        block_on(handle.set_muted(true))?;
    }

    println!("Measuring 1.2 s ambient noise floor...");
    let ambient =
        block_on(handle.start_capture(CaptureMode::FixedAgeSample { duration_ms: 1200 }))?;
    let ambient_db = rms_to_dbfs(ambient.rms_level);
    println!(
        "Ambient noise floor: {:.2} dBFS (rms_level={:.5}, frames={})",
        ambient_db,
        ambient.rms_level,
        ambient.samples_16k.len()
    );

    if !cli.no_playback {
        println!("Playing ListenStart + CmdRecognized chimes...");
        block_on(handle.play_earcon(EarconKind::ListenStart))?;
        block_on(handle.play_earcon(EarconKind::CmdRecognized))?;
        wait_for_idle(
            &handle,
            Duration::from_millis(u64::from(cfg.audio.echo_cooldown_ms) + 500),
        )?;
    }

    let duration_ms = cli.duration.saturating_mul(1000).max(500);
    println!("Capturing speech for {} ms (please speak)...", duration_ms);
    let captured = block_on(handle.start_capture(CaptureMode::FixedAgeSample { duration_ms }))?;
    println!(
        "Capture metrics: voiced_ms={}, snr_db={:.2} dB, clipping_ratio={:.5}, rms_level={:.5}, samples_16k={}",
        captured.voiced_ms,
        captured.snr_db,
        captured.clipping_ratio,
        captured.rms_level,
        captured.samples_16k.len()
    );

    if !cli.no_playback && !captured.samples_16k.is_empty() {
        println!("Echoing captured 16 kHz mono clip back to speaker...");
        let pcm_i16 = f32_to_i16_pcm(&captured.samples_16k);
        block_on(handle.play_pcm_i16(pcm_i16, 16_000, None))?;
        wait_for_idle(
            &handle,
            Duration::from_millis(u64::from(cfg.audio.echo_cooldown_ms) + 1000),
        )?;
        println!("Echo Guard verified: AudioState returned to Idle.");
    }

    handle.shutdown();
    Ok(())
}

fn wait_for_idle(handle: &AudioHandle, timeout: Duration) -> Result<(), DiagnosticFailure> {
    let state_rx = handle.subscribe_state();
    let start = Instant::now();
    while *state_rx.borrow() != AudioState::Idle {
        if start.elapsed() >= timeout {
            return Err(DiagnosticFailure::Other(format!(
                "Echo Guard did not return to AudioState::Idle within {:?} (stuck in {:?})",
                timeout,
                *state_rx.borrow()
            )));
        }
        thread::sleep(Duration::from_millis(16));
    }
    Ok(())
}

fn rms_to_dbfs(rms: f32) -> f32 {
    20.0 * rms.max(1e-7).log10()
}

fn f32_to_i16_pcm(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            let clamped = if s.is_finite() {
                s.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            (clamped * 32767.0).round() as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[test]
    fn cli_parses_defaults_and_flags() {
        let default_cli = Cli::try_parse_from(["audio_test"]).expect("parse defaults");
        assert!(!default_cli.list);
        assert_eq!(default_cli.duration, 3);
        assert!(!default_cli.no_playback);
        assert!(default_cli.config.is_none());

        let custom = Cli::try_parse_from([
            "audio_test",
            "--list",
            "--input",
            "usb",
            "--output",
            "hdmi",
            "--duration",
            "5",
            "--no-playback",
        ])
        .expect("parse custom");
        assert!(custom.list);
        assert_eq!(custom.input.as_deref(), Some("usb"));
        assert_eq!(custom.output.as_deref(), Some("hdmi"));
        assert_eq!(custom.duration, 5);
        assert!(custom.no_playback);
    }

    #[test]
    fn block_on_wakes_on_tokio_oneshot() {
        let (tx, rx) = oneshot::channel();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            let _ = tx.send(42_u32);
        });
        let val = block_on(rx).expect("oneshot recv");
        assert_eq!(val, 42);
    }

    #[test]
    fn conversion_and_dbfs_helpers_sanitize_inputs() {
        assert!((rms_to_dbfs(1.0) - 0.0).abs() < 1e-4);
        assert!(rms_to_dbfs(0.0).is_finite());
        let converted = f32_to_i16_pcm(&[0.0, 1.0, -1.0, f32::NAN, 2.0]);
        assert_eq!(converted, vec![0, 32767, -32767, 0, 32767]);
    }
}

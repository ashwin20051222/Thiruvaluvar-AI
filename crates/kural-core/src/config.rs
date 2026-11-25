use crate::CoreError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Top-level offline application configuration loaded from `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub paths: PathsConfig,
    pub audio: AudioConfig,
    pub age_ai: AgeAiConfig,
    pub ai: AiConfig,
    pub ui: UiConfig,
}

impl AppConfig {
    /// Load configuration from `--config <PATH>`, `./config.toml`, or compiled defaults.
    /// Relative `[paths]` entries are resolved against the loaded file's parent directory
    /// (or workspace root fallback when compiled defaults are used).
    pub fn load(cli_config: Option<&Path>) -> Result<Self, CoreError> {
        let workspace_base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .ok()
            .filter(|p| p.is_dir())
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| PathBuf::from("."));

        let (mut cfg, base_dir) = if let Some(path) = cli_config {
            if !path.is_file() {
                return Err(CoreError::Io {
                    path: path.to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "explicit config file not found",
                    ),
                });
            }
            let parsed = Self::from_file(path)?;
            let parent = config_base_dir(path, &workspace_base);
            (parsed, parent)
        } else {
            let cwd_cfg = PathBuf::from("config.toml");
            if cwd_cfg.is_file() {
                let parsed = Self::from_file(&cwd_cfg)?;
                let parent = config_base_dir(&cwd_cfg, &workspace_base);
                (parsed, parent)
            } else {
                let ws_cfg = workspace_base.join("config.toml");
                if ws_cfg.is_file() {
                    let parsed = Self::from_file(&ws_cfg)?;
                    let parent = config_base_dir(&ws_cfg, &workspace_base);
                    (parsed, parent)
                } else {
                    (Self::default(), workspace_base)
                }
            }
        };

        cfg.resolve_paths(&base_dir);
        cfg.validate()?;
        Ok(cfg)
    }

    fn from_file(path: &Path) -> Result<Self, CoreError> {
        let raw = fs::read_to_string(path).map_err(|source| CoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&raw).map_err(|source| CoreError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Resolve relative `[paths]` against `base_dir`.
    pub fn resolve_paths(&mut self, base_dir: &Path) {
        if self.paths.kural_db.is_relative() {
            self.paths.kural_db = base_dir.join(&self.paths.kural_db);
        }
        if self.paths.user_state_db.is_relative() {
            self.paths.user_state_db = base_dir.join(&self.paths.user_state_db);
        }
        if self.paths.models_dir.is_relative() {
            self.paths.models_dir = base_dir.join(&self.paths.models_dir);
        }
    }

    /// Validate numerical ranges and enum string invariants.
    pub fn validate(&self) -> Result<(), CoreError> {
        if !self.audio.master_volume.is_finite()
            || !(0.0..=1.0).contains(&self.audio.master_volume)
        {
            return Err(CoreError::ConfigValidation(format!(
                "audio.master_volume ({}) must be finite and in [0.0, 1.0]",
                self.audio.master_volume
            )));
        }
        if !self.audio.vad_threshold.is_finite()
            || !(0.0..=1.0).contains(&self.audio.vad_threshold)
        {
            return Err(CoreError::ConfigValidation(format!(
                "audio.vad_threshold ({}) must be finite and in [0.0, 1.0]",
                self.audio.vad_threshold
            )));
        }
        if self.audio.earcon_guard_ms == 0
            || self.audio.echo_cooldown_ms == 0
            || self.audio.age_sample_duration_ms == 0
        {
            return Err(CoreError::ConfigValidation(
                "audio.earcon_guard_ms, echo_cooldown_ms, and age_sample_duration_ms must be > 0"
                    .into(),
            ));
        }
        if self.audio.listening_timeout_ms == 0 || self.audio.max_command_duration_ms == 0 {
            return Err(CoreError::ConfigValidation(
                "audio.listening_timeout_ms and audio.max_command_duration_ms must be > 0".into(),
            ));
        }
        if !self.age_ai.max_clipping_ratio.is_finite()
            || !(0.0..=1.0).contains(&self.age_ai.max_clipping_ratio)
        {
            return Err(CoreError::ConfigValidation(format!(
                "age_ai.max_clipping_ratio ({}) must be in [0.0, 1.0]",
                self.age_ai.max_clipping_ratio
            )));
        }
        if !self.age_ai.low_confidence_threshold.is_finite()
            || !(0.0..=1.0).contains(&self.age_ai.low_confidence_threshold)
        {
            return Err(CoreError::ConfigValidation(format!(
                "age_ai.low_confidence_threshold ({}) must be in [0.0, 1.0]",
                self.age_ai.low_confidence_threshold
            )));
        }
        if !self.age_ai.min_snr_db.is_finite()
            || !self.age_ai.min_rms_level.is_finite()
            || !self.age_ai.f0_offset_hz.is_finite()
            || !self.age_ai.vtl_offset_cm.is_finite()
            || self.age_ai.min_rms_level < 0.0
        {
            return Err(CoreError::ConfigValidation(
                "age_ai calibration fields must be finite with min_rms_level >= 0".into(),
            ));
        }
        if self.ai.onnx_intra_threads < 1 {
            return Err(CoreError::ConfigValidation(format!(
                "ai.onnx_intra_threads ({}) must be >= 1",
                self.ai.onnx_intra_threads
            )));
        }
        for (name, v) in [
            ("ai.min_vec_candidate_cosine", self.ai.min_vec_candidate_cosine),
            ("ai.oos_min_vector_cosine", self.ai.oos_min_vector_cosine),
            ("ai.kb_min_vector_cosine", self.ai.kb_min_vector_cosine),
        ] {
            if !v.is_finite() || !(-1.0..=1.0).contains(&v) {
                return Err(CoreError::ConfigValidation(format!(
                    "{name} ({v}) must be finite and in [-1.0, 1.0]"
                )));
            }
        }
        if !self.ai.oos_min_rrf_score.is_finite()
            || self.ai.oos_min_rrf_score < 0.0
            || !self.ai.kb_min_rrf_score.is_finite()
            || self.ai.kb_min_rrf_score < 0.0
        {
            return Err(CoreError::ConfigValidation(
                "ai RRF thresholds must be finite and >= 0.0".into(),
            ));
        }
        if !matches!(
            self.ui.viewport_mode.as_str(),
            "desktop" | "pi_preview" | "kiosk"
        ) {
            return Err(CoreError::ConfigValidation(format!(
                "ui.viewport_mode '{}' must be one of desktop | pi_preview | kiosk",
                self.ui.viewport_mode
            )));
        }
        Ok(())
    }
}

fn config_base_dir(config_path: &Path, fallback: &Path) -> PathBuf {
    let abs = if config_path.is_absolute() {
        config_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(config_path))
            .unwrap_or_else(|_| fallback.join(config_path))
    };
    abs.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fallback.to_path_buf())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    pub kural_db: PathBuf,
    pub user_state_db: PathBuf,
    pub models_dir: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            kural_db: PathBuf::from("data/thirukkural.db"),
            user_state_db: PathBuf::from("data/user_state.db"),
            models_dir: PathBuf::from("models"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub input_device: String,
    pub output_device: String,
    pub master_volume: f32,
    pub earcon_guard_ms: u32,
    pub echo_cooldown_ms: u32,
    pub listening_timeout_ms: u32,
    pub max_command_duration_ms: u32,
    pub age_sample_duration_ms: u32,
    pub vad_threshold: f32,
    pub aec_enabled: bool,
    pub mic_to_listen_guard_ms: u64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: "default".to_string(),
            output_device: "default".to_string(),
            master_volume: 0.85,
            earcon_guard_ms: 220,
            echo_cooldown_ms: 250,
            listening_timeout_ms: 5000,
            max_command_duration_ms: 7000,
            age_sample_duration_ms: 3200,
            vad_threshold: 0.50,
            aec_enabled: false,
            mic_to_listen_guard_ms: 400,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgeAiConfig {
    pub min_voiced_ms: u32,
    pub min_snr_db: f32,
    pub max_clipping_ratio: f32,
    pub min_rms_level: f32,
    pub low_confidence_threshold: f32,
    pub f0_offset_hz: f32,
    pub vtl_offset_cm: f32,
}

impl Default for AgeAiConfig {
    fn default() -> Self {
        Self {
            min_voiced_ms: 1200,
            min_snr_db: 8.0,
            max_clipping_ratio: 0.02,
            min_rms_level: 0.015,
            low_confidence_threshold: 0.70,
            f0_offset_hz: 0.0,
            vtl_offset_cm: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub onnx_intra_threads: i32,
    pub onnx_allow_spinning: bool,
    pub min_vec_candidate_cosine: f32,
    pub oos_min_vector_cosine: f32,
    pub oos_min_rrf_score: f32,
    pub kb_min_vector_cosine: f32,
    pub kb_min_rrf_score: f32,
    pub tts_cache_max_entries: usize,
    pub tts_cache_max_bytes: usize,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            onnx_intra_threads: 1,
            onnx_allow_spinning: false,
            min_vec_candidate_cosine: 0.25,
            oos_min_vector_cosine: 0.36,
            oos_min_rrf_score: 0.022,
            kb_min_vector_cosine: 0.42,
            kb_min_rrf_score: 0.025,
            tts_cache_max_entries: 16,
            tts_cache_max_bytes: 16_777_216,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub viewport_mode: String,
    pub toast_duration_ms: u64,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            viewport_mode: "desktop".to_string(),
            toast_duration_ms: 1800,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_config_validates_cleanly() {
        let cfg = AppConfig::default();
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.ai.tts_cache_max_bytes, 16_777_216);
        assert_eq!(cfg.audio.earcon_guard_ms, 220);
    }

    #[test]
    fn load_resolves_relative_paths_and_rejects_invalid_bounds() {
        let dir = TempDir::new().expect("tempdir");
        let cfg_path = dir.path().join("custom.toml");
        fs::write(
            &cfg_path,
            "[paths]\nkural_db = \"db/k.db\"\n[audio]\nmaster_volume = 0.6\n",
        )
        .expect("write cfg");

        let cfg = AppConfig::load(Some(&cfg_path)).expect("load config");
        assert_eq!(cfg.paths.kural_db, dir.path().join("db/k.db"));
        assert!((cfg.audio.master_volume - 0.6).abs() < 1e-6);

        let bad_path = dir.path().join("bad.toml");
        fs::write(&bad_path, "[audio]\nmaster_volume = 1.5\n").expect("write bad cfg");
        let err = AppConfig::load(Some(&bad_path)).unwrap_err();
        assert!(matches!(err, CoreError::ConfigValidation(_)));
    }
}


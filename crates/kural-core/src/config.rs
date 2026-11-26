use crate::CoreError;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const APP_DIR_NAME: &str = "thiruvalluvar-ai";

#[derive(Debug, Clone)]
struct ConfigEnv {
    kural_config: Option<PathBuf>,
    cwd_config: PathBuf,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    system_share_config: PathBuf,
    opt_config: PathBuf,
    workspace_base: Option<PathBuf>,
    exe_dir: Option<PathBuf>,
}

impl ConfigEnv {
    fn from_process() -> Self {
        let workspace_base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .ok()
            .filter(|p| p.is_dir());
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));

        Self {
            kural_config: non_empty_env_path("KURAL_CONFIG"),
            cwd_config: PathBuf::from("config.toml"),
            xdg_config_home: non_empty_env_path("XDG_CONFIG_HOME"),
            home: non_empty_env_path("HOME"),
            xdg_data_home: non_empty_env_path("XDG_DATA_HOME"),
            system_share_config: PathBuf::from("/usr/share/thiruvalluvar-ai/config.toml"),
            opt_config: PathBuf::from("/opt/thiruvalluvar-ai/config.toml"),
            workspace_base,
            exe_dir,
        }
    }

    fn xdg_config_file(&self) -> Option<PathBuf> {
        let base = self
            .xdg_config_home
            .clone()
            .or_else(|| self.home.as_ref().map(|h| h.join(".config")))?;
        Some(base.join(APP_DIR_NAME).join("config.toml"))
    }

    fn xdg_data_dir(&self) -> PathBuf {
        self.xdg_data_home
            .clone()
            .or_else(|| self.home.as_ref().map(|h| h.join(".local").join("share")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_DIR_NAME)
    }
}

fn non_empty_env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

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
    /// Load configuration using the precedence order:
    /// 1. Explicit `--config <PATH>` or `KURAL_CONFIG` environment variable
    /// 2. `./config.toml`
    /// 3. `$XDG_CONFIG_HOME/thiruvalluvar-ai/config.toml` (`~/.config/thiruvalluvar-ai/config.toml`)
    /// 4. `/usr/share/thiruvalluvar-ai/config.toml`
    /// 5. `/opt/thiruvalluvar-ai/config.toml`
    /// 6. Workspace dev fallback or compiled defaults.
    ///
    /// When corpus/models reside in a read-only system directory (or `user_state_db`'s
    /// target directory is not writable by the current user), `paths.user_state_db`
    /// automatically redirects to `$XDG_DATA_HOME/thiruvalluvar-ai/user_state.db`.
    pub fn load(cli_config: Option<&Path>) -> Result<Self, CoreError> {
        let env = ConfigEnv::from_process();
        Self::load_with_env(cli_config, &env)
    }

    fn load_with_env(cli_config: Option<&Path>, env: &ConfigEnv) -> Result<Self, CoreError> {
        let fallback_base = env
            .workspace_base
            .clone()
            .or_else(|| env.exe_dir.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        let load_explicit = |path: &Path| -> Result<(Self, PathBuf, Option<PathBuf>), CoreError> {
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
            let parent = config_base_dir(path, &fallback_base);
            Ok((parsed, parent, Some(path.to_path_buf())))
        };

        let (mut cfg, base_dir, loaded_path) = if let Some(path) = cli_config {
            load_explicit(path)?
        } else if let Some(env_cfg) = env.kural_config.as_deref() {
            load_explicit(env_cfg)?
        } else if env.cwd_config.is_file() {
            let parsed = Self::from_file(&env.cwd_config)?;
            let parent = config_base_dir(&env.cwd_config, &fallback_base);
            (parsed, parent, Some(env.cwd_config.clone()))
        } else if let Some(xdg_cfg) = env.xdg_config_file().filter(|p| p.is_file()) {
            let parsed = Self::from_file(&xdg_cfg)?;
            let parent = config_base_dir(&xdg_cfg, &fallback_base);
            (parsed, parent, Some(xdg_cfg))
        } else if env.system_share_config.is_file() {
            let parsed = Self::from_file(&env.system_share_config)?;
            let parent = config_base_dir(&env.system_share_config, &fallback_base);
            (parsed, parent, Some(env.system_share_config.clone()))
        } else if env.opt_config.is_file() {
            let parsed = Self::from_file(&env.opt_config)?;
            let parent = config_base_dir(&env.opt_config, &fallback_base);
            (parsed, parent, Some(env.opt_config.clone()))
        } else if let Some(ws_cfg) = env
            .workspace_base
            .as_ref()
            .map(|b| b.join("config.toml"))
            .filter(|p| p.is_file())
        {
            let parsed = Self::from_file(&ws_cfg)?;
            let parent = config_base_dir(&ws_cfg, &fallback_base);
            (parsed, parent, Some(ws_cfg))
        } else {
            (Self::default(), fallback_base, None)
        };

        cfg.resolve_paths(&base_dir);
        cfg.redirect_user_state_if_readonly(loaded_path.as_deref(), env)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn redirect_user_state_if_readonly(
        &mut self,
        loaded_config_path: Option<&Path>,
        env: &ConfigEnv,
    ) -> Result<(), CoreError> {
        let sys_share_dir = env.system_share_config.parent();
        let opt_dir = env.opt_config.parent();

        let points_to_system_share = is_system_share_path(loaded_config_path, sys_share_dir, opt_dir)
            || is_system_share_path(Some(&self.paths.kural_db), sys_share_dir, opt_dir)
            || is_system_share_path(Some(&self.paths.models_dir), sys_share_dir, opt_dir)
            || is_system_share_path(Some(&self.paths.user_state_db), sys_share_dir, opt_dir);

        let needs_redirect =
            points_to_system_share || !user_state_dir_is_writable(&self.paths.user_state_db);

        if needs_redirect {
            let xdg_dir = env.xdg_data_dir();
            fs::create_dir_all(&xdg_dir).map_err(|source| CoreError::Io {
                path: xdg_dir.clone(),
                source,
            })?;
            self.paths.user_state_db = xdg_dir.join("user_state.db");
        }
        Ok(())
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
            "desktop" | "pi_preview" | "pi7_preview" | "pi7_hd_preview" | "kiosk"
        ) {
            return Err(CoreError::ConfigValidation(format!(
                "ui.viewport_mode '{}' must be one of desktop | pi_preview | pi7_preview | pi7_hd_preview | kiosk",
                self.ui.viewport_mode
            )));
        }
        Ok(())
    }
}

fn is_system_share_path(
    path: Option<&Path>,
    sys_share_dir: Option<&Path>,
    opt_dir: Option<&Path>,
) -> bool {
    let Some(p) = path else {
        return false;
    };
    if p.starts_with("/usr/share/")
        || p.starts_with("/usr/local/share/")
        || p.starts_with("/opt/")
        || p == Path::new("/usr/share")
        || p == Path::new("/opt")
    {
        return true;
    }
    if let Some(sys) = sys_share_dir
        && p.starts_with(sys)
    {
        return true;
    }
    if let Some(opt) = opt_dir
        && p.starts_with(opt)
    {
        return true;
    }
    false
}

fn user_state_dir_is_writable(db_path: &Path) -> bool {
    let target_dir = db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Find the nearest existing ancestor directory and verify actual write access.
    let mut probe_dir = target_dir;
    while !probe_dir.exists() {
        let Some(parent) = probe_dir.parent().filter(|p| !p.as_os_str().is_empty()) else {
            probe_dir = Path::new(".");
            break;
        };
        probe_dir = parent;
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe = probe_dir.join(format!(
        ".kural_write_probe_{}_{nanos}",
        std::process::id()
    ));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
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

    #[test]
    fn config_precedence_checks_cli_kural_env_cwd_xdg_usr_share_and_opt() {
        let root = TempDir::new().expect("tempdir");
        let cwd_dir = root.path().join("cwd");
        let xdg_cfg = root.path().join("xdg_cfg");
        let home = root.path().join("home");
        let usr_dir = root.path().join("usr_share_app");
        let opt_dir = root.path().join("opt_app");
        let cli_cfg = root.path().join("cli.toml");
        let env_cfg = root.path().join("env.toml");

        for d in [
            &cwd_dir,
            &xdg_cfg.join("thiruvalluvar-ai"),
            &home.join(".config/thiruvalluvar-ai"),
            &usr_dir,
            &opt_dir,
        ] {
            fs::create_dir_all(d).expect("mkdir");
        }

        fs::write(&cli_cfg, "[audio]\nmaster_volume = 0.11\n").unwrap();
        fs::write(&env_cfg, "[audio]\nmaster_volume = 0.22\n").unwrap();
        let cwd_toml = cwd_dir.join("config.toml");
        fs::write(&cwd_toml, "[audio]\nmaster_volume = 0.33\n").unwrap();
        let xdg_toml = xdg_cfg.join("thiruvalluvar-ai/config.toml");
        fs::write(&xdg_toml, "[audio]\nmaster_volume = 0.44\n").unwrap();
        let home_xdg_toml = home.join(".config/thiruvalluvar-ai/config.toml");
        fs::write(&home_xdg_toml, "[audio]\nmaster_volume = 0.45\n").unwrap();
        let usr_toml = usr_dir.join("config.toml");
        fs::write(&usr_toml, "[audio]\nmaster_volume = 0.55\n").unwrap();
        let opt_toml = opt_dir.join("config.toml");
        fs::write(&opt_toml, "[audio]\nmaster_volume = 0.66\n").unwrap();

        let base_env = ConfigEnv {
            kural_config: Some(env_cfg.clone()),
            cwd_config: cwd_toml.clone(),
            xdg_config_home: Some(xdg_cfg.clone()),
            home: Some(home.clone()),
            xdg_data_home: Some(root.path().join("xdg_data")),
            system_share_config: usr_toml.clone(),
            opt_config: opt_toml.clone(),
            workspace_base: None,
            exe_dir: None,
        };

        // 1. CLI --config beats KURAL_CONFIG & everything else
        let c1 = AppConfig::load_with_env(Some(&cli_cfg), &base_env).unwrap();
        assert!((c1.audio.master_volume - 0.11).abs() < 1e-6);

        // 2. KURAL_CONFIG beats ./config.toml
        let c2 = AppConfig::load_with_env(None, &base_env).unwrap();
        assert!((c2.audio.master_volume - 0.22).abs() < 1e-6);

        // 3. ./config.toml beats XDG
        let mut e3 = base_env.clone();
        e3.kural_config = None;
        let c3 = AppConfig::load_with_env(None, &e3).unwrap();
        assert!((c3.audio.master_volume - 0.33).abs() < 1e-6);

        // 4. $XDG_CONFIG_HOME/thiruvalluvar-ai/config.toml beats ~/.config and /usr/share
        let mut e4 = e3.clone();
        e4.cwd_config = root.path().join("missing/config.toml");
        let c4 = AppConfig::load_with_env(None, &e4).unwrap();
        assert!((c4.audio.master_volume - 0.44).abs() < 1e-6);

        // 5. ~/.config/thiruvalluvar-ai/config.toml fallback when XDG_CONFIG_HOME is unset
        let mut e5 = e4.clone();
        e5.xdg_config_home = None;
        let c5 = AppConfig::load_with_env(None, &e5).unwrap();
        assert!((c5.audio.master_volume - 0.45).abs() < 1e-6);

        // 6. /usr/share/thiruvalluvar-ai/config.toml beats /opt
        let mut e6 = e5.clone();
        fs::remove_file(&home_xdg_toml).unwrap();
        let c6 = AppConfig::load_with_env(None, &e6).unwrap();
        assert!((c6.audio.master_volume - 0.55).abs() < 1e-6);

        // 7. /opt/thiruvalluvar-ai/config.toml fallback when /usr/share missing
        fs::remove_file(&usr_toml).unwrap();
        let c7 = AppConfig::load_with_env(None, &e6).unwrap();
        assert!((c7.audio.master_volume - 0.66).abs() < 1e-6);
        e6.home = None;
        let _ = e6;
    }

    #[test]
    fn system_share_and_readonly_dir_redirect_user_state_to_xdg_data_home() {
        let root = TempDir::new().expect("tempdir");
        let fake_home = root.path().join("fake_home");
        fs::create_dir_all(&fake_home).unwrap();

        // Case A: Config pointing to /usr/share/thiruvalluvar-ai/ (like config.package.toml)
        let pkg_cfg = root.path().join("config.package.toml");
        fs::write(
            &pkg_cfg,
            "[paths]\nkural_db = \"/usr/share/thiruvalluvar-ai/data/thirukkural.db\"\n\
             user_state_db = \"data/user_state.db\"\n\
             models_dir = \"/usr/share/thiruvalluvar-ai/models\"\n",
        )
        .unwrap();

        let env = ConfigEnv {
            kural_config: None,
            cwd_config: root.path().join("none.toml"),
            xdg_config_home: None,
            home: Some(fake_home.clone()),
            xdg_data_home: None,
            system_share_config: PathBuf::from("/usr/share/thiruvalluvar-ai/config.toml"),
            opt_config: PathBuf::from("/opt/thiruvalluvar-ai/config.toml"),
            workspace_base: None,
            exe_dir: None,
        };

        let loaded = AppConfig::load_with_env(Some(&pkg_cfg), &env).expect("pkg cfg loads");
        let expected_state = fake_home.join(".local/share/thiruvalluvar-ai/user_state.db");
        assert_eq!(loaded.paths.user_state_db, expected_state);
        assert!(expected_state.parent().unwrap().is_dir());

        // Case B: Custom read-only 0o555 directory redirects to XDG_DATA_HOME when set
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ro_dir = root.path().join("ro_bundle");
            let ro_data = ro_dir.join("data");
            fs::create_dir_all(&ro_data).unwrap();
            let ro_cfg = ro_dir.join("config.toml");
            fs::write(
                &ro_cfg,
                "[paths]\nkural_db = \"data/k.db\"\nuser_state_db = \"data/user_state.db\"\n",
            )
            .unwrap();
            fs::set_permissions(&ro_data, fs::Permissions::from_mode(0o555)).unwrap();

            let xdg_data = root.path().join("custom_xdg_data");
            let mut env_ro = env;
            env_ro.xdg_data_home = Some(xdg_data.clone());
            let loaded_ro = AppConfig::load_with_env(Some(&ro_cfg), &env_ro).expect("ro loads");
            assert_eq!(
                loaded_ro.paths.user_state_db,
                xdg_data.join("thiruvalluvar-ai/user_state.db")
            );
            // Restore perms so TempDir cleanup succeeds
            fs::set_permissions(&ro_data, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}


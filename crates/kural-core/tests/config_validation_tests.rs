use kural_core::{AppConfig, CoreError};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn load_honors_kural_config_env_var() {
    if std::env::var_os("KURAL_CONFIG_TEST_CHILD").is_some() {
        let expect_missing = std::env::var_os("KURAL_CONFIG_EXPECT_MISSING").is_some();
        if expect_missing {
            let err = AppConfig::load(None).expect_err("missing KURAL_CONFIG path must fail");
            assert!(
                matches!(err, CoreError::Io { .. }),
                "expected CoreError::Io for missing KURAL_CONFIG, got {err:?}"
            );
            return;
        }

        let cfg = AppConfig::load(None).expect("AppConfig::load(None) must honor KURAL_CONFIG");
        assert!((cfg.audio.master_volume - 0.42).abs() < 1e-6);
        assert_eq!(cfg.audio.voice_loop_max_silence_timeouts, 4);
        assert_eq!(cfg.ai.stt_timeout_ms, 12_345);
        assert_eq!(cfg.ai.tts_timeout_ms, 23_456);
        assert_eq!(cfg.ai.inference_timeout_ms, 6_789);
        assert_eq!(cfg.ui.viewport_mode, "pi7_preview");
        let expected_db = PathBuf::from(
            std::env::var_os("KURAL_EXPECTED_DB").expect("KURAL_EXPECTED_DB env var"),
        );
        assert_eq!(cfg.paths.kural_db, expected_db);
        return;
    }

    let dir = TempDir::new().expect("tempdir");
    let toml_path = dir.path().join("env_config.toml");
    fs::write(
        &toml_path,
        "[paths]\n\
         kural_db = \"custom/kural.db\"\n\
         [audio]\n\
         master_volume = 0.42\n\
         voice_loop_max_silence_timeouts = 4\n\
         [ai]\n\
         stt_timeout_ms = 12345\n\
         tts_timeout_ms = 23456\n\
         inference_timeout_ms = 6789\n\
         [ui]\n\
         viewport_mode = \"pi7_preview\"\n",
    )
    .expect("write temp config");

    let exe = std::env::current_exe().expect("current_exe");
    let ok_out = std::process::Command::new(&exe)
        .arg("--exact")
        .arg("load_honors_kural_config_env_var")
        .env("KURAL_CONFIG", &toml_path)
        .env("KURAL_CONFIG_TEST_CHILD", "1")
        .env("KURAL_EXPECTED_DB", dir.path().join("custom/kural.db"))
        .output()
        .expect("spawn child test process");
    assert!(
        ok_out.status.success(),
        "KURAL_CONFIG valid child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&ok_out.stdout),
        String::from_utf8_lossy(&ok_out.stderr)
    );

    let missing_path = dir.path().join("does_not_exist.toml");
    let err_out = std::process::Command::new(&exe)
        .arg("--exact")
        .arg("load_honors_kural_config_env_var")
        .env("KURAL_CONFIG", &missing_path)
        .env("KURAL_CONFIG_TEST_CHILD", "1")
        .env("KURAL_CONFIG_EXPECT_MISSING", "1")
        .output()
        .expect("spawn child missing-config test process");
    assert!(
        err_out.status.success(),
        "KURAL_CONFIG missing-file child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&err_out.stdout),
        String::from_utf8_lossy(&err_out.stderr)
    );
}

#[test]
fn validate_rejects_zero_timeouts_and_silence_limit_and_accepts_positive() {
    let mut cfg = AppConfig::default();
    assert!(cfg.validate().is_ok());

    let mut bad_stt = cfg.clone();
    bad_stt.ai.stt_timeout_ms = 0;
    assert!(matches!(
        bad_stt.validate(),
        Err(CoreError::ConfigValidation(_))
    ));

    let mut bad_tts = cfg.clone();
    bad_tts.ai.tts_timeout_ms = 0;
    assert!(matches!(
        bad_tts.validate(),
        Err(CoreError::ConfigValidation(_))
    ));

    let mut bad_inf = cfg.clone();
    bad_inf.ai.inference_timeout_ms = 0;
    assert!(matches!(
        bad_inf.validate(),
        Err(CoreError::ConfigValidation(_))
    ));

    let mut bad_silence = cfg.clone();
    bad_silence.audio.voice_loop_max_silence_timeouts = 0;
    assert!(matches!(
        bad_silence.validate(),
        Err(CoreError::ConfigValidation(_))
    ));

    // Boundary positive values (1) and non-trivial positive values must validate cleanly.
    cfg.ai.stt_timeout_ms = 1;
    cfg.ai.tts_timeout_ms = 1;
    cfg.ai.inference_timeout_ms = 1;
    cfg.audio.voice_loop_max_silence_timeouts = 1;
    assert!(cfg.validate().is_ok());

    cfg.ai.stt_timeout_ms = 8_000;
    cfg.ai.tts_timeout_ms = 15_000;
    cfg.ai.inference_timeout_ms = 5_000;
    cfg.audio.voice_loop_max_silence_timeouts = 3;
    assert!(cfg.validate().is_ok());
}

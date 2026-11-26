use kural_ai::{AiError, SpeechEngine};
use kural_audio::PcmChunk;
use kural_core::{AiConfig, Language};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy)]
pub struct SpeechCapabilities {
    pub active_stt_loaded: bool,
    pub active_tts_loaded: bool,
    pub tts_neural_loaded: bool,
    pub tts_model_fingerprint: u64,
}

fn snapshot_caps(engine: &SpeechEngine) -> SpeechCapabilities {
    SpeechCapabilities {
        active_stt_loaded: engine.stt_loaded(),
        active_tts_loaded: engine.tts_loaded(),
        tts_neural_loaded: engine.tts_backend_name() == "sherpa-vits",
        tts_model_fingerprint: engine.active_tts_model_fingerprint(),
    }
}

pub type CollectedPcm = (Vec<i16>, u32);
pub type SpeakOutput = (SpeechCapabilities, Option<CollectedPcm>);

enum WorkerCmd {
    EnsureLanguage {
        lang: Language,
        seq: u64,
        reply: oneshot::Sender<Result<SpeechCapabilities, AiError>>,
    },
    Transcribe {
        samples_16k: Vec<f32>,
        lang: Language,
        reply: oneshot::Sender<Result<(String, SpeechCapabilities), AiError>>,
    },
    Speak {
        chunks: Vec<String>,
        lang: Language,
        speed: f32,
        cancel: CancellationToken,
        pcm_tx: mpsc::Sender<PcmChunk>,
        reply: oneshot::Sender<Result<SpeakOutput, AiError>>,
    },
    Shutdown,
}

/// Dedicated single-owner OS thread for [`SpeechEngine`] (`stt` + `tts` + drop-before-load).
#[derive(Clone)]
pub struct AiWorkerHandle {
    tx: std_mpsc::SyncSender<WorkerCmd>,
    shutting_down: Arc<AtomicBool>,
    ensure_lang_seq: Arc<AtomicU64>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
    stt_timeout: Duration,
    tts_timeout: Duration,
    inference_timeout: Duration,
}

impl AiWorkerHandle {
    /// Spawn `"kural-ai-worker"` and load the initial language STT/TTS pair synchronously.
    pub fn spawn(
        models_dir: &Path,
        initial_lang: Language,
        ai_cfg: &AiConfig,
    ) -> Result<(Self, SpeechCapabilities), AiError> {
        let (tx, rx) = std_mpsc::sync_channel::<WorkerCmd>(16);
        let (ready_tx, ready_rx) = std_mpsc::sync_channel::<Result<SpeechCapabilities, AiError>>(1);
        let shutting_down = Arc::new(AtomicBool::new(false));
        let worker_shutting_down = Arc::clone(&shutting_down);
        let ensure_lang_seq = Arc::new(AtomicU64::new(0));
        let worker_ensure_seq = Arc::clone(&ensure_lang_seq);
        let dir = models_dir.to_path_buf();
        let cfg = ai_cfg.clone();
        let stt_timeout = Duration::from_millis(ai_cfg.stt_timeout_ms);
        let tts_timeout = Duration::from_millis(ai_cfg.tts_timeout_ms);
        let inference_timeout = Duration::from_millis(ai_cfg.inference_timeout_ms);

        let join = thread::Builder::new()
            .name("kural-ai-worker".to_string())
            .spawn(move || {
                let mut engine = match SpeechEngine::new(&dir, initial_lang, &cfg) {
                    Ok(e) => e,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                let caps = snapshot_caps(&engine);
                if ready_tx.send(Ok(caps)).is_err() {
                    return;
                }

                while let Ok(cmd) = rx.recv() {
                    if worker_shutting_down.load(Ordering::Acquire) {
                        break;
                    }
                    match cmd {
                        WorkerCmd::EnsureLanguage { lang, seq, reply } => {
                            let res = if seq != worker_ensure_seq.load(Ordering::Acquire) {
                                Ok(snapshot_caps(&engine))
                            } else {
                                engine
                                    .ensure_language(lang)
                                    .map(|()| snapshot_caps(&engine))
                            };
                            let _ = reply.send(res);
                        }
                        WorkerCmd::Transcribe {
                            samples_16k,
                            lang,
                            reply,
                        } => {
                            let res = engine.transcribe_timed(&samples_16k, lang).map(|stt| {
                                tracing::debug!(
                                    %lang,
                                    stt_latency_ms = stt.latency_ms,
                                    chars = stt.text.len(),
                                    "ai_worker STT transcription finished"
                                );
                                (stt.text, snapshot_caps(&engine))
                            });
                            let _ = reply.send(res);
                        }
                        WorkerCmd::Speak {
                            chunks,
                            lang,
                            speed,
                            cancel,
                            pcm_tx,
                            reply,
                        } => {
                            let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
                            let synth_res = engine.synthesize_streaming_collecting_timed(
                                &refs, lang, speed, &cancel, &pcm_tx,
                            );
                            let caps = snapshot_caps(&engine);
                            let mapped = synth_res.map(|(collected, metrics)| {
                                tracing::debug!(
                                    %lang,
                                    tts_first_chunk_ms = metrics.first_chunk_ms,
                                    tts_total_ms = metrics.total_ms,
                                    "ai_worker TTS synthesis finished"
                                );
                                (caps, collected)
                            });
                            let _ = reply.send(mapped);
                        }
                        WorkerCmd::Shutdown => break,
                    }
                }
            })
            .map_err(|err| AiError::Inference(format!("spawning kural-ai-worker failed: {err}")))?;

        let caps = ready_rx.recv().map_err(|_| {
            AiError::Inference("kural-ai-worker exited during startup".to_string())
        })??;

        Ok((
            Self {
                tx,
                shutting_down,
                ensure_lang_seq,
                thread: Arc::new(Mutex::new(Some(join))),
                stt_timeout,
                tts_timeout,
                inference_timeout,
            },
            caps,
        ))
    }

    pub async fn ensure_language(&self, lang: Language) -> Result<SpeechCapabilities, AiError> {
        let load_timeout = self
            .stt_timeout
            .max(self.tts_timeout)
            .max(self.inference_timeout);
        let timeout_ms = load_timeout.as_millis() as u64;
        let seq = self.ensure_lang_seq.fetch_add(1, Ordering::AcqRel) + 1;
        let (reply, rx) = oneshot::channel();
        let tx = self.tx.clone();
        let fut = async move {
            let sent = tokio::task::spawn_blocking(move || {
                tx.send(WorkerCmd::EnsureLanguage { lang, seq, reply })
            })
            .await;
            if !matches!(sent, Ok(Ok(()))) {
                return Err(AiError::Inference(
                    "kural-ai-worker send failed".to_string(),
                ));
            }
            rx.await
                .map_err(|_| AiError::Inference("kural-ai-worker dropped reply".to_string()))?
        };
        tokio::time::timeout(load_timeout, fut)
            .await
            .map_err(|_| AiError::Timeout {
                operation: "ensure_language",
                timeout_ms,
            })?
    }

    pub async fn transcribe(
        &self,
        samples_16k: Vec<f32>,
        lang: Language,
    ) -> Result<(String, SpeechCapabilities), AiError> {
        let timeout_ms = self.stt_timeout.as_millis() as u64;
        let (reply, rx) = oneshot::channel();
        let tx = self.tx.clone();
        let fut = async move {
            let sent = tokio::task::spawn_blocking(move || {
                tx.send(WorkerCmd::Transcribe {
                    samples_16k,
                    lang,
                    reply,
                })
            })
            .await;
            if !matches!(sent, Ok(Ok(()))) {
                return Err(AiError::Inference(
                    "kural-ai-worker unavailable".to_string(),
                ));
            }
            rx.await
                .map_err(|_| AiError::Inference("kural-ai-worker dropped reply".to_string()))?
        };
        tokio::time::timeout(self.stt_timeout, fut)
            .await
            .map_err(|_| AiError::Timeout {
                operation: "stt_transcribe",
                timeout_ms,
            })?
    }

    pub async fn speak_collecting(
        &self,
        chunks: Vec<String>,
        lang: Language,
        speed: f32,
        cancel: CancellationToken,
        pcm_tx: mpsc::Sender<PcmChunk>,
    ) -> Result<SpeakOutput, AiError> {
        let timeout_ms = self.tts_timeout.as_millis() as u64;
        let cancel_on_timeout = cancel.clone();
        let (reply, rx) = oneshot::channel();
        let tx = self.tx.clone();
        let fut = async move {
            let sent = tokio::task::spawn_blocking(move || {
                tx.send(WorkerCmd::Speak {
                    chunks,
                    lang,
                    speed,
                    cancel,
                    pcm_tx,
                    reply,
                })
            })
            .await;
            if !matches!(sent, Ok(Ok(()))) {
                return Err(AiError::Inference(
                    "kural-ai-worker unavailable".to_string(),
                ));
            }
            rx.await
                .map_err(|_| AiError::Inference("kural-ai-worker dropped reply".to_string()))?
        };
        match tokio::time::timeout(self.tts_timeout, fut).await {
            Ok(res) => res,
            Err(_) => {
                cancel_on_timeout.cancel();
                Err(AiError::Timeout {
                    operation: "tts_synthesize",
                    timeout_ms,
                })
            }
        }
    }

    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        let _ = self.tx.try_send(WorkerCmd::Shutdown);
        if let Ok(mut guard) = self.thread.lock()
            && let Some(join) = guard.take()
        {
            let _ = join.join();
        }
    }
}

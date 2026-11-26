use kural_ai::{AiError, SpeechEngine};
use kural_audio::PcmChunk;
use kural_core::{AiConfig, Language};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy)]
pub struct SpeechCapabilities {
    pub active_stt_loaded: bool,
    pub active_tts_loaded: bool,
}

pub type CollectedPcm = (Vec<i16>, u32);
pub type SpeakOutput = (SpeechCapabilities, Option<CollectedPcm>);

enum WorkerCmd {
    EnsureLanguage {
        lang: Language,
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
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
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
        let dir = models_dir.to_path_buf();
        let cfg = ai_cfg.clone();

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
                let caps = SpeechCapabilities {
                    active_stt_loaded: engine.stt_loaded(),
                    active_tts_loaded: engine.tts_loaded(),
                };
                if ready_tx.send(Ok(caps)).is_err() {
                    return;
                }

                while let Ok(cmd) = rx.recv() {
                    if worker_shutting_down.load(Ordering::Acquire) {
                        break;
                    }
                    match cmd {
                        WorkerCmd::EnsureLanguage { lang, reply } => {
                            let res = engine.ensure_language(lang).map(|()| SpeechCapabilities {
                                active_stt_loaded: engine.stt_loaded(),
                                active_tts_loaded: engine.tts_loaded(),
                            });
                            let _ = reply.send(res);
                        }
                        WorkerCmd::Transcribe {
                            samples_16k,
                            lang,
                            reply,
                        } => {
                            let res = engine.transcribe(&samples_16k, lang).map(|txt| {
                                (
                                    txt,
                                    SpeechCapabilities {
                                        active_stt_loaded: engine.stt_loaded(),
                                        active_tts_loaded: engine.tts_loaded(),
                                    },
                                )
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
                            let synth_res = engine.synthesize_streaming_collecting(
                                &refs, lang, speed, &cancel, &pcm_tx,
                            );
                            let caps = SpeechCapabilities {
                                active_stt_loaded: engine.stt_loaded(),
                                active_tts_loaded: engine.tts_loaded(),
                            };
                            let _ = reply.send(synth_res.map(|collected| (caps, collected)));
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
                thread: Arc::new(Mutex::new(Some(join))),
            },
            caps,
        ))
    }

    pub async fn ensure_language(&self, lang: Language) -> Result<SpeechCapabilities, AiError> {
        let (reply, rx) = oneshot::channel();
        let tx = self.tx.clone();
        if tokio::task::spawn_blocking(move || tx.send(WorkerCmd::EnsureLanguage { lang, reply }))
            .await
            .is_err()
        {
            return Err(AiError::Inference(
                "kural-ai-worker send failed".to_string(),
            ));
        }
        rx.await
            .map_err(|_| AiError::Inference("kural-ai-worker dropped reply".to_string()))?
    }

    pub async fn transcribe(
        &self,
        samples_16k: Vec<f32>,
        lang: Language,
    ) -> Result<(String, SpeechCapabilities), AiError> {
        let (reply, rx) = oneshot::channel();
        let tx = self.tx.clone();
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
    }

    pub async fn speak_collecting(
        &self,
        chunks: Vec<String>,
        lang: Language,
        speed: f32,
        cancel: CancellationToken,
        pcm_tx: mpsc::Sender<PcmChunk>,
    ) -> Result<SpeakOutput, AiError> {
        let (reply, rx) = oneshot::channel();
        let tx = self.tx.clone();
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

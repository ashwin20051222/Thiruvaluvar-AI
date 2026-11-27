//! Optional Raspberry Pi hardware push-button that starts microphone voice capture.
//!
//! Wiring: a momentary push button between **BCM GPIO17 (physical Pin 11)** and **GND (Pin 6)**.
//! Never wire it to 5V. The internal pull-up is enabled, so the line idles high and a press pulls
//! it low (a falling edge).
//!
//! A press dispatches the same commands as the on-screen microphone button:
//! [`BackendCommand::StartVoiceCommand`] when idle, and [`BackendCommand::CancelVoice`] when a
//! capture is already in progress (clean toggle). Presses during transcription are ignored.
//!
//! The listener uses the Linux GPIO character device (`/dev/gpiochipN`) through the pure-Rust
//! `gpiocdev` crate (no `unsafe` in this crate). On machines without a Raspberry Pi header chip,
//! or without permission to open it, it logs once and does nothing.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use kural_core::{AudioState, BackendCommand};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Minimum gap between two accepted presses (software debounce / lockout).
pub(crate) const BUTTON_LOCKOUT: Duration = Duration::from_millis(250);
/// Kernel-side contact-bounce filter applied to the GPIO line when the chip supports it.
const KERNEL_DEBOUNCE: Duration = Duration::from_millis(10);
/// How often the listener thread wakes to check for a shutdown request.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Consecutive read failures tolerated before the listener gives up.
const MAX_CONSECUTIVE_ERRORS: u32 = 5;

/// What a debounced button press should do for the current audio state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButtonAction {
    StartVoice,
    CancelVoice,
    Ignore,
}

impl ButtonAction {
    fn command(self) -> Option<BackendCommand> {
        match self {
            Self::StartVoice => Some(BackendCommand::StartVoiceCommand),
            Self::CancelVoice => Some(BackendCommand::CancelVoice),
            Self::Ignore => None,
        }
    }
}

/// Map the live audio state onto the button action, mirroring the on-screen mic button.
pub(crate) fn action_for_audio_state(state: AudioState) -> ButtonAction {
    match state {
        AudioState::Listening | AudioState::CapturingSpeech => ButtonAction::CancelVoice,
        AudioState::Processing => ButtonAction::Ignore,
        AudioState::Idle | AudioState::PlayingTts | AudioState::Cooldown => {
            ButtonAction::StartVoice
        }
    }
}

/// Software debounce plus decision gate: a single physical press triggers exactly once.
#[derive(Debug)]
pub(crate) struct PressGate {
    lockout: Duration,
    last_accepted: Option<Instant>,
}

impl PressGate {
    pub(crate) fn new(lockout: Duration) -> Self {
        Self {
            lockout,
            last_accepted: None,
        }
    }

    /// Decide what to do for a press seen at `now`. Only accepted presses start the lockout,
    /// so an ignored press (for example while transcribing) never delays the next real one.
    pub(crate) fn on_press(&mut self, now: Instant, state: AudioState) -> ButtonAction {
        if let Some(last) = self.last_accepted
            && now.saturating_duration_since(last) < self.lockout
        {
            return ButtonAction::Ignore;
        }
        let action = action_for_audio_state(state);
        if action != ButtonAction::Ignore {
            self.last_accepted = Some(now);
        }
        action
    }
}

/// Source of button edge events (the real GPIO line, or a fake in tests).
pub(crate) trait PressSource {
    /// Block up to `timeout`; `Ok(true)` means an edge event is ready to read.
    fn wait(&mut self, timeout: Duration) -> Result<bool, String>;
    /// Read one edge event; `Ok(true)` when it is a press (falling edge to GND).
    fn read_is_press(&mut self) -> Result<bool, String>;
}

/// Run the press loop until `stop` is set or the source keeps failing.
pub(crate) fn run_press_loop<S, D, A>(
    source: &mut S,
    stop: &AtomicBool,
    gate: &mut PressGate,
    dispatch: &D,
    audio_state: &A,
) where
    S: PressSource,
    D: Fn(BackendCommand),
    A: Fn() -> AudioState,
{
    let mut consecutive_errors = 0_u32;
    while !stop.load(Ordering::Acquire) {
        let ready = match source.wait(POLL_INTERVAL) {
            Ok(ready) => ready,
            Err(err) => {
                consecutive_errors += 1;
                tracing::warn!(%err, "GPIO button wait failed");
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    tracing::warn!("GPIO button listener stopping after repeated errors");
                    return;
                }
                thread::sleep(Duration::from_millis(20));
                continue;
            }
        };
        if !ready {
            continue;
        }
        match source.read_is_press() {
            Ok(is_press) => {
                consecutive_errors = 0;
                if !is_press {
                    continue;
                }
                let action = gate.on_press(Instant::now(), audio_state());
                tracing::info!(?action, "GPIO push button pressed");
                if let Some(cmd) = action.command() {
                    dispatch(cmd);
                }
            }
            Err(err) => {
                consecutive_errors += 1;
                tracing::warn!(%err, "GPIO button read failed");
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    tracing::warn!("GPIO button listener stopping after repeated errors");
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Handle to the running listener thread; stops and joins it on [`GpioButtonListener::stop`].
pub(crate) struct GpioButtonListener {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl GpioButtonListener {
    /// Ask the listener to stop and wait for the thread (bounded by the poll interval).
    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.thread.take() {
            let _ = join.join();
        }
    }
}

impl Drop for GpioButtonListener {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether a GPIO chip label identifies the Raspberry Pi 40-pin header controller.
///
/// Pi 5 exposes the header through the RP1 chip; earlier boards use the BCM2835/BCM2711 pin
/// controller. The Pi 5 SoC chip (`pinctrl-bcm2712`) is deliberately not matched because it does
/// not drive the header.
pub(crate) fn is_raspberry_pi_header_chip(label: &str) -> bool {
    matches!(
        label.trim(),
        "pinctrl-rp1" | "pinctrl-bcm2711" | "pinctrl-bcm2835"
    )
}

/// Start the button listener if enabled and a Raspberry Pi header GPIO chip is available.
///
/// `dispatch` sends commands to the backend and `audio_state` reads the live audio state.
/// Returns `None` (after logging) on every non-Pi or unavailable-GPIO situation.
pub(crate) fn spawn_listener<D, A>(
    enabled: bool,
    pin: u16,
    dispatch: D,
    audio_state: A,
) -> Option<GpioButtonListener>
where
    D: Fn(BackendCommand) + Send + 'static,
    A: Fn() -> AudioState + Send + 'static,
{
    if !enabled {
        tracing::debug!("GPIO push button disabled in config");
        return None;
    }
    spawn_hardware_listener(pin, dispatch, audio_state)
}

#[cfg(not(target_os = "linux"))]
fn spawn_hardware_listener<D, A>(
    _pin: u16,
    _dispatch: D,
    _audio_state: A,
) -> Option<GpioButtonListener>
where
    D: Fn(BackendCommand) + Send + 'static,
    A: Fn() -> AudioState + Send + 'static,
{
    tracing::debug!("GPIO push button unsupported on this OS; skipping");
    None
}

#[cfg(target_os = "linux")]
fn spawn_hardware_listener<D, A>(
    pin: u16,
    dispatch: D,
    audio_state: A,
) -> Option<GpioButtonListener>
where
    D: Fn(BackendCommand) + Send + 'static,
    A: Fn() -> AudioState + Send + 'static,
{
    let (mut source, chip_name) = match hardware::open(pin) {
        hardware::Opened::Armed { source, chip_name } => (source, chip_name),
        hardware::Opened::NoHeaderChip => {
            tracing::debug!("No Raspberry Pi GPIO header chip found; push button disabled");
            return None;
        }
        hardware::Opened::Unavailable(reason) => {
            tracing::warn!(pin, %reason, "GPIO push button unavailable; continuing without it");
            return None;
        }
    };
    tracing::info!(
        pin,
        chip = %chip_name,
        "GPIO push button armed (wire between BCM GPIO{pin} and GND)"
    );

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let spawned = thread::Builder::new()
        .name("kural-gpio-button".to_string())
        .spawn(move || {
            let mut gate = PressGate::new(BUTTON_LOCKOUT);
            run_press_loop(
                &mut source,
                &thread_stop,
                &mut gate,
                &dispatch,
                &audio_state,
            );
        });
    match spawned {
        Ok(join) => Some(GpioButtonListener {
            stop,
            thread: Some(join),
        }),
        Err(err) => {
            tracing::warn!(%err, "failed to spawn GPIO button thread");
            None
        }
    }
}

#[cfg(target_os = "linux")]
mod hardware {
    use super::{KERNEL_DEBOUNCE, PressSource, is_raspberry_pi_header_chip};
    use gpiocdev::Request;
    use gpiocdev::chip::Chip;
    use gpiocdev::line::{Bias, EdgeDetection, EdgeKind};
    use std::path::PathBuf;
    use std::time::Duration;

    pub(super) enum Opened {
        Armed {
            source: GpioSource,
            chip_name: String,
        },
        NoHeaderChip,
        Unavailable(String),
    }

    pub(super) struct GpioSource {
        request: Request,
    }

    impl PressSource for GpioSource {
        fn wait(&mut self, timeout: Duration) -> Result<bool, String> {
            self.request
                .wait_edge_event(timeout)
                .map_err(|e| e.to_string())
        }

        fn read_is_press(&mut self) -> Result<bool, String> {
            self.request
                .read_edge_event()
                .map(|ev| ev.kind == EdgeKind::Falling)
                .map_err(|e| e.to_string())
        }
    }

    struct HeaderChip {
        path: PathBuf,
        name: String,
        num_lines: u32,
    }

    /// Find the Raspberry Pi header chip by label, remembering whether a chip was unreadable.
    fn find_header_chip() -> Result<Option<HeaderChip>, String> {
        let paths = gpiocdev::chip::chips().map_err(|e| e.to_string())?;
        let mut denied = false;
        for path in paths {
            let chip = match Chip::from_path(&path) {
                Ok(chip) => chip,
                Err(err) => {
                    if let gpiocdev::Error::Os(errno) = &err
                        && errno.0 == 13
                    {
                        denied = true;
                    }
                    continue;
                }
            };
            let Ok(info) = chip.info() else {
                continue;
            };
            if is_raspberry_pi_header_chip(&info.label) {
                return Ok(Some(HeaderChip {
                    path,
                    name: info.name,
                    num_lines: info.num_lines,
                }));
            }
        }
        if denied {
            return Err(
                "permission denied opening /dev/gpiochip*; add the user to the `gpio` group"
                    .to_string(),
            );
        }
        Ok(None)
    }

    pub(super) fn open(pin: u16) -> Opened {
        let chip = match find_header_chip() {
            Ok(Some(chip)) => chip,
            Ok(None) => return Opened::NoHeaderChip,
            Err(reason) => return Opened::Unavailable(reason),
        };
        let offset = u32::from(pin);
        if offset >= chip.num_lines {
            return Opened::Unavailable(format!(
                "GPIO{pin} is out of range for {} ({} lines)",
                chip.name, chip.num_lines
            ));
        }

        let request_line = |with_kernel_debounce: bool| {
            let mut builder = Request::builder();
            builder
                .on_chip(chip.path.clone())
                .with_consumer("thiruvalluvar-ai-button")
                .with_line(offset)
                .as_input()
                .with_bias(Bias::PullUp)
                .with_edge_detection(EdgeDetection::FallingEdge);
            if with_kernel_debounce {
                builder.with_debounce_period(KERNEL_DEBOUNCE);
            }
            builder.request()
        };

        // Some GPIO drivers reject hardware debounce; the software lockout still protects us.
        let request = match request_line(true).or_else(|_| request_line(false)) {
            Ok(request) => request,
            Err(err) => {
                let reason = match &err {
                    gpiocdev::Error::Os(errno) if errno.0 == 16 => format!(
                        "GPIO{pin} is already in use by another driver (for example an LCD touch controller): {err}"
                    ),
                    other => other.to_string(),
                };
                return Opened::Unavailable(reason);
            }
        };
        Opened::Armed {
            source: GpioSource { request },
            chip_name: chip.name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[test]
    fn audio_state_maps_to_mic_button_behaviour() {
        for state in [
            AudioState::Idle,
            AudioState::Cooldown,
            AudioState::PlayingTts,
        ] {
            assert_eq!(action_for_audio_state(state), ButtonAction::StartVoice);
        }
        for state in [AudioState::Listening, AudioState::CapturingSpeech] {
            assert_eq!(action_for_audio_state(state), ButtonAction::CancelVoice);
        }
        assert_eq!(
            action_for_audio_state(AudioState::Processing),
            ButtonAction::Ignore
        );
    }

    #[test]
    fn single_press_triggers_once_and_bounce_inside_lockout_is_ignored() {
        let mut gate = PressGate::new(BUTTON_LOCKOUT);
        let t0 = Instant::now();
        assert_eq!(
            gate.on_press(t0, AudioState::Idle),
            ButtonAction::StartVoice
        );
        // Contact bounce / double event 40 ms and 249 ms later is swallowed.
        assert_eq!(
            gate.on_press(t0 + Duration::from_millis(40), AudioState::Idle),
            ButtonAction::Ignore
        );
        assert_eq!(
            gate.on_press(t0 + Duration::from_millis(249), AudioState::Idle),
            ButtonAction::Ignore
        );
        // A deliberate second press after the lockout toggles listening off.
        assert_eq!(
            gate.on_press(t0 + Duration::from_millis(250), AudioState::Listening),
            ButtonAction::CancelVoice
        );
    }

    #[test]
    fn ignored_processing_press_does_not_start_lockout() {
        let mut gate = PressGate::new(BUTTON_LOCKOUT);
        let t0 = Instant::now();
        assert_eq!(
            gate.on_press(t0, AudioState::Processing),
            ButtonAction::Ignore
        );
        assert_eq!(
            gate.on_press(t0 + Duration::from_millis(10), AudioState::Idle),
            ButtonAction::StartVoice
        );
    }

    #[test]
    fn only_raspberry_pi_header_chips_are_matched() {
        assert!(is_raspberry_pi_header_chip("pinctrl-rp1"));
        assert!(is_raspberry_pi_header_chip("pinctrl-bcm2711"));
        assert!(is_raspberry_pi_header_chip("pinctrl-bcm2835\n"));
        // Pi 5 SoC chip does not drive the header; desktop/laptop chips must never match.
        assert!(!is_raspberry_pi_header_chip("pinctrl-bcm2712"));
        assert!(!is_raspberry_pi_header_chip("INT34C5:00"));
        assert!(!is_raspberry_pi_header_chip("pinctrl-sunxi"));
        assert!(!is_raspberry_pi_header_chip(""));
    }

    enum Step {
        Idle,
        Edge(bool),
        WaitError,
        ReadError,
    }

    struct FakeSource {
        steps: VecDeque<Step>,
        pending_edge: Option<bool>,
        stop: Arc<AtomicBool>,
    }

    impl FakeSource {
        fn new(steps: Vec<Step>, stop: Arc<AtomicBool>) -> Self {
            Self {
                steps: steps.into(),
                pending_edge: None,
                stop,
            }
        }
    }

    impl PressSource for FakeSource {
        fn wait(&mut self, _timeout: Duration) -> Result<bool, String> {
            match self.steps.pop_front() {
                None => {
                    self.stop.store(true, Ordering::Release);
                    Ok(false)
                }
                Some(Step::Idle) => Ok(false),
                Some(Step::Edge(is_press)) => {
                    self.pending_edge = Some(is_press);
                    Ok(true)
                }
                Some(Step::WaitError) => Err("wait boom".to_string()),
                Some(Step::ReadError) => {
                    self.pending_edge = None;
                    Ok(true)
                }
            }
        }

        fn read_is_press(&mut self) -> Result<bool, String> {
            self.pending_edge
                .take()
                .ok_or_else(|| "read boom".to_string())
        }
    }

    fn drive(steps: Vec<Step>, state: AudioState) -> Vec<BackendCommand> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut source = FakeSource::new(steps, Arc::clone(&stop));
        let mut gate = PressGate::new(BUTTON_LOCKOUT);
        let sent: Mutex<Vec<BackendCommand>> = Mutex::new(Vec::new());
        run_press_loop(
            &mut source,
            &stop,
            &mut gate,
            &|cmd| sent.lock().expect("lock").push(cmd),
            &|| state,
        );
        sent.into_inner().expect("into_inner")
    }

    #[test]
    fn press_loop_dispatches_start_voice_command_for_each_isolated_press_only() {
        // Release edges are ignored; back-to-back presses collapse into one trigger.
        let sent = drive(
            vec![
                Step::Idle,
                Step::Edge(false),
                Step::Edge(true),
                Step::Edge(true),
                Step::Idle,
            ],
            AudioState::Idle,
        );
        assert_eq!(sent, vec![BackendCommand::StartVoiceCommand]);
    }

    #[test]
    fn press_loop_cancels_when_already_listening_and_ignores_during_processing() {
        assert_eq!(
            drive(vec![Step::Edge(true)], AudioState::Listening),
            vec![BackendCommand::CancelVoice]
        );
        assert!(drive(vec![Step::Edge(true)], AudioState::Processing).is_empty());
    }

    #[test]
    fn press_loop_gives_up_after_repeated_errors_without_dispatching() {
        let wait_errors = (0..MAX_CONSECUTIVE_ERRORS)
            .map(|_| Step::WaitError)
            .collect();
        assert!(drive(wait_errors, AudioState::Idle).is_empty());
        let read_errors = (0..MAX_CONSECUTIVE_ERRORS)
            .map(|_| Step::ReadError)
            .collect();
        assert!(drive(read_errors, AudioState::Idle).is_empty());
    }

    #[test]
    fn disabled_or_absent_gpio_is_a_clean_noop() {
        // Disabled in config: never touches hardware.
        assert!(spawn_listener(false, 17, |_| {}, || AudioState::Idle).is_none());
        // Enabled on a machine without a Raspberry Pi header chip: also a clean no-op.
        let listener = spawn_listener(true, 17, |_| {}, || AudioState::Idle);
        if let Some(mut listener) = listener {
            // Only possible on a real Pi; make sure it shuts down promptly.
            listener.stop();
        }
    }
}

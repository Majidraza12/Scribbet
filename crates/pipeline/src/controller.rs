//! Session controller: the state machine that owns the microphone.
//!
//! Single owner of "are we recording" (docs/06-security.md T1): the capture
//! session is created and dropped on the controller thread, so every surface
//! (tray, overlay, OS mic indicator) derives from one state. Commands arrive
//! over a channel from hotkey handlers / the UI; domain events leave over the
//! [`EventBus`].
//!
//! Threading: one dedicated OS thread. In `Idle` it blocks on the command
//! channel (zero CPU). While `Listening` it drains the audio ring and feeds
//! the [`Transcriber`] (STT decodes run inline here — this *is* the compute
//! thread from docs/02).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use od_audio::{CaptureConfig, CaptureSession};
use od_cleanup::Chain;
use od_core_types::{AppEvent, PipelineCtx, Segment, SegmentKind, SessionState};
use od_insertion::{FocusInfo, TextInserter};
use od_stt::SttEngine;

use crate::{EventBus, Transcriber, TranscriberConfig, TranscriberError};

/// Commands accepted by the controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCommand {
    /// Toggle listening (press-to-toggle hotkey).
    Toggle,
    /// Push-to-talk pressed: start listening.
    PttPressed,
    /// Push-to-talk released: finalize and stop.
    PttReleased,
    /// Exit the controller thread (app shutdown).
    Shutdown,
}

/// How the controller polls the ring while listening. 10 ms keeps added
/// latency negligible against the VAD hangover while staying far from busy
/// polling.
const LISTEN_POLL: Duration = Duration::from_millis(10);
/// Ring drain chunk: 100 ms of audio.
const CHUNK_SAMPLES: usize = 1600;

/// Handle to a running controller thread.
pub struct SessionHandle {
    commands: Sender<SessionCommand>,
    bus: Arc<EventBus>,
    /// Mirror of the live capture level for UI polling (f32 bits).
    level_bits: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SessionHandle {
    /// Sends a command; returns false if the controller thread is gone.
    pub fn send(&self, cmd: SessionCommand) -> bool {
        self.commands.send(cmd).is_ok()
    }

    /// Subscribes to the app event bus.
    pub fn subscribe(&self) -> Receiver<AppEvent> {
        self.bus.subscribe()
    }

    /// Current input level (0 when idle); safe to poll at UI frame rate.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level_bits.load(Ordering::Relaxed))
    }

    /// Requests shutdown and joins the controller thread.
    pub fn shutdown(mut self) {
        let _ = self.commands.send(SessionCommand::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Spawns the controller thread.
///
/// `make_engine` constructs the STT engine *on the controller thread* at
/// startup (models load once, off the UI thread, before the first
/// utterance). A failure is returned through the handle's bus as a log and
/// the thread exits; callers that need construction errors synchronously
/// should probe the model path first.
pub fn spawn<E, I, FE, FI>(
    capture: CaptureConfig,
    transcriber: TranscriberConfig,
    ctx: PipelineCtx,
    make_engine: FE,
    make_inserter: FI,
) -> SessionHandle
where
    E: SttEngine + Send + 'static,
    I: TextInserter + 'static,
    FE: FnOnce() -> Result<E, od_stt::SttError> + Send + 'static,
    FI: FnOnce() -> Option<I> + Send + 'static,
{
    let (tx, rx) = channel();
    let bus = Arc::new(EventBus::new());
    let level_bits = Arc::new(AtomicU32::new(0.0f32.to_bits()));

    let thread_bus = Arc::clone(&bus);
    let thread_level = Arc::clone(&level_bits);
    let thread = std::thread::Builder::new()
        .name("od-session-controller".into())
        .spawn(move || {
            let engine = match make_engine() {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("stt engine construction failed: {e}");
                    return;
                }
            };
            let t = match Transcriber::new(&transcriber, engine, ctx.clone()) {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("transcriber construction failed: {e}");
                    return;
                }
            };
            // Constructed on this thread on purpose: the Windows backend
            // holds apartment-threaded COM objects and is !Send.
            let inserter = make_inserter();
            if inserter.is_none() {
                tracing::warn!("no text inserter; running display-only");
            }
            let chain = Chain::from_ctx(&ctx);
            tracing::info!(
                profile = %ctx.profile.name,
                processors = ?chain.names(),
                "cleanup chain ready"
            );
            Controller {
                capture_config: capture,
                transcriber: t,
                chain,
                ctx,
                inserter,
                focus: None,
                bus: thread_bus,
                level_bits: thread_level,
                commands: rx,
                segments: Vec::new(),
                chunk: vec![0.0; CHUNK_SAMPLES],
            }
            .run();
        })
        .expect("spawn controller thread");

    SessionHandle {
        commands: tx,
        bus,
        level_bits,
        thread: Some(thread),
    }
}

struct Controller<E: SttEngine, I: TextInserter> {
    capture_config: CaptureConfig,
    transcriber: Transcriber<E>,
    /// Cleanup chain built from the active profile (M5); finals pass
    /// through it before insertion.
    chain: Chain,
    /// Active per-utterance context (profile snapshot, language, vocab).
    ctx: PipelineCtx,
    /// Insertion backend; `None` = display-only (tests, missing platform).
    inserter: Option<I>,
    /// Target captured at session start (hotkey press).
    focus: Option<FocusInfo>,
    bus: Arc<EventBus>,
    level_bits: Arc<AtomicU32>,
    commands: Receiver<SessionCommand>,
    /// Reusable segment scratch.
    segments: Vec<Segment>,
    /// Reusable ring-drain scratch.
    chunk: Vec<f32>,
}

impl<E: SttEngine, I: TextInserter> Controller<E, I> {
    fn run(mut self) {
        tracing::info!("session controller ready");
        loop {
            // Idle: block, zero CPU.
            match self.commands.recv() {
                Ok(SessionCommand::Toggle | SessionCommand::PttPressed) => {
                    if !self.listen() {
                        break; // shutdown requested while listening
                    }
                }
                Ok(SessionCommand::PttReleased) => {} // stale release; ignore
                Ok(SessionCommand::Shutdown) | Err(_) => break,
            }
        }
        tracing::info!("session controller exiting");
    }

    /// One listening session: open mic → stream → finalize → close mic.
    /// Returns false if shutdown was requested.
    fn listen(&mut self) -> bool {
        let t0 = Instant::now();
        // Capture the insertion target FIRST: this is the window the user
        // was in when they pressed the hotkey (docs/02 focus contract).
        self.focus = self
            .inserter
            .as_mut()
            .and_then(|ins| match ins.capture_focus() {
                Ok(f) => {
                    tracing::info!(app = %f.process, "insertion target captured");
                    Some(f)
                }
                Err(e) => {
                    tracing::warn!("focus capture failed ({e}); display-only session");
                    None
                }
            });
        let (session, mut consumer) = match CaptureSession::start(&self.capture_config) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("capture start failed: {e}");
                self.publish_state(SessionState::Idle);
                return true;
            }
        };
        let meter = session.meter();
        self.publish_state(SessionState::Listening);
        tracing::info!(
            hotkey_to_listening_ms = t0.elapsed().as_millis() as u64,
            device = session.device_name(),
            "listening"
        );

        let mut was_in_speech = false;
        let mut shutdown = false;
        loop {
            // Commands take priority over audio.
            match self.commands.recv_timeout(LISTEN_POLL) {
                Ok(SessionCommand::Toggle | SessionCommand::PttReleased) => break,
                Ok(SessionCommand::PttPressed) => {} // already listening
                Ok(SessionCommand::Shutdown) => {
                    shutdown = true;
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    shutdown = true;
                    break;
                }
            }

            self.level_bits
                .store(meter.level().to_bits(), Ordering::Relaxed);

            // Drain everything buffered, in chunks.
            loop {
                let n = consumer.pop_slice(&mut self.chunk);
                if n == 0 {
                    break;
                }
                let chunk = std::mem::take(&mut self.chunk);
                let fed = self.feed(&chunk[..n], was_in_speech);
                self.chunk = chunk;
                match fed {
                    Ok(in_speech) => was_in_speech = in_speech,
                    Err(e) => tracing::error!("transcription error: {e}"),
                }
            }

            if session.is_disconnected() {
                tracing::warn!("capture device disconnected; stopping session");
                break;
            }
        }

        // Finalize: drain the ring's tail, close any open utterance.
        self.publish_state(SessionState::Finalizing);
        loop {
            let n = consumer.pop_slice(&mut self.chunk);
            if n == 0 {
                break;
            }
            let chunk = std::mem::take(&mut self.chunk);
            let fed = self.feed(&chunk[..n], was_in_speech);
            self.chunk = chunk;
            if let Ok(in_speech) = fed {
                was_in_speech = in_speech;
            }
        }
        self.segments.clear();
        let mut segments = std::mem::take(&mut self.segments);
        if let Err(e) = self.transcriber.finish(&mut segments) {
            tracing::error!("finalize failed: {e}");
        }
        self.publish_segments(&mut segments);
        self.segments = segments;

        session.stop();
        self.level_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.publish_state(SessionState::Idle);
        !shutdown
    }

    /// Feeds one chunk; publishes segments and speech-boundary transitions.
    /// Returns the post-feed in-speech flag.
    fn feed(&mut self, samples: &[f32], was_in_speech: bool) -> Result<bool, TranscriberError> {
        self.segments.clear();
        let mut segments = std::mem::take(&mut self.segments);
        let result = self.transcriber.feed(samples, &mut segments);
        self.publish_segments(&mut segments);
        self.segments = segments;
        result?;

        let in_speech = self.transcriber.in_speech();
        if in_speech != was_in_speech {
            let at_ms = self.transcriber.stream_position_ms();
            self.bus.publish(&if in_speech {
                AppEvent::SpeechStarted { at_ms }
            } else {
                AppEvent::SpeechEnded { at_ms }
            });
        }
        Ok(in_speech)
    }

    fn publish_segments(&mut self, segments: &mut [Segment]) {
        for seg in segments {
            match seg.kind {
                SegmentKind::Partial => {
                    // Partials are display-only; they never pass through the
                    // cleanup chain (stable_len refinement is a later
                    // milestone — the overlay treats the whole partial as
                    // provisional).
                    self.bus.publish(&AppEvent::PartialUpdated {
                        segment_id: seg.id,
                        text: seg.text.clone(),
                        stable_len: 0,
                    });
                }
                SegmentKind::Final => {
                    // Finals: raw STT text → cleanup chain → insertion.
                    let raw = seg.text.clone();
                    self.chain.run(seg, &self.ctx);
                    self.bus.publish(&AppEvent::FinalReady {
                        segment_id: seg.id,
                        raw,
                        cleaned: seg.text.clone(),
                    });
                    self.insert_final(seg);
                }
            }
        }
    }

    /// Delivers a final (cleaned) segment into the captured target
    /// application. A trailing space separates consecutive segments unless
    /// the cleanup chain already ended the segment with layout whitespace
    /// (email newlines, meeting bullets).
    fn insert_final(&mut self, seg: &Segment) {
        let (Some(inserter), Some(focus)) = (self.inserter.as_mut(), self.focus.as_ref()) else {
            return;
        };
        if seg.text.is_empty() {
            return;
        }
        let text = if seg.text.ends_with(char::is_whitespace) {
            seg.text.clone()
        } else {
            format!("{} ", seg.text)
        };
        match inserter.insert(&text, focus) {
            Ok(outcome) => self.bus.publish(&AppEvent::Inserted {
                segment_id: seg.id,
                tier: outcome.tier.as_str().to_owned(),
                latency_ms: outcome.duration.as_millis() as u64,
            }),
            Err(e) => {
                tracing::error!("insertion failed: {e}");
                self.bus.publish(&AppEvent::InsertFailed {
                    segment_id: seg.id,
                    error: e.to_string(),
                });
            }
        }
    }

    fn publish_state(&self, state: SessionState) {
        tracing::info!(?state, "session state");
        self.bus.publish(&AppEvent::StateChanged { state });
    }
}

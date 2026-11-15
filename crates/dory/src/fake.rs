//! Fake engines and capture for lanes and demos. The engines come from `dory_core::testing`.

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dory_core::capture::FrameSink;
use dory_core::domain::Channel;
use dory_core::models::{DiarizationSupport, Llm, Stt};
use dory_core::ports::ModelProvider;
use dory_core::ports::capture::{
    Access, CaptureBackend, CaptureCaps, CaptureError, CaptureHandle, CaptureSpec, InputDevice,
    Source,
};
use dory_core::testing::{
    FakeLanguageModel, FakeLlmProvider, FakeSttProvider, FakeTranscriber, llm_entry, stt_entry,
};

use crate::Providers;

pub const FAKE_STT: &str = "fake/stt";
pub const FAKE_LLM: &str = "fake/llm";

const SUMMARY: &str = "## Fake summary\n- The fake engines wrote this.";
const CHUNK: Duration = Duration::from_millis(100);
const TONE_AMPLITUDE: f32 = 0.25;

pub(crate) fn providers() -> Providers {
    let mic = [
        (0, 900, 0, "Hello from the fake microphone."),
        (2_000, 2_800, 0, "Wrapping up."),
    ];
    let system = [(1_000, 1_900, 1, "Hello from the fake system audio.")];
    let mixed = [
        (0, 900, 1, "Hello from the fake mixed audio."),
        (1_000, 1_900, 2, "Second fake speaker."),
    ];
    let transcriber = Arc::new(
        FakeTranscriber::new()
            .with_segments(Channel::Mic, &mic)
            .with_segments(Channel::System, &system)
            .with_segments(Channel::Mixed, &mixed)
            .detecting("en"),
    );
    let stt: Arc<dyn ModelProvider<Stt>> = Arc::new(FakeSttProvider::new(
        "fake",
        vec![stt_entry(FAKE_STT, DiarizationSupport::Native)],
        transcriber,
    ));
    let llm: Arc<dyn ModelProvider<Llm>> = Arc::new(FakeLlmProvider::new(
        "fake",
        vec![llm_entry(FAKE_LLM)],
        Arc::new(FakeLanguageModel::new(SUMMARY)),
    ));
    Providers {
        stt: vec![stt],
        llm: vec![llm],
    }
}

/// A capture backend that plays a tone on every channel in real time until it is stopped. It
/// stands in for `dory_core::testing::FakeCapture`, whose script thread ignores `stop`, so a
/// pause, an undo, or a stop mid-clip would block until the script ended.
pub(crate) struct ToneCapture {
    rate_hz: u32,
}

impl ToneCapture {
    pub fn new(rate_hz: u32) -> Self {
        Self { rate_hz }
    }
}

impl CaptureBackend for ToneCapture {
    fn probe(&self) -> CaptureCaps {
        CaptureCaps {
            microphone: Access::Granted,
            system_audio: Access::Granted,
            unsupported: None,
        }
    }

    fn input_devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
        Ok(vec![InputDevice {
            uid: "fake-mic".into(),
            name: "Fake Microphone".into(),
            is_default: true,
        }])
    }

    fn request_access(&self, _: Source) -> Result<Access, CaptureError> {
        Ok(Access::Granted)
    }

    fn start(
        &self,
        spec: &CaptureSpec,
        mut sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        let channels = spec.layout.channel_count();
        let rate_hz = self.rate_hz;
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let chunk_frames = u64::from(rate_hz / 10);
            let mut frames = 0u64;
            let mut next = Instant::now();
            loop {
                let mut samples = Vec::with_capacity(chunk_frames as usize * channels);
                for frame in frames..frames + chunk_frames {
                    let at = frame as f32 / rate_hz as f32;
                    for channel in 0..channels {
                        let hz = 440.0 + 110.0 * channel as f32;
                        samples.push(TONE_AMPLITUDE * (std::f32::consts::TAU * hz * at).sin());
                    }
                }
                sink.push(&samples);
                frames += chunk_frames;
                next += CHUNK;
                match stopped.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        Ok(Box::new(ToneHandle { stop, thread }))
    }
}

struct ToneHandle {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

impl CaptureHandle for ToneHandle {
    fn stop(self: Box<Self>) {
        let _ = self.stop.send(());
        let _ = self.thread.join();
    }
}

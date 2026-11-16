//! `CoreAudioCapture`: a private global process tap plus the chosen mic in one private aggregate
//! device, read by one IOProc that pushes `[mic, system]` frames into the core's `FrameSink`.
//! See "The capture contract" in `docs/architecture/hexagonal.md`.

mod dsp;
pub(crate) mod hal;
mod io;
mod permission;
mod rebuild;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dory_core::capture::{FrameSink, IssueReporter};
use dory_core::domain::Channel;
use dory_core::ports::capture::{
    Access, CaptureBackend, CaptureCaps, CaptureError, CaptureHandle, CaptureIssue, CaptureSpec,
    InputDevice, MicSelection, Source,
};
use objc2_core_audio::{
    AudioGetHostClockFrequency, AudioObjectID, AudioObjectPropertySelector,
    kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyDeviceIsAlive,
    kAudioDevicePropertyNominalSampleRate, kAudioHardwarePropertyDefaultInputDevice,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices,
};
use objc2_foundation::NSProcessInfo;

use dsp::{LinearResampler, Role, Segment, SilenceAlarm, Watchdog};
use hal::{GLOBAL, HalError, INPUT, IoProc, Listeners, SubDevice, Tap};
use io::{Clocks, IoCell, IoState, Shared, Span, Timeline};
use rebuild::{Rebuilder, Verdict};

pub use hal::PcmFormat;

/// UID prefix of the private aggregates this adapter creates, so device lists can skip them.
const AGGREGATE_UID_PREFIX: &str = "dory.capture.";
const SILENCE_SECONDS: f64 = 3.0;
const MONITOR_PERIOD: Duration = Duration::from_millis(250);
/// How long a started IOProc may go without a cycle before the graph is rebuilt.
const STALL_LIMIT: Duration = Duration::from_secs(2);
/// How long a rebuilt graph may take to run its first cycle before the attempt counts as failed.
const FIRST_CYCLE_LIMIT: Duration = Duration::from_secs(1);

/// Set once this process has seen non-zero system audio, which proves System Audio Recording is granted.
static SYSTEM_AUDIO_HEARD: AtomicBool = AtomicBool::new(false);

/// Captures the mic and system audio through one private aggregate device.
///
/// A supervisor thread watches the devices the graph is built from. When the default output
/// changes, the default input changes while following it, a device changes rate or dies, a
/// pinned mic comes back, or the IOProc stalls, it tears the graph down and builds a new one
/// against the current devices. The same `FrameSink` moves into the new graph, whose first cycle
/// fills the gap with silence and reports [`CaptureIssue::Rebuilt`], so the timeline stays
/// continuous. Rebuilds that fail are retried for 5 s, then reported as [`CaptureIssue::Fatal`].
#[derive(Default)]
pub struct CoreAudioCapture;

impl CoreAudioCapture {
    pub fn new() -> Self {
        Self
    }

    /// Same as [`CaptureBackend::start`], with the concrete handle so callers can read the graph.
    pub fn start_capture(
        &self,
        spec: &CaptureSpec,
        sink: FrameSink,
    ) -> Result<CoreAudioHandle, CaptureError> {
        if let Some(reason) = unsupported_reason() {
            return Err(CaptureError::Unsupported(reason));
        }
        let wants_mic = spec.layout.index_of(Channel::Mic).is_some();
        let wants_system = spec.layout.index_of(Channel::System).is_some();
        if !wants_mic && !wants_system {
            return Err(CaptureError::Unsupported(format!(
                "{:?} is not a capture layout",
                spec.layout
            )));
        }
        if wants_mic && permission::microphone() == Access::Denied {
            return Err(CaptureError::AccessDenied(Source::Microphone));
        }

        let (events, inbox) = mpsc::channel();
        let ctx = Context {
            spec: spec.clone(),
            wants_mic,
            wants_system,
            own: hal::own_process_object().map_err(platform)?,
            // SAFETY: a plain query with no arguments.
            host_hz: unsafe { AudioGetHostClockFrequency() },
            events: events.clone(),
            reporter: sink.issue_reporter(),
        };
        let span = Arc::new(Span::default());
        let timeline = Timeline::new(sink, Arc::clone(&span));
        let graph = Graph::build(&ctx, timeline, 0, Strict::Yes).map_err(|f| f.error)?;
        let info = Arc::new(Mutex::new(graph.info.clone()));
        let clock = TimelineClock {
            span,
            host_hz: ctx.host_hz,
        };
        let supervisor = Supervisor {
            last_mic: graph.mic,
            ctx,
            inbox,
            generation: 0,
            rebuilder: Rebuilder::new(rebuild::POLICY),
            epoch: Instant::now(),
            graph_started: Duration::ZERO,
            watchdog: Watchdog::new(STALL_LIMIT),
            alarm: SilenceAlarm::new(graph.info.device_rate_hz, SILENCE_SECONDS),
            info: Arc::clone(&info),
        };
        let thread = std::thread::Builder::new()
            .name("dory-capture-supervisor".into())
            .spawn(move || supervisor.run(Live::Running(Box::new(graph))))
            .map_err(|e| platform_msg(format!("spawn capture supervisor: {e}")))?;
        Ok(CoreAudioHandle {
            events,
            thread: Some(thread),
            info,
            clock,
        })
    }
}

impl CaptureBackend for CoreAudioCapture {
    fn probe(&self) -> CaptureCaps {
        let caps = CaptureCaps {
            microphone: permission::microphone(),
            system_audio: system_audio_access(),
            unsupported: unsupported_reason(),
        };
        log_access_change(&caps);
        caps
    }

    fn input_devices(&self) -> Result<Vec<InputDevice>, CaptureError> {
        let default = hal::default_device(true).map_err(platform)?;
        let mut devices = Vec::new();
        for id in hal::all_devices().map_err(platform)? {
            let Ok(uid) = hal::device_uid(id) else {
                continue;
            };
            let has_input =
                hal::buffer_shapes(id, INPUT).is_ok_and(|b| b.iter().any(|s| s.channels > 0));
            if !has_input || uid.starts_with(AGGREGATE_UID_PREFIX) {
                continue;
            }
            let name = hal::object_name(id).unwrap_or_else(|_| uid.clone());
            devices.push(InputDevice {
                uid,
                name,
                is_default: Some(id) == default,
            });
        }
        Ok(devices)
    }

    fn request_access(&self, source: Source) -> Result<Access, CaptureError> {
        Ok(match source {
            Source::Microphone => permission::request_microphone(),
            Source::SystemAudio => {
                permission::request_system_audio().map_or_else(system_audio_access, heard_or)
            }
        })
    }

    fn start(
        &self,
        spec: &CaptureSpec,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        Ok(Box::new(self.start_capture(spec, sink)?))
    }
}

/// What a running capture is built from, for diagnostics.
#[derive(Clone, Debug)]
pub struct CaptureGraph {
    /// 0 for the graph `start` built, then one more per rebuild.
    pub generation: u64,
    pub aggregate_id: u32,
    pub aggregate_uid: String,
    pub tap_id: Option<u32>,
    pub tap_format: Option<PcmFormat>,
    /// The process object the tap leaves out: this process, so Dory never records its own playback.
    pub excluded_process: Option<u32>,
    pub clock_device: String,
    pub mic_device: Option<String>,
    pub device_rate_hz: f64,
    pub output_rate_hz: u32,
    pub buffer_frames: u32,
    /// Channel count of each buffer in the aggregate's input list.
    pub input_buffers: Vec<usize>,
    pub mic_channels: usize,
    pub system_channels: usize,
}

/// Host-time length of the frames pushed so far, rebuild gaps included.
#[derive(Clone, Debug)]
pub struct TimelineClock {
    span: Arc<Span>,
    host_hz: f64,
}

impl TimelineClock {
    /// Seconds from the first frame pushed to the end of the last one.
    pub fn seconds(&self) -> f64 {
        let first = self.span.first.load(Ordering::Relaxed);
        let end = self.span.end.load(Ordering::Relaxed);
        end.saturating_sub(first) as f64 / self.host_hz
    }

    /// Silent frames pushed in place of input the HAL skipped inside one graph.
    pub fn skipped_frames(&self) -> u64 {
        self.span.skipped.load(Ordering::Relaxed)
    }
}

/// A running capture. Dropping it, or calling `stop`, releases the hardware.
pub struct CoreAudioHandle {
    events: mpsc::Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    info: Arc<Mutex<CaptureGraph>>,
    clock: TimelineClock,
}

impl CoreAudioHandle {
    /// The graph running now. It changes on every rebuild.
    pub fn graph(&self) -> CaptureGraph {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn timeline_clock(&self) -> TimelineClock {
        self.clock.clone()
    }

    /// Rebuilds the graph against the current devices, as a device change would. For test
    /// harnesses: it exercises recovery without touching the user's devices.
    #[doc(hidden)]
    pub fn rebuild(&self) {
        let _ = self.events.send(Msg::Rebuild);
    }
}

impl CaptureHandle for CoreAudioHandle {
    fn stop(self: Box<Self>) {}
}

impl Drop for CoreAudioHandle {
    fn drop(&mut self) {
        let _ = self.events.send(Msg::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn platform(e: HalError) -> CaptureError {
    CaptureError::Platform {
        message: e.to_string(),
        source: Some(Box::new(e)),
    }
}

fn platform_msg(message: impl Into<String>) -> CaptureError {
    CaptureError::Platform {
        message: message.into(),
        source: None,
    }
}

fn unsupported_reason() -> Option<String> {
    let v = NSProcessInfo::processInfo().operatingSystemVersion();
    let ok = (v.majorVersion, v.minorVersion) >= (14, 2);
    (!ok).then(|| {
        format!(
            "Core Audio process taps need macOS 14.2 at minimum (the API floor), this is {}.{}",
            v.majorVersion, v.minorVersion
        )
    })
}

fn system_audio_access() -> Access {
    heard_or(permission::system_audio().unwrap_or(Access::Undetermined))
}

/// Logs the permissions when they differ from the last probe, so a grant made in System Settings
/// shows in the log when the app re-probes.
fn log_access_change(caps: &CaptureCaps) {
    static LAST: std::sync::Mutex<Option<(Access, Access)>> = std::sync::Mutex::new(None);
    let now = (caps.microphone, caps.system_audio);
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *last != Some(now) {
        *last = Some(now);
        eprintln!("capture: microphone {:?}, system audio {:?}", now.0, now.1);
    }
}

/// A tap that has heard audio this run proves the grant, whatever TCC says.
fn heard_or(access: Access) -> Access {
    if SYSTEM_AUDIO_HEARD.load(Ordering::Relaxed) {
        Access::Granted
    } else {
        access
    }
}

fn find_device(uid: &str) -> Result<Option<AudioObjectID>, HalError> {
    Ok(hal::all_devices()?
        .into_iter()
        .find(|id| hal::device_uid(*id).is_ok_and(|u| u == uid)))
}

/// The mic the selection points at now, or `None` when there is none.
fn resolve_mic(selection: &MicSelection) -> Result<Option<AudioObjectID>, CaptureError> {
    match selection {
        MicSelection::SystemDefault => hal::default_device(true).map_err(platform),
        MicSelection::Uid(uid) => find_device(uid).map_err(platform),
    }
}

/// What the supervisor thread hears.
enum Msg {
    /// A watched property left the value it had when graph `generation` was built.
    Changed {
        generation: u64,
    },
    /// Rebuild against the current devices, whatever they are.
    Rebuild,
    /// Stops the IOProc behind the capture's back, as a hung device would.
    #[cfg(test)]
    Stall,
    Stop,
}

/// What every graph of one capture is built from.
struct Context {
    spec: CaptureSpec,
    wants_mic: bool,
    wants_system: bool,
    own: AudioObjectID,
    host_hz: f64,
    events: mpsc::Sender<Msg>,
    reporter: IssueReporter,
}

/// `start` fails when the chosen mic is missing. A rebuild builds a mic-less graph instead, whose
/// mic channel is silence, and rebuilds again when the mic returns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Strict {
    Yes,
    No,
}

/// One device graph: tap, aggregate, IOProc, and the listeners that watch what it was built from.
/// Fields drop in order, so a graph dropped without `teardown` still unwinds safely.
struct Graph {
    info: CaptureGraph,
    mic: Option<AudioObjectID>,
    shared: Arc<Shared>,
    /// A watched property already differed from the value the graph was built against.
    stale: bool,
    listeners: Listeners,
    io: Io,
    aggregate: hal::Aggregate,
    tap: Option<Tap>,
}

impl Graph {
    fn build(
        ctx: &Context,
        timeline: Timeline,
        generation: u64,
        strict: Strict,
    ) -> Result<Graph, Box<Failed>> {
        match Parts::create(ctx, generation, strict) {
            Ok(parts) => parts.start(ctx, timeline),
            Err(error) => Err(Failed::new(timeline, error)),
        }
    }

    /// Stops the IOProc and destroys the aggregate and tap. Returns the timeline, sink included.
    fn teardown(self) -> (Timeline, Result<(), HalError>) {
        let Graph {
            mut listeners,
            mut io,
            aggregate,
            tap,
            ..
        } = self;
        listeners.remove_all();
        let (state, closed) = io.shutdown();
        drop(aggregate);
        drop(tap);
        let state = state.expect("only teardown closes a running graph's cell");
        (state.into_timeline(), closed)
    }

    /// Waits for the first IO cycle, so a graph that never runs counts as a failed attempt.
    fn await_first_cycle(self, limit: Duration) -> Result<Graph, Box<Failed>> {
        let started = Instant::now();
        while started.elapsed() < limit {
            if self.shared.cycles.load(Ordering::Relaxed) > 0 {
                return Ok(self);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let (timeline, _) = self.teardown();
        Err(Failed::new(
            timeline,
            platform_msg(format!(
                "no audio callback within {} ms of the rebuild",
                limit.as_millis()
            )),
        ))
    }
}

/// A build that failed. The timeline comes back so the sink survives for the next attempt.
struct Failed {
    timeline: Timeline,
    error: CaptureError,
}

impl Failed {
    fn new(timeline: Timeline, error: CaptureError) -> Box<Self> {
        Box::new(Self { timeline, error })
    }
}

/// A graph's HAL objects before the IOProc exists. Nothing here owns the sink yet, so a failure
/// leaves the timeline with the caller.
struct Parts {
    info: CaptureGraph,
    mic: Option<AudioObjectID>,
    map: dsp::InputMap,
    shapes: Vec<dsp::BufferShape>,
    stale: bool,
    listeners: Listeners,
    aggregate: hal::Aggregate,
    tap: Option<Tap>,
}

impl Parts {
    fn create(ctx: &Context, generation: u64, strict: Strict) -> Result<Parts, CaptureError> {
        let mic = if ctx.wants_mic {
            match resolve_mic(&ctx.spec.mic)? {
                Some(id) => Some(id),
                None if strict == Strict::No && ctx.wants_system => None,
                None => {
                    return Err(CaptureError::DeviceNotFound(match &ctx.spec.mic {
                        MicSelection::SystemDefault => "default input".into(),
                        MicSelection::Uid(uid) => uid.clone(),
                    }));
                }
            }
        } else {
            None
        };
        let output = if ctx.wants_system {
            Some(
                hal::default_device(false)
                    .map_err(platform)?
                    .ok_or_else(|| platform_msg("no default output device"))?,
            )
        } else {
            None
        };
        let tap = if ctx.wants_system {
            Some(Tap::create(ctx.own).map_err(platform_msg)?)
        } else {
            None
        };

        let plan = plan_graph(mic, output, tap.as_ref())?;
        let aggregate = hal::Aggregate::create(&plan.main_uid, &plan.subdevices, tap.as_ref())
            .map_err(platform)?;
        let shapes = hal::buffer_shapes(aggregate.id, INPUT).map_err(platform)?;
        let map =
            dsp::resolve_inputs(&shapes, &plan.parts).map_err(|e| platform_msg(e.to_string()))?;
        let device_rate: f64 =
            hal::get(aggregate.id, kAudioDevicePropertyNominalSampleRate, GLOBAL)
                .map_err(platform)?;
        let buffer_frames: u32 =
            hal::get(aggregate.id, kAudioDevicePropertyBufferFrameSize, GLOBAL)
                .map_err(platform)?;

        let mut listeners = Listeners::new();
        let watches = watches(&ctx.spec, aggregate.id, output, mic, device_rate);
        for watch in &watches {
            let events = ctx.events.clone();
            watch
                .clone()
                .listen(&mut listeners, move || {
                    let _ = events.send(Msg::Changed { generation });
                })
                .map_err(platform)?;
        }
        // A change between reading the devices and registering the listeners sends no
        // notification, so compare once now.
        let stale = watches
            .iter()
            .any(|w| !w.expect.holds(w.object, w.selector));

        let info = CaptureGraph {
            generation,
            aggregate_id: aggregate.id,
            aggregate_uid: aggregate.uid.clone(),
            tap_id: tap.as_ref().map(|t| t.id),
            tap_format: tap.as_ref().map(|t| t.format),
            excluded_process: tap.as_ref().map(|t| t.excluded),
            clock_device: plan.main_name,
            mic_device: plan.mic_name,
            device_rate_hz: device_rate,
            output_rate_hz: ctx.spec.rate_hz,
            buffer_frames,
            input_buffers: shapes.iter().map(|s| s.channels).collect(),
            mic_channels: map.mic.map_or(0, |m| m.width()),
            system_channels: map.system.map_or(0, |s| s.width()),
        };
        Ok(Parts {
            info,
            mic,
            map,
            shapes,
            stale,
            listeners,
            aggregate,
            tap,
        })
    }

    /// Moves `timeline` into a new IOProc on the aggregate and starts it. On failure the IOProc is
    /// closed and the timeline comes back. `self` is moved from field by field, so on an early
    /// return its remaining fields drop in declaration order: listeners, aggregate, tap.
    fn start(self, ctx: &Context, timeline: Timeline) -> Result<Graph, Box<Failed>> {
        let channels = ctx.spec.layout.channel_count();
        let device_hz = self.info.device_rate_hz;
        let out_hz = ctx.spec.rate_hz as f64;
        let resampler =
            (device_hz != out_hz).then(|| LinearResampler::new(channels, device_hz, out_hz));
        let shared = Arc::new(Shared::default());
        let clocks = Clocks {
            device_hz,
            out_hz,
            host_hz: ctx.host_hz,
        };
        let state = IoState::new(
            timeline,
            ctx.spec.layout,
            self.map,
            self.shapes,
            resampler,
            clocks,
            self.info.buffer_frames as usize,
            Arc::clone(&shared),
        );
        let cell = Arc::new(IoCell::new(state));
        let proc = match IoProc::create(self.aggregate.id, io::io_block(Arc::clone(&cell))) {
            Ok(proc) => proc,
            Err(e) => {
                let state = cell.close().expect("no IOProc ever saw this cell");
                return Err(Failed::new(state.into_timeline(), platform(e)));
            }
        };
        let mut io = Io { cell, proc };
        if let Err(e) = io.proc.disable_output().and_then(|()| io.proc.start()) {
            let (state, _) = io.shutdown();
            let state = state.expect("only this call closed the cell");
            return Err(Failed::new(state.into_timeline(), platform(e)));
        }
        Ok(Graph {
            info: self.info,
            mic: self.mic,
            shared,
            stale: self.stale,
            listeners: self.listeners,
            io,
            aggregate: self.aggregate,
            tap: self.tap,
        })
    }
}

/// The aggregate's composition and the input parts it should produce, in aggregate order:
/// sub-devices in list order, then the tap.
struct GraphPlan {
    main_uid: String,
    main_name: String,
    mic_name: Option<String>,
    subdevices: Vec<SubDevice>,
    parts: Vec<Segment>,
}

/// The output device only clocks the aggregate and is not a sub-device. Listing it as one made
/// `corespeechd` run output and input for the whole capture (observed on macOS 26.5), which tripped
/// the silence alarm. The mic is the only sub-device, drift-compensated unless it is the clock.
fn plan_graph(
    mic: Option<AudioObjectID>,
    output: Option<AudioObjectID>,
    tap: Option<&Tap>,
) -> Result<GraphPlan, CaptureError> {
    let describe = |id: AudioObjectID| -> Result<(String, String), CaptureError> {
        let uid = hal::device_uid(id).map_err(platform)?;
        let name = hal::object_name(id).unwrap_or_else(|_| uid.clone());
        Ok((uid, name))
    };
    let clock = output
        .or(mic)
        .ok_or_else(|| platform_msg("no device to capture from"))?;
    let (main_uid, main_name) = describe(clock)?;
    let mut plan = GraphPlan {
        main_uid,
        main_name,
        mic_name: None,
        subdevices: Vec::new(),
        parts: Vec::new(),
    };
    if let Some(mic) = mic {
        let (uid, name) = describe(mic)?;
        let inputs = hal::buffer_shapes(mic, INPUT).map_err(platform)?;
        plan.subdevices.push(SubDevice {
            uid,
            drift_compensation: mic != clock,
        });
        plan.parts.push(Segment {
            role: Role::Mic,
            buffers: inputs,
        });
        plan.mic_name = Some(name);
    }
    if let Some(tap) = tap {
        plan.parts.push(Segment {
            role: Role::System,
            buffers: tap.format.buffers(),
        });
    }
    Ok(plan)
}

/// A property whose change means the graph no longer matches the devices it should use.
#[derive(Clone)]
struct Watch {
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    expect: Expect,
}

/// The value a watched property had when the graph was built.
#[derive(Clone)]
enum Expect {
    DefaultDevice {
        input: bool,
        id: Option<AudioObjectID>,
    },
    Rate(f64),
    Alive,
    /// No device has this UID: the pinned mic is gone.
    Absent(String),
}

impl Expect {
    fn holds(&self, object: AudioObjectID, selector: AudioObjectPropertySelector) -> bool {
        match self {
            Expect::DefaultDevice { input, id } => {
                hal::default_device(*input).is_ok_and(|d| d == *id)
            }
            Expect::Rate(rate) => {
                hal::get::<f64>(object, selector, GLOBAL).is_ok_and(|r| r == *rate)
            }
            Expect::Alive => hal::get::<u32>(object, selector, GLOBAL).is_ok_and(|a| a != 0),
            Expect::Absent(uid) => find_device(uid).is_ok_and(|d| d.is_none()),
        }
    }
}

impl Watch {
    /// Calls `changed` once, from the listener queue, the first time the property leaves its
    /// expected value.
    fn listen(
        self,
        listeners: &mut Listeners,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), HalError> {
        let Watch {
            object,
            selector,
            expect,
        } = self;
        let fired = AtomicBool::new(false);
        listeners.add(object, selector, move || {
            if !fired.load(Ordering::Relaxed) && !expect.holds(object, selector) {
                fired.store(true, Ordering::Relaxed);
                changed();
            }
        })
    }
}

fn watches(
    spec: &CaptureSpec,
    aggregate: AudioObjectID,
    output: Option<AudioObjectID>,
    mic: Option<AudioObjectID>,
    device_rate: f64,
) -> Vec<Watch> {
    let watch = |object, selector, expect| Watch {
        object,
        selector,
        expect,
    };
    let mut list = vec![
        watch(
            aggregate,
            kAudioDevicePropertyNominalSampleRate,
            Expect::Rate(device_rate),
        ),
        watch(aggregate, kAudioDevicePropertyDeviceIsAlive, Expect::Alive),
    ];
    if let Some(id) = output {
        list.push(watch(
            hal::SYSTEM,
            kAudioHardwarePropertyDefaultOutputDevice,
            Expect::DefaultDevice {
                input: false,
                id: Some(id),
            },
        ));
    }
    let wants_mic = spec.layout.index_of(Channel::Mic).is_some();
    match (&spec.mic, mic) {
        (MicSelection::SystemDefault, _) if wants_mic => list.push(watch(
            hal::SYSTEM,
            kAudioHardwarePropertyDefaultInputDevice,
            Expect::DefaultDevice {
                input: true,
                id: mic,
            },
        )),
        (MicSelection::Uid(uid), None) if wants_mic => list.push(watch(
            hal::SYSTEM,
            kAudioHardwarePropertyDevices,
            Expect::Absent(uid.clone()),
        )),
        _ => {}
    }
    for device in [output, mic].into_iter().flatten() {
        if let Ok(rate) = hal::get::<f64>(device, kAudioDevicePropertyNominalSampleRate, GLOBAL) {
            list.push(watch(
                device,
                kAudioDevicePropertyNominalSampleRate,
                Expect::Rate(rate),
            ));
        }
        list.push(watch(
            device,
            kAudioDevicePropertyDeviceIsAlive,
            Expect::Alive,
        ));
    }
    list
}

/// The IOProc and the state it runs on. Shutting down closes the cell even if the HAL refuses to
/// destroy the IOProc, so a late callback finds nothing.
struct Io {
    cell: Arc<IoCell>,
    proc: IoProc,
}

impl Io {
    /// Stops the IOProc and takes the state back. The state is `None` only on a second call.
    fn shutdown(&mut self) -> (Option<IoState>, Result<(), HalError>) {
        let closed = self.proc.close();
        (self.cell.close(), closed)
    }
}

impl Drop for Io {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// The capture's graph: running, or down between a teardown and a successful rebuild. Either way
/// exactly one value owns the timeline and its sink.
enum Live {
    Running(Box<Graph>),
    Down(Timeline),
}

/// Owns the live graph on its own thread. Watches the IO counters (stall, silence), hears device
/// changes from the listener queue, and rebuilds when the [`Rebuilder`] says so.
struct Supervisor {
    ctx: Context,
    inbox: mpsc::Receiver<Msg>,
    /// The generation of the graph built last. Changes from older graphs are stale.
    generation: u64,
    rebuilder: Rebuilder,
    epoch: Instant,
    graph_started: Duration,
    watchdog: Watchdog,
    alarm: SilenceAlarm,
    /// The last mic a graph used, to tell a new default input from one that came back.
    last_mic: Option<AudioObjectID>,
    info: Arc<Mutex<CaptureGraph>>,
}

impl Supervisor {
    fn run(mut self, mut live: Live) {
        if let Live::Running(graph) = &live
            && graph.stale
        {
            self.rebuilder.trigger(self.now());
        }
        loop {
            let wait = self.rebuilder.deadline().map_or(MONITOR_PERIOD, |due| {
                due.saturating_sub(self.now()).min(MONITOR_PERIOD)
            });
            match self.inbox.recv_timeout(wait) {
                Ok(Msg::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(Msg::Changed { generation }) if generation == self.generation => {
                    self.rebuilder.trigger(self.now())
                }
                Ok(Msg::Changed { .. }) => {}
                Ok(Msg::Rebuild) => self.rebuilder.trigger(self.now()),
                #[cfg(test)]
                Ok(Msg::Stall) => {
                    if let Live::Running(graph) = &live {
                        graph.io.proc.stall();
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Live::Running(graph) = &live {
                self.monitor(graph);
            }
            if self.rebuilder.due(self.now()) {
                live = self.rebuild(live);
            }
        }
        if let Live::Running(graph) = live {
            let (mut timeline, closed) = graph.teardown();
            if let Err(e) = closed {
                timeline.report(CaptureIssue::Fatal(format!(
                    "teardown: {e}; the IOProc block was leaked"
                )));
            }
        }
    }

    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    /// Reports `SystemAudioSilent` while other processes play audio, and asks for a rebuild when
    /// the IOProc stops calling back.
    fn monitor(&mut self, graph: &Graph) {
        let cycles = graph.shared.cycles.load(Ordering::Relaxed);
        let since_start = self.now().saturating_sub(self.graph_started);
        if self.watchdog.update(cycles, since_start) {
            self.rebuilder.trigger(self.now());
        }
        if !self.ctx.wants_system {
            return;
        }
        let silent = graph.shared.silent_frames.load(Ordering::Relaxed);
        let playing = silent >= self.alarm.threshold_frames() && hal::others_playing(self.ctx.own);
        if self.alarm.update(silent, playing) {
            self.ctx.reporter.report(CaptureIssue::SystemAudioSilent);
        }
    }

    /// Tears the graph down and builds one against the current devices. On failure the timeline
    /// waits, down, for the retry the [`Rebuilder`] schedules.
    fn rebuild(&mut self, live: Live) -> Live {
        let mut timeline = match live {
            // A graph whose IOProc the HAL would not destroy is about to lose its aggregate, which
            // stops it for good, so its teardown error changes nothing here.
            Live::Running(graph) => graph.teardown().0,
            Live::Down(timeline) => timeline,
        };
        timeline.resume();
        self.generation += 1;
        let built = Graph::build(&self.ctx, timeline, self.generation, Strict::No)
            .and_then(|graph| graph.await_first_cycle(FIRST_CYCLE_LIMIT));
        match built {
            Ok(graph) => {
                self.rebuilder.record(self.now(), true);
                self.adopt(&graph);
                Live::Running(Box::new(graph))
            }
            Err(failed) => {
                let Failed {
                    mut timeline,
                    error,
                } = *failed;
                if self.rebuilder.record(self.now(), false) == Verdict::GiveUp {
                    timeline.report(CaptureIssue::Fatal(format!(
                        "could not rebuild the capture graph within {} s: {error}",
                        rebuild::POLICY.retry_for.as_secs(),
                    )));
                }
                Live::Down(timeline)
            }
        }
    }

    fn adopt(&mut self, graph: &Graph) {
        if let Some(name) = mic_changed(&self.ctx.spec.mic, self.last_mic, graph) {
            self.ctx.reporter.report(CaptureIssue::MicChanged { name });
        }
        self.last_mic = graph.mic.or(self.last_mic);
        self.graph_started = self.now();
        self.watchdog = Watchdog::new(STALL_LIMIT);
        self.alarm = SilenceAlarm::new(graph.info.device_rate_hz, SILENCE_SECONDS);
        *self.info.lock().unwrap_or_else(|e| e.into_inner()) = graph.info.clone();
        if graph.stale {
            self.rebuilder.trigger(self.now());
        }
    }
}

/// The name to report as `MicChanged`: only when following the default input and the new graph
/// uses a different mic than the last one that existed. A mic that comes back is `SourceRestored`.
fn mic_changed(
    selection: &MicSelection,
    last: Option<AudioObjectID>,
    graph: &Graph,
) -> Option<String> {
    let changed = new_default_mic(selection, last, graph.mic);
    changed.then(|| graph.info.mic_device.clone()).flatten()
}

fn new_default_mic(
    selection: &MicSelection,
    last: Option<AudioObjectID>,
    now: Option<AudioObjectID>,
) -> bool {
    matches!(selection, MicSelection::SystemDefault) && now.is_some() && now != last
}

#[cfg(test)]
mod tests {
    use super::*;
    use dory_core::capture::{FrameSource, frame_channel};
    use dory_core::domain::ChannelLayout;

    #[test]
    fn mic_changed_only_for_a_new_default_input() {
        let default = MicSelection::SystemDefault;
        let pinned = MicSelection::Uid("x".into());
        assert!(new_default_mic(&default, Some(1), Some(2)));
        assert!(
            !new_default_mic(&default, Some(1), Some(1)),
            "same mic, rebuilt"
        );
        assert!(
            !new_default_mic(&default, Some(1), None),
            "lost, not changed"
        );
        assert!(
            new_default_mic(&default, None, Some(1)),
            "a first default input appeared"
        );
        assert!(
            !new_default_mic(&pinned, Some(1), Some(2)),
            "a pinned mic never changes"
        );
    }

    struct Run {
        issues: Vec<CaptureIssue>,
        frames: u64,
        padded: u64,
        host_seconds: f64,
        skipped: u64,
        before: CaptureGraph,
        after: CaptureGraph,
    }

    /// Captures for `seconds`, calling `poke` on the handle at `poke_at`, then stops and drains.
    fn capture(
        layout: ChannelLayout,
        seconds: f64,
        poke_at: f64,
        poke: impl FnOnce(&CoreAudioHandle),
    ) -> Run {
        let spec = CaptureSpec {
            layout,
            rate_hz: 48_000,
            mic: MicSelection::SystemDefault,
        };
        let (sink, mut source): (_, FrameSource) = frame_channel(spec.layout, 48_000 * 20);
        let handle = CoreAudioCapture::new().start_capture(&spec, sink).unwrap();
        let before = handle.graph();
        let clock = handle.timeline_clock();
        std::thread::sleep(Duration::from_secs_f64(poke_at));
        poke(&handle);
        std::thread::sleep(Duration::from_secs_f64(seconds - poke_at));
        let after = handle.graph();
        let handle: Box<dyn CaptureHandle> = Box::new(handle);
        handle.stop();
        let mut out = Vec::new();
        let drained = source.drain(&mut out);
        assert!(source.is_finished(), "stop dropped the sink");
        Run {
            issues: source.issues().collect(),
            frames: drained.frames,
            padded: drained.padded_frames,
            host_seconds: clock.seconds(),
            skipped: clock.skipped_frames(),
            before,
            after,
        }
    }

    fn rebuilt_gaps(issues: &[CaptureIssue]) -> Vec<u64> {
        issues
            .iter()
            .filter_map(|i| match i {
                CaptureIssue::Rebuilt { gap_frames } => Some(*gap_frames),
                _ => None,
            })
            .collect()
    }

    fn assert_continuous(run: &Run) {
        let total = run.frames + run.padded;
        let host_frames = run.host_seconds * 48_000.0;
        let off = (total as f64 - host_frames).abs();
        eprintln!(
            "frames {} + padded {} = {total} (skipped input filled {}); host span {:.4} s = {host_frames:.0} frames; off by {off:.0}; buffer {}",
            run.frames, run.padded, run.skipped, run.host_seconds, run.after.buffer_frames
        );
        assert_eq!(run.padded, 0, "no ring overflow");
        assert!(
            off <= run.after.buffer_frames as f64,
            "frames out match host time within one buffer"
        );
        assert!(
            !run.issues
                .iter()
                .any(|i| matches!(i, CaptureIssue::Fatal(_))),
            "{:?}",
            run.issues
        );
    }

    #[test]
    #[ignore = "runs a capture through the Core Audio HAL"]
    fn an_internal_rebuild_keeps_one_continuous_timeline() {
        let run = capture(ChannelLayout::MicSystem, 3.0, 1.0, CoreAudioHandle::rebuild);
        eprintln!("issues {:?}", run.issues);
        let gaps = rebuilt_gaps(&run.issues);
        assert_eq!(gaps.len(), 1, "{:?}", run.issues);
        assert!(gaps[0] < 48_000, "gap {} frames", gaps[0]);
        assert_eq!((run.before.generation, run.after.generation), (0, 1));
        assert_ne!(run.before.aggregate_id, run.after.aggregate_id);
        assert_eq!(run.before.mic_device, run.after.mic_device);
        assert!(
            !run.issues
                .iter()
                .any(|i| matches!(i, CaptureIssue::MicChanged { .. })),
            "same default input, no MicChanged"
        );
        assert_continuous(&run);
    }

    #[test]
    #[ignore = "runs a capture through the Core Audio HAL"]
    fn a_stalled_ioproc_is_rebuilt_and_the_stall_is_filled() {
        let stall = |h: &CoreAudioHandle| {
            h.events.send(Msg::Stall).unwrap();
        };
        let run = capture(ChannelLayout::SystemOnly, 5.0, 0.5, stall);
        eprintln!("issues {:?}", run.issues);
        let gaps = rebuilt_gaps(&run.issues);
        assert_eq!(gaps.len(), 1, "{:?}", run.issues);
        let stalled = gaps[0] as f64 / 48_000.0;
        assert!(
            (STALL_LIMIT.as_secs_f64()..STALL_LIMIT.as_secs_f64() + 1.0).contains(&stalled),
            "the gap covers the stall: {stalled:.3} s"
        );
        assert_eq!(run.after.generation, 1);
        assert_continuous(&run);
    }
}

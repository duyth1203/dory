//! The C ABI of `swift/DoryAI.swift` and `swift/DorySpeech.swift`. Every `unsafe` line of the
//! Swift bridge lives in this file.

use std::any::Any;
use std::ffi::{CStr, c_char, c_void};
use std::marker::{PhantomData, PhantomPinned};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::ptr::{NonNull, null_mut};

pub(super) const OK: i32 = 0;
pub(super) const CANCELLED: i32 = 1;
pub(super) const INVALID_ARGUMENT: i32 = 2;
pub(super) const EXCEEDED_CONTEXT_WINDOW_SIZE: i32 = 10;
pub(super) const ASSETS_UNAVAILABLE: i32 = 11;
pub(super) const GUARDRAIL_VIOLATION: i32 = 12;
pub(super) const UNSUPPORTED_GUIDE: i32 = 13;
pub(super) const UNSUPPORTED_LANGUAGE_OR_LOCALE: i32 = 14;
pub(super) const DECODING_FAILURE: i32 = 15;
pub(super) const RATE_LIMITED: i32 = 16;
pub(super) const CONCURRENT_REQUESTS: i32 = 17;
pub(super) const REFUSAL: i32 = 18;
pub(super) const OTHER_GENERATION_ERROR: i32 = 19;
pub(super) const SPEECH_ASSETS_MISSING: i32 = 30;
pub(super) const SPEECH_UNSUPPORTED_LOCALE: i32 = 31;
pub(super) const SPEECH_RESERVATION_LIMIT: i32 = 32;
pub(super) const SPEECH_AUDIO_UNREADABLE: i32 = 33;
pub(super) const SPEECH_BUSY: i32 = 34;
pub(super) const SPEECH_NETWORK: i32 = 35;

pub(super) const FM_AVAILABLE: i32 = 0;
pub(super) const FM_NOT_ENABLED: i32 = 1;
pub(super) const FM_NOT_ELIGIBLE: i32 = 2;
pub(super) const FM_NOT_READY: i32 = 3;
pub(super) const FM_OS_TOO_OLD: i32 = 4;

pub(super) const ASSET_INSTALLED: i32 = 0;
pub(super) const ASSET_SUPPORTED: i32 = 1;
pub(super) const ASSET_DOWNLOADING: i32 = 2;

#[repr(C)]
struct DoryTask {
    _opaque: [u8; 0],
    _swift_owned: PhantomData<(*mut u8, PhantomPinned)>,
}

type OnPiece = extern "C" fn(ctx: *mut c_void, piece: *const c_char, len: usize) -> bool;
type OnSegment =
    extern "C" fn(ctx: *mut c_void, start_ms: u64, end_ms: u64, text: *const c_char) -> bool;
type OnProgress = extern "C" fn(ctx: *mut c_void, fraction: f64) -> bool;

unsafe extern "C" {
    fn dory_task_new() -> *mut DoryTask;
    fn dory_task_cancel(task: *mut DoryTask);
    fn dory_task_free(task: *mut DoryTask);
    fn dory_string_free(string: *mut c_char);
    fn dory_fm_info(
        state: *mut i32,
        context_size: *mut u32,
        languages_csv: *mut *mut c_char,
    ) -> i32;
    fn dory_fm_complete(
        task: *mut DoryTask,
        system: *const c_char,
        user: *const c_char,
        max_out: u32,
        temperature: f32,
        ctx: *mut c_void,
        on_piece: Option<OnPiece>,
        full: *mut *mut c_char,
        error: *mut *mut c_char,
    ) -> i32;
    fn dory_speech_locales(csv: *mut *mut c_char) -> i32;
    fn dory_speech_asset_state(locale: *const c_char) -> i32;
    fn dory_speech_install(
        task: *mut DoryTask,
        locale: *const c_char,
        ctx: *mut c_void,
        on_progress: Option<OnProgress>,
        error: *mut *mut c_char,
    ) -> i32;
    fn dory_speech_transcribe(
        task: *mut DoryTask,
        path: *const c_char,
        start_ms: u64,
        end_ms: u64,
        locale: *const c_char,
        ctx: *mut c_void,
        on_segment: Option<OnSegment>,
        on_progress: Option<OnProgress>,
        error: *mut *mut c_char,
    ) -> i32;
}

/// One Swift `DoryTask`, freed on drop. Clone it into an `Arc` to cancel from another thread.
pub(super) struct Task(NonNull<DoryTask>);

// SAFETY: the Swift `DoryTask` guards its state with an `NSLock`, so `cancel` may run on any thread
// while `dory_fm_complete` blocks another, and the handle stays valid until `Drop` frees it.
unsafe impl Send for Task {}
// SAFETY: as for `Send`, every method takes `&self` and the Swift side locks.
unsafe impl Sync for Task {}

impl Task {
    pub(super) fn new() -> Self {
        // SAFETY: `dory_task_new` takes no arguments and returns a retained, non-null handle.
        let raw = unsafe { dory_task_new() };
        Self(NonNull::new(raw).expect("dory_task_new never returns null"))
    }

    pub(super) fn cancel(&self) {
        // SAFETY: the handle is live until `Drop`, and Swift's `cancel` locks.
        unsafe { dory_task_cancel(self.0.as_ptr()) }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        // SAFETY: this is the only owner, so no call can use the handle after this release.
        unsafe { dory_task_free(self.0.as_ptr()) }
    }
}

/// Copies a Swift-allocated string and hands it back to Swift. Null reads as empty.
fn take_string(raw: *mut c_char) -> String {
    if raw.is_null() {
        return String::new();
    }
    // SAFETY: Swift returned a NUL-terminated `strdup` copy that only this call reads.
    let text = unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: `raw` came from Swift's `strdup` and is freed once, here.
    unsafe { dory_string_free(raw) };
    text
}

pub(super) struct RawInfo {
    pub state: i32,
    pub context_size: u32,
    pub languages_csv: String,
}

pub(super) fn fm_info() -> RawInfo {
    let mut state = -1;
    let mut context_size = 0;
    let mut languages = null_mut();
    // SAFETY: each out pointer is a live local, and Swift writes each once before returning.
    unsafe { dory_fm_info(&mut state, &mut context_size, &mut languages) };
    RawInfo {
        state,
        context_size,
        languages_csv: take_string(languages),
    }
}

pub(super) struct RawOutcome {
    pub code: i32,
    pub full: String,
    pub error: String,
}

/// The callback and any panic it raised, behind the `ctx` pointer Swift passes back.
struct PieceSink<'a> {
    on_piece: &'a mut dyn FnMut(&str) -> bool,
    panic: Option<Box<dyn Any + Send>>,
}

extern "C" fn on_piece_trampoline(ctx: *mut c_void, piece: *const c_char, len: usize) -> bool {
    // SAFETY: `ctx` is the `PieceSink` that `fm_complete` borrows for the whole blocking call, and
    // Swift calls back only on that same thread while it waits, so this is the only live reference.
    let sink = unsafe { &mut *ctx.cast::<PieceSink<'_>>() };
    if piece.is_null() || sink.panic.is_some() {
        return false;
    }
    // SAFETY: Swift passes `len` bytes of one Swift string, valid for the duration of this call.
    let bytes = unsafe { std::slice::from_raw_parts(piece.cast::<u8>(), len) };
    let text = String::from_utf8_lossy(bytes);
    guarded(&mut sink.panic, || (sink.on_piece)(&text))
}

/// Runs a Rust callback for Swift. A panic must not unwind into Swift, so it is kept for the
/// blocked caller to resume, and the call reports "stop".
fn guarded(panic: &mut Option<Box<dyn Any + Send>>, call: impl FnOnce() -> bool) -> bool {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(keep_going) => keep_going,
        Err(payload) => {
            *panic = Some(payload);
            false
        }
    }
}

/// Blocks until Swift finishes the completion. A panic in `on_piece` stops it and resumes here.
pub(super) fn fm_complete(
    task: &Task,
    system: &CStr,
    user: &CStr,
    max_out: u32,
    temperature: f32,
    on_piece: &mut dyn FnMut(&str) -> bool,
) -> RawOutcome {
    let mut sink = PieceSink {
        on_piece,
        panic: None,
    };
    let mut full = null_mut();
    let mut error = null_mut();
    // SAFETY: the task handle, both C strings, `sink`, and the out pointers all outlive this
    // blocking call, and Swift calls `on_piece_trampoline` only on this thread before it returns.
    let code = unsafe {
        dory_fm_complete(
            task.0.as_ptr(),
            system.as_ptr(),
            user.as_ptr(),
            max_out,
            temperature,
            (&raw mut sink).cast(),
            Some(on_piece_trampoline),
            &mut full,
            &mut error,
        )
    };
    let outcome = RawOutcome {
        code,
        full: take_string(full),
        error: take_string(error),
    };
    if let Some(payload) = sink.panic {
        resume_unwind(payload);
    }
    outcome
}

/// Every supported locale, comma-separated BCP 47 tags. Empty when the Mac has no SpeechTranscriber.
pub(super) fn speech_locales() -> String {
    let mut csv = null_mut();
    // SAFETY: the out pointer is a live local, and Swift writes it once before returning.
    unsafe { dory_speech_locales(&mut csv) };
    take_string(csv)
}

pub(super) fn speech_asset_state(locale: &CStr) -> i32 {
    // SAFETY: `locale` is a NUL-terminated string that outlives this blocking call.
    unsafe { dory_speech_asset_state(locale.as_ptr()) }
}

pub(super) struct RawStatus {
    pub code: i32,
    pub error: String,
}

/// What Swift reports during a speech call.
pub(super) enum SpeechEvent<'t> {
    /// Times are relative to the start of the range Rust asked for.
    Segment {
        start_ms: u64,
        end_ms: u64,
        text: &'t str,
    },
    Progress(f64),
}

/// The callback of one speech call and any panic it raised, behind the `ctx` pointer.
struct SpeechSink<'a> {
    on_event: &'a mut dyn FnMut(SpeechEvent<'_>) -> bool,
    panic: Option<Box<dyn Any + Send>>,
}

impl<'a> SpeechSink<'a> {
    /// Hands the sink to Swift for one blocking call, then resumes any panic the callback raised.
    fn run(
        on_event: &'a mut dyn FnMut(SpeechEvent<'_>) -> bool,
        call: impl FnOnce(*mut c_void) -> (i32, *mut c_char),
    ) -> RawStatus {
        let mut sink = SpeechSink {
            on_event,
            panic: None,
        };
        let (code, error) = call((&raw mut sink).cast());
        let status = RawStatus {
            code,
            error: take_string(error),
        };
        if let Some(payload) = sink.panic {
            resume_unwind(payload);
        }
        status
    }

    fn deliver(&mut self, event: SpeechEvent<'_>) -> bool {
        if self.panic.is_some() {
            return false;
        }
        guarded(&mut self.panic, || (self.on_event)(event))
    }
}

extern "C" fn on_segment_trampoline(
    ctx: *mut c_void,
    start_ms: u64,
    end_ms: u64,
    text: *const c_char,
) -> bool {
    // SAFETY: `ctx` is the `SpeechSink` that `SpeechSink::run` borrows for the whole blocking call,
    // and Swift calls back only on that same thread while it waits, so this is the only reference.
    let sink = unsafe { &mut *ctx.cast::<SpeechSink<'_>>() };
    if text.is_null() {
        return false;
    }
    // SAFETY: Swift passes a NUL-terminated string valid for the duration of this call.
    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    sink.deliver(SpeechEvent::Segment {
        start_ms,
        end_ms,
        text: &text,
    })
}

extern "C" fn on_progress_trampoline(ctx: *mut c_void, fraction: f64) -> bool {
    // SAFETY: as in `on_segment_trampoline`.
    let sink = unsafe { &mut *ctx.cast::<SpeechSink<'_>>() };
    sink.deliver(SpeechEvent::Progress(fraction))
}

/// Blocks until Apple has the locale's assets, the task is cancelled, or `on_event` returns false.
/// A panic in `on_event` stops it and resumes here.
pub(super) fn speech_install(
    task: &Task,
    locale: &CStr,
    on_event: &mut dyn FnMut(SpeechEvent<'_>) -> bool,
) -> RawStatus {
    SpeechSink::run(on_event, |ctx| {
        let mut error = null_mut();
        // SAFETY: the task handle, `locale`, the sink behind `ctx`, and `error` all outlive this
        // blocking call, and Swift calls the trampoline only on this thread before it returns.
        let code = unsafe {
            dory_speech_install(
                task.0.as_ptr(),
                locale.as_ptr(),
                ctx,
                Some(on_progress_trampoline),
                &mut error,
            )
        };
        (code, error)
    })
}

/// Blocks until Apple has transcribed `start_ms..end_ms` of the file, the task is cancelled, or
/// `on_event` returns false. A panic in `on_event` stops the call and resumes here.
pub(super) fn speech_transcribe(
    task: &Task,
    path: &CStr,
    (start_ms, end_ms): (u64, u64),
    locale: &CStr,
    on_event: &mut dyn FnMut(SpeechEvent<'_>) -> bool,
) -> RawStatus {
    SpeechSink::run(on_event, |ctx| {
        let mut error = null_mut();
        // SAFETY: the task handle, both strings, the sink behind `ctx`, and `error` all outlive
        // this blocking call, and Swift calls the trampolines only on this thread before it returns.
        let code = unsafe {
            dory_speech_transcribe(
                task.0.as_ptr(),
                path.as_ptr(),
                start_ms,
                end_ms,
                locale.as_ptr(),
                ctx,
                Some(on_segment_trampoline),
                Some(on_progress_trampoline),
                &mut error,
            )
        };
        (code, error)
    })
}

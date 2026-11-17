# Dory architecture: ports and adapters

This is the target shape of the Dory rebuild. The core holds every product rule as plain data and pure functions. Platforms, engines, storage, and UIs plug in as adapters. Swapping a speech model, a summary model, or the audio stack on a platform touches one adapter and one registration line, never the core.

It comes from a pstack architect arena. Two runners on different models designed it independently, a third model judged them, and this document is the synthesis. The base is candidate B, with grafts from candidate A. The full trail is in `docs/plans/decisions.tsv`.

## The rules the layout enforces

- `dory-core` depends on no platform crate, no async runtime, no HTTP client, no database, and no codec. It is `#![forbid(unsafe_code)]`.
- Only `dory` (the composition root) names adapters. Apps depend on `dory-core` for types and on `dory` for `open()`. An app that imports an adapter crate fails review.
- Each platform gets its own adapter crate. `dory-macos` is first and targets macOS 26. `dory-windows` and `dory-linux` come later and implement the same ports.
- Domain types carry no serde. Each storage adapter owns versioned DTOs, so a domain change cannot silently change a file format.
- Every persisted fact goes through one write, `Store::apply(Vec<Change>)`. Events come from the same `Change` list. The search index is derived from those facts, not a fact of its own. A store updates it inside the same commit as the text, and may rebuild it outside `apply` the way a schema migration rewrites tables (see "Search").

## Crates

| Crate | Role | Depends on |
| --- | --- | --- |
| `crates/dory-core` | Domain types, ports, pure policy, the `Dory` facade, actors, fakes | `thiserror`, `crossbeam-channel`, `async-channel`, `rtrb`, `unicode-normalization` |
| `crates/dory-store` | `Store` over SQLite, `Takes` over the file system, legacy import | core, `rusqlite`, `hound`, `flacenc`, `claxon`, `rubato` |
| `crates/dory-engines` | Portable model providers: Sona, Ollama, sherpa-onnx | core, `ureq`, `sha2`, optional `sherpa-rs` |
| `crates/dory-macos` | macOS 26 adapters: Core Audio capture, Foundation Models, and SpeechAnalyzer next. Its `dory-capture` example is the capture harness, which drives the adapter directly. | core, `objc2`, `objc2-core-audio`, `objc2-core-foundation`, `objc2-avf-audio`, one Swift static library behind the `apple-intelligence` feature |
| `crates/dory` | Composition root: `dory::open(Config) -> Dory` and `dory::open_library(Config) -> LibraryReader` | all of the above, per target |
| `apps/dory-cli` | Headless driving adapter and test bench | core, `dory` |
| `apps/dory-gpui` | The GPUI app | core, `dory`, `gpui` |

The GPUI app replaced the old app, and the old code is gone from the tree.

## Usage

```rust
let dory = dory::open(dory::Config::default_paths()?)?;
let events = dory.events();

dory.recorder().start(StartOptions::default())?;   // cheap, returns at once
dory.recorder().stop()?;                            // 5 s undo grace begins
dory.recorder().undo_stop()?;                       // same take, same file

let rows = dory.library().list()?;
dory.pipeline().enqueue(&id, Goal::ALL)?;

for row in dory.models().list::<Stt>().await {      // probes off the UI thread
    // Ready, NeedsProvision { bytes }, or Unavailable { fix }
}
dory.models().choose::<Stt>(Choice::Pinned("sherpa/qwen3-asr-0.6b".parse()?))?;
```

A `Dory` owns its data folder. `dory::open` takes an exclusive lock on `<data_dir>/.lock` and holds it until the `Dory` is dropped, after the last take is sealed. A second `open` of the same folder, in this process or another, fails at once with `OpenError::InUse`. Startup recovery cannot tell a take another process is still writing from one a crash left open, so only the lock holder may run it. The OS drops the lock when the process dies, so a crash never leaves the folder locked.

`dory::open_library` returns a `LibraryReader`: store reads and the model list, with no actors, no recovery, and no lock. It is what `list`, `show`, and `models` use, and it works while another process records into the same folder.

Facade methods that only send a command to an actor are synchronous and return at once. Methods that probe the machine or wait on the user (`models().list`, `recorder().request_access`) return futures that await an `async-channel` reply, so they run on GPUI's executor or under `block_on` in the CLI without blocking a UI thread.

## Domain types

- **Ids and time.** `RecordingId` (path-safe, legacy stems accepted), `Ms` (recorded media time, pauses never appear on it), `TimeRange` (start before end by construction), `LangTag`, `LanguagePref { Auto, Pinned(LangTag) }`, `Revision`.
- **Recording.** Title, created time, language preference, `ChannelLayout { MicSystem, MicOnly, SystemOnly, Mixed }`, and `TakeInfo { container: Wav | Flac | LegacyWebm, rate_hz, duration }`. Stereo is always `[mic, system]`. `Mixed` is the honest label for legacy WebM and imports.
- **Transcript.** `revision`, `segments: Vec<Segment { range: Option<TimeRange>, speaker: SpeakerId, channel, text }>`, `detected_languages`, `produced_by: ModelRef`. A range is `None` only for transcripts imported from the old `[Speaker N]` string. `SpeakerId(0)` is the local user, and only on the mic channel of a `MicSystem` take. A mic-only or mixed take may hold a whole room, so its diarized speakers are numbered from 1.
- **Speaker names.** User-owned, stored with the recording as `SpeakerNames { for_revision, names }`. A re-transcribe that renumbers remote speakers does not silently apply old names to new clusters. The names come back if the user reverts.
- **Summary.** Markdown, language, `produced_by`, and the `transcript_revision` it was made from. Staleness is derived by comparing revisions.
- **Job.** `goal`, `done` stages, `state: JobState { Queued, Running { stage }, Blocked { stage, blocker }, Succeeded, Failed { stage, error }, Cancelled }`, priority, attempts, `not_before`. `Job::on(&JobEvent) -> Result<Job, IllegalTransition>` is the only transition function.
- **Recorder.** `State { Idle, Recording, Paused, Grace { deadline } }` and the pure `step(state, input, limits, now) -> (state, effects)`. Elapsed time is the take writer's running total of frames divided by the (non-zero) sample rate, so the 2-hour cap fires at exactly that many frames whatever the chunk sizes, and pauses never count. Each `StartCapture` carries a generation, so a late `CaptureFailed` from a replaced device handle is ignored, and a `StartFailed` with nothing recorded discards the take and returns to Idle.
- **Waveform.** `Levels` is the RMS of each of up to `BUCKETS` (1,000) even shares of a take, from its louder channel, measured by the pure `LevelMeter` in one pass. `Waveform::from_levels` maps them to bar heights on a decibel scale: the loudest bucket (never taken as quieter than -30 dBFS) is full height, and 40 dB below it is the floor, which silence also gets. `FsTakes` measures while sealing and keeps `<id>.levels`; legacy takes and older takes measure on the first `Library::waveform` call.
- **Model selection.** A `ModelKind` marker per kind (`Stt`, `Llm`, `Diar`) with associated `Caps`, `Needs`, and engine type. `ModelId<K>` cannot cross kinds. `Choice<K> { Auto, Pinned, Off }`. A pure `fit(caps, needs)` and a pure `resolve(choice, statuses, needs) -> Use | Skip | Blocked(Blocker)`.

## Ports

```rust
// Audio. One adapter per platform.
pub trait CaptureBackend: Send + Sync {
    fn probe(&self) -> CaptureCaps;                                   // no prompts
    fn input_devices(&self) -> Result<Vec<InputDevice>, CaptureError>;
    fn request_access(&self, source: Source) -> Result<Access, CaptureError>;
    fn start(&self, spec: &CaptureSpec, sink: FrameSink) -> Result<Box<dyn CaptureHandle>, CaptureError>;
}
pub trait CaptureHandle: Send {
    fn stop(self: Box<Self>);                                          // releases the device and drops the sink; cannot run twice
}

// Storage.
pub trait StoreRead: Send + Sync {
    fn recordings(&self, query: &RecordingQuery) -> Result<Vec<RecordingRow>, StoreError>;   // newest first
    fn search(&self, query: &Query) -> Result<Vec<Found>, StoreError>;                      // newest first, unranked
    fn vocabulary(&self) -> Result<Arc<Vocabulary>, StoreError>;                            // every indexed word, for typo correction
    fn assistant_recordings(&self, query: &AssistantQuery) -> Result<Vec<Found>, StoreError>; // search inside a grant's fields, never a hidden recording
    fn assistant_vocabulary(&self, fields: Fields) -> Result<Vocabulary, StoreError>;      // the words that search can find
    // recording, transcript, summary, speaker_names, jobs, settings, checkpoints
}
pub trait Store: StoreRead + Send + Sync { fn apply(&self, changes: Vec<Change>) -> Result<(), StoreError>; }
pub trait Takes: Send + Sync {
    fn begin(&self, id: &RecordingId, layout: ChannelLayout, rate_hz: u32) -> Result<Box<dyn TakeWriter>, TakeError>;
    fn seal(&self, id: &RecordingId) -> Result<TakeInfo, TakeError>;   // WAV to FLAC, idempotent
    fn clip(&self, id: &RecordingId, channel: Channel, span: Option<TimeRange>) -> Result<Clip, TakeError>;
    fn orphans(&self) -> Result<Vec<RecordingId>, TakeError>;          // open WAVs a crash left
    fn stored(&self) -> Result<Vec<RecordingId>, TakeError>;           // every id with any file
    // inspect, import, playback_path, delete
    fn legacy_original(&self, id: &RecordingId) -> Result<Option<PathBuf>, TakeError>; // the old app's file a pointer names
    fn adopt(&self, id: &RecordingId) -> Result<TakeInfo, TakeError>;                   // copy it in as a FLAC take, then drop the pointer
    fn files(&self, id: &RecordingId) -> Result<Vec<PathBuf>, TakeError>; // what delete removes, for the trash
    fn levels(&self, id: &RecordingId) -> Result<Levels, TakeError>;     // loudness per bucket, one pass, kept beside the take
}
pub trait Trash: Send + Sync {
    fn move_to_trash(&self, paths: &[PathBuf]) -> Result<TrashReceipt, TrashError>; // all or nothing
    fn put_back(&self, receipt: &TrashReceipt) -> Result<(), TrashError>;          // never overwrites
}

// Models. One lifecycle for every kind.
pub trait ModelProvider<K: ModelKind>: Send + Sync {
    fn id(&self) -> ProviderId;
    fn catalog(&self) -> Vec<ModelEntry<K>>;                           // static rows, no IO
    fn availability(&self, model: &ModelId<K>) -> Availability;         // probe, no mutation
    fn provision(&self, model: &ModelId<K>, progress: &mut dyn ProvisionSink, cancel: &CancelToken) -> Result<(), ProvisionError>;
    fn open(&self, model: &ModelId<K>, cancel: &CancelToken) -> Result<Arc<K::Engine>, EngineError>;
}
pub trait Transcriber: Send + Sync {
    fn transcribe(&self, req: &TranscribeRequest<'_>, sink: &mut dyn SegmentSink, cancel: &CancelToken) -> Result<TranscribeOutcome, EngineError>;
}
pub trait LanguageModel: Send + Sync {
    fn count_tokens(&self, text: &str) -> u32;
    fn complete(&self, req: &CompletionRequest<'_>, sink: &mut dyn TextSink, cancel: &CancelToken) -> Result<String, EngineError>;
}
pub trait Diarizer: Send + Sync {
    fn diarize(&self, clip: &Clip, progress: &mut dyn FnMut(Fraction), cancel: &CancelToken) -> Result<Vec<Turn>, EngineError>;
}
pub trait Clock: Send + Sync { fn now(&self) -> Timestamp; fn monotonic_ms(&self) -> u64; }

// The machine the pipeline runs on. One adapter per platform.
pub trait Machine: Send + Sync {
    fn conditions(&self) -> Conditions;                                // power, Low Power Mode, thermal state; no prompts
}
```

### The capture contract

`FrameSink` is a core type with private fields. The core creates it around a wait-free single-producer ring (`rtrb`), and the adapter only calls `push(&[f32])` and `report(CaptureIssue)`. An adapter writes no ring and no drain thread.

1. `push` is wait-free and allocation-free, so the adapter may call it on the real-time audio thread.
2. Frames are f32, interleaved in `spec.layout` order, at `spec.rate_hz` (48 kHz on macOS). Drift correction and resampling stay inside the adapter.
3. A lost source is filled with silence of the right length plus a `CaptureIssue`, so the core never sees ragged channels.
4. If the ring overflows, the core's writer thread pads the dropped frame count with silence. Elapsed time stays equal to media time.
5. The adapter never changes the user's default devices and creates only private devices.
6. Pause and the 5-second grace stop the device (`CaptureHandle::stop`), so the system's recording indicator is accurate. Resume calls `start` again with a fresh `FrameSink` that writes into the same open `TakeWriter`. Every start has a generation, and `CaptureFailed` and `StartFailed` carry the generation of the handle they come from, so a failure that arrives after the handle was replaced is ignored. If the first start fails and nothing was recorded, the recorder discards the take instead of leaving an empty recording.
7. If system audio stays digital silence for 3 seconds while the output device is running, the adapter reports `CaptureIssue::SystemAudioSilent`. On macOS that is how a denied System Audio Recording permission shows up, because there is no API to query it.

## Adapters

| Port | Adapter | Notes |
| --- | --- | --- |
| `CaptureBackend` | `dory_macos::CoreAudioCapture` | A global stereo process tap that excludes Dory's own process, plus the chosen mic, in one private aggregate device. The default output device is the clock source. The mic gets drift compensation. One IOProc downmixes the tap to mono and pushes `[mic, system]`. Pure Rust through `objc2-core-audio`. |
| `CaptureBackend` | `dory_core::testing::FakeCapture` | Scripted frames for tests. |
| `Machine` | `dory_macos::MacMachine`, `dory_core::testing::FakeMachine` | `conditions` reads the power source through IOKit, and Low Power Mode and the thermal state through `NSProcessInfo`. It only reads; no adapter changes thread priority (see "Processing pace"). Other platforms get a no-op adapter that reports `Unknown` from the composition root, so `Auto` runs fast there. |
| `Trash` | `dory_macos::MacTrash`, `dory_core::testing::FakeTrash` | `NSFileManager trashItemAtURL`. The receipt keeps each original and trashed path, and `put_back` renames them back. Other platforms get `Unsupported` from the composition root. |
| `Store`, `Takes` | `dory_store::{SqliteStore, FsTakes}` | SQLite with one writer connection. Takes are files: crash-tolerant WAV during recording, FLAC after sealing. |
| `ModelProvider<Stt>` | `dory_engines::Sona`, `dory_engines::Sherpa`, `dory_macos::AppleSpeech` | Apple Speech declares `FixedLocale` caps, so `Auto` resolves past it unless the recording pins a supported locale. |
| `ModelProvider<Llm>` | `dory_engines::Ollama`, `dory_macos::FoundationModels` | Ollama sets `num_ctx` explicitly and reports that number as `context_tokens`. Foundation Models is one row, `apple/foundation-model` ("Apple Intelligence"), ranked below Gemma and not recommended, so a Ready Gemma wins and Apple stands in when Gemma cannot run. It reads Apple's context window (4096 on macOS 26.4 and 26.5) and languages once when the composition root builds it, so `catalog` does no IO, and probes availability on every call. It needs macOS 26.4. `count_tokens` estimates `ceil(utf8_bytes / 2.7)` instead of calling Apple. `DORY_APPLE` pretends its state when `DORY_DATA_DIR` is set. |
| `ModelProvider<Diar>` | `dory_engines::Sherpa` diarizers | Optional. With none, remote speakers stay one "Remote" speaker. |

`dory-macos` reaches Apple's Swift-only frameworks (Foundation Models now, SpeechAnalyzer next) through `swift/DoryAI.swift`. Its `build.rs` compiles that one file with `xcrun swiftc -emit-library -static` into `libdoryai.a`, with no SwiftPM package, and links Foundation Models, Foundation, and the system Swift runtime in `/usr/lib/swift`. It needs Xcode with the macOS 26.4 SDK or later. The file exports a small blocking `@_cdecl` C ABI: each call takes a cancel handle another thread can trip, and streams pieces through a (context, callback) pair that runs on the calling thread. Every `unsafe` line of the bridge is in `src/ai/ffi.rs`, and `src/ai/foundation_models.rs` is safe Rust over it. The bridge sits behind the `apple-intelligence` feature, which `dory`'s `app` feature turns on, so `dory-mcp`, built on its own without `app`, links no Swift. A binary that links it must target macOS 12 or later, where the concurrency runtime links by absolute path, so the workspace's `.cargo/config.toml` sets `MACOSX_DEPLOYMENT_TARGET` to 26.0, Dory's `LSMinimumSystemVersion`. The `-rpath` that `build.rs` asks for applies only to dory-macos's own targets, because cargo never passes a library's link arguments to its dependents. Capture does not use Swift.

## Search

`dory_core::search` owns every search rule as pure functions. `tokens(text)` splits text into words (a run of letters and digits, and each Han, Hiragana, and Katakana character on its own), each with its byte span and its `fold`: lowercase, NFKD, no combining marks, and a table for `ß`, `ø`, `ł`, `æ`, `œ`, `đ`, and `þ`. `Query::parse` turns typed text into clauses joined by AND: an unquoted word matches the start of any word, and a quoted run, or a typed word that splits into several tokens such as "e-mail", matches whole words in order. `matches`, `highlights`, `locate`, and `snippet` answer where a query matches inside text the caller already has. Because the index, the marks, and the snippet all read the same tokens, a hit always has something to mark.

`StoreRead::search(&Query) -> Vec<Found>` returns the recordings where every clause matches the title, a speaker name of the current transcript revision, the summary, or the transcript. `Found` is the list row plus `in_title`, true when the title alone matches every clause. Hits come newest first and are not ranked. `Library::search(text)` parses, looks up, logs `search ms=`, and returns `SearchResults { query, hits }`, or `None` for text with no word in it. `MemStore` implements search with `matches` over its entries, and is the reference the store contract suite holds `SqliteStore` to.

`SqliteStore` keeps one row per recording in a contentless FTS5 table (`content=''`, `contentless_delete=1`, `prefix='1 2 3'`). A row holds the core's folded tokens joined by spaces, never the raw text, so FTS5's `unicode61` tokenizer only splits on spaces and agrees with the core on every word. The FTS rowid is `search_keys.key`, because `recordings` has a text key and VACUUM may renumber its implicit rowid. `apply_change` reindexes a recording in the same transaction as the change that wrote its text: `RecordingCreated`, a title edit, `TranscriptReplaced`, `SpeakerNamesReplaced`, and `SummaryReplaced`. `RecordingDeleted` drops the row first. The legacy importer calls `apply_change`, so imports are indexed too. A query compiles to one `MATCH` string with every token double-quoted, so a typed "or" or "near" is a word, and a second `{title} :` match sets `in_title`.

`Query::corrected(&Vocabulary)` is the typo rule, a pure function: a word of five letters or more that starts no term gets every term within one edit (two from nine letters), counted as optimal string alignment distance, so a swap is one edit. Phrases, short words, and numbers are never corrected. `Library::search` runs the exact query first and asks `StoreRead::vocabulary` only when that finds nothing, so a search that finds something is never corrected. `SqliteStore` reads the terms from a temporary `fts5vocab` table on its reader connection and caches them in a `Mutex<Option<Arc<Vocabulary>>>`, rebuilding when `search_meta.generation` differs from the cached one. `MemStore` builds a vocabulary on every call.

`SearchResults::sections()` splits the hits into `Section::Titles` (`in_title`) and `Section::Conversations`, each keeping the newest-first order, with one pure function, `search::sections`. `Library::snippets(&Query, &[RecordingId]) -> Vec<Option<Excerpt>>` reads each recording's transcript, summary, and speaker names, groups the segments into turns, and asks the pure `search::excerpt` for the first place the query matches: a turn, else a summary line (its Markdown stripped by `summary_blocks`), else a speaker name. An `Excerpt` is a `Snippet` and its `Source`. It is a store read like `search`, so the app asks for the first 30 conversation hits in the same background call as the search and for the rest right after.

Assistants search through the same rules and the same index. `Assistant::search` parses with `Query::parse` and runs the shared `app::find` (exact first, the correction only when that finds nothing) over `StoreRead::assistant_recordings` and `assistant_vocabulary` instead of `search` and `vocabulary`. An `AssistantQuery` carries the grant's `Fields` (title; speakers and summary from Summaries; transcript from Transcripts), so `SqliteStore` compiles one `{title speakers summary} : (...)` column filter into the same MATCH and adds `hidden = 0`, the window, and the keyset cursor in SQL. The grant's vocabulary comes from a second temporary `fts5vocab` table in `col` mode: a term is kept when the recordings that hold it in a granted column outnumber the hidden ones that do, and the hidden ones' words are read from their text. It is not cached, because it is read only when the exact search found nothing. Each hit's snippet comes from `search::highlights` for a title hit, and from `search::excerpt_within` over only the granted text for a conversation hit, with the transcript grouped into the same turns `get_transcript` pages through. `SqliteReader` refuses a lookup with `StoreError::SearchIndexStale` when the index was built by another `TOKENS_VERSION`, since only the owner may rebuild it.

`search_meta` holds `tokens_version` and `generation`. Schema v8 builds the index for every recording inside its migration transaction, so a crash leaves v7 and the next open runs v8 again. When `dory_core::search::TOKENS_VERSION` differs from the stored one, `SqliteStore::open` rebuilds the index the same way. Both log `search-backfill ms=`. These two rebuilds are the only index writes outside `apply`, like any schema migration. `generation` goes up with every index write, for caches built from the index.

## How a model is chosen and added

- Settings store a `Choice<K>` per kind, never a path.
- `resolve` filters the catalog by `fit` (language support, diarization, input length) and availability, then prefers Ready, then `recommended`, then the provider's `rank`. `Auto` never downloads. A job that needs an absent model becomes `Blocked(NeedsProvision)` and the UI offers the button. `Llm::fit` ignores `context_tokens` on purpose: summarization sizes its chunks from it with map-reduce, so a small window costs more passes but never rules a model out. On an optional kind (`Llm`, `Diar`), `Auto` resolves to `Skip` when the catalog is empty, which is how a missing diarizer leaves remote speakers as one "Remote" speaker. When every model rules itself out, `Auto` blocks on the best of them with `Incompatible`, so a meeting no summary model can read says why. `Pinned` never skips.
- Language fit. `Llm::fit` needs every tag in `LlmNeeds::all`, the union of the recording's pinned language, `Settings::meeting_languages`, and the transcript's `detected_languages`, and an empty `all` fits every model. A `FixedLocale` speech model fits `Auto` when the meeting languages share one language it covers, and `run_language(caps, needs)` pins that language for the run and its checkpoint key. When a summary model chosen by `Auto` refuses the meeting (`UnsupportedLanguage` or `InputRejected`), the stage publishes `Event::DraftReset`, resolves again with that model excluded, and starts over from the transcript. `EngineError::Busy` is transient and retried.
- `Catalog::new` rejects duplicate ids at startup. An empty catalog is valid for every kind: on a required kind (`Stt`), a job blocks on `Blocker::NoModel`, which each app renders with its own fix. A settings value that names a removed model falls back to `Auto` and emits an event.
- To add a model to an existing provider, add one catalog row. To add an engine, implement `ModelProvider<K>` and its engine trait in one module, then add one line to `dory::providers::<K>()`.

## Pipeline

The queue is the store. The worker takes `next_runnable_job(now)`, publishes the job's cancel token, commits `Started`, runs the next stage, and commits the stage output together with `StageDone` in one `apply`. The token is published before `Started` commits, and the facade commits a cancel or a delete before it cancels the token, so a cancel either makes `Started` fail or finds the token. Shutdown sets its stop flag before it cancels the token, and the worker reads the flag after publishing it, so shutdown never waits out a whole stage.

`transcribe` (core policy):

1. Resolve the speech model and, if it lacks native diarization, the diarizer.
2. Plan clips from the layout. `MicSystem` gives a mic clip (speaker is the local user, no diarization) and a system clip (diarized). Other layouts give one clip.
3. Cut each clip into windows at silence, up to the engine's `SttCaps::window`. `Takes::clip` reports `Activity`: `Speech(spans)` from the adapter's energy VAD, or `Unmeasured`, which counts the whole clip as speech. A measured silent clip gets no engine call at all, which makes the mic track cheap during long remote talks. Windows hold whole runs of speech and end at a silence; speech longer than the cap with no silence in it is cut into equal pieces. Engines with native diarization run the whole diarized clip in one window, because speaker ids only hold within one call, so those runs only resume at a clip boundary.
4. After each window, commit its segments as `Change::CheckpointSaved`, keyed by recording, model, channel, and language. A rerun reads `StoreRead::checkpoints` for its key and skips every window already saved, so another model or language never reuses them. Diarized speakers are numbered by first appearance across the whole clip once every window is in.
5. Merge by time, drop mic segments that repeat system speech (speaker bleed: system speech covers at least half the mic segment's time, and at least 60% of its normalized words appear in that speech in order), and commit the transcript with `revision + 1`, together with `Change::CheckpointsCleared` and the stage's `StageDone`.

`summarize` (core policy): build the prompt in the meeting's main language, then run one pass or a map-reduce sized from `context_tokens`. One pass when `count(system) + count(transcript) + min(1024, context / 4) + 64 <= context`. Otherwise the transcript lines are packed into chunks of at most `context - count(notes system) - min(512, context / 8) - 64` tokens, each chunk becomes notes, and the notes become the three-section summary. Notes that still do not fit are merged in groups first, as many rounds as it takes. Only the last pass streams drafts.

## Processing pace

`pace` (core policy, `app/pace.rs`) is `Pacer::next(setting: Processing, now: Conditions, recording: bool, at_mono_ms) -> Budget`, a pure step whose only memory is the last light condition Automatic saw and when. `Budget` is `Fast { threads } | Light { reason: LightReason }`, so a fast budget cannot carry a reason. `LightReason` is `Chosen | Battery | LowPower | Hot | Recording`.

- `Fast` and `Light` settings return that pace whatever the conditions. `Light` says `Chosen`.
- `Auto` returns `Light` when on battery, in Low Power Mode, at thermal state serious or critical, or while the recorder is not idle. The first matching condition is the `reason`. Otherwise it returns `Fast`, but only after no condition has held for 2 minutes on the monotonic clock. Before that it keeps `Light` with the last condition's reason.
- `Fast` uses `min(4, available cores)` threads, today's count. `Light` uses 2.

The worker evaluates the budget at the start of the transcribe stage and before each speech window that runs the engine, rereading the setting each time, and publishes `StageProgress` with the `Budget`, so a change shows within one window. `threads` goes in `TranscribeRequest` per window. Sherpa builds its recognizer on the first window and rebuilds it when `threads` or the Whisper language changes; the 2-minute hold bounds those rebuilds. `Options::processing` (the CLI's `--pace`) overrides the setting for a bench.

Light is a thread cap at normal priority, not a QoS class. P6b measured both on macOS 26: threads onnxruntime creates do not inherit a background QoS set on the worker, so a background worker made the engine wait on one efficiency-core share of every operation. On the 132-minute take that ran at 8.7 times Fast's time, against 1.7 times for 2 threads at default QoS on a 6-minute clip. Summaries are not paced: Ollama 0.40.16 with the MLX runner ignores `num_thread`, and Gemma works mostly on the GPU, so `CompletionRequest` carries no thread count and a summary pass reports no light reason.

## Threads

| Thread | Owns |
| --- | --- |
| Adapter audio thread | Nothing shared. Calls `FrameSink::push` only. |
| Take writer (core) | Drains the ring into the `TakeWriter`, pads overruns, reports `Wrote { total_frames }` (a running total, so no chunk size loses time) to the recorder. Exits after one last drain once the actor flags that `CaptureHandle::stop` returned, so an adapter that keeps the sink cannot hang it. |
| Recorder actor (core) | Recorder state, the `CaptureHandle`, the `TakeWriter`, the grace deadline, and the set of takes being sealed. `Library::delete` refuses a take in that set, so a seal never lands after its row is gone. |
| Seal threads (core) | One per finished take: WAV to FLAC, then the commit, retried with backoff while the store reports a transient error. What still fails is finished by the next start. |
| Pipeline worker (core) | The running job, loaded engines (one per kind, unloaded when idle), and the current `Pace`. The `threads` it passes in each `TranscribeRequest` is what bounds the engine. It reads whether the recorder is busy through a shared view of the recorder's status, so the recorder starts first. |
| Provision threads | One per download. |
| UI | Nothing in core. Calls the facade and receives events. |

Events are hints. A slow subscriber loses droppable events (levels, progress, drafts) and gets `Event::Resync`.

## Recovery

`Dory::open` repairs what a crash left before any actor starts, and each step reads the current state, so a second start finds nothing to do.

1. Every id in `Takes::stored()` whose recording row is gone loses its files, in whatever state they are: open WAV, half-written FLAC, sealed FLAC, layout file, or legacy pointer. A legacy pointer's target in the old app's folder is never touched.
2. Every open WAV (`Takes::orphans()`) is sealed and committed.
3. A row with no take whose audio is sealed gets its take committed.
4. Jobs left Running go back to the queue through `JobEvent::Interrupted`.

`Library::trash` moves the files to the system Trash, then deletes the row. A crash between the two leaves a row whose audio is in the Trash, which the user can put back by hand. `Library::restore` puts the files back, then recreates the row, transcript, summary, and names in one `apply`. A crash between those two steps leaves audio with no row, and step 1 deletes it at the next start, so the recording is lost for good. The window is one store commit. Jobs are not restored, and `trash` cancels any that are queued, running, or blocked.

The recorder creates a take's files before its row, so a crash between the two leaves files that step 1 deletes, never a row with no audio.

## Legacy data

`dory-store` imports each old `metadata/<id>.json` once, keyed by content hash, in one `apply`. It points at the old audio file in place (`LegacyWebm`, `Mixed`), parses the transcript into untimed segments, and maps the old pipeline status to a job state. Nothing is copied or deleted, so the old app keeps working on the same folder if it is still installed.

## Open questions

- Live drafts while recording (UX-27). The proposed shape is a fourth model kind, `Live`, whose engine takes pushed frames and returns draft segments. The take writer thread tees frames to it at low priority, and drafts are events only, never stored. The post-stop `Stt` pass stays the source of truth. The speech plan's step E7 picks the engine before this lands.

- Mic bleed without headphones. Text-similarity dedupe is the first answer. A measured acceptance test decides whether the macOS adapter also needs voice-processing echo cancellation with the tap as reference.
- Cost of two passes on two-sided meetings. The speech bake-off measures wall time per channel on the operator's Mac.
- Whether the macOS aggregate device keeps mic and tap sample-aligned in practice. The capture harness measures it with a click track.

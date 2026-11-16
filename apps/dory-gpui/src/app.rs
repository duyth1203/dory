//! `AppState`, the one owner of what the window shows, and the private boundary to the core.
//!
//! Data flows one way. The core's event `Subscription` feeds a task spawned here; `AppState`
//! reloads rows through `library().list` on relevant events and on `Resync`, and views read
//! `AppState`, `Session`, and `PlayerHandle`. Views never call the store: the `Dory` facade lives
//! in the private `core` module, so only this module tree can reach it.
//!
//! `record` adds the record flow and processing (P5): recorder commands, the recorder's events,
//! progress and estimates, finish and fix actions.

mod core;
mod models;
mod player;
mod queue;
mod record;
mod session;
mod setup;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use dory_core::app::{Dory, Event, RecorderStatus, Trashed};
use dory_core::domain::{JobId, RecordingId, RecordingQuery, RecordingRow, Settings};
use dory_core::models::ModelRef;
use dory_core::ports::capture::{Access, Source};
use gpui_kit::{AppContext as _, Context, Entity, EventEmitter, SharedString, Task};

pub use player::PlayerHandle;
pub use record::{Download, ModelInfo};
pub use session::{Detail, Loadable, Session, Turn, speaker_label, turns};

use self::core::Core;
use crate::library::{Progress, RowStatus, SearchQuery, Selection, StatusFilter, after_removal};
use crate::onboarding::OnboardingStep;
use crate::prefs::Prefs;
use crate::processing::{Rate, Run};
use crate::record_control::Live;

/// How long typing pauses before the search runs.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailTab {
    #[default]
    Summary,
    Transcript,
}

/// What the start screen's sources card and onboarding show (UX-6, UX-16), probed at launch and
/// whenever the window comes back to the front, with no prompts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources {
    pub microphone: Access,
    pub microphone_name: Option<String>,
    pub system_audio: Access,
}

/// How long a status line stays up.
pub const NOTICE_TTL: Duration = Duration::from_secs(8);

/// The one-line status under the detail pane (UX-22). `undo` adds the Undo button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: SharedString,
    pub undo: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEvent {
    SelectionChanged,
    RowsReloaded,
}

/// A take just stopped is listed with its audio, so its finish card can take focus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved(pub RecordingId);

pub struct AppState {
    core: Core,
    /// The store's rows for the current search, newest first.
    pub recordings: Vec<RecordingRow>,
    pub selection: Selection,
    pub search: SearchQuery,
    pub filter: StatusFilter,
    pub prefs: Prefs,
    /// Here rather than in the detail pane, so Command-1 and Command-2 work from any focus.
    pub tab: DetailTab,
    pub sources: Option<Sources>,
    recorder: RecorderStatus,
    /// Droppable `StageProgress`, never stored. Cleared when the job moves on and on `Resync`.
    progress: HashMap<RecordingId, Progress>,
    /// A job never changes recording, so this cache cannot go stale.
    job_owner: HashMap<JobId, RecordingId>,
    session: Option<Entity<Session>>,
    loaded: bool,
    error: Option<SharedString>,
    /// Newest last. In memory only, so Undo works until the app quits.
    trashed: Vec<Trashed>,
    notice: Option<Notice>,
    notice_generation: u64,
    /// Set by Undo so the restored row is selected once the reload that lists it lands.
    reselect: Option<RecordingId>,
    rows_generation: u64,
    search_generation: u64,
    /// The recorded time and level, in their own entity so the 10 Hz meter repaints only the
    /// views that show it.
    live: Entity<Live>,
    /// Seconds the Undo toast shows while the recorder is in grace. The core owns the deadline.
    grace_left: u8,
    grace_generation: u64,
    /// Takes closed and not yet listed with their audio, so the detail says it is saving.
    sealing: HashSet<RecordingId>,
    /// The first progress of each running stage, for the time left (UX-12).
    runs: HashMap<RecordingId, Run>,
    /// How fast this Mac transcribed in this session, for the finish card's estimate.
    rate: Option<Rate>,
    downloads: HashMap<ModelRef, Download>,
    /// Speech and speaker models by id, for "Download <model>".
    models: HashMap<ModelRef, ModelInfo>,
    /// The core's settings as last announced.
    settings: Settings,
    /// Sources whose macOS prompt is up.
    asking: HashSet<Source>,
    epoch: Instant,
    _pump: Task<()>,
}

impl EventEmitter<AppEvent> for AppState {}
impl EventEmitter<Saved> for AppState {}

impl AppState {
    pub fn new(dory: Dory, mut prefs: Prefs, cx: &mut Context<Self>) -> Self {
        let events = dory.events();
        let recorder = dory.recorder().status();
        if prefs.onboarding.is_none() {
            // One indexed row, once per install, before the first frame, so a first run never
            // flashes the start screen.
            let one = RecordingQuery {
                text: None,
                limit: Some(1),
            };
            let has_recordings = dory.library().list(&one).is_ok_and(|rows| !rows.is_empty());
            prefs.onboarding = Some(OnboardingStep::resolve(None, has_recordings));
        }
        if let Some(step) = prefs.onboarding {
            eprintln!("onboarding: {}", step.as_str());
        }
        let core = Core::new(dory);
        record::load_models(&core, cx);
        record::load_settings(&core, cx);
        let pump = cx.spawn(async move |this, cx| {
            while let Some(first) = events.recv().await {
                let mut batch = vec![first];
                batch.extend(std::iter::from_fn(|| events.try_recv()));
                if this
                    .update(cx, |state, cx| state.on_events(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut state = Self {
            core,
            recordings: Vec::new(),
            selection: Selection::None,
            search: SearchQuery::default(),
            filter: StatusFilter::All,
            prefs,
            tab: DetailTab::Summary,
            sources: None,
            recorder,
            progress: HashMap::new(),
            job_owner: HashMap::new(),
            session: None,
            loaded: false,
            error: None,
            trashed: Vec::new(),
            notice: None,
            notice_generation: 0,
            reselect: None,
            rows_generation: 0,
            search_generation: 0,
            live: cx.new(|_| Live::default()),
            grace_left: 0,
            grace_generation: 0,
            sealing: HashSet::new(),
            runs: HashMap::new(),
            rate: None,
            downloads: HashMap::new(),
            models: HashMap::new(),
            settings: Settings::default(),
            asking: HashSet::new(),
            epoch: Instant::now(),
            _pump: pump,
        };
        state.reload(cx);
        state.reprobe(cx);
        state
    }

    /// False until the first list arrives, so the sidebar never flashes "No recordings yet".
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// The last failed read or write, for the window to show.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn recorder(&self) -> &RecorderStatus {
        &self.recorder
    }

    pub fn status(&self, row: &RecordingRow) -> RowStatus {
        RowStatus::derive(
            row,
            &self.recorder,
            self.progress.get(&row.recording.id).copied(),
        )
    }

    /// The rows the sidebar shows, in order, after the status filter.
    pub fn visible(&self) -> Vec<(&RecordingRow, RowStatus)> {
        self.recordings
            .iter()
            .map(|row| (row, self.status(row)))
            .filter(|(_, status)| self.filter.admits(*status))
            .collect()
    }

    pub fn visible_ids(&self) -> Vec<RecordingId> {
        self.visible()
            .into_iter()
            .map(|(row, _)| row.recording.id.clone())
            .collect()
    }

    /// The selected recording's detail and player. `None` shows the start screen.
    pub fn session(&self) -> Option<&Entity<Session>> {
        self.session.as_ref()
    }

    pub fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        if selection == self.selection {
            return;
        }
        let row = selection
            .id()
            .and_then(|id| self.recordings.iter().find(|r| &r.recording.id == id))
            .cloned();
        self.selection = if row.is_some() {
            selection
        } else {
            Selection::None
        };
        let core = self.core.clone();
        self.session = row.map(|row| cx.new(|cx| Session::new(core, &row, cx)));
        cx.emit(AppEvent::SelectionChanged);
        cx.notify();
    }

    /// Moves the selection `delta` rows through the visible list: the sidebar's arrow keys.
    /// With nothing selected, Down picks the first row and Up the last.
    pub fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids = self.visible_ids();
        if ids.is_empty() {
            return;
        }
        let last = ids.len() as isize - 1;
        let at = match self
            .selection
            .id()
            .and_then(|id| ids.iter().position(|i| i == id))
        {
            Some(ix) => (ix as isize + delta).clamp(0, last),
            None if delta > 0 => 0,
            None => last,
        };
        self.select(Selection::One(ids[at as usize].clone()), cx);
    }

    /// Searches title, transcript, and summary once typing pauses for [`SEARCH_DEBOUNCE`].
    pub fn set_search(&mut self, text: &str, cx: &mut Context<Self>) {
        let search = SearchQuery::new(text);
        if search == self.search {
            return;
        }
        self.search = search;
        self.search_generation += 1;
        let generation = self.search_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |state, cx| {
                if state.search_generation == generation {
                    state.reload(cx);
                }
            });
        })
        .detach();
    }

    pub fn set_filter(&mut self, filter: StatusFilter, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
    }

    pub fn set_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Moves the recording to the macOS Trash and keeps what Undo needs. When the core announces
    /// the delete, the selection moves to the next row, else the previous, else none.
    pub fn delete(&mut self, id: RecordingId, cx: &mut Context<Self>) {
        let trash = self.core.run(cx, move |dory| {
            dory.library().trash(&id).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let _ = match trash.await {
                Ok(trashed) => this.update(cx, |state, cx| {
                    state.trashed.push(trashed);
                    state.show(
                        Notice {
                            text: "Moved to Trash.".into(),
                            undo: true,
                        },
                        cx,
                    );
                }),
                Err(error) => this.update(cx, |state, cx| state.fail(error, cx)),
            };
        })
        .detach();
    }

    /// The Move to Trash command: the selected recording.
    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.selection.id().cloned() {
            self.delete(id, cx);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.trashed.is_empty()
    }

    /// Puts the latest trashed recording back and selects it.
    pub fn undo_delete(&mut self, cx: &mut Context<Self>) {
        let Some(trashed) = self.trashed.pop() else {
            return;
        };
        let restore = self.core.run(cx, move |dory| {
            dory.library().restore(trashed).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let _ = match restore.await {
                Ok(id) => this.update(cx, |state, cx| {
                    state.reselect = Some(id);
                    state.show(
                        Notice {
                            text: "Restored from the Trash.".into(),
                            undo: false,
                        },
                        cx,
                    );
                    state.reload(cx);
                }),
                Err(error) => this.update(cx, |state, cx| state.fail(error, cx)),
            };
        })
        .detach();
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// A status line without Undo, for an export or a failed write.
    pub fn say(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.show(
            Notice {
                text: text.into(),
                undo: false,
            },
            cx,
        );
    }

    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        self.say(error.clone(), cx);
        self.error = Some(error.into());
    }

    fn show(&mut self, notice: Notice, cx: &mut Context<Self>) {
        crate::a11y::announce(&notice.text);
        self.notice = Some(notice);
        self.notice_generation += 1;
        let generation = self.notice_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_TTL).await;
            let _ = this.update(cx, |state, cx| {
                if state.notice_generation == generation {
                    state.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.rows_generation += 1;
        let generation = self.rows_generation;
        let query = self.search.to_query();
        let list = self.core.run(cx, move |dory| dory.library().list(&query));
        cx.spawn(async move |this, cx| {
            let rows = list.await;
            let _ = this.update(cx, |state, cx| {
                if state.rows_generation != generation {
                    return;
                }
                match rows {
                    Ok(rows) => {
                        let before = std::mem::replace(&mut state.recordings, rows);
                        state.error = None;
                        state.on_rows(&before, cx);
                    }
                    Err(error) => state.error = Some(error.to_string().into()),
                }
                state.loaded = true;
                cx.emit(AppEvent::RowsReloaded);
                if let Some(id) = state.reselect.take() {
                    state.select(Selection::One(id), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_events(&mut self, events: Vec<Event>, cx: &mut Context<Self>) {
        let mut rows = false;
        let mut detail = false;
        // A batch of only `Levels` repaints the record control, not the window.
        let mut quiet = true;
        let selected = self.selection.id().cloned();
        let is_selected = |id: &RecordingId| selected.as_ref() == Some(id);
        for event in events {
            quiet &= matches!(event, Event::Levels { .. });
            match event {
                Event::RecordingAdded(_) | Event::JobEnqueued(_) => rows = true,
                Event::RecordingChanged(id)
                | Event::TranscriptChanged(id)
                | Event::SummaryChanged(id) => {
                    rows = true;
                    detail |= is_selected(&id);
                }
                Event::SpeakerNamesChanged(id) => detail |= is_selected(&id),
                Event::RecordingDeleted(id) => {
                    rows = true;
                    self.progress.remove(&id);
                    if is_selected(&id) {
                        let next = after_removal(&self.visible_ids(), &id);
                        self.recordings.retain(|r| r.recording.id != id);
                        self.select(next, cx);
                    }
                }
                Event::JobChanged(job) => {
                    rows = true;
                    if let Some(owner) = self.job_owner.get(&job) {
                        self.progress.remove(owner);
                        self.runs.remove(owner);
                    }
                }
                Event::StageProgress {
                    job,
                    stage,
                    fraction,
                    budget,
                } => self.on_progress(
                    job,
                    Progress {
                        stage,
                        fraction,
                        budget,
                    },
                    cx,
                ),
                Event::Recorder(status) => {
                    self.on_recorder(status, cx);
                    rows = true;
                }
                Event::Resync => {
                    self.progress.clear();
                    self.runs.clear();
                    let status = self.core.dory().recorder().status();
                    self.on_recorder(status, cx);
                    rows = true;
                    detail = true;
                }
                Event::SettingsChanged => record::load_settings(&self.core, cx),
                Event::ProvisionFinished { .. } => {
                    rows = true;
                    self.on_record_event(event, cx);
                }
                other => self.on_record_event(other, cx),
            }
        }
        if rows {
            self.reload(cx);
        }
        if detail && let Some(session) = &self.session {
            session.update(cx, |session, cx| session.reload(cx));
        }
        if !quiet {
            cx.notify();
        }
    }

    fn on_progress(&mut self, job: JobId, progress: Progress, cx: &mut Context<Self>) {
        if let Some(owner) = self.job_owner.get(&job).cloned() {
            self.track(owner, progress);
            return;
        }
        let lookup = self
            .core
            .run(cx, move |dory| dory.pipeline().job(job).ok().flatten());
        cx.spawn(async move |this, cx| {
            let Some(found) = lookup.await else { return };
            let _ = this.update(cx, |state, cx| {
                state.job_owner.insert(job, found.recording.clone());
                state.track(found.recording, progress);
                cx.notify();
            });
        })
        .detach();
    }
}

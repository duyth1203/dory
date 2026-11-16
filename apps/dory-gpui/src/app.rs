//! `AppState`, the one owner of what the window shows, and the private boundary to the core.
//!
//! Data flows one way. The core's event `Subscription` feeds a task spawned here; `AppState`
//! reloads rows through `library().list` on relevant events and on `Resync`, and views read
//! `AppState`, `Session`, and `PlayerHandle`. Views never call the store: the `Dory` facade lives
//! in the private `core` module, so only this module tree can reach it.

mod core;
mod player;
mod session;

use std::collections::HashMap;
use std::time::Duration;

use dory_core::app::{Dory, Event, RecorderStatus};
use dory_core::domain::{JobId, RecordingId, RecordingRow};
use dory_core::ports::capture::{Access, CaptureCaps};
use gpui_kit::{AppContext as _, Context, Entity, EventEmitter, SharedString, Task};

pub use player::PlayerHandle;
pub use session::{Detail, Loadable, Session, Turn, speaker_label, turns};

use self::core::Core;
use crate::library::{Progress, RowStatus, SearchQuery, Selection, StatusFilter, after_removal};
use crate::prefs::Prefs;

/// How long typing pauses before the search runs.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailTab {
    #[default]
    Summary,
    Transcript,
}

/// What the start screen's sources card shows (UX-6), probed once at launch with no prompts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources {
    pub microphone: Access,
    pub microphone_name: Option<String>,
    pub system_audio: Access,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEvent {
    SelectionChanged,
    RowsReloaded,
}

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
    rows_generation: u64,
    search_generation: u64,
    _pump: Task<()>,
}

impl EventEmitter<AppEvent> for AppState {}

impl AppState {
    pub fn new(dory: Dory, prefs: Prefs, cx: &mut Context<Self>) -> Self {
        let events = dory.events();
        let recorder = dory.recorder().status();
        let probe = dory.recorder().probe();
        let devices = dory.recorder().input_devices();
        let core = Core::new(dory);
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
        cx.spawn(async move |this, cx| {
            let caps = probe.await.unwrap_or(CaptureCaps {
                microphone: Access::Unsupported,
                system_audio: Access::Unsupported,
                unsupported: None,
            });
            let microphone_name = devices
                .await
                .ok()
                .and_then(|d| d.into_iter().find(|d| d.is_default))
                .map(|d| d.name);
            let _ = this.update(cx, |state, cx| {
                state.sources = Some(Sources {
                    microphone: caps.microphone,
                    microphone_name,
                    system_audio: caps.system_audio,
                });
                cx.notify();
            });
        })
        .detach();
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
            rows_generation: 0,
            search_generation: 0,
            _pump: pump,
        };
        state.reload(cx);
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

    /// Deletes for good: the core has no Trash yet (UX-22 needs one). When the core announces
    /// the delete, the selection moves to the next row, else the previous, else none.
    pub fn delete(&mut self, id: RecordingId, cx: &mut Context<Self>) {
        let delete = self.core.run(cx, move |dory| {
            dory.library().delete(&id).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = delete.await {
                let _ = this.update(cx, |state, cx| {
                    state.error = Some(error.into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The toolbar's Record button. Recording is P5's work, so it only says so.
    pub fn start_recording(&mut self, _cx: &mut Context<Self>) {
        eprintln!("record: P5 wires this");
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
                        state.recordings = rows;
                        state.error = None;
                    }
                    Err(error) => state.error = Some(error.to_string().into()),
                }
                state.loaded = true;
                cx.emit(AppEvent::RowsReloaded);
                cx.notify();
            });
        })
        .detach();
    }

    fn on_events(&mut self, events: Vec<Event>, cx: &mut Context<Self>) {
        let mut rows = false;
        let mut detail = false;
        let selected = self.selection.id().cloned();
        let is_selected = |id: &RecordingId| selected.as_ref() == Some(id);
        for event in events {
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
                    }
                }
                Event::StageProgress {
                    job,
                    stage,
                    fraction,
                } => self.on_progress(job, Progress { stage, fraction }, cx),
                Event::Recorder(status) => {
                    self.recorder = status;
                    rows = true;
                }
                Event::Resync => {
                    self.progress.clear();
                    self.recorder = self.core.dory().recorder().status();
                    rows = true;
                    detail = true;
                }
                _ => {}
            }
        }
        if rows {
            self.reload(cx);
        }
        if detail && let Some(session) = &self.session {
            session.update(cx, |session, cx| session.reload(cx));
        }
        cx.notify();
    }

    fn on_progress(&mut self, job: JobId, progress: Progress, cx: &mut Context<Self>) {
        if let Some(owner) = self.job_owner.get(&job) {
            self.progress.insert(owner.clone(), progress);
            return;
        }
        let lookup = self
            .core
            .run(cx, move |dory| dory.pipeline().job(job).ok().flatten());
        cx.spawn(async move |this, cx| {
            let Some(found) = lookup.await else { return };
            let _ = this.update(cx, |state, cx| {
                state.job_owner.insert(job, found.recording.clone());
                state.progress.insert(found.recording, progress);
                cx.notify();
            });
        })
        .detach();
    }
}

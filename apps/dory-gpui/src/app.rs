//! `AppState`, the one owner of what the window shows, and the private boundary to the core.
//!
//! Data flows one way. The core's event `Subscription` feeds a task spawned here; `AppState`
//! reloads rows through `library().list` on relevant events and on `Resync`, and views read
//! `AppState`, `Session`, and `PlayerHandle`. Views never call the store: the `Dory` facade lives
//! in the private `core` module, so only this module tree can reach it.
//!
//! `record` adds the record flow and processing (P5): recorder commands, the recorder's events,
//! progress and estimates, finish and fix actions.

mod assistants;
mod core;
mod lifecycle;
mod models;
mod player;
pub mod queue;
mod record;
mod session;
mod setup;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use dory_core::app::{Dory, Event, RecorderStatus, Trashed};
use dory_core::domain::{Job, JobId, RecordingId, RecordingQuery, RecordingRow, Segment, Settings};
use dory_core::models::ModelRef;
use dory_core::ports::capture::{Access, Source};
use dory_core::search::{Excerpt, Query, SearchResults, Section};
use gpui_kit::{AppContext as _, Context, Entity, EventEmitter, SharedString, Task};

pub use player::PlayerHandle;
pub use session::{
    Detail, Loadable, Moved, Session, Shown, SummaryVersion, TranscriptText, Turn, speaker_label,
    translated_turns, turns,
};

pub use self::assistants::{
    ACTIVITY_LINES, Assistants, AssistantsView, COPIED_FOR, Connected, Target, helper_path,
    setup_text, shorten_key,
};
use self::core::Core;
pub use self::models::{Originals, count};
use crate::estimate::{Estimate, Rates};
use crate::library::{Progress, RowStatus, SearchText, Selection, StatusFilter, after_removal};
use crate::onboarding::OnboardingStep;
use crate::prefs::Prefs;
pub use crate::processing::{Download, ModelInfo};
use crate::processing::{Phase, Run};
use crate::record_control::Live;

/// Rows with the status each shows, in the order the sidebar lists them.
pub type Listed<'a> = Vec<(&'a RecordingRow, RowStatus)>;

/// The conversation hits, newest first, whose snippets the sidebar draws.
fn conversation_ids(results: &SearchResults) -> Vec<RecordingId> {
    let [_, (_, conversations)] = results.sections();
    conversations
        .into_iter()
        .map(|found| found.row.recording.id.clone())
        .collect()
}

fn millis(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Logs once the main window has painted. A next-frame callback runs before its frame draws, so
/// the second one runs after the frame that shows the change.
fn log_after_paint(cx: &mut gpui_kit::App, line: impl FnOnce() -> String + 'static) {
    let Some(window) = crate::main_window(cx) else {
        return;
    };
    let _ = window.update(cx, |_, window, _| {
        window.on_next_frame(|window, _| window.on_next_frame(move |_, _| eprintln!("{}", line())));
    });
}

/// How long typing pauses before the search runs.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

/// How many conversation hits get their snippet in the same read as the search, so the first
/// screen paints once. The rest fill in right after.
pub const FIRST_SNIPPETS: usize = 30;

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
    /// The search field should read this, as if it had been typed. Only the visual pass asks.
    SearchFilled(String),
}

/// A take just stopped is listed with its audio, so its finish card can take focus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved(pub RecordingId);

pub struct AppState {
    core: Core,
    /// The store's rows for the current search, newest first.
    pub recordings: Vec<RecordingRow>,
    pub selection: Selection,
    /// What is in the search field, trimmed. Empty means no search.
    pub typed: SearchText,
    /// The last settled search: the query that found the hits, which is the corrected one when
    /// the typed words matched nothing, the hits, and the corrections. `None` while `typed` is
    /// empty, or holds no word.
    pub search: Option<SearchResults>,
    /// Where each conversation hit matched, as far as they have loaded. A hit with no entry
    /// shows no snippet.
    pub snippets: HashMap<RecordingId, Excerpt>,
    /// The words last spoken about the search: the result count when it settled, or the match
    /// the user last stepped to.
    pub announced: Option<String>,
    pub filter: StatusFilter,
    pub prefs: Prefs,
    /// Here rather than in the detail pane, so Command-1 and Command-2 work from any focus.
    pub tab: DetailTab,
    /// The tab the user last picked, which each selection starts on.
    chosen_tab: DetailTab,
    /// The user picked a tab for the selected recording, so processing no longer switches it.
    tab_chosen: bool,
    /// The selected recording's phase as last seen, so VoiceOver speaks only when it changes.
    card: Option<(RecordingId, Phase)>,
    pub sources: Option<Sources>,
    recorder: RecorderStatus,
    /// Droppable `StageProgress`, never stored. Cleared when the job moves on and on `Resync`.
    progress: HashMap<RecordingId, Progress>,
    /// A job never changes recording, so this cache cannot go stale.
    job_owner: HashMap<JobId, RecordingId>,
    /// The newest progress of jobs whose recording is being looked up.
    unowned: HashMap<JobId, Progress>,
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
    /// When the debounce fired for the search whose rows have not painted yet.
    settling: Option<Instant>,
    /// The recorded time and level, in their own entity so the 10 Hz meter repaints only the
    /// views that show it.
    live: Entity<Live>,
    /// The recorder's running total of the take, for the card before the take is saved.
    last_recorded: dory_core::domain::Ms,
    /// Seconds the Undo toast shows while the recorder is in grace. The core owns the deadline.
    grace_left: u8,
    grace_generation: u64,
    /// Takes closed and not yet listed with their audio, so the detail says it is saving.
    sealing: HashSet<RecordingId>,
    /// The first progress of each running stage, for the time left (UX-12).
    runs: HashMap<RecordingId, Run>,
    /// Each running stage's smoothed time left (UX-12).
    estimates: HashMap<RecordingId, Estimate>,
    /// How fast this Mac transcribed in this session at each pace.
    rates: Rates,
    /// Running, queued, and blocked jobs in queue order, read with the rows.
    queue: Vec<Job>,
    /// Lines the engine wrote for each running job, before the transcript commits (UX-12).
    drafts: HashMap<JobId, Vec<Segment>>,
    /// Jobs that finished or were cancelled since launch, newest first (the Processing window).
    finished: Vec<crate::processing_queue::Finished>,
    /// The last Cancel, while its Undo is offered.
    cancelled: Option<queue::Cancelled>,
    cancel_generation: u64,
    /// Levels of recordings the Processing window draws, `None` while being read.
    waveforms: HashMap<RecordingId, Option<std::sync::Arc<dory_core::domain::Waveform>>>,
    /// The Processing window pinned for design previews and the layout checks.
    queue_preview: Option<crate::processing_queue::Queue>,
    /// Phases and lines pinned for design previews and the layout checks (`preview_phase`).
    previews: HashMap<RecordingId, (Phase, Vec<Segment>)>,
    downloads: HashMap<ModelRef, Download>,
    /// Speech and speaker models by id, for "Download <model>".
    models: HashMap<ModelRef, ModelInfo>,
    /// The core's settings as last announced.
    settings: Settings,
    /// Sources whose macOS prompt is up.
    asking: HashSet<Source>,
    /// Settings > Storage's old-app WebM files (P7, moved from P8).
    originals: Originals,
    /// The core was shut down for a quit, so nothing may start again.
    shut_down: bool,
    /// Settings > Assistants: access, the connected assistants, and their latest reads.
    assistants: Assistants,
    /// The last Connect in Settings > Assistants.
    connected: Option<Connected>,
    /// `dory-mcp`, which Connect points assistants at.
    assistant_helper: Option<std::path::PathBuf>,
    /// Copy Setup just copied: its icon shows a check until this clears.
    copied: bool,
    /// Counts copies, so only the latest one's timer clears `copied`.
    copy_flash: u64,
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
            let one = RecordingQuery { limit: Some(1) };
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
            typed: SearchText::default(),
            search: None,
            snippets: HashMap::new(),
            announced: None,
            filter: StatusFilter::All,
            prefs,
            tab: DetailTab::Summary,
            chosen_tab: DetailTab::Summary,
            tab_chosen: false,
            card: None,
            sources: None,
            recorder,
            progress: HashMap::new(),
            job_owner: HashMap::new(),
            unowned: HashMap::new(),
            session: None,
            loaded: false,
            error: None,
            trashed: Vec::new(),
            notice: None,
            notice_generation: 0,
            reselect: None,
            rows_generation: 0,
            search_generation: 0,
            settling: None,
            live: cx.new(|_| Live::default()),
            last_recorded: dory_core::domain::Ms(0),
            grace_left: 0,
            grace_generation: 0,
            sealing: HashSet::new(),
            runs: HashMap::new(),
            estimates: HashMap::new(),
            rates: Rates::default(),
            queue: Vec::new(),
            finished: Vec::new(),
            cancelled: None,
            cancel_generation: 0,
            waveforms: HashMap::new(),
            queue_preview: None,
            drafts: HashMap::new(),
            previews: HashMap::new(),
            downloads: HashMap::new(),
            models: HashMap::new(),
            settings: Settings::default(),
            asking: HashSet::new(),
            originals: Originals::Unread,
            shut_down: false,
            assistants: Assistants::Unread,
            connected: None,
            assistant_helper: helper_path(),
            copied: false,
            copy_flash: 0,
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
        self.phase(row).row()
    }

    /// The rows the sidebar shows, in order, after the status filter. While a search has results,
    /// that is the Titles section, then the Conversations section.
    pub fn visible(&self) -> Listed<'_> {
        match self.visible_sections() {
            Some(sections) => sections.into_iter().flat_map(|(_, rows)| rows).collect(),
            None => self.shown(self.recordings.iter()),
        }
    }

    /// The search's two sections after the status filter, each newest first and possibly empty.
    /// `None` when no search has settled, so the sidebar groups by date.
    pub fn visible_sections(&self) -> Option<Vec<(Section, Listed<'_>)>> {
        let results = self.search.as_ref()?;
        Some(
            results
                .sections()
                .into_iter()
                .map(|(section, hits)| {
                    (
                        section,
                        self.shown(hits.into_iter().map(|found| &found.row)),
                    )
                })
                .collect(),
        )
    }

    fn shown<'a>(&self, rows: impl Iterator<Item = &'a RecordingRow>) -> Listed<'a> {
        rows.map(|row| (row, self.status(row)))
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
        let query = self.query();
        self.session = row.map(|row| cx.new(|cx| Session::new(core, &row, query, cx)));
        self.tab_chosen = false;
        self.tab = self.chosen_tab;
        self.follow_card();
        cx.emit(AppEvent::SelectionChanged);
        cx.notify();
    }

    /// The query that found the settled results, which is what marks matches in the detail pane.
    fn query(&self) -> Option<Query> {
        self.search.as_ref().map(|results| results.query.clone())
    }

    /// Shows `tab` because a match is there, which does not count as the user's pick.
    pub fn show_match_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            cx.notify();
        }
    }

    /// Find Next and Find Previous: moves to the neighbouring match of the selected recording,
    /// shows its tab, and says where it is. `None` when there is no match list.
    pub fn step_match(&mut self, delta: isize, cx: &mut Context<Self>) -> Option<Moved> {
        let session = self.session.clone()?;
        let moved = session.update(cx, |session, cx| session.step_match(delta, cx))?;
        if let Some(tab) = moved.at.place.tab() {
            self.show_match_tab(tab, cx);
        }
        crate::a11y::announce(&moved.spoken);
        self.announced = Some(moved.spoken.clone());
        cx.notify();
        Some(moved)
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

    /// Puts `text` in the search field and searches it, as typing it would.
    pub fn fill_search(&mut self, text: &str, cx: &mut Context<Self>) {
        cx.emit(AppEvent::SearchFilled(text.to_owned()));
        self.set_search(text, cx);
    }

    /// Searches titles, speaker names, summaries, and transcripts once typing pauses for
    /// [`SEARCH_DEBOUNCE`].
    pub fn set_search(&mut self, text: &str, cx: &mut Context<Self>) {
        let typed = SearchText::new(text);
        if typed == self.typed {
            return;
        }
        self.typed = typed;
        self.search_generation += 1;
        let generation = self.search_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |state, cx| {
                if state.search_generation == generation {
                    state.settling = Some(Instant::now());
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

    /// View > Bigger and Smaller (A11Y-7): saved with the window state, applied to the theme.
    pub fn set_text_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        let scale = crate::theme::clamp_text_scale(scale);
        if self.prefs.text_scale == scale {
            return;
        }
        self.prefs.text_scale = scale;
        crate::theme::set_display(|display| display.text_scale = scale, cx);
        crate::a11y::announce(&format!("Text size {}%", (scale * 100.).round() as u32));
        cx.notify();
    }

    /// Settings › General › Theme and View › Theme (UX-20): remembered in the UI prefs, and shown
    /// with a cross-fade.
    pub fn set_theme(&mut self, name: crate::theme::ThemeName, cx: &mut Context<Self>) {
        self.prefs.theme = name;
        if cx.try_global::<crate::theme::Display>().map(|d| d.theme) != Some(name) {
            crate::theme::switch_to(name, cx);
        }
        cx.notify();
    }

    /// The user picks a tab: a click, Command-1, or Command-2.
    pub fn set_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.chosen_tab = tab;
        self.tab_chosen = true;
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
        let text = self.typed.text().to_owned();
        let list = self.core.run(cx, move |dory| {
            let library = dory.library();
            let (rows, search, first) = match text.is_empty() {
                true => (library.list(&RecordingQuery::default())?, None, Vec::new()),
                false => match library.search(&text)? {
                    None => (Vec::new(), None, Vec::new()),
                    Some(results) => {
                        let ids = conversation_ids(&results);
                        let first = &ids[..ids.len().min(FIRST_SNIPPETS)];
                        let found = library.snippets(&results.query, first)?;
                        let first = first.iter().cloned().zip(found).collect();
                        let rows = results.hits.iter().map(|hit| hit.row.clone()).collect();
                        (rows, Some(results), first)
                    }
                },
            };
            Ok::<_, dory_core::ports::StoreError>((rows, search, first, dory.pipeline().queue()?))
        });
        cx.spawn(async move |this, cx| {
            let rows = list.await;
            let _ = this.update(cx, |state, cx| {
                if state.rows_generation != generation {
                    return;
                }
                match rows {
                    Ok((rows, search, first, queue)) => {
                        let before = std::mem::replace(&mut state.recordings, rows);
                        let queued_before = std::mem::replace(&mut state.queue, queue);
                        state.track_finished(&queued_before);
                        state.error = None;
                        state.set_results(search, first);
                        state.follow_query(cx);
                        state.on_rows(&before, cx);
                        if let Some(started) = state.settling.take() {
                            state.announce_results();
                            log_after_paint(cx, move || {
                                format!("search-settled ms={:.1}", millis(started))
                            });
                        }
                        state.fill_snippets(generation, cx);
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

    /// Swaps in the settled search. Snippets of the same query that are already loaded stay, so a
    /// reload for an event does not blank them while the next read fills them again.
    fn set_results(
        &mut self,
        search: Option<SearchResults>,
        first: Vec<(RecordingId, Option<Excerpt>)>,
    ) {
        let same_query = match (&search, &self.search) {
            (Some(new), Some(old)) => new.query == old.query,
            _ => false,
        };
        if !same_query {
            self.snippets.clear();
        }
        self.search = search;
        self.keep_snippets(first);
    }

    /// Hands the selected recording the settled query, or none once the search is cleared.
    fn follow_query(&mut self, cx: &mut Context<Self>) {
        let query = self.query();
        if let Some(session) = &self.session {
            session.update(cx, |session, cx| session.set_query(query, cx));
        }
    }

    fn keep_snippets(&mut self, found: Vec<(RecordingId, Option<Excerpt>)>) {
        for (id, excerpt) in found {
            match excerpt {
                Some(excerpt) => self.snippets.insert(id, excerpt),
                None => self.snippets.remove(&id),
            };
        }
    }

    /// Loads the snippets of the conversation hits past the first screen, off the UI thread. A
    /// newer reload makes the answer stale, so it is dropped.
    fn fill_snippets(&mut self, generation: u64, cx: &mut Context<Self>) {
        let Some(results) = &self.search else {
            return;
        };
        let ids = conversation_ids(results);
        let Some(rest) = ids.get(FIRST_SNIPPETS..).filter(|rest| !rest.is_empty()) else {
            return;
        };
        let (query, rest) = (results.query.clone(), rest.to_vec());
        let work = self.core.run(cx, move |dory| {
            let found = dory.library().snippets(&query, &rest)?;
            Ok::<_, dory_core::ports::StoreError>(rest.into_iter().zip(found).collect::<Vec<_>>())
        });
        cx.spawn(async move |this, cx| {
            let found = match work.await {
                Ok(found) => found,
                Err(error) => {
                    eprintln!("search snippets failed: {error}");
                    return;
                }
            };
            let _ = this.update(cx, |state, cx| {
                if state.rows_generation == generation {
                    state.keep_snippets(found);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Says how many results a search that just settled found, "No results" for text that holds
    /// no word. Typing that has not settled says nothing, and neither does clearing the field.
    fn announce_results(&mut self) {
        self.announced = None;
        if self.typed.is_empty() {
            return;
        }
        let corrections = self.search.as_ref().map_or(&[][..], |r| &r.corrections);
        let said = crate::library::results_announcement(self.visible().len(), corrections);
        crate::a11y::announce(&said);
        self.announced = Some(said);
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
                Event::SpeakerNamesChanged(id) => {
                    rows |= !self.typed.is_empty();
                    detail |= is_selected(&id);
                }
                Event::RecordingDeleted(id) => {
                    rows = true;
                    self.progress.remove(&id);
                    if is_selected(&id) {
                        let next = after_removal(&self.visible_ids(), &id);
                        self.recordings.retain(|r| r.recording.id != id);
                        self.select(next, cx);
                    }
                }
                Event::JobChanged(_) => rows = true,
                Event::DraftSegment { job, segment } => {
                    self.drafts.entry(job).or_default().push(segment);
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
                    self.estimates.clear();
                    self.drafts.clear();
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
        self.follow_card();
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
        // One lookup per job, and only the newest progress is tracked when it lands: lookups
        // finish in any order, and an older fraction must not overwrite a newer one.
        if self.unowned.insert(job, progress).is_some() {
            return;
        }
        let lookup = self
            .core
            .run(cx, move |dory| dory.pipeline().job(job).ok().flatten());
        cx.spawn(async move |this, cx| {
            let found = lookup.await;
            let _ = this.update(cx, |state, cx| {
                let latest = state.unowned.remove(&job);
                if let (Some(found), Some(progress)) = (found, latest) {
                    state.job_owner.insert(job, found.recording.clone());
                    state.track(found.recording, progress);
                    state.follow_card();
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

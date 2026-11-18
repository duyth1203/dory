//! The Transcript tab (UX-3, A11Y-10).
//!
//! - One list item per `Detail.turns` entry: a 30 pt initial circle, the label (14 semibold), the
//!   timestamp as a button (12, `accent_text`, AX label "Play from 12:24"), and the text
//!   (15/1.6). VoiceOver reads a turn as "Sam, 12 minutes 30 seconds, then the text".
//! - Click-to-seek: the timestamp, and Return on the current turn, call
//!   `PlayerHandle::play_from`. Turns with no range (legacy transcripts) show no timestamp, do
//!   not seek, and say so in their AX description.
//! - The turn under the playhead gets `playing_bg`, following the session's `PlayerHandle`.
//!   The play glyph and the word "Playing" show only while the audio plays, not when paused.
//! - Speaker rename: clicking the label, or Shift-Return on the current turn, turns the label into
//!   a 150 pt field with the hint "Renames Speaker 3 in all 14 turns. Return to save, Escape to
//!   cancel." Saving goes through `Session::rename_speaker`; the new labels arrive in the next
//!   `Detail`, so every turn of that speaker changes together. The core lets "Me" be renamed too.
//! - The list is one Tab stop, like the sidebar: Up and Down move the current turn, which is the
//!   list's active descendant.
//! - Up to [`PAGED_TURNS`] turns render as one column that scrolls with the page. Longer
//!   transcripts render through GPUI's `list`, which lays out only the turns in view, as a
//!   viewport one window tall less the toolbar and the sticky tab row. The page scrolls the
//!   header away and the list then fills the column. The list is a nested scroll area
//!   (`a11y::nested_list`, UX-31): it takes the wheel while it can move and passes the rest to
//!   the page, and a trackpad gesture stays with whichever of the two it began in.
//! - Copy Transcript and Export sit in the tab row, which the detail pane draws.
//! - While a search is active the session holds the recording's matches (`crate::find`). Each
//!   turn marks its matches the way the sidebar does, and says "Search match" to VoiceOver. When
//!   the session asks to show the current match and it is in a turn, the cursor moves to that
//!   turn and the page scrolls until the turn sits in view. A probe laid in the turn, or over
//!   the list's row, measures it each frame until it does, then emits [`Revealed`].
//!
//! The detail pane creates this view once per session and places it under the Transcript tab.

use std::ops::Range;
use std::sync::Arc;

use dory_core::domain::{Ms, SpeakerId};
use dory_core::search::Query;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Font, FontStyle,
    FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ListAlignment, ListOffset,
    ListState, ParentElement as _, Pixels, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, TestSupportExt as _,
    TextRun, Window, canvas, div, list, point, prelude::FluentBuilder as _, px, relative,
    transparent_black,
};

use crate::a11y;
use crate::app::{Detail, PlayerHandle, Session, TranscriptText, Turn};
use crate::find::{Place, shift_into_view};
use crate::fonts;
use crate::icons::Icon;
use crate::marks::marked;
use crate::theme::{self, Presentation, Theme};

/// Past this many turns the transcript virtualizes. A column this short renders in well under a
/// frame, and keeping it in the page keeps the page one scrolling surface.
pub const PAGED_TURNS: usize = 80;

/// The toolbar stays on screen above the list, 52 pt, and the sticky tab row under it, 34 pt
/// scaled with the text.
const TOOLBAR: f32 = 52.;
pub(crate) const TAB_ROW: f32 = 34.;
/// Keeps the last turn clear of the docked player: 52 pt tall, 14 pt above the edge.
pub(crate) const DOCK_CLEARANCE: f32 = 80.;

/// The turn under the playhead: the last turn to start at or before `at`, if it has not ended.
/// Turns run in time order, so this is a binary search.
pub fn playing_turn(turns: &[Turn], at: Ms) -> Option<usize> {
    let started = turns.partition_point(|t| t.range.is_some_and(|r| r.start() <= at));
    let ix = started.checked_sub(1)?;
    turns[ix].range.filter(|r| r.contains(at)).map(|_| ix)
}

/// What VoiceOver reads for a turn: "Sam, 12 minutes 30 seconds, English, then the text". The
/// language is only there on a tagged turn. A translated turn says "translated from Tagalog"
/// instead, which already names the language it was spoken in.
pub fn spoken_turn(turn: &Turn) -> String {
    let mut parts = vec![turn.label.to_string()];
    if let Some(range) = turn.range {
        parts.push(a11y::spoken_duration(range.start()));
    }
    match turn.translated_note() {
        Some(note) => parts.push(note),
        None => parts.extend(turn.tag.as_ref().map(ToString::to_string)),
    }
    parts.push(turn.text.clone());
    parts.join(", ")
}

/// "Renames Speaker 3 in all 14 turns. Return to save, Escape to cancel."
pub fn rename_hint(turns: &[Turn], speaker: SpeakerId, label: &str) -> String {
    let count = turns.iter().filter(|t| t.speaker == speaker).count();
    let scope = if count == 1 {
        "its one turn".to_owned()
    } else {
        format!("all {count} turns")
    };
    format!("Renames {label} in {scope}. Return to save, Escape to cancel.")
}

/// The spoken language after the speaker's name, as text in `text_secondary` and never as color
/// alone (A11Y-8). VoiceOver already hears it in the turn's own label.
fn language_tag(ix: usize, turn: &Turn, theme: &Theme) -> Option<a11y::Node> {
    let tag = turn.tag.clone()?;
    Some(
        a11y::node(format!("turn-{ix}-language"), Role::Label, tag.clone())
            .text_size(theme.text(12.))
            .text_color(theme.text_secondary)
            .child(tag),
    )
}

fn initial(turn: &Turn) -> SharedString {
    if turn.label.as_ref() == format!("Speaker {}", turn.speaker.0) {
        return turn.speaker.0.to_string().into();
    }
    turn.label
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_default()
        .into()
}

struct Rename {
    speaker: SpeakerId,
    turn: usize,
    input: Entity<InputState>,
    _events: Subscription,
}

/// A match turn that is being scrolled into view, and how many frames that has taken.
#[derive(Clone, Copy, Debug)]
struct Reveal {
    turn: usize,
    frames: u8,
}

/// Frames to keep nudging the page toward a turn before giving up, so a page that cannot scroll
/// far enough never loops.
const REVEAL_FRAMES: u8 = 6;

/// What one frame's measurement of the turn being revealed found.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Probe {
    /// The turn is in view.
    Seen,
    /// The page was moved by this much, and the next frame measures again.
    Moved(Pixels),
    /// The list has not laid the turn out yet.
    Waiting,
}

/// Emitted once the turn that is the current match is in view, or the page cannot show it better.
pub struct Revealed;

pub struct TranscriptView {
    session: Entity<Session>,
    player: Entity<PlayerHandle>,
    detail: Option<Arc<Detail>>,
    /// Which text of a translated transcript the list shows, as the session last said.
    text: TranscriptText,
    list: ListState,
    playing: Option<usize>,
    cursor: Option<usize>,
    focus: FocusHandle,
    rename: Option<Rename>,
    page: ScrollHandle,
    /// The session's `reveal` count this view last acted on.
    seen: u64,
    reveal: Option<Reveal>,
    /// The query the turns are drawn with marks for, so a change measures the rows again.
    marked: Option<Query>,
    /// What the list last measured its rows for.
    measured_for: Measured,
    /// The page's left margin beside the column, as the detail pane last measured it.
    gutter: f32,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<Revealed> for TranscriptView {}

impl TranscriptView {
    /// `page` is the scroll area the detail pane sits in, which a match scrolls into view.
    pub fn new(
        session: Entity<Session>,
        page: ScrollHandle,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let player = session.read(cx).player().clone();
        let on_session = cx.observe(&session, |this, _, cx| this.sync_detail(cx));
        let on_player = cx.observe(&player, |this, _, cx| this.sync_playing(cx));
        let mut view = Self {
            session,
            player,
            detail: None,
            text: TranscriptText::default(),
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            playing: None,
            cursor: None,
            focus: cx.focus_handle(),
            rename: None,
            page,
            seen: 0,
            reveal: None,
            marked: None,
            measured_for: Measured::of(&Theme::get(cx), false),
            gutter: 0.,
            _subscriptions: [on_session, on_player],
        };
        view.sync_detail(cx);
        view
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    fn turns(&self) -> &[Turn] {
        self.detail.as_deref().map_or(&[], |d| d.shown(self.text))
    }

    fn sync_detail(&mut self, cx: &mut Context<Self>) {
        let detail = self.session.read(cx).detail();
        let text = self.session.read(cx).text();
        let same = match (&detail, &self.detail) {
            (Some(new), Some(old)) => Arc::ptr_eq(new, old),
            (None, None) => true,
            _ => false,
        };
        if same {
            if text != self.text {
                // Same turns in the same order, so only their words and heights change.
                self.text = text;
                self.list.remeasure();
                cx.notify();
            }
            self.follow_search(cx);
            return;
        }
        self.text = text;
        let count = detail.as_ref().map_or(0, |d| d.turns.len());
        if count == self.list.item_count() {
            self.list.remeasure();
        } else {
            self.list.reset(count);
        }
        self.detail = detail;
        self.cursor = self.cursor.filter(|&ix| ix < count);
        if self.rename.as_ref().is_some_and(|r| r.turn >= count) {
            self.rename = None;
        }
        self.playing = playing_turn(self.turns(), self.player.read(cx).position());
        self.follow_search(cx);
        cx.notify();
    }

    /// Measures the rows again when the marks change, and shows the current match when the
    /// session asks and it is in a turn.
    fn follow_search(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let query = session.found().map(|found| found.query().clone());
        let target = match session.found().map(|found| found.current_at().place) {
            Some(Place::Turn(ix)) if ix < self.turns().len() => Some(ix),
            _ => None,
        };
        let reveal = session.reveal();
        if query != self.marked {
            self.marked = query;
            self.list.remeasure();
            cx.notify();
        }
        if reveal != self.seen {
            self.seen = reveal;
            match target {
                Some(ix) => self.show_turn(ix, cx),
                None => self.reveal = None,
            }
        }
    }

    /// Puts the cursor on turn `ix` and starts scrolling it into view.
    fn show_turn(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        self.reveal = Some(Reveal {
            turn: ix,
            frames: 0,
        });
        if self.turns().len() > PAGED_TURNS && !self.list_shows(ix) {
            self.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    }

    fn list_shows(&self, ix: usize) -> bool {
        let viewport = self.list.viewport_bounds();
        self.list
            .bounds_for_item(ix)
            .is_some_and(|row| row.top() >= viewport.top() && row.bottom() <= viewport.bottom())
    }

    /// A probe for the frame: where the turn being revealed sits, and the page moved toward it.
    /// A paged turn carries its own probe and is measured by its bounds. For `in_list` the probe
    /// sits beside the list and reads the row from the list, which has no element for a turn
    /// until it lays that turn out.
    fn probe(
        &self,
        turn: usize,
        in_list: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (page, list) = (self.page.clone(), self.list.clone());
        let this = cx.entity().downgrade();
        let (top, bottom) = (Theme::get(cx).text(TAB_ROW) + px(8.), px(DOCK_CLEARANCE));
        canvas(
            move |bounds, window, cx: &mut App| {
                let item = if in_list {
                    list.bounds_for_item(turn)
                } else {
                    Some(bounds)
                };
                let found = match item {
                    None => Probe::Waiting,
                    Some(item) => {
                        let visible = window.content_mask().bounds;
                        match shift_into_view(item, visible, top, bottom) {
                            None => Probe::Seen,
                            Some(dy) => {
                                let offset = page.offset();
                                let floor = -page.max_offset().y;
                                let y = (offset.y + dy).max(floor).min(px(0.));
                                page.set_offset(point(offset.x, y));
                                Probe::Moved(y - offset.y)
                            }
                        }
                    }
                };
                cx.defer(move |cx| {
                    let _ = this.update(cx, |this, cx| this.measured(turn, found, cx));
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    /// Takes one frame's [`Probe`]. A page that did not move, or has moved for [`REVEAL_FRAMES`],
    /// is as far as it goes.
    fn measured(&mut self, turn: usize, found: Probe, cx: &mut Context<Self>) {
        let Some(reveal) = self.reveal.as_mut().filter(|r| r.turn == turn) else {
            return;
        };
        reveal.frames += 1;
        let stuck = matches!(found, Probe::Moved(by) if by.abs() < px(0.5));
        if found == Probe::Seen || stuck || reveal.frames >= REVEAL_FRAMES {
            self.reveal = None;
            cx.emit(Revealed);
        }
        cx.notify();
    }

    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let playing = playing_turn(self.turns(), self.player.read(cx).position());
        if playing != self.playing {
            self.playing = playing;
            cx.notify();
        }
    }

    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        if let Some(range) = self.turns().get(ix).and_then(|t| t.range) {
            self.player
                .update(cx, |player, cx| player.play_from(range.start(), cx));
        }
        cx.notify();
    }

    fn move_cursor(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        self.list.scroll_to_reveal_item(ix);
        cx.notify();
    }

    fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.is_some() {
            self.finish_rename(true, window, cx);
        }
        let Some(turn) = self.turns().get(ix).cloned() else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(turn.label.clone()));
        a11y::register_text_field(&input, cx);
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.finish_rename(true, window, cx);
            }
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.cursor = Some(ix);
        self.rename = Some(Rename {
            speaker: turn.speaker,
            turn: ix,
            input,
            _events: events,
        });
        self.list.remeasure_items(ix..ix + 1);
        cx.notify();
    }

    fn finish_rename(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        if save {
            let name = rename.input.read(cx).value().trim().to_owned();
            let unchanged = self
                .turns()
                .get(rename.turn)
                .is_some_and(|t| t.label.as_ref() == name);
            if !unchanged {
                let name = (!name.is_empty()).then_some(name);
                self.session.update(cx, |session, cx| {
                    session.rename_speaker(rename.speaker, name, cx)
                });
            }
        }
        window.focus(&self.focus, cx);
        self.list.remeasure_items(rename.turn..rename.turn + 1);
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.turns().len();
        if count == 0 || self.rename.is_some() {
            return;
        }
        let at = self.cursor.unwrap_or(0);
        match event.keystroke.key.as_str() {
            "down" => self.move_cursor(self.cursor.map_or(0, |ix| (ix + 1).min(count - 1)), cx),
            "up" => self.move_cursor(at.saturating_sub(1), cx),
            "enter" if event.keystroke.modifiers.shift => self.start_rename(at, window, cx),
            "enter" => self.play_from(at, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_turn(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::get(cx);
        let Some(detail) = self.detail.clone() else {
            return div().into_any_element();
        };
        let Some(turn) = detail.shown(self.text).get(ix) else {
            return div().into_any_element();
        };
        let playing = self.playing == Some(ix);
        let sounding = playing && self.player.read(cx).is_playing();
        let current = self.cursor == Some(ix);
        let ring = current && self.focus.is_focused(window);
        let spoken = spoken_turn(turn);
        // Matches are found in what was spoken, so they are drawn only on the spoken text.
        let marks: Vec<Range<usize>> = if detail.translated(self.text) {
            Vec::new()
        } else {
            self.session
                .read(cx)
                .found()
                .map(|found| found.in_turn(ix).iter().map(|m| m.span.clone()).collect())
                .unwrap_or_default()
        };
        let mut described = Vec::new();
        if turn.range.is_none() {
            described.push("No timestamp, so it cannot play from here");
        }
        if sounding {
            described.push("Playing");
        }
        if !marks.is_empty() {
            described.push("Search match");
        }
        let probing =
            self.reveal.is_some_and(|r| r.turn == ix) && self.turns().len() <= PAGED_TURNS;

        let name =
            match self.rename.as_ref().filter(|r| r.turn == ix) {
                Some(rename) => div()
                    .w(theme.text(150.))
                    .on_action(cx.listener(|this, _: &Escape, window, cx| {
                        this.finish_rename(false, window, cx)
                    }))
                    .child(
                        Input::new(&rename.input)
                            .id(SharedString::from(format!("turn-{ix}-rename")))
                            .text_size(theme.text(14.))
                            .aria_label(format!("Name for {}", turn.label)),
                    )
                    .into_any_element(),
                None => a11y::node(
                    format!("turn-{ix}-speaker"),
                    Role::Button,
                    format!("Rename {}", turn.label),
                )
                .aria_keyshortcuts("Shift+Return")
                .min_h(px(24.))
                .min_w(px(24.))
                .flex()
                .items_center()
                .text_size(theme.text(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.start_rename(ix, window, cx);
                }))
                .child(turn.label.clone())
                .into_any_element(),
            };
        let hint = self.rename.as_ref().filter(|r| r.turn == ix).map(|rename| {
            let hint = rename_hint(detail.shown(self.text), rename.speaker, &turn.label);
            a11y::node(format!("turn-{ix}-rename-hint"), Role::Label, hint.clone())
                .text_size(theme.text(12.))
                .text_color(theme.text_secondary)
                .child(hint)
        });
        let timestamp = turn.range.map(|range| {
            let at = a11y::clock(range.start());
            a11y::node(
                format!("turn-{ix}-time"),
                Role::Button,
                format!("Play from {at}"),
            )
            .aria_keyshortcuts("Return")
            .min_h(px(24.))
            .min_w(px(24.))
            .flex()
            .items_center()
            .text_size(theme.text(12.))
            .text_color(theme.accent_text)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.play_from(ix, cx);
            }))
            .child(at)
        });

        let presentation = theme.presentation(f32::from(window.viewport_size().width));
        if presentation.script_transcript && self.rename.as_ref().is_none_or(|r| r.turn != ix) {
            let script = Script {
                spoken,
                described,
                timestamp,
                marks: &marks,
                probing,
                presentation,
            };
            return self.script_turn(ix, turn, script, window, cx);
        }

        a11y::node(format!("turn-{ix}"), Role::ListItem, spoken)
            .aria_position_in_set(ix + 1)
            .aria_size_of_set(detail.shown(self.text).len())
            .when(current, |el| el.aria_active_descendant())
            .when(!described.is_empty(), |el| {
                el.aria_description(described.join(". "))
            })
            .relative()
            .flex()
            .gap(px(12.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(10.))
            .border_2()
            .border_color(if ring {
                theme.focus_ring(false)
            } else {
                transparent_black()
            })
            .when(playing, |el| el.bg(theme.playing_bg))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.cursor = Some(ix);
                window.focus(&this.focus, cx);
                cx.notify();
            }))
            .child(
                div()
                    .id(format!("turn-{ix}-avatar"))
                    .test_support()
                    .min_w(px(30.))
                    .min_h(px(30.))
                    .flex_none()
                    .self_start()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme.avatar_bg)
                    .text_size(theme.text(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(initial(turn)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(name)
                            .children(language_tag(ix, turn, &theme))
                            .children(timestamp)
                            .when(sounding, |row| {
                                row.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .text_size(theme.text(12.))
                                        .text_color(theme.accent_text)
                                        .child(Icon::Play.view(12., theme.accent_text))
                                        .child("Playing"),
                                )
                            }),
                    )
                    .children(hint)
                    .child(
                        div()
                            .text_size(theme.text(15.))
                            .line_height(relative(1.6))
                            .child(marked(
                                turn.text.clone(),
                                &marks,
                                FontWeight::SEMIBOLD,
                                false,
                                &theme,
                            )),
                    ),
            )
            .when(probing, |el| el.child(self.probe(ix, false, cx)))
            .into_any_element()
    }
}

impl TranscriptView {
    /// Book's play-script turn (`book-theme.md` section 4): "SAM. Okay, …" as one serif
    /// paragraph, the timestamp hanging in the left margin or, below 900 pt, before it. The name
    /// is still the Rename button, laid over the name's run at its measured width, and the turn
    /// reads to VoiceOver exactly as in Modern.
    fn script_turn(
        &self,
        ix: usize,
        turn: &Turn,
        script: Script,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Script {
            spoken,
            described,
            timestamp,
            marks,
            probing,
            presentation,
        } = script;
        let theme = Theme::get(cx);
        let count = self.turns().len();
        let playing = self.playing == Some(ix);
        let sounding = playing && self.player.read(cx).is_playing();
        let current = self.cursor == Some(ix);
        let ring = current && self.focus.is_focused(window);
        let size = theme.text(17.);
        let line_height = size * 1.7;
        let family = SharedString::from(theme.reading_font.family().unwrap_or_default());
        let font = |weight, features| Font {
            family: family.clone(),
            features,
            fallbacks: None,
            weight,
            style: FontStyle::Normal,
        };
        let name_font = font(FontWeight::MEDIUM, fonts::small_caps());
        let run = |len, font| TextRun {
            len,
            font,
            color: theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let name = format!("{}.", turn.label);
        let name_width = window
            .text_system()
            .shape_line(
                name.clone().into(),
                size,
                &[run(name.len(), name_font.clone())],
                None,
            )
            .width;
        let body_font = font(FontWeight::NORMAL, Default::default());
        let tag = turn.tag.as_ref().map(|tag| format!("({tag})"));
        let lead = match &tag {
            Some(tag) => format!("{name} {tag}"),
            None => name.clone(),
        };
        let text = format!("{lead} {}", turn.text);
        let mut runs = vec![run(name.len(), name_font)];
        if let Some(tag) = &tag {
            runs.push(TextRun {
                color: theme.text_secondary,
                ..run(tag.len() + 1, body_font.clone())
            });
        }
        let mut at = lead.len();
        for mark in marks {
            let (start, end) = (lead.len() + 1 + mark.start, lead.len() + 1 + mark.end);
            if start < at || end > text.len() {
                continue;
            }
            if start > at {
                runs.push(run(start - at, body_font.clone()));
            }
            runs.push(TextRun {
                background_color: Some(theme.search_match),
                ..run(end - start, font(FontWeight::SEMIBOLD, Default::default()))
            });
            at = end;
        }
        if text.len() > at {
            runs.push(run(text.len() - at, body_font));
        }
        let paragraph = StyledText::new(text).with_runs(runs);
        let speaker = a11y::node(
            format!("turn-{ix}-speaker"),
            Role::Button,
            format!("Rename {}", turn.label),
        )
        .aria_keyshortcuts("Shift+Return")
        .absolute()
        .top_0()
        .left_0()
        .w(name_width)
        .min_w(px(24.))
        .h(line_height)
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.start_rename(ix, window, cx);
        }));
        let playing_word = sounding.then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(theme.text(12.))
                .text_color(theme.accent_text)
                .child(Icon::Play.view(12., theme.accent_text))
                .child("Playing")
        });
        let hang = hangs(presentation, self.gutter);
        let timestamp = timestamp.map(|t| t.font_features(theme::tabular()));
        let stamp = if hang {
            div()
                .flex_none()
                .w(px(MARGIN))
                .flex()
                .flex_col()
                .items_end()
                .children(timestamp.map(|t| t.h(line_height)))
                .children(playing_word)
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .children(timestamp)
                .children(playing_word)
        };
        let body = div()
            .relative()
            .flex_1()
            .min_w_0()
            .text_size(size)
            .line_height(relative(1.7))
            .child(theme.reading(div()).child(paragraph))
            .child(speaker);
        let outline = playing.then(|| theme.highlight_outline()).flatten();
        a11y::node(format!("turn-{ix}"), Role::ListItem, spoken)
            .aria_position_in_set(ix + 1)
            .aria_size_of_set(count)
            .when(current, |el| el.aria_active_descendant())
            .when(!described.is_empty(), |el| {
                el.aria_description(described.join(". "))
            })
            .flex()
            .when(hang, |el| el.gap(px(MARGIN_GAP)))
            .when(!hang, |el| el.flex_col().gap(px(2.)))
            .pl(px(if hang { 0. } else { 12. }))
            .pr(px(12.))
            .py(px(6.))
            .rounded(px(6.))
            .border_2()
            .border_color(if ring {
                theme.focus_ring(false)
            } else {
                transparent_black()
            })
            .when(playing, |el| el.bg(theme.playing_bg))
            .when_some(outline, |el, color| {
                el.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(color),
                )
            })
            .relative()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.cursor = Some(ix);
                window.focus(&this.focus, cx);
                cx.notify();
            }))
            .child(stamp)
            .child(body)
            .when(probing, |el| el.child(self.probe(ix, false, cx)))
            .into_any_element()
    }
}

/// What a Book script turn shows besides the turn itself.
struct Script<'a> {
    spoken: String,
    described: Vec<&'static str>,
    timestamp: Option<a11y::Node>,
    marks: &'a [Range<usize>],
    probing: bool,
    presentation: Presentation,
}

/// What changes a turn's height besides its text: the text scale and the theme's type.
#[derive(Clone, Copy, PartialEq)]
struct Measured {
    text_scale: f32,
    theme: theme::ThemeName,
    hang: bool,
}

impl Measured {
    fn of(theme: &Theme, hang: bool) -> Measured {
        Measured {
            text_scale: theme.text_scale,
            theme: theme.name,
            hang,
        }
    }
}

/// Whether the script's times hang left of the column: the flag says so at this width, and the
/// page's margin is wide enough to hold them. Otherwise they sit above the turn.
pub fn hangs(presentation: Presentation, gutter: f32) -> bool {
    presentation.hanging_timestamps && gutter >= MARGIN + MARGIN_GAP
}

impl TranscriptView {
    /// The detail pane reports the margin beside its column after each layout that moves it.
    pub fn set_gutter(&mut self, gutter: f32, cx: &mut Context<Self>) {
        if self.gutter != gutter {
            self.gutter = gutter;
            cx.notify();
        }
    }
}

/// The script's timestamp column and the gap after it. The transcript moves left by both so the
/// text keeps the page's measure while the times hang in the margin.
pub const MARGIN: f32 = 56.;
pub const MARGIN_GAP: f32 = 16.;

impl Render for TranscriptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let measured_for = Measured::of(
            &theme,
            hangs(
                theme.presentation(f32::from(window.viewport_size().width)),
                self.gutter,
            ),
        );
        if measured_for != self.measured_for {
            self.measured_for = measured_for;
            self.list.remeasure();
        }
        let count = self.turns().len();
        if self.cursor.is_none() && count > 0 && self.focus.is_focused(window) {
            self.cursor = Some(self.playing.unwrap_or(0));
        }
        if count == 0 {
            let empty = if self.detail.is_some() {
                "No transcript yet."
            } else {
                ""
            };
            // Nothing to move through, so the empty list is no Tab stop.
            return a11y::node("transcript", Role::List, "Transcript")
                .w_full()
                .text_size(theme.text(15.))
                .text_color(theme.text_secondary)
                .child(empty)
                .into_any_element();
        }
        let hang = hangs(
            theme.presentation(f32::from(window.viewport_size().width)),
            self.gutter,
        );
        let container = a11y::node("transcript", Role::List, "Transcript")
            .track_focus(&self.focus.clone().tab_stop(true))
            .on_key_down(cx.listener(Self::on_key))
            // The script's timestamps hang left of the page; stretched with a negative margin,
            // the list grows by the margin instead of narrowing the text.
            .map(|el| {
                if hang {
                    el.ml(px(-(MARGIN + MARGIN_GAP)))
                } else {
                    el.w_full()
                }
            });
        if count <= PAGED_TURNS {
            let turns: Vec<AnyElement> = (0..count)
                .map(|ix| self.render_turn(ix, window, cx))
                .collect();
            return container
                .flex()
                .flex_col()
                .children(turns)
                .into_any_element();
        }
        let height = window.viewport_size().height - px(TOOLBAR) - Theme::get(cx).text(TAB_ROW);
        // The clearance sits outside the list: GPUI's list reports its scroll range without its
        // own padding, and the chaining rule reads that range to find the bottom.
        let probe = self.reveal.map(|r| self.probe(r.turn, true, cx));
        container
            .relative()
            .child(
                div().pb(px(DOCK_CLEARANCE)).child(a11y::nested_list(
                    "transcript-scroll",
                    &self.list,
                    list(
                        self.list.clone(),
                        cx.processor(|this, ix, window, cx| this.render_turn(ix, window, cx)),
                    )
                    .w_full()
                    .h(height - px(DOCK_CLEARANCE)),
                )),
            )
            .children(probe)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use dory_core::domain::TimeRange;

    use super::*;

    fn turn(speaker: u16, range: Option<(u64, u64)>) -> Turn {
        Turn {
            speaker: SpeakerId(speaker),
            label: format!("Speaker {speaker}").into(),
            range: range.and_then(|(a, b)| TimeRange::new(Ms(a), Ms(b))),
            text: String::new(),
            language: None,
            tag: None,
            translated_from: None,
        }
    }

    #[test]
    fn script_times_hang_only_where_the_margin_holds_them() {
        let script = Presentation {
            script_transcript: true,
            hanging_timestamps: true,
            ..Presentation::default()
        };
        assert!(hangs(script, 72.));
        assert!(
            !hangs(script, 71.),
            "a narrower margin keeps the times above the turn"
        );
        let narrow = Presentation {
            hanging_timestamps: false,
            ..script
        };
        assert!(!hangs(narrow, 400.), "below 900 pt they never hang");
    }

    #[test]
    fn the_playing_turn_is_the_one_whose_range_holds_the_playhead() {
        let turns = [
            turn(1, Some((0, 10))),
            turn(2, Some((10, 20))),
            turn(1, Some((25, 40))),
        ];
        let cases = [
            (0, Some(0)),
            (9, Some(0)),
            (10, Some(1)),
            (22, None),
            (25, Some(2)),
            (40, None),
        ];
        for (at, want) in cases {
            assert_eq!(playing_turn(&turns, Ms(at)), want, "at {at}");
        }
        assert_eq!(playing_turn(&[turn(1, None), turn(2, None)], Ms(5)), None);
        assert_eq!(playing_turn(&[], Ms(5)), None);
    }

    #[test]
    fn the_rename_hint_counts_the_speakers_turns() {
        let turns = [turn(3, None), turn(1, None), turn(3, None)];
        assert_eq!(
            rename_hint(&turns, SpeakerId(3), "Speaker 3"),
            "Renames Speaker 3 in all 2 turns. Return to save, Escape to cancel."
        );
        assert_eq!(
            rename_hint(&turns, SpeakerId(1), "Speaker 1"),
            "Renames Speaker 1 in its one turn. Return to save, Escape to cancel."
        );
    }

    #[test]
    fn default_labels_show_their_number_and_names_their_initial() {
        let mut named = turn(3, None);
        assert_eq!(initial(&named), "3");
        named.label = "alex".into();
        assert_eq!(initial(&named), "A");
    }
}

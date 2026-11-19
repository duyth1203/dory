//! The detail pane for a selected recording (UX-2 to UX-4).
//!
//! Inside the shell's centered 720 pt column, top to bottom: the editable title (30 bold, saved
//! with `Session::set_title`), the metadata line (date, duration, detected languages, layout; 14
//! secondary), the card (the processing card from Stop, then the player in the same place, see
//! `player_view`), then [`Body`]: the tab row and tab content, or the state that takes their
//! place. The page scrolls as one surface, the shell's.
//!
//! While a recording is processed, the Transcript tab shows a placeholder, then the lines the
//! engine writes for this recording as they arrive, with a footnote; the Summary tab says when the
//! summary comes, and shows the dots glyph while it is written. `AppState` picks the tab: the
//! transcript while it is being written, then the summary, unless the user picked one.
//!
//! The player card docks to a slim bar 14 pt above the column's bottom edge once it scrolls out
//! of view, and the tab row sticks to the top once it would scroll off, showing the title on its
//! left. Both measure their slot during prepaint against the scroll area's clip, so the shell
//! keeps the one scroll container. One `PlayerView`, so focus and VoiceOver see one slider.
//!
//! The tab row reads `app.tab`, which Command-1 and Command-2 set at the window root. Its Copy,
//! Export, and ⋯ buttons dispatch `crate::actions`, which the shell hands to `export.rs`.
//!
//! While a search is active (UX-32) the session holds the recording's matches (`crate::find`).
//! The pane then marks the title, picks the tab the first match is on when the recording opens,
//! and shows "2 of 9 matches" with Previous and Next in the tab row. The transcript and summary
//! views mark their own text and scroll to the current match. `match-revealed ms=` waits for the
//! first of them to report the match in view.
//!
//! The shell creates one `DetailPane` per selection and drops it when the selection changes.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use chrono::{Local, TimeZone};
use dory_core::domain::{
    ChannelLayout, Goal, Recording, RecordingId, RecordingRow, Segment, Stage, SummaryFlag,
    Transcript,
};
use dory_core::models::{Llm, ModelId};

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::{
    Anchor, AnchoredPositionMode, AnyElement, App, AppContext as _, Bounds, Context, Entity,
    FocusHandle, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Pixels, Render, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, TestSupportExt as _, WeakEntity, Window,
    anchored, canvas, deferred, div, point, prelude::FluentBuilder as _, px, relative,
};

use crate::a11y;
use crate::actions::{
    CopySummary, CopyTranscript, ExportMarkdown, ExportSrt, ExportText, FindNext, FindPrevious,
};
use crate::app::{
    AppState, Detail, DetailTab, Loadable, Saved, Session, Shown, SummaryVersion, TranscriptText,
};
use crate::combobox::LanguageCombobox;
use crate::export::{ExportFormat, Has};
use crate::find::Place;
use crate::fonts;
use crate::icons::{self, Icon};
use crate::library;
use crate::marks::marked;
use crate::menus;
use crate::motion::{self, ease_standard, elapsed};
use crate::player_view::PlayerView;
use crate::processing::{self, Fix, Phase};
use crate::summary_view::{self, SummaryText};
use crate::theme::Theme;
use crate::transcript_view::{self, TranscriptView};
use crate::{glyphs, set_up_summaries};

/// What the page shows under the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// Still recording: the transcript starts after Stop.
    Recording,
    /// No frames landed, so there is nothing to play or transcribe.
    NoAudio,
    /// Audio and no job: the finish card (UX-11).
    Finish,
    /// Transcribed, but the transcript is empty and there is no summary.
    NothingSaid,
    /// The tab row and its content, from Stop on.
    Tabs,
}

impl Body {
    /// `phase` is the selected row's; `None` when the row is outside the list.
    pub fn derive(detail: &Detail, phase: Option<&Phase>) -> Body {
        match phase {
            Some(Phase::Recording) => return Body::Recording,
            Some(phase) if phase.is_processing() => return Body::Tabs,
            _ => {}
        }
        if detail.recording.take.is_none() {
            return Body::NoAudio;
        }
        let Some(transcript) = &detail.transcript else {
            return match phase {
                Some(Phase::Failed(_)) => Body::Tabs,
                _ => Body::Finish,
            };
        };
        if transcript.segments.is_empty() && detail.summary.is_none() {
            Body::NothingSaid
        } else {
            Body::Tabs
        }
    }
}

/// The title as the sidebar shows it, with the same date-based fallback.
pub fn shown_title<Tz: TimeZone>(recording: &Recording, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let row = RecordingRow {
        recording: recording.clone(),
        job: None,
        has_transcript: false,
        summary: SummaryFlag::None,
    };
    library::title(&row, tz)
}

/// "Thu 12 Nov 2026, 10:30 · 48 min · English · Mic and system audio". The detected language sits
/// next to the duration (UX-26).
pub fn meta_line<Tz: TimeZone>(
    recording: &Recording,
    transcript: Option<&Transcript>,
    tz: &Tz,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let mut parts = vec![
        library::local(recording.created_at, tz)
            .format("%a %-d %b %Y, %H:%M")
            .to_string(),
    ];
    if let Some(take) = recording.take {
        parts.push(library::short_duration(take.duration));
    }
    let languages: Vec<String> = transcript
        .map(|t| {
            t.detected_languages
                .iter()
                .map(crate::languages::primary_name)
                .collect()
        })
        .unwrap_or_default();
    if !languages.is_empty() {
        parts.push(languages.join(" and "));
    }
    parts.push(
        match recording.layout {
            ChannelLayout::MicSystem => "Mic and system audio",
            ChannelLayout::MicOnly => "Mic only",
            ChannelLayout::SystemOnly => "System audio only",
            ChannelLayout::Mixed => "Mixed audio",
        }
        .to_owned(),
    );
    parts.join(" · ")
}

/// Where the column, the player card, and the tab row sat in the last prepaint.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Pinning {
    /// The scroll area's clip.
    visible: Bounds<Pixels>,
    column_x: Pixels,
    column_w: Pixels,
    card_h: Pixels,
    tabs_h: Pixels,
    docked: bool,
    stuck: bool,
}

#[derive(Clone, Copy)]
enum Slot {
    Card,
    Tabs,
}

struct TitleEdit {
    field: Entity<InputState>,
    _events: Subscription,
}

/// What the Summary tab's **Translate…** slot shows.
#[derive(Clone, Copy)]
enum TranslateSlot {
    Ready,
    Translating,
    /// Greyed out with [`menus::WAITS`] as its tooltip while another job for the recording is
    /// underway.
    Waiting,
}

pub struct DetailPane {
    app: Entity<AppState>,
    session: Entity<Session>,
    player: Entity<PlayerView>,
    transcript: Entity<TranscriptView>,
    summary: Entity<SummaryText>,
    /// **Translate…** in the Summary tab's action row.
    translate: Entity<LanguageCombobox>,
    page: ScrollHandle,
    tab_focus: [FocusHandle; 2],
    /// The Original and Translated buttons.
    text_focus: [FocusHandle; 2],
    /// The summary's Original and Translated buttons.
    summary_focus: [FocusHandle; 2],
    title_focus: FocusHandle,
    title_edit: Option<TitleEdit>,
    /// What this frame lays out by.
    pinning: Pinning,
    /// What prepaint measured; a change re-renders with it.
    measured: Rc<Cell<Pinning>>,
    finish_focus: FocusHandle,
    transcript_only_focus: FocusHandle,
    always_focus: FocusHandle,
    fix_focus: FocusHandle,
    /// The finish card's "Always do this when I stop".
    always: bool,
    /// When each streamed line arrived, in order, for its fade in.
    arrivals: Vec<Instant>,
    /// Set when this recording's stop is saved; the finish card takes focus once it shows.
    focus_finish: bool,
    /// When the selection made this pane, until its loaded content first paints, or until the
    /// match it opens at is in view.
    opened: Option<Instant>,
    /// The session's `reveal` count this pane last acted on.
    seen: u64,
    _reveals: [Subscription; 2],
}

impl DetailPane {
    /// `page` is the scroll area the pane sits in, which a match scrolls into view.
    pub fn new(
        app: Entity<AppState>,
        session: Entity<Session>,
        page: ScrollHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&session, |this, _, cx| {
            this.show_first_match(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&app, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&app, |this, _, Saved(id): &Saved, cx| {
            if this.session.read(cx).id() == id {
                this.focus_finish = true;
                cx.notify();
            }
        })
        .detach();
        let handle = session.read(cx).player().clone();
        let player = cx.new(|cx| PlayerView::new(handle, app.clone(), cx));
        let transcript =
            cx.new(|cx| TranscriptView::new(session.clone(), page.clone(), window, cx));
        let summary = cx.new(|cx| SummaryText::new(session.clone(), page.clone(), cx));
        let translate =
            cx.new(|cx| LanguageCombobox::translate(app.clone(), session.clone(), window, cx));
        let revealed = [
            cx.subscribe(
                &transcript,
                |this, _, _: &crate::transcript_view::Revealed, _| this.reveal_done(),
            ),
            cx.subscribe(&summary, |this, _, _: &summary_view::Revealed, _| {
                this.reveal_done()
            }),
        ];
        Self {
            app,
            session,
            player,
            transcript,
            summary,
            translate,
            page,
            tab_focus: [cx.focus_handle(), cx.focus_handle()],
            text_focus: [cx.focus_handle(), cx.focus_handle()],
            summary_focus: [cx.focus_handle(), cx.focus_handle()],
            title_focus: cx.focus_handle(),
            title_edit: None,
            pinning: Pinning::default(),
            measured: Rc::default(),
            finish_focus: cx.focus_handle(),
            transcript_only_focus: cx.focus_handle(),
            always_focus: cx.focus_handle(),
            fix_focus: cx.focus_handle(),
            always: false,
            arrivals: Vec::new(),
            focus_finish: false,
            opened: Some(Instant::now()),
            seen: 0,
            _reveals: revealed,
        }
    }

    /// Opens **Translate…** to translate with `llm` this one time, or with the Settings choice for
    /// `None`. **Translate Summary with…** and the launch previews in `show` use it.
    pub fn open_translate_with(
        &mut self,
        llm: Option<ModelId<Llm>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.translate
            .update(cx, |translate, cx| translate.open_with(llm, window, cx));
    }

    /// When the session starts showing a match, shows the tab that match is on. Opening a hit
    /// and a new search both count, and neither is the user's pick of a tab.
    fn show_first_match(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        if session.reveal() == self.seen {
            return;
        }
        self.seen = session.reveal();
        let place = session.found().map(|found| found.current_at().place);
        if let Some(tab) = place.and_then(Place::tab) {
            self.app.update(cx, |app, cx| app.show_match_tab(tab, cx));
        }
        if place == Some(Place::Title) {
            self.scroll_to_title();
        }
    }

    /// The title heads the page, so the page's top shows it.
    fn scroll_to_title(&self) {
        let offset = self.page.offset();
        self.page.set_offset(point(offset.x, px(0.)));
    }

    /// A view reports the current match in view, which ends the open's `match-revealed` time.
    fn reveal_done(&mut self) {
        if let Some(opened) = self.opened.take() {
            let ms = opened.elapsed().as_secs_f64() * 1000.0;
            eprintln!("match-revealed ms={ms:.1}");
        }
    }

    /// Where Find Next and Find Previous leave the keyboard when they land on a turn, so Return
    /// plays from it.
    pub fn focus_transcript(&self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.transcript.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
    }

    pub fn session(&self) -> &Entity<Session> {
        &self.session
    }

    pub fn summary(&self) -> &Entity<SummaryText> {
        &self.summary
    }

    /// Where F6 lands in the detail pane: the title, the first thing in it.
    pub fn first_focus(&self) -> &FocusHandle {
        &self.title_focus
    }

    pub fn is_editing_title(&self) -> bool {
        self.title_edit.is_some()
    }

    /// Swaps the title for a field holding it, all selected, and scrolls it into view: Rename
    /// also comes from menus and the sidebar while the page is scrolled down.
    pub fn edit_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.session.read(cx).detail() else {
            return;
        };
        self.scroll_to_title();
        let current = detail.recording.title.clone().unwrap_or_default();
        let placeholder = shown_title(&detail.recording, &Local);
        let field = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(current)
        });
        a11y::register_text_field(&field, cx);
        field.update(cx, |field, cx| {
            field.focus(window, cx);
            field.select_all(window, cx);
        });
        let events = cx.subscribe_in(&field, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.finish_title_edit(true, window, cx);
            }
        });
        self.title_edit = Some(TitleEdit {
            field,
            _events: events,
        });
        cx.notify();
    }

    /// Saves the field when `commit` and it changed; either way the title comes back.
    fn finish_title_edit(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.title_edit.take() else {
            return;
        };
        if commit {
            let typed = edit.field.read(cx).value().trim().to_owned();
            let session = self.session.read(cx);
            let current = session
                .detail()
                .and_then(|d| d.recording.title.clone())
                .unwrap_or_default();
            if typed != current.trim() {
                let title = (!typed.is_empty()).then_some(typed);
                self.session
                    .update(cx, |session, cx| session.set_title(title, cx));
            }
        }
        window.focus(&self.title_focus, cx);
        cx.notify();
    }

    fn relayout(&mut self, cx: &mut Context<Self>) {
        let next = self.measured.get();
        if next == self.pinning {
            return;
        }
        if next.docked != self.pinning.docked {
            self.player
                .update(cx, |player, cx| player.set_docked(next.docked, cx));
        }
        self.pinning = next;
        let gutter = f32::from(next.column_x - next.visible.left());
        self.transcript
            .update(cx, |transcript, cx| transcript.set_gutter(gutter, cx));
        cx.notify();
    }

    /// Measures `slot` during prepaint, and re-renders the pane when pinning changes.
    fn watch(&self, slot: Slot, cx: &mut Context<Self>) -> impl IntoElement {
        let laid_out = self.pinning;
        let measured = Rc::clone(&self.measured);
        let pane: WeakEntity<Self> = cx.entity().downgrade();
        canvas(
            move |bounds, window, cx: &mut App| {
                let visible = window.content_mask().bounds;
                let mut next = measured.get();
                next.visible = visible;
                next.column_x = bounds.origin.x;
                next.column_w = bounds.size.width;
                match slot {
                    Slot::Card => {
                        next.card_h = bounds.size.height;
                        next.docked = bounds.bottom() < visible.top();
                    }
                    Slot::Tabs => {
                        next.tabs_h = bounds.size.height;
                        next.stuck = bounds.top() < visible.top();
                    }
                }
                measured.set(next);
                if next != laid_out {
                    cx.defer(move |cx| {
                        let _ = pane.update(cx, |pane, cx| pane.relayout(cx));
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    fn title(
        &self,
        detail: &Detail,
        marks: &[std::ops::Range<usize>],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(edit) = &self.title_edit {
            return a11y::node("title-editor", Role::Group, "Title")
                .border_2()
                .border_color(theme.ring)
                .rounded(px(6.))
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.finish_title_edit(false, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    // Kit's frame fixes its height and line height for body text; the
                    // title's own line height and an auto height keep 30 pt text whole.
                    Input::new(&edit.field)
                        .aria_label("Title")
                        .appearance(false)
                        .map(|el| title_type(el, theme))
                        .h_auto(),
                )
                .into_any_element();
        }
        let title = shown_title(&detail.recording, &Local);
        a11y::focusable(
            a11y::node("title", Role::Heading, title.clone()),
            &self.title_focus,
            theme,
            false,
        )
        .aria_level(1)
        .aria_description("Press Return to rename")
        .on_click(cx.listener(|this, _, window, cx| this.edit_title(window, cx)))
        .rounded(px(6.))
        .map(|el| title_type(el, theme))
        .child(marked(title, marks, FontWeight::EXTRA_BOLD, false, theme))
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn tab_row(
        &self,
        tab: DetailTab,
        title: Option<String>,
        detail: &Detail,
        shown: Shown,
        translate: TranslateSlot,
        summarizing: Option<Option<u64>>,
        readout: Option<String>,
        phase: Option<&Phase>,
        theme: &Theme,
    ) -> AnyElement {
        let tab_button = |id: &'static str,
                          label: &'static str,
                          which: DetailTab,
                          ix: usize,
                          leading: Option<AnyElement>| {
            let app = self.app.clone();
            let tab_el = a11y::tab_with(
                id,
                label,
                leading,
                tab == which,
                &self.tab_focus[ix],
                theme,
                move |_, cx| app.update(cx, |app, cx| app.set_tab(which, cx)),
            )
            .min_h(theme.text(34.));
            // Book: serif small caps, the selected tab in ink and the other in secondary.
            if theme.serif() {
                theme
                    .reading(tab_el)
                    .font_features(fonts::small_caps())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if tab == which {
                        theme.text
                    } else {
                        theme.text_secondary
                    })
            } else {
                tab_el
            }
        };
        let dots =
            summarizing.map(|at| glyphs::dots(theme.accent_text, 14., at).into_any_element());
        let (copy_label, copy_name, copy): (&str, &str, Box<dyn gpui_kit::Action>) = match tab {
            DetailTab::Summary => ("Copy", "Copy summary", Box::new(CopySummary)),
            DetailTab::Transcript => (
                "Copy Transcript",
                "Copy transcript",
                Box::new(CopyTranscript),
            ),
        };
        let id = detail.recording.id.clone();
        let action = |id: &'static str, label: &str| {
            Button::new(id)
                .ghost()
                .compact()
                .label(label.to_owned())
                .accessibility_id(id)
                .text_color(theme.accent_text)
        };
        let has = Has::of(detail, phase);
        let (copy_on, export_on) = (has.copy(tab), has.export(tab));
        let gated = |button: Button, on: bool| {
            if on {
                button
            } else {
                button.disabled(true).text_color(theme.text_secondary)
            }
        };
        // Copy, Export, and Translate wrap among themselves. More actions never wraps off: its
        // menu holds the only way to Rename and Move to Trash from here.
        let wrapping = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap(px(4.))
            .flex_shrink(1.)
            .min_w_0()
            .child(gated(
                action("copy", copy_label)
                    .accessibility_label(copy_name.to_owned())
                    .on_click(move |_, window, cx| window.dispatch_action(copy.boxed_clone(), cx)),
                copy_on,
            ))
            .child(
                gated(
                    action("export", "Export")
                        .accessibility_label("Export")
                        .child(Icon::ChevronDown.view(
                            12.,
                            if export_on {
                                theme.accent_text
                            } else {
                                theme.text_secondary
                            },
                        )),
                    export_on,
                )
                .dropdown_menu(move |menu, _, _| {
                    menu.menu_with_disabled(
                        "Markdown",
                        Box::new(ExportMarkdown),
                        !has.can_export(ExportFormat::Markdown),
                    )
                    .menu_with_disabled(
                        "Plain Text",
                        Box::new(ExportText),
                        !has.can_export(ExportFormat::Text),
                    )
                    .menu_with_disabled(
                        "Subtitles (SRT)",
                        Box::new(ExportSrt),
                        !has.can_export(ExportFormat::Srt),
                    )
                }),
            )
            .children(
                (tab == DetailTab::Summary && detail.summary.is_some()).then(|| match translate {
                    TranslateSlot::Translating => action("translate", "Translating…")
                        .accessibility_label("Translating the summary")
                        .into_any_element(),
                    TranslateSlot::Waiting => gated(
                        action("translate", "Translate…")
                            .accessibility_label("Translate summary")
                            .tooltip(menus::WAITS)
                            .child(Icon::ChevronDown.view(12., theme.text_secondary)),
                        false,
                    )
                    .into_any_element(),
                    TranslateSlot::Ready => self.translate.clone().into_any_element(),
                }),
            );
        let actions = div()
            .flex()
            .items_start()
            .gap(px(4.))
            .flex_shrink(1.)
            .min_w_0()
            .child(wrapping)
            .child(
                div().flex_none().child(
                    Button::new("more")
                        .ghost()
                        .compact()
                        .accessibility_id("more")
                        .child(Icon::Ellipsis.view(14., theme.accent_text))
                        .accessibility_label("More actions")
                        .dropdown_menu(move |menu, window, cx| {
                            menus::recording_menu(menu, &id, window, cx)
                        }),
                ),
            );
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(16.))
            .gap_y(px(4.))
            .border_b_1()
            .border_color(theme.divider)
            .bg(theme.window_bg)
            .children(title.map(|title| {
                div()
                    .flex_shrink(1.)
                    .min_w_0()
                    .truncate()
                    .text_size(theme.text(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title)
            }))
            .child(
                a11y::node("detail-tabs", Role::TabList, "Recording views")
                    .flex()
                    .gap(px(16.))
                    .child(tab_button(
                        "summary-tab",
                        "Summary",
                        DetailTab::Summary,
                        0,
                        dots,
                    ))
                    .child(tab_button(
                        "transcript-tab",
                        "Transcript",
                        DetailTab::Transcript,
                        1,
                        None,
                    )),
            )
            .children(
                (tab == DetailTab::Transcript && detail.translated_turns.is_some())
                    .then(|| self.text_switch(detail, shown.transcript, theme)),
            )
            .children(
                (tab == DetailTab::Summary && detail.summary_translation.is_some())
                    .then(|| self.summary_switch(detail, shown.summary, theme)),
            )
            .child(div().flex_1())
            .children(readout.map(|readout| self.find_group(readout, &action, theme)))
            .child(actions)
            .into_any_element()
    }

    /// "2 of 9 matches" with Previous and Next, in the tab row while a search has matches here.
    /// The words are not a live region: each step is announced once, by `AppState::step_match`.
    fn find_group<B: Fn(&'static str, &str) -> Button>(
        &self,
        readout: String,
        action: &B,
        theme: &Theme,
    ) -> AnyElement {
        let step = |id: &'static str,
                    label: &'static str,
                    spoken: &'static str,
                    go: Box<dyn gpui_kit::Action>| {
            action(id, label)
                .accessibility_label(spoken)
                .on_click(move |_, window, cx| window.dispatch_action(go.boxed_clone(), cx))
        };
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                a11y::node("match-readout", Role::Label, readout.clone())
                    .font_features(crate::theme::tabular())
                    .text_size(theme.text(13.))
                    .text_color(theme.text_secondary)
                    .whitespace_nowrap()
                    .child(readout),
            )
            .child(step(
                "find-previous",
                "Previous",
                "Previous match",
                Box::new(FindPrevious),
            ))
            .child(step("find-next", "Next", "Next match", Box::new(FindNext)))
            .into_any_element()
    }

    /// The Original / Translated switch in the transcript's header. Translated is first
    /// chosen, and the choice is what the list, Copy, and Export show.
    fn text_switch(&self, detail: &Detail, text: TranscriptText, theme: &Theme) -> AnyElement {
        let written_in = detail
            .translation
            .as_ref()
            .map(|t| crate::languages::name(&t.target));
        let session = self.session.clone();
        self.switch(
            ("transcript-text", "Transcript text"),
            [
                ("text-original", "Original"),
                ("text-translated", "Translated"),
            ],
            &self.text_focus,
            detail.translated(text),
            [
                Some("The transcript as it was spoken".to_owned()),
                written_in.map(|name| format!("The transcript written in {name}")),
            ],
            move |translated, cx| {
                let text = if translated {
                    TranscriptText::Translated
                } else {
                    TranscriptText::Spoken
                };
                session.update(cx, |s, cx| s.set_text(text, cx))
            },
            theme,
        )
    }

    /// The same switch in the summary's header, once the summary has a translation.
    fn summary_switch(
        &self,
        detail: &Detail,
        version: SummaryVersion,
        theme: &Theme,
    ) -> AnyElement {
        let written_in = detail
            .summary_translation
            .as_ref()
            .map(|t| crate::languages::name(&t.target));
        let session = self.session.clone();
        self.switch(
            ("summary-text", "Summary text"),
            [
                ("summary-original", "Original"),
                ("summary-translated", "Translated"),
            ],
            &self.summary_focus,
            detail.summary_translated(version),
            [
                Some("The summary as it was written".to_owned()),
                written_in.map(|name| format!("The summary written in {name}")),
            ],
            move |translated, cx| {
                let version = if translated {
                    SummaryVersion::Translated
                } else {
                    SummaryVersion::Original
                };
                session.update(cx, |s, cx| s.set_summary_version(version, cx))
            },
            theme,
        )
    }

    /// A two-button Original / Translated radio group. `pick` gets `true` for Translated.
    #[allow(clippy::too_many_arguments)]
    fn switch(
        &self,
        (id, label): (&'static str, &'static str),
        ids: [(&'static str, &'static str); 2],
        focus: &[FocusHandle; 2],
        translated: bool,
        descriptions: [Option<String>; 2],
        pick: impl Fn(bool, &mut App) + Clone + 'static,
        theme: &Theme,
    ) -> AnyElement {
        let option = |ix: usize| {
            let (id, label) = ids[ix];
            let is_translated = ix == 1;
            let selected = translated == is_translated;
            let pick = pick.clone();
            let el = a11y::focusable(
                a11y::node(id, Role::RadioButton, label),
                &focus[ix],
                theme,
                selected,
            )
            .aria_toggled(if selected {
                gpui_kit::accesskit::Toggled::True
            } else {
                gpui_kit::accesskit::Toggled::False
            })
            .aria_position_in_set(ix + 1)
            .aria_size_of_set(2)
            .on_click(move |_, _, cx| pick(is_translated, cx))
            .cursor_pointer()
            .min_h(theme.text(26.).max(px(24.)))
            .px(px(10.))
            .flex()
            .items_center()
            .text_size(theme.text(13.))
            .rounded(px(6.));
            let el = match &descriptions[ix] {
                Some(description) => el.aria_description(description.clone()),
                None => el,
            };
            if selected {
                el.bg(theme.accent)
                    .text_color(theme.on_accent)
                    .font_weight(FontWeight::SEMIBOLD)
            } else {
                el.text_color(theme.text_secondary)
            }
            .child(label)
        };
        a11y::node(id, Role::RadioGroup, label)
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.control_border)
            .child(option(0))
            .child(option(1))
            .into_any_element()
    }
}

/// A one- or two-line state in place of the tabs. Its label is its text, for VoiceOver.
/// The recording title: 30 bold in Modern, a serif chapter title (38, weight 500) in Book.
fn title_type<E: Styled>(el: E, theme: &Theme) -> E {
    if theme.serif() {
        theme
            .reading(el)
            .text_size(theme.text(38.))
            .font_weight(FontWeight::MEDIUM)
            .line_height(relative(1.15))
    } else {
        el.text_size(theme.text(30.))
            .font_weight(FontWeight::BOLD)
            .line_height(relative(1.2))
    }
}

/// The metadata line: 14 secondary in Modern, a serif italic 16 byline in Book. Same words.
fn byline_type<E: Styled>(el: E, theme: &Theme) -> E {
    let el = el.text_color(theme.text_secondary);
    if theme.serif() {
        theme
            .reading(el)
            .italic()
            .text_size(theme.text(16.))
            .line_height(relative(1.5))
    } else {
        el.text_size(theme.text(14.)).line_height(relative(1.4))
    }
}

/// Book's running head: the recording's name and date in small caps over a hairline. Decorative,
/// so it is no accessibility node; the title and byline say the same.
fn running_head(recording: &Recording, theme: &Theme) -> impl IntoElement {
    let date = library::local(recording.created_at, &Local)
        .format("%A, %-d %B")
        .to_string();
    theme
        .reading(div())
        .id("running-head")
        .test_support()
        .flex()
        .justify_between()
        .gap(px(16.))
        .mb(px(22.))
        .pb(px(8.))
        .border_b_1()
        .border_color(theme.divider)
        .text_size(theme.text(13.))
        .font_features(fonts::small_caps())
        .text_color(theme.text_secondary)
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(shown_title(recording, &Local)),
        )
        .child(div().flex_none().child(date))
}

fn state(
    id: &'static str,
    role: Role,
    text: impl Into<SharedString>,
    fix: Option<String>,
    theme: &Theme,
) -> a11y::Node {
    let text: SharedString = text.into();
    let label: SharedString = match &fix {
        Some(fix) => format!("{text} {fix}").into(),
        None => text.clone(),
    };
    a11y::node(id, role, label)
        .flex()
        .flex_col()
        .gap(px(4.))
        .py(px(24.))
        .child(
            div()
                .text_size(theme.text(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(text),
        )
        .children(fix.map(|fix| {
            div()
                .text_size(theme.text(14.))
                .text_color(theme.text_secondary)
                .child(fix)
        }))
}

/// A button in the detail column: at least 28 pt tall, accent-filled when `primary`. The resting
/// border of a plain one is `focusable`'s 2 pt border, so the focus ring keeps its full width.
pub(crate) fn button(
    id: &'static str,
    label: impl Into<SharedString>,
    primary: bool,
    focus: &FocusHandle,
    theme: &Theme,
    on_press: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> a11y::Node {
    let label: SharedString = label.into();
    spoken_button(id, label.clone(), label, primary, focus, theme, on_press)
}

/// A [`button`] whose spoken name says more than its short visible text. The spoken name starts
/// with the visible text, so Voice Control still finds it by what it shows.
pub(crate) fn spoken_button(
    id: &'static str,
    visible: impl Into<SharedString>,
    spoken: impl Into<SharedString>,
    primary: bool,
    focus: &FocusHandle,
    theme: &Theme,
    on_press: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> a11y::Node {
    let label: SharedString = visible.into();
    a11y::focusable(
        a11y::node(id, Role::Button, spoken.into()),
        focus,
        theme,
        primary,
    )
    .on_click(move |_, window, cx| on_press(window, cx))
    .min_h(px(28.))
    .px(px(12.))
    .flex()
    .items_center()
    .gap(px(8.))
    .rounded(px(7.))
    .text_size(theme.text(13.))
    .font_weight(FontWeight::MEDIUM)
    .when(primary, |b| b.bg(theme.accent).text_color(theme.on_accent))
    .when(!primary, |b| {
        b.bg(theme.control_bg).border_color(theme.control_border)
    })
    .flex_none()
    .child(label)
}

impl DetailPane {
    /// The Summary tab's one button when the summary failed (UX-13). The transcript stays
    /// readable, and only this tab shows the failure.
    fn summary_fix(&self, id: &RecordingId, fix: &Fix, theme: &Theme) -> Option<a11y::Node> {
        let label = match fix {
            Fix::SetUpSummaries => "Set Up Summaries",
            Fix::TryAgain => "Try Again",
            _ => return None,
        };
        let (app, id, fix) = (self.app.clone(), id.clone(), fix.clone());
        Some(button(
            "summary-fix",
            label,
            true,
            &self.fix_focus,
            theme,
            move |window, cx| match fix {
                Fix::SetUpSummaries => set_up_summaries::sheet(window, cx),
                _ => app.update(cx, |app, cx| app.try_again(id.clone(), cx)),
            },
        ))
    }

    /// The Transcript tab before the transcript: the placeholder, then the lines as the engine
    /// writes them, newest last, with the footnote (UX-12). Each line fades and rises 4 pt into
    /// place, or appears at once under Reduce Motion.
    fn lines_so_far(
        &mut self,
        lines: &[Segment],
        placeholder: &'static str,
        window: &mut Window,
        theme: &Theme,
        cx: &App,
    ) -> AnyElement {
        if lines.len() < self.arrivals.len() {
            self.arrivals.clear();
        }
        let now = Instant::now();
        let still = cx.reduce_motion();
        while self.arrivals.len() < lines.len() {
            self.arrivals
                .push(if still { now - motion::SLOW } else { now });
        }
        if lines.is_empty() {
            return a11y::node("transcript-placeholder", Role::Status, placeholder)
                .py(px(16.))
                .text_size(theme.text(15.))
                .line_height(relative(1.6))
                .text_color(theme.text_secondary)
                .child(placeholder)
                .into_any_element();
        }
        let script = theme.presentation(f32::from(window.viewport_size().width));
        let hang = transcript_view::hangs(
            script,
            f32::from(self.pinning.column_x - self.pinning.visible.left()),
        );
        let stamp_w = if hang {
            px(transcript_view::MARGIN)
        } else {
            px(42. + 12. * (theme.text_scale - 1.).max(0.))
        };
        let (body_size, body_line) = if script.script_transcript {
            (17., 1.7)
        } else {
            (15., 1.6)
        };
        let arriving = self
            .arrivals
            .iter()
            .any(|at| elapsed(Some(*at), motion::BASE) < 1.);
        if arriving && window.visibility().is_visible() {
            window.request_animation_frame();
        }
        let items = lines
            .iter()
            .zip(&self.arrivals)
            .enumerate()
            .map(|(ix, (line, at))| {
                let shown = ease_standard(elapsed(Some(*at), motion::BASE));
                let start = line.range.map(|r| r.start());
                let spoken = match start {
                    Some(start) => format!("{}, {}", a11y::spoken_duration(start), line.text),
                    None => line.text.clone(),
                };
                a11y::node(format!("draft-{ix}"), Role::ListItem, spoken)
                    .flex()
                    .gap(px(if hang {
                        transcript_view::MARGIN_GAP
                    } else {
                        8.
                    }))
                    .opacity(shown)
                    .relative()
                    .top(px(4. * (1. - shown)))
                    .child(
                        div()
                            .flex_none()
                            .w(stamp_w)
                            .when(hang, |stamp| {
                                stamp
                                    .h(theme.text(body_size) * body_line)
                                    .flex()
                                    .items_center()
                                    .justify_end()
                            })
                            .when(!hang, |stamp| stamp.pt(px(2.)))
                            .font_features(crate::theme::tabular())
                            .text_size(theme.text(12.))
                            .text_color(theme.text_secondary)
                            .child(start.map(a11y::clock).unwrap_or_default()),
                    )
                    .child(
                        theme
                            .reading(div())
                            .flex_1()
                            .min_w_0()
                            .text_size(theme.text(body_size))
                            .line_height(relative(body_line))
                            .child(line.text.clone()),
                    )
            });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            .child(
                a11y::node("transcript-drafts", Role::List, "Transcript so far")
                    .when(hang, |list| {
                        list.ml(px(-(transcript_view::MARGIN + transcript_view::MARGIN_GAP)))
                    })
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .children(items),
            )
            .child(
                a11y::node("transcript-footnote", Role::Label, processing::FOOTNOTE)
                    .text_size(theme.text(12.))
                    .text_color(theme.text_secondary)
                    .child(processing::FOOTNOTE),
            )
            .into_any_element()
    }

    /// UX-11's finish card, shown when automatic processing did not queue the recording.
    fn finish_card(&self, detail: &Detail, theme: &Theme, cx: &mut Context<Self>) -> a11y::Node {
        let id = detail.recording.id.clone();
        let audio = detail
            .recording
            .take
            .map_or(dory_core::domain::Ms(0), |t| t.duration);
        let estimate = self.app.read(cx).finish_estimate(audio);
        let always = self.always;
        let finish = |goal: Goal| {
            let (app, id) = (self.app.clone(), id.clone());
            move |_: &mut Window, cx: &mut App| {
                app.update(cx, |app, cx| app.finish(id.clone(), goal, always, cx))
            }
        };
        let this = cx.entity().downgrade();
        let toggle = move |_: &mut Window, cx: &mut App| {
            let _ = this.update(cx, |this, cx| {
                this.always = !this.always;
                cx.notify();
            });
        };
        let tick = div()
            .min_w(px(16.))
            .min_h(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .border_1()
            .border_color(theme.control_border)
            .when(always, |b| {
                b.bg(theme.accent)
                    .child(Icon::Check.view(12., theme.on_accent))
            });
        let checkbox = a11y::focusable(
            a11y::node(
                "finish-always",
                Role::CheckBox,
                "Always do this when I stop",
            ),
            &self.always_focus,
            theme,
            false,
        )
        .aria_toggled(if always {
            gpui_kit::accesskit::Toggled::True
        } else {
            gpui_kit::accesskit::Toggled::False
        })
        .on_click(move |_, window, cx| toggle(window, cx))
        .min_h(px(28.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(6.))
        .text_size(theme.text(13.))
        .child(tick)
        .child("Always do this when I stop");
        a11y::node("finish-card", Role::Group, "Finish this recording?")
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(20.))
            .rounded(px(14.))
            .bg(theme.card_bg)
            .border_1()
            .border_color(theme.card_border)
            .child(
                div()
                    .text_size(theme.text(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Finish this recording?"),
            )
            .child(
                a11y::node("finish-estimate", Role::Label, estimate.clone())
                    .text_size(theme.text(14.))
                    .text_color(theme.text_secondary)
                    .child(estimate),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(button(
                        "finish-all",
                        "Transcribe and Summarize",
                        true,
                        &self.finish_focus,
                        theme,
                        finish(Goal::ALL),
                    ))
                    .child(button(
                        "finish-transcript",
                        "Transcript Only",
                        false,
                        &self.transcript_only_focus,
                        theme,
                        finish(Goal::TRANSCRIBE),
                    )),
            )
            .child(checkbox)
    }
}

impl Render for DetailPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let (detail, write_error) = {
            let session = self.session.read(cx);
            match session.loaded() {
                Loadable::Ready(detail) => (detail.clone(), session.error().cloned()),
                Loadable::Loading => {
                    return state("detail-loading", Role::Status, "Loading…", None, &theme)
                        .text_color(theme.text_secondary)
                        .into_any_element();
                }
                Loadable::Failed(why) => {
                    return state("detail-failed", Role::Status, why.clone(), None, &theme)
                        .into_any_element();
                }
            }
        };
        let (title_marks, readout, opens_at_title) = match self.session.read(cx).found() {
            Some(found) => (
                found
                    .in_title()
                    .iter()
                    .map(|m| m.span.clone())
                    .collect::<Vec<_>>(),
                Some(found.readout()),
                found.current_at().place == Place::Title,
            ),
            None => (Vec::new(), None, false),
        };
        let (tab, phase, lines) = {
            let app = self.app.read(cx);
            let id = &detail.recording.id;
            (app.tab, app.phase_of(id), app.drafts(id).to_vec())
        };
        let body = Body::derive(&detail, phase.as_ref());
        // With a match to show on a tab, the transcript or summary view ends the time instead.
        if self.opened.is_some() && (readout.is_none() || opens_at_title || body != Body::Tabs) {
            let opened = self.opened.take().expect("checked above");
            window.on_next_frame(move |_, _| {
                let ms = opened.elapsed().as_secs_f64() * 1000.0;
                eprintln!("match-revealed ms={ms:.1}");
            });
        }
        match body {
            Body::Finish if self.focus_finish => {
                self.focus_finish = false;
                window.focus(&self.finish_focus, cx);
            }
            // The row can list the saved audio a frame before the session reloads it.
            Body::Finish | Body::Tabs | Body::NoAudio => {}
            _ => self.focus_finish = false,
        }
        let processing = phase.as_ref().is_some_and(Phase::is_processing);
        let pinning = self.pinning;
        let has_card = processing || detail.recording.take.is_some();
        let docked = !processing && detail.recording.take.is_some() && pinning.docked;
        let stuck = body == Body::Tabs && pinning.stuck;

        let card_slot = has_card.then(|| {
            div()
                .relative()
                .mt(px(4.))
                .when(docked, |slot| slot.h(pinning.card_h))
                .when(!docked, |slot| slot.child(self.player.clone()))
                .child(self.watch(Slot::Card, cx))
        });
        let docked_player = docked.then(|| {
            deferred(
                anchored()
                    .position_mode(AnchoredPositionMode::Window)
                    .anchor(Anchor::BottomLeft)
                    .position(point(pinning.column_x, pinning.visible.bottom() - px(14.)))
                    .child(div().w(pinning.column_w).child(self.player.clone())),
            )
            .with_priority(1)
        });

        let (content, sticky_row) = match &body {
            Body::Tabs => {
                let shown = shown_title(&detail.recording, &Local);
                let summarizing = matches!(phase, Some(Phase::Summarizing { .. }));
                if summarizing && motion::looping(window, cx) {
                    window.request_animation_frame();
                }
                let dots = summarizing.then(|| motion::looping(window, cx).then(motion::clock_ms));
                let text = self.session.read(cx).shown();
                let translate = if matches!(phase, Some(Phase::TranslatingSummary { .. })) {
                    TranslateSlot::Translating
                } else if self.app.read(cx).is_underway(&detail.recording.id) {
                    TranslateSlot::Waiting
                } else {
                    TranslateSlot::Ready
                };
                let row = self.tab_row(
                    tab,
                    stuck.then_some(shown),
                    &detail,
                    text,
                    translate,
                    dots,
                    readout.clone(),
                    phase.as_ref(),
                    &theme,
                );
                let (in_flow, pinned) = if stuck {
                    (None, Some(row))
                } else {
                    (Some(row), None)
                };
                let slot = div()
                    .relative()
                    .when(stuck, |slot| slot.h(pinning.tabs_h))
                    .children(in_flow)
                    .child(self.watch(Slot::Tabs, cx));
                let page = match tab {
                    DetailTab::Summary => {
                        let fix = match &phase {
                            Some(Phase::Failed(problem))
                                if matches!(
                                    problem.stage(),
                                    Stage::Summarize | Stage::TranslateSummary
                                ) =>
                            {
                                problem.fix().and_then(|fix| {
                                    self.summary_fix(&detail.recording.id, &fix, &theme)
                                })
                            }
                            _ => None,
                        };
                        summary_view::summary(
                            &detail,
                            phase.as_ref(),
                            text.summary,
                            &self.summary,
                            fix.map(IntoElement::into_any_element),
                            &theme,
                            theme.presentation(f32::from(window.viewport_size().width)),
                        )
                    }
                    DetailTab::Transcript => match &phase {
                        Some(phase) if phase.before_transcript() => {
                            self.lines_so_far(&lines, phase.placeholder(), window, &theme, cx)
                        }
                        _ => self.transcript.clone().into_any_element(),
                    },
                };
                let sticky = pinned.map(|row| {
                    deferred(
                        anchored()
                            .position_mode(AnchoredPositionMode::Window)
                            .position(point(pinning.visible.left(), pinning.visible.top()))
                            .child(
                                div()
                                    .w(pinning.visible.size.width)
                                    .bg(theme.window_bg)
                                    .child(
                                        div()
                                            .ml(pinning.column_x - pinning.visible.left())
                                            .w(pinning.column_w)
                                            .child(row),
                                    ),
                            ),
                    )
                    .with_priority(1)
                });
                (
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .child(slot)
                        .child(page)
                        .into_any_element(),
                    sticky,
                )
            }
            Body::Recording => (
                state(
                    "detail-state",
                    Role::Status,
                    "Recording",
                    Some("Transcription starts after you stop.".into()),
                    &theme,
                )
                .into_any_element(),
                None,
            ),
            Body::Finish => (
                self.finish_card(&detail, &theme, cx).into_any_element(),
                None,
            ),
            Body::NoAudio => (
                state(
                    "detail-state",
                    Role::Group,
                    "No audio was recorded.",
                    Some("Nothing reached the microphone or system audio.".into()),
                    &theme,
                )
                .into_any_element(),
                None,
            ),
            Body::NothingSaid => (
                state(
                    "detail-state",
                    Role::Group,
                    "Nothing was said.",
                    Some("The transcript is empty, so there is nothing to summarize.".into()),
                    &theme,
                )
                .into_any_element(),
                None,
            ),
        };

        let meta = meta_line(&detail.recording, detail.transcript.as_ref(), &Local);
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .pt(px(24.))
            .when(docked, |page| page.pb(px(80.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(
                        theme
                            .presentation(f32::from(window.viewport_size().width))
                            .running_head
                            .then(|| running_head(&detail.recording, &theme)),
                    )
                    .child(self.title(&detail, &title_marks, &theme, cx))
                    .child(
                        byline_type(a11y::node("detail-meta", Role::Label, meta.clone()), &theme)
                            .child(meta),
                    )
                    .children(write_error.map(|error| {
                        a11y::node("detail-error", Role::Status, error.clone())
                            .text_size(theme.text(14.))
                            .text_color(theme.attention)
                            .child(icons::beside(
                                Icon::TriangleAlert,
                                14.,
                                theme.attention,
                                theme.text(14.) * 1.4,
                                error,
                            ))
                    })),
            )
            .children(card_slot)
            .child(content)
            .children(docked_player)
            .children(sticky_row)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use dory_core::domain::{Fraction, LangTag, Revision, Transcript};

    use super::*;
    use crate::app::Detail;
    use crate::estimate::Left;
    use crate::fixtures::{self, SAMPLES};
    use crate::processing::Problem;

    fn detail(title: &str) -> Detail {
        let sample = SAMPLES.iter().find(|s| s.title == title).unwrap();
        Detail {
            recording: sample.recording(),
            transcript: sample.transcript(),
            summary: sample.summary(),
            names: Default::default(),
            turns: Vec::new(),
            translation: None,
            translated_turns: None,
            summary_translation: None,
            translating_into: None,
        }
    }

    #[test]
    fn the_body_follows_the_take_the_transcript_and_the_phase() {
        let done = detail("Design review");
        assert_eq!(
            Body::derive(&done, Some(&Phase::Done { summary: true })),
            Body::Tabs
        );

        let mut weekly = detail("Weekly sync");
        let transcribing = Phase::Transcribing {
            done: Fraction::new(0.4),
            left: Left::Estimating,
            light: None,
            chosen: None,
        };
        assert_eq!(
            Body::derive(&weekly, Some(&transcribing)),
            Body::Tabs,
            "the card and the tabs from Stop on"
        );
        assert_eq!(Body::derive(&weekly, Some(&Phase::Ready)), Body::Finish);
        assert_eq!(
            Body::derive(&weekly, Some(&Phase::Failed(Problem::NoSpeechModel))),
            Body::Tabs
        );
        weekly.recording.take = None;
        assert_eq!(
            Body::derive(
                &weekly,
                Some(&Phase::Stopped {
                    starts_in: 5,
                    auto: true
                })
            ),
            Body::Tabs,
            "stopped, before the take is saved"
        );
        assert_eq!(
            Body::derive(&weekly, Some(&Phase::Recording)),
            Body::Recording
        );

        let mut silent = detail("Weekly sync");
        silent.transcript = Some(Transcript {
            segments: Vec::new(),
            ..detail("Design review").transcript.unwrap()
        });
        assert_eq!(Body::derive(&silent, None), Body::NothingSaid);

        let mut no_take = detail("Design review");
        no_take.recording.take = None;
        assert_eq!(Body::derive(&no_take, None), Body::NoAudio);
    }

    #[test]
    fn the_meta_line_has_date_duration_languages_and_layout() {
        let mut done = detail("Design review");
        assert_eq!(
            meta_line(&done.recording, done.transcript.as_ref(), &fixtures::zone()),
            "Thu 12 Nov 2026, 10:30 · 48 min · Mic and system audio"
        );
        let transcript = done.transcript.as_mut().unwrap();
        transcript.revision = Revision(2);
        transcript.detected_languages = vec![
            LangTag::parse("en").unwrap(),
            LangTag::parse("fr-FR").unwrap(),
        ];
        assert_eq!(
            meta_line(&done.recording, done.transcript.as_ref(), &fixtures::zone()),
            "Thu 12 Nov 2026, 10:30 · 48 min · English and French · Mic and system audio"
        );
    }
}

//! Model rows, the model explainer (UX-28), meeting languages (UX-29), and the onboarding speech
//! step (UX-16), shared by the Settings window, onboarding, and the Set up summaries sheet.
//!
//! [`Models`] is the one cache of the settings and both model lists. Every view here reads it, so
//! a choice made in one window shows in the others, and every edit goes through it to `AppState`.
//! It refreshes after each edit and when a listed model's download ends. The wording of rows and
//! explainers is pure ([`row`], [`about`], [`fit_text`]) so tests pin it without a window.

use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

use dory_core::app::{AppKind, ModelRow};
use dory_core::domain::{Blocker, LangSet, LangTag, LanguagePref, Settings};
use dory_core::models::{
    Availability, Choice, Fit, LangSupport, Llm, Mismatch, ModelEntry, ModelId, ModelRef,
    ModelStatus, Resolution, Stt, Unavailable, UnavailableReason, resolve,
};
use gpui_kit::component::dialog::Confirm;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, FontWeight, Global,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Render, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, anchored,
    deferred, div, prelude::FluentBuilder as _, px,
};

use crate::a11y;
use crate::app::{AppState, Download};
use crate::icons::Icon;
use crate::languages;
use crate::radio::{Radio, RadioGroup};
use crate::theme::Theme;

/// "670 MB", "9.5 GB".
pub fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("{} MB", (bytes as f64 / 1e6).round() as u64)
    }
}

/// The kinds a list shows, with where their languages live.
pub trait ListKind: AppKind {
    /// "Speech", "Summary".
    const NOUN: &'static str;
    fn languages(caps: &Self::Caps) -> &LangSet;
    /// The languages the settings ask a model of this kind to handle.
    fn wanted(settings: &Settings) -> Vec<LangTag>;
    fn rows(models: &Models) -> Option<&[ModelRow<Self>]>;
}

impl ListKind for Stt {
    const NOUN: &'static str = "Speech";

    fn languages(caps: &Self::Caps) -> &LangSet {
        match &caps.languages {
            LangSupport::Multilingual { tags, .. } | LangSupport::FixedLocale { tags } => tags,
        }
    }

    fn wanted(settings: &Settings) -> Vec<LangTag> {
        match &settings.language {
            LanguagePref::Pinned(tag) => vec![tag.clone()],
            LanguagePref::Auto => settings.meeting_languages.clone(),
        }
    }

    fn rows(models: &Models) -> Option<&[ModelRow<Self>]> {
        models.speech.as_deref()
    }
}

impl ListKind for Llm {
    const NOUN: &'static str = "Summary";

    fn languages(caps: &Self::Caps) -> &LangSet {
        &caps.languages
    }

    fn wanted(settings: &Settings) -> Vec<LangTag> {
        match &settings.language {
            LanguagePref::Pinned(tag) => vec![tag.clone()],
            LanguagePref::Auto => Vec::new(),
        }
    }

    fn rows(models: &Models) -> Option<&[ModelRow<Self>]> {
        models.summaries.as_deref()
    }
}

/// Where a model stands on this Mac, worded for its row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowState {
    Ready,
    NeedsDownload,
    Downloading(Option<u8>),
    Unavailable(String),
}

impl RowState {
    pub fn words(&self) -> String {
        match self {
            RowState::Ready => "Ready".into(),
            RowState::NeedsDownload => "Not downloaded".into(),
            RowState::Downloading(Some(p)) => format!("Downloading, {p}%"),
            RowState::Downloading(None) => "Downloading".into(),
            RowState::Unavailable(why) => why.clone(),
        }
    }
}

/// One model row, worded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub model: ModelRef,
    pub name: String,
    /// What the row shows after the name: "487 MB to download" while the model is not
    /// installed, "671 MB on disk" once it is.
    pub size: Option<String>,
    /// What is left to fetch while the model is not installed: the compressed download, less
    /// any part already on disk.
    pub download: Option<String>,
    /// The model's size once unpacked.
    pub disk: Option<String>,
    /// Why the model cannot do the job, or does it with a limit: "Doesn't support Tagalog".
    pub fit: Option<String>,
    /// False when the fit rules the model out, so Choose is not offered.
    pub usable: bool,
    pub state: RowState,
    pub chosen: bool,
}

impl Row {
    /// What VoiceOver reads for the row: everything it shows, in order.
    pub fn label(&self) -> String {
        let mut parts = vec![self.name.clone()];
        parts.extend(self.size.clone());
        parts.extend(self.fit.clone());
        parts.push(self.state.words());
        if self.chosen {
            parts.push("in use".into());
        }
        parts.join(", ")
    }
}

pub fn unavailable_text(why: &Unavailable) -> String {
    if let Some(fix) = &why.fix {
        return fix.clone();
    }
    match &why.reason {
        UnavailableReason::UnsupportedPlatform => "Not supported on this Mac".into(),
        UnavailableReason::OsTooOld { required } => format!("Needs {required}"),
        UnavailableReason::NotEligible { detail } => detail.clone(),
        UnavailableReason::ServerUnreachable { endpoint } => format!("Can't reach {endpoint}"),
    }
}

/// The core decides the fit. This names it, and for a language mismatch names the languages.
pub fn fit_text<K: ListKind>(fit: Fit, caps: &K::Caps, settings: &Settings) -> Option<String> {
    let why = match fit {
        Fit::Yes => return None,
        Fit::Degraded(why) | Fit::No(why) => why,
    };
    Some(match why {
        Mismatch::LanguageUnsupported => {
            let set = K::languages(caps);
            let missing: Vec<String> = K::wanted(settings)
                .iter()
                .filter(|tag| !set.supports(tag))
                .map(languages::name)
                .collect();
            if missing.is_empty() {
                "Doesn't support this language".into()
            } else {
                format!("Doesn't support {}", missing.join(" or "))
            }
        }
        Mismatch::AutoLanguageUnsupported => "Needs a language chosen, can't detect one".into(),
        Mismatch::NoDiarization => "Can't tell speakers apart".into(),
        Mismatch::InputTooLong => "Long recordings get cut short".into(),
    })
}

pub fn row<K: ListKind>(row: &ModelRow<K>, settings: &Settings, download: Option<Download>) -> Row {
    let entry = &row.status.entry;
    let state = match (&row.status.availability, download) {
        (_, Some(d)) => RowState::Downloading(d.percent().map(|p| p.get())),
        (Availability::Ready, None) => RowState::Ready,
        (Availability::NeedsProvision { .. }, None) => RowState::NeedsDownload,
        (Availability::Unavailable(why), None) => RowState::Unavailable(unavailable_text(why)),
    };
    // Two different sizes: the compressed download, and the unpacked model on disk. Each is
    // named, so the row and the explainer never show one under the other's label.
    let download = match &row.status.availability {
        Availability::NeedsProvision { bytes: Some(b) } => Some(size_text(*b)),
        _ => None,
    };
    let disk = entry.disk_bytes.map(size_text);
    let size = match (&download, &disk, &state) {
        (Some(d), _, _) => Some(format!("{d} to download")),
        (None, Some(s), RowState::Ready) => Some(format!("{s} on disk")),
        (None, Some(s), _) => Some(s.clone()),
        (None, None, _) => None,
    };
    Row {
        model: entry.id.erase(),
        name: entry.display_name.clone(),
        size,
        download,
        disk,
        fit: fit_text::<K>(row.fit, &entry.caps, settings),
        usable: !matches!(row.fit, Fit::No(_)),
        state,
        chosen: row.chosen,
    }
}

/// The explainer popover's content (UX-28), all from the catalog row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct About {
    pub name: String,
    pub summary: String,
    pub good_at: Vec<String>,
    pub watch_out: Vec<String>,
    pub download: Option<String>,
    pub disk: Option<String>,
    pub languages: String,
}

/// How many language names the explainer lists before it says how many more there are.
const NAMED_LANGUAGES: usize = 6;

pub fn languages_text(set: &LangSet, first: &[LangTag]) -> String {
    let tags = match set {
        LangSet::Any => return "Any language".into(),
        LangSet::Only(tags) => tags,
    };
    let mut names: Vec<String> = first
        .iter()
        .filter(|tag| set.supports(tag))
        .map(languages::name)
        .collect();
    let mut rest: Vec<String> = tags
        .iter()
        .map(languages::name)
        .filter(|name| !names.contains(name))
        .collect();
    rest.sort();
    rest.dedup();
    names.extend(rest);
    if names.len() <= NAMED_LANGUAGES {
        return languages::name_list(&names, "and");
    }
    format!(
        "{} languages, including {}",
        names.len(),
        names[..NAMED_LANGUAGES].join(", ")
    )
}

pub fn about<K: ListKind>(entry: &ModelEntry<K>, row: &Row, settings: &Settings) -> About {
    About {
        name: entry.display_name.clone(),
        summary: entry.about.summary.clone(),
        good_at: entry.about.pros.clone(),
        watch_out: entry.about.cons.clone(),
        download: row.download.clone(),
        disk: row.disk.clone(),
        languages: languages_text(K::languages(&entry.caps), &K::wanted(settings)),
    }
}

/// What `Auto` picks with these rows and settings, by the core's own rule.
pub fn auto_pick<K: ListKind>(rows: &[ModelRow<K>], settings: &Settings) -> Resolution<K> {
    resolve_with(&Choice::Auto, rows, settings)
}

/// What the settings' own choice resolves to, as a job would see it.
pub fn chosen_pick<K: ListKind>(rows: &[ModelRow<K>], settings: &Settings) -> Resolution<K> {
    resolve_with(K::choice(settings), rows, settings)
}

fn resolve_with<K: ListKind>(
    choice: &Choice<K>,
    rows: &[ModelRow<K>],
    settings: &Settings,
) -> Resolution<K> {
    let statuses: Vec<ModelStatus<K>> = rows.iter().map(|r| r.status.clone()).collect();
    resolve(choice, &statuses, &K::listing_needs(settings))
}

fn name_of<K: ListKind>(rows: &[ModelRow<K>], model: &ModelRef) -> String {
    rows.iter()
        .find(|r| &r.status.entry.id.erase() == model)
        .map_or_else(
            || model.to_string(),
            |r| r.status.entry.display_name.clone(),
        )
}

/// The line under **Automatic**: which model it uses now, and why not when it can't.
pub fn auto_line<K: ListKind>(rows: &[ModelRow<K>], settings: &Settings) -> String {
    match auto_pick(rows, settings) {
        Resolution::Use(id) => format!("Uses {}.", name_of(rows, &id.erase())),
        Resolution::Skip => "No model fits, so this step is skipped.".into(),
        Resolution::Blocked(Blocker::NeedsProvision { model, .. }) => {
            format!("Uses {} once it is downloaded.", name_of(rows, &model))
        }
        Resolution::Blocked(Blocker::Unavailable { model, why }) => format!(
            "Would use {}: {}.",
            name_of(rows, &model),
            unavailable_text(&why)
        ),
        Resolution::Blocked(Blocker::Incompatible { .. }) => {
            "No model covers your languages.".into()
        }
        Resolution::Blocked(Blocker::NoModel { .. }) => "No model is installed.".into(),
    }
}

/// The settings and both model lists, shared by every view in this module and the summaries
/// sheet. One per app.
pub struct Models {
    app: Entity<AppState>,
    settings: Option<Settings>,
    speech: Option<Vec<ModelRow<Stt>>>,
    summaries: Option<Vec<ModelRow<Llm>>>,
    error: Option<SharedString>,
    /// Said once Language falls back to Automatic, and kept for the rest of the session.
    language_note: Option<SharedString>,
    checking: bool,
    generation: u64,
    /// Listed models downloading at the last look, so the end of one triggers a refresh.
    downloading: HashSet<ModelRef>,
    /// `AppState`'s copy at the last look. It follows `SettingsChanged`, so a change made
    /// elsewhere, such as onboarding's language preselect, refreshes this cache too.
    seen: Option<Settings>,
    _observe: Subscription,
}

struct Shared(Entity<Models>);

impl Global for Shared {}

/// The app's [`Models`], made on first use. `set_up_summaries::sheet` has no `AppState` to make
/// one, so `build` calls this at launch.
pub fn install(app: &Entity<AppState>, cx: &mut App) -> Entity<Models> {
    if let Some(Shared(models)) = cx.try_global::<Shared>()
        && models.read(cx).app == *app
    {
        return models.clone();
    }
    let models = cx.new(|cx| {
        let mut models = Models::new(app.clone(), cx);
        models.refresh(cx);
        models
    });
    cx.set_global(Shared(models.clone()));
    models
}

pub fn shared(cx: &App) -> Option<Entity<Models>> {
    cx.try_global::<Shared>().map(|s| s.0.clone())
}

impl Models {
    fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&app, |this, app, cx| {
            let app = app.read(cx);
            let now: HashSet<ModelRef> = this
                .listed()
                .filter(|model| app.download_of(model).is_some())
                .collect();
            let finished = this.downloading.difference(&now).next().is_some();
            this.downloading = now;
            let changed = this.seen.as_ref() != Some(app.settings());
            if changed {
                this.seen = Some(app.settings().clone());
            }
            if finished || changed {
                this.refresh(cx);
            }
            cx.notify();
        });
        let seen = Some(app.read(cx).settings().clone());
        Self {
            app,
            settings: None,
            speech: None,
            summaries: None,
            error: None,
            language_note: None,
            checking: false,
            generation: 0,
            downloading: HashSet::new(),
            seen,
            _observe: observe,
        }
    }

    fn listed(&self) -> impl Iterator<Item = ModelRef> + '_ {
        let speech = self
            .speech
            .iter()
            .flatten()
            .map(|r| r.status.entry.id.erase());
        let summaries = self
            .summaries
            .iter()
            .flatten()
            .map(|r| r.status.entry.id.erase());
        speech.chain(summaries)
    }

    pub fn app(&self) -> &Entity<AppState> {
        &self.app
    }

    pub fn settings(&self) -> Option<&Settings> {
        self.settings.as_ref()
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// True while a refresh probes the providers, for "Checking".
    pub fn is_checking(&self) -> bool {
        self.checking
    }

    pub fn rows<K: ListKind>(&self) -> Option<&[ModelRow<K>]> {
        K::rows(self)
    }

    /// Rereads the settings and probes every model again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.checking = true;
        let app = self.app.read(cx);
        let settings = app.read_settings(cx);
        let speech = app.model_rows::<Stt>(cx);
        let summaries = app.model_rows::<Llm>(cx);
        cx.spawn(async move |this, cx| {
            let (settings, speech, summaries) = (settings.await, speech.await, summaries.await);
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.checking = false;
                let mut errors = Vec::new();
                match settings {
                    Ok(settings) => this.settings = Some(settings),
                    Err(error) => errors.push(error),
                }
                match speech {
                    Ok(rows) => this.speech = Some(rows),
                    Err(error) => errors.push(error),
                }
                match summaries {
                    Ok(rows) => this.summaries = Some(rows),
                    Err(error) => errors.push(error),
                }
                this.error = errors.into_iter().next().map(Into::into);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows the edit at once, then saves it and refreshes the fits it may change.
    pub fn edit(&mut self, edit: impl Fn(&mut Settings) + Send + 'static, cx: &mut Context<Self>) {
        if let Some(settings) = &mut self.settings {
            edit(settings);
        }
        let saved = self
            .app
            .update(cx, |app, cx| app.edit_settings(move |s| edit(s), cx));
        self.after(saved, cx);
    }

    /// Adds or removes a meeting language by the rule in [`languages::toggle`], and saves the
    /// result. `None` until the settings have loaded.
    pub fn toggle_language(
        &mut self,
        tag: LangTag,
        cx: &mut Context<Self>,
    ) -> Option<languages::Edit> {
        let edit = languages::toggle(self.settings.as_mut()?, &tag);
        match &edit {
            languages::Edit::KeptLast => return Some(edit),
            languages::Edit::Removed { note: Some(note) } => {
                self.language_note = Some(note.clone().into());
            }
            languages::Edit::Added | languages::Edit::Removed { note: None } => {}
        }
        let saved = self.app.update(cx, |app, cx| {
            app.edit_settings(
                move |s| {
                    languages::toggle(s, &tag);
                },
                cx,
            )
        });
        self.after(saved, cx);
        Some(edit)
    }

    pub fn language_note(&self) -> Option<&SharedString> {
        self.language_note.as_ref()
    }

    pub fn choose<K: ListKind>(&mut self, choice: Choice<K>, cx: &mut Context<Self>) {
        let saved = self.app.update(cx, |app, cx| app.choose_model(choice, cx));
        self.after(saved, cx);
    }

    pub fn download<K: ListKind>(&mut self, id: ModelId<K>, cx: &mut Context<Self>) {
        self.downloading.insert(id.erase());
        self.app.update(cx, |app, cx| app.provision_model(id, cx));
    }

    pub fn cancel(&mut self, model: &ModelRef, cx: &mut Context<Self>) {
        self.app.update(cx, |app, _| app.cancel_provision(model));
    }

    fn after(&mut self, saved: gpui_kit::Task<Result<(), String>>, cx: &mut Context<Self>) {
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = saved.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.refresh(cx),
                Err(error) => {
                    this.error = Some(error.into());
                    this.refresh(cx);
                }
            });
        })
        .detach();
    }
}

/// A pill button that takes focus and acts on a click, Return, or Space. GPUI turns Return and
/// Space on a focused element into a click; in a dialog Return is Confirm instead, so the button
/// answers `Confirm` itself or the dialog would eat it.
pub(crate) fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    text: impl Into<SharedString>,
    primary: bool,
    focus: &FocusHandle,
    theme: &Theme,
    press: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> a11y::Node {
    let on_confirm = press.clone();
    a11y::focusable(a11y::node(id, Role::Button, label), focus, theme, primary)
        .on_click(move |_, window, cx| press(window, cx))
        .on_action(move |_: &Confirm, window, cx| {
            on_confirm(window, cx);
            cx.stop_propagation();
        })
        .flex_none()
        .min_h(px(28.))
        .px(px(12.))
        .flex()
        .items_center()
        .rounded(px(14.))
        .text_size(theme.text(13.))
        .font_weight(FontWeight::MEDIUM)
        .when(primary, |b| b.bg(theme.accent).text_color(theme.on_accent))
        .when(!primary, |b| {
            b.bg(theme.control_bg).border_color(theme.control_border)
        })
        .child(text.into())
}

pub(crate) fn note(text: impl Into<SharedString>, theme: &Theme) -> gpui_kit::Div {
    div()
        .text_size(theme.text(12.))
        .text_color(theme.text_secondary)
        .child(text.into())
}

/// A determinate bar, or a steady one when the total is unknown.
pub(crate) fn progress_bar(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    percent: Option<u8>,
    theme: &Theme,
) -> a11y::Node {
    let label: SharedString = label.into();
    let fill = percent.unwrap_or(0) as f32 / 100.0;
    a11y::node(id, Role::ProgressIndicator, label)
        .aria_numeric_value(f64::from(percent.unwrap_or(0)))
        .aria_min_numeric_value(0.)
        .aria_max_numeric_value(100.)
        .h(px(6.))
        .w_full()
        .rounded(px(3.))
        .bg(theme.track)
        .when(theme.high_contrast, |bar| {
            bar.border_1().border_color(theme.text_secondary)
        })
        .child(
            div()
                .h_full()
                .rounded(px(3.))
                .bg(theme.progress_fill())
                .w(gpui_kit::relative(fill)),
        )
}

/// The popover's distance below its button, and its clearance from the window's edges.
const POPOVER_GAP: f32 = 30.;
const POPOVER_MARGIN: f32 = 8.;

/// The info button and, while open, its popover (UX-28). Click, Return, or Space toggles it
/// (GPUI clicks a focused element on both keys); Escape or a click outside closes it.
///
/// `room` is the window's height. The popover never grows past it: a longer explainer scrolls
/// inside the popover instead of running off the window.
fn explainer(
    about: About,
    model: &ModelRef,
    open: bool,
    room: Pixels,
    focus: &FocusHandle,
    theme: &Theme,
    toggle: impl Fn(Option<bool>, &mut Window, &mut App) + Clone + 'static,
) -> gpui_kit::Div {
    let (on_click, on_confirm, on_escape, on_out) =
        (toggle.clone(), toggle.clone(), toggle.clone(), toggle);
    let info = a11y::focusable(
        a11y::node(
            format!("about-{model}"),
            Role::Button,
            format!("About {}", about.name),
        ),
        focus,
        theme,
        false,
    )
    .aria_expanded(open)
    // Set, not toggle: a click on this button while the popover is open first closes it through
    // the popover's click-outside handler, and a toggle would then open it again.
    .on_click(move |_, window, cx| on_click(Some(!open), window, cx))
    .on_action(move |_: &Confirm, window, cx| {
        on_confirm(Some(!open), window, cx);
        cx.stop_propagation();
    })
    .on_key_down(move |event: &KeyDownEvent, window, cx| {
        if open && event.keystroke.key == "escape" {
            on_escape(Some(false), window, cx);
            cx.stop_propagation();
        }
    })
    .flex_none()
    .min_w(px(24.))
    .min_h(px(24.))
    .flex()
    .items_center()
    .justify_center()
    .rounded_full()
    .child(Icon::Info.view(16., theme.accent_text));
    let panel = open.then(|| {
        let section = |id: String, title: &'static str, items: Vec<String>| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    a11y::node(format!("{id}-title"), Role::Heading, title)
                        .text_size(theme.text(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(a11y::node(id.clone(), Role::List, title).children(
                    items.into_iter().enumerate().map(|(ix, item)| {
                        a11y::node(format!("{id}-{ix}"), Role::ListItem, item.clone())
                            .text_size(theme.text(12.))
                            .child(format!("• {item}"))
                    }),
                ))
        };
        let mut facts = Vec::new();
        if let Some(download) = &about.download {
            facts.push(("download", format!("Download: {download}")));
        }
        if let Some(disk) = &about.disk {
            facts.push(("disk", format!("On disk: {disk}")));
        }
        facts.push(("languages", format!("Languages: {}", about.languages)));
        let line = |id: String, text: String, size: f32| {
            a11y::node(id, Role::Label, text.clone())
                .text_size(theme.text(size))
                .child(text)
        };
        deferred(
            anchored()
                .anchor(gpui_kit::Anchor::TopRight)
                // An offset, not a margin: the window snap measures the popover without its
                // margins, so a margin pushed the popover past the bottom edge.
                .offset(gpui_kit::point(px(0.), px(POPOVER_GAP)))
                .snap_to_window_with_margin(px(POPOVER_MARGIN))
                .child(
                    a11y::node(
                        format!("about-panel-{model}"),
                        Role::Group,
                        about.name.clone(),
                    )
                    .occlude()
                    .on_mouse_down_out(move |_, window, cx| on_out(Some(false), window, cx))
                    .w(px(300.))
                    .max_h((room - px(2. * POPOVER_MARGIN)).max(px(120.)))
                    .overflow_y_scroll()
                    .px(px(16.))
                    .py(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .map(|pop| crate::surface::paint(pop, crate::surface::Kind::Popover, theme))
                    .child(
                        div()
                            .text_size(theme.text(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(about.name.clone()),
                    )
                    .child(line(
                        format!("about-summary-{model}"),
                        about.summary.clone(),
                        12.,
                    ))
                    .when(!about.good_at.is_empty(), |p| {
                        p.child(section(
                            format!("about-good-{model}"),
                            "Good at",
                            about.good_at.clone(),
                        ))
                    })
                    .when(!about.watch_out.is_empty(), |p| {
                        p.child(section(
                            format!("about-watch-{model}"),
                            "Watch out for",
                            about.watch_out.clone(),
                        ))
                    })
                    .children(facts.into_iter().map(|(key, fact)| {
                        line(format!("about-{key}-{model}"), fact, 12.)
                            .text_color(theme.text_secondary)
                    })),
                ),
        )
        .with_priority(2)
    });
    div().relative().flex_none().child(info).children(panel)
}

/// Which explainer is open, and the focus handles a list's controls keep across frames.
#[derive(Default)]
struct Controls {
    open: Option<ModelRef>,
    focus: HashMap<(ModelRef, &'static str), FocusHandle>,
}

impl Controls {
    fn focus(&mut self, model: &ModelRef, control: &'static str, cx: &mut App) -> FocusHandle {
        self.focus
            .entry((model.clone(), control))
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    fn toggle(&mut self, model: &ModelRef, want: Option<bool>) {
        let open = want.unwrap_or(self.open.as_ref() != Some(model));
        self.open = open.then(|| model.clone());
    }
}

/// Download, or the progress bar and Cancel while it runs. `None` when there is nothing to fetch.
fn download_controls<K: ListKind>(
    models: &Entity<Models>,
    id: &ModelId<K>,
    row: &Row,
    controls: &mut Controls,
    theme: &Theme,
    cx: &mut App,
) -> Option<gpui_kit::Div> {
    let model = row.model.clone();
    match &row.state {
        RowState::NeedsDownload => {
            let focus = controls.focus(&model, "download", cx);
            let (models, id) = (models.clone(), id.clone());
            let label = match &row.download {
                Some(download) => format!("Download {}, {download}", row.name),
                None => format!("Download {}", row.name),
            };
            Some(div().child(button(
                format!("download-{model}"),
                label,
                "Download",
                false,
                &focus,
                theme,
                move |_, cx| models.update(cx, |m, cx| m.download(id.clone(), cx)),
            )))
        }
        RowState::Downloading(percent) => {
            let focus = controls.focus(&model, "cancel", cx);
            let models = models.clone();
            let cancel_model = model.clone();
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().w(px(120.)).child(progress_bar(
                        format!("progress-{model}"),
                        format!("Downloading {}", row.name),
                        *percent,
                        theme,
                    )))
                    .child(button(
                        format!("cancel-{model}"),
                        format!("Cancel downloading {}", row.name),
                        "Cancel",
                        false,
                        &focus,
                        theme,
                        move |_, cx| models.update(cx, |m, cx| m.cancel(&cancel_model, cx)),
                    )),
            )
        }
        RowState::Ready | RowState::Unavailable(_) => None,
    }
}

/// Every model of one kind as one radio group, with Automatic first and Off last for a kind that
/// can be off: what Settings > Transcription and Settings > Summaries show.
pub struct ModelList<K: ListKind> {
    models: Entity<Models>,
    controls: Controls,
    group: FocusHandle,
    _observe: Subscription,
    kind: PhantomData<K>,
}

impl<K: ListKind> ModelList<K> {
    pub fn new(models: Entity<Models>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&models, |_, _, cx| cx.notify());
        Self {
            models,
            controls: Controls::default(),
            group: cx.focus_handle(),
            _observe: observe,
            kind: PhantomData,
        }
    }
}

impl<K: ListKind> ModelList<K> {
    /// Opens the first model's explainer. False until the models are listed.
    pub fn open_first_explainer(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(first) = K::rows(self.models.read(cx)).and_then(|rows| rows.first()) else {
            return false;
        };
        self.controls.open = Some(first.status.entry.id.erase());
        cx.notify();
        true
    }
}

impl<K: ListKind> Render for ModelList<K> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let tag = K::TAG.as_str();
        let models = self.models.read(cx);
        let (Some(settings), Some(rows)) = (models.settings.clone(), K::rows(models)) else {
            return div()
                .px(px(14.))
                .py(px(11.))
                .child(note("Checking models", &theme))
                .into_any_element();
        };
        let rows: Vec<ModelRow<K>> = rows.to_vec();
        let app = models.app.clone();
        let current = K::choice(&settings).clone();
        let this = cx.entity().downgrade();

        let mut choices: Vec<Choice<K>> = vec![Choice::Auto];
        let mut group = RadioGroup::new(format!("{tag}-models"), format!("{} model", K::NOUN))
            .option(
                Radio::new(format!("choice-auto-{tag}"), "Automatic")
                    .description(auto_line(&rows, &settings)),
            );
        for model_row in &rows {
            let entry = &model_row.status.entry;
            let worded = row(
                model_row,
                &settings,
                app.read(cx).download_of(&entry.id.erase()),
            );
            let model = worded.model.clone();
            let about_focus = self.controls.focus(&model, "about", cx);
            let open = self.controls.open.as_ref() == Some(&model);
            let toggle = {
                let this = this.clone();
                let model = model.clone();
                move |want: Option<bool>, _: &mut Window, cx: &mut App| {
                    let _ = this.update(cx, |list, cx| {
                        list.controls.toggle(&model, want);
                        cx.notify();
                    });
                }
            };
            let fetch = download_controls(
                &self.models,
                &entry.id,
                &worded,
                &mut self.controls,
                &theme,
                cx,
            );
            let detail = {
                let mut parts: Vec<String> = worded.size.iter().cloned().collect();
                parts.push(worded.state.words());
                parts.join(" · ")
            };
            let trailing = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .children(fetch)
                .child(explainer(
                    about(entry, &worded, &settings),
                    &model,
                    open,
                    window.viewport_size().height,
                    &about_focus,
                    &theme,
                    toggle,
                ));
            choices.push(Choice::Pinned(entry.id.clone()));
            group = group.option(
                Radio::new(format!("model-{model}"), worded.name.clone())
                    .description(detail)
                    .warning(worded.fit.clone())
                    .enabled(worded.usable || worded.chosen)
                    .trailing(trailing),
            );
        }
        if K::OPTIONAL {
            choices.push(Choice::Off);
            group = group.option(
                Radio::new(format!("choice-off-{tag}"), "Off")
                    .description(format!("Dory makes no {} at all.", K::NOUN.to_lowercase())),
            );
        }
        let selected = choices.iter().position(|c| *c == current);
        let models = self.models.clone();
        group
            .selected(selected)
            .render(&self.group, &theme, window, move |ix, _, cx| {
                if let Some(choice) = choices.get(ix).cloned() {
                    models.update(cx, |m, cx| m.choose(choice, cx));
                }
            })
            .into_any_element()
    }
}

/// The onboarding step that gets the speech model ready (UX-16): the model Automatic would pick
/// for the meeting languages, with Download and its progress.
pub fn speech_step(app: &Entity<AppState>, window: &mut Window, cx: &mut App) -> AnyElement {
    let models = install(app, cx);
    window
        .use_keyed_state("speech-step", cx, |_, cx| SpeechStep::new(models, cx))
        .into_any_element()
}

struct SpeechStep {
    models: Entity<Models>,
    controls: Controls,
    _observe: Subscription,
}

impl SpeechStep {
    fn new(models: Entity<Models>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&models, |_, _, cx| cx.notify());
        models.update(cx, |m, cx| m.refresh(cx));
        Self {
            models,
            controls: Controls::default(),
            _observe: observe,
        }
    }
}

impl Render for SpeechStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let group = a11y::node("speech-step", Role::Group, "Speech model")
            .flex()
            .flex_col()
            .gap(px(8.));
        let models = self.models.read(cx);
        let (Some(settings), Some(rows)) = (models.settings.clone(), models.speech.clone()) else {
            return group.child(note("Checking speech models", &theme));
        };
        let app = models.app.clone();
        let model = match auto_pick(&rows, &settings) {
            Resolution::Use(id) => Some(id.erase()),
            Resolution::Blocked(
                Blocker::NeedsProvision { model, .. } | Blocker::Unavailable { model, .. },
            ) => Some(model),
            Resolution::Blocked(_) | Resolution::Skip => None,
        };
        let Some(model_row) =
            model.and_then(|m| rows.iter().find(|r| r.status.entry.id.erase() == m))
        else {
            let wanted: Vec<String> = Stt::wanted(&settings).iter().map(languages::name).collect();
            let text = if wanted.is_empty() {
                "No speech model is available on this Mac.".to_owned()
            } else {
                format!(
                    "No speech model covers {}.",
                    languages::name_list(&wanted, "and")
                )
            };
            return group.child(
                a11y::node("speech-step-status", Role::Label, text.clone())
                    .text_size(theme.text(13.))
                    .text_color(theme.attention)
                    .child(text),
            );
        };
        let entry = model_row.status.entry.clone();
        let worded = row(
            model_row,
            &settings,
            app.read(cx).download_of(&entry.id.erase()),
        );
        let model = worded.model.clone();
        let this = cx.entity().downgrade();
        let toggle = {
            let model = model.clone();
            move |want: Option<bool>, _: &mut Window, cx: &mut App| {
                let _ = this.update(cx, |step, cx| {
                    step.controls.toggle(&model, want);
                    cx.notify();
                });
            }
        };
        let about_focus = self.controls.focus(&model, "about", cx);
        let open = self.controls.open.as_ref() == Some(&model);
        let status = match &worded.state {
            RowState::Ready => format!("{} is ready.", worded.name),
            RowState::NeedsDownload => {
                "Download it to transcribe. You can record while it downloads.".to_owned()
            }
            RowState::Downloading(_) => "You can record while it downloads.".to_owned(),
            RowState::Unavailable(why) => why.clone(),
        };
        let fetch = download_controls(
            &self.models,
            &entry.id,
            &worded,
            &mut self.controls,
            &theme,
            cx,
        );
        let heading = match &worded.size {
            Some(size) => format!("{}, {size}", worded.name),
            None => worded.name.clone(),
        };
        group
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        a11y::node("speech-step-model", Role::Label, heading.clone())
                            .flex_1()
                            .min_w_0()
                            .text_size(theme.text(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(heading),
                    )
                    .child(explainer(
                        about(&entry, &worded, &settings),
                        &model,
                        open,
                        window.viewport_size().height,
                        &about_focus,
                        &theme,
                        toggle,
                    ))
                    .children(fetch),
            )
            .child(
                a11y::node("speech-step-status", Role::Label, status.clone())
                    .text_size(theme.text(12.))
                    .text_color(theme.text_secondary)
                    .child(status),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    /// Parakeet's real figures: a 487 MB archive that unpacks to 671 MB. The row once showed
    /// one and the explainer the other, both as the download size.
    #[test]
    fn the_download_and_the_size_on_disk_are_named_apart() {
        let mut entry = dory_core::testing::stt_entry(
            "sherpa/parakeet",
            dory_core::models::DiarizationSupport::None,
        );
        entry.display_name = "Parakeet".into();
        entry.disk_bytes = Some(670_576_784);
        let model_row = |availability| ModelRow {
            status: dory_core::models::ModelStatus {
                entry: entry.clone(),
                availability,
            },
            fit: dory_core::models::Fit::Yes,
            chosen: false,
        };
        let settings = Settings::default();

        let missing = row(
            &model_row(Availability::NeedsProvision {
                bytes: Some(487_211_443),
            }),
            &settings,
            None,
        );
        assert_eq!(missing.size.as_deref(), Some("487 MB to download"));
        let info = about(&entry, &missing, &settings);
        assert_eq!(info.download.as_deref(), Some("487 MB"));
        assert_eq!(info.disk.as_deref(), Some("671 MB"));

        let ready = row(&model_row(Availability::Ready), &settings, None);
        assert_eq!(ready.size.as_deref(), Some("671 MB on disk"));
        let info = about(&entry, &ready, &settings);
        assert_eq!(info.download, None, "nothing left to download");
        assert_eq!(info.disk.as_deref(), Some("671 MB"));
    }

    #[test]
    fn sizes_read_in_megabytes_then_gigabytes() {
        assert_eq!(size_text(670_576_784), "671 MB");
        assert_eq!(size_text(1_036_613_791), "1.0 GB");
        assert_eq!(size_text(9_461_576_343), "9.5 GB");
    }

    #[test]
    fn long_language_lists_name_the_wanted_ones_first() {
        let set = LangSet::Only(
            ["en", "de", "fr", "es", "it", "nl", "pl", "tl"]
                .iter()
                .map(|t| tag(t))
                .collect(),
        );
        assert_eq!(
            languages_text(&set, &[tag("tl"), tag("en")]),
            "8 languages, including Tagalog, English, Dutch, French, German, Italian"
        );
        let short = LangSet::Only(vec![tag("en"), tag("fr")]);
        assert_eq!(languages_text(&short, &[]), "English and French");
        assert_eq!(languages_text(&LangSet::Any, &[]), "Any language");
    }
}

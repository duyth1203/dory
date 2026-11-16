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
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, anchored,
    deferred, div, prelude::FluentBuilder as _, px,
};

use crate::a11y;
use crate::app::{AppState, Download};
use crate::theme::Theme;

/// BCP-47 tags and English names, for every language a catalog row lists today.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("as", "Assamese"),
    ("az", "Azerbaijani"),
    ("ba", "Bashkir"),
    ("be", "Belarusian"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("bo", "Tibetan"),
    ("br", "Breton"),
    ("bs", "Bosnian"),
    ("ca", "Catalan"),
    ("cs", "Czech"),
    ("cy", "Welsh"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("eu", "Basque"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fil", "Filipino"),
    ("fo", "Faroese"),
    ("fr", "French"),
    ("gl", "Galician"),
    ("gu", "Gujarati"),
    ("ha", "Hausa"),
    ("haw", "Hawaiian"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("ht", "Haitian Creole"),
    ("hu", "Hungarian"),
    ("hy", "Armenian"),
    ("id", "Indonesian"),
    ("is", "Icelandic"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jv", "Javanese"),
    ("ka", "Georgian"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("kn", "Kannada"),
    ("ko", "Korean"),
    ("la", "Latin"),
    ("lb", "Luxembourgish"),
    ("ln", "Lingala"),
    ("lo", "Lao"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mg", "Malagasy"),
    ("mi", "Māori"),
    ("mk", "Macedonian"),
    ("ml", "Malayalam"),
    ("mn", "Mongolian"),
    ("mr", "Marathi"),
    ("ms", "Malay"),
    ("mt", "Maltese"),
    ("my", "Burmese"),
    ("nb", "Norwegian Bokmål"),
    ("ne", "Nepali"),
    ("nl", "Dutch"),
    ("nn", "Norwegian Nynorsk"),
    ("no", "Norwegian"),
    ("oc", "Occitan"),
    ("pa", "Punjabi"),
    ("pl", "Polish"),
    ("ps", "Pashto"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sa", "Sanskrit"),
    ("sd", "Sindhi"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sn", "Shona"),
    ("so", "Somali"),
    ("sq", "Albanian"),
    ("sr", "Serbian"),
    ("su", "Sundanese"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("tg", "Tajik"),
    ("th", "Thai"),
    ("tk", "Turkmen"),
    ("tl", "Tagalog"),
    ("tr", "Turkish"),
    ("tt", "Tatar"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
    ("yue", "Cantonese"),
    ("zh", "Chinese"),
];

/// "Tagalog" for `tl`, "English (US)" for `en-US`, and the tag itself when the table lacks it.
pub fn language_name(tag: &LangTag) -> String {
    let named = |code: &str| {
        LANGUAGES
            .iter()
            .find(|(t, _)| *t == code)
            .map(|(_, name)| *name)
    };
    match (named(tag.as_str()), named(tag.primary())) {
        (Some(name), _) => name.to_owned(),
        (None, Some(name)) => format!("{name} ({})", &tag.as_str()[tag.primary().len() + 1..]),
        (None, None) => tag.to_string(),
    }
}

/// "Tagalog", "Tagalog and English", "Tagalog, English, and French".
pub fn name_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

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
    pub size: Option<String>,
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
                .map(language_name)
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
    let bytes = match &row.status.availability {
        Availability::NeedsProvision { bytes: Some(b) } => Some(*b),
        _ => entry.disk_bytes,
    };
    Row {
        model: entry.id.erase(),
        name: entry.display_name.clone(),
        size: bytes.map(size_text),
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
    pub size: Option<String>,
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
        .map(language_name)
        .collect();
    let mut rest: Vec<String> = tags
        .iter()
        .map(language_name)
        .filter(|name| !names.contains(name))
        .collect();
    rest.sort();
    rest.dedup();
    names.extend(rest);
    if names.len() <= NAMED_LANGUAGES {
        return name_list(&names);
    }
    format!(
        "{} languages, including {}",
        names.len(),
        names[..NAMED_LANGUAGES].join(", ")
    )
}

pub fn about<K: ListKind>(entry: &ModelEntry<K>, settings: &Settings) -> About {
    About {
        name: entry.display_name.clone(),
        summary: entry.about.summary.clone(),
        good_at: entry.about.pros.clone(),
        watch_out: entry.about.cons.clone(),
        size: entry.disk_bytes.map(size_text),
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
        .h(px(28.))
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

/// A radio button or a checkbox, and whether it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Radio(bool),
    Check(bool),
}

/// A radio button or checkbox row: the option on one line and its effect on the next (UX-20).
pub(crate) fn option(
    id: impl Into<SharedString>,
    mark: Mark,
    label: impl Into<SharedString>,
    effect: impl Into<SharedString>,
    focus: &FocusHandle,
    theme: &Theme,
    press: impl Fn(&mut Window, &mut App) + Clone + 'static,
) -> a11y::Node {
    let label: SharedString = label.into();
    let effect: SharedString = effect.into();
    let (role, on, mark) = match mark {
        Mark::Radio(on) => (Role::RadioButton, on, if on { "●" } else { "○" }),
        Mark::Check(on) => (Role::CheckBox, on, if on { "☑" } else { "☐" }),
    };
    a11y::focusable(a11y::node(id, role, label.clone()), focus, theme, false)
        .aria_toggled(if on {
            gpui_kit::accesskit::Toggled::True
        } else {
            gpui_kit::accesskit::Toggled::False
        })
        .aria_description(effect.clone())
        .on_click(move |_, window, cx| press(window, cx))
        .flex()
        .items_start()
        .gap(px(8.))
        .px(px(6.))
        .py(px(4.))
        .min_h(px(28.))
        .rounded(px(6.))
        .child(
            div()
                .flex_none()
                .w(px(18.))
                .text_size(theme.text(14.))
                .text_color(if on {
                    theme.accent_text
                } else {
                    theme.text_secondary
                })
                .child(mark),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_size(theme.text(13.)).child(label))
                .when(!effect.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(theme.text(12.))
                            .text_color(theme.text_secondary)
                            .child(effect),
                    )
                }),
        )
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
        .bg(theme.field_bg)
        .child(
            div()
                .h_full()
                .rounded(px(3.))
                .bg(theme.accent)
                .w(gpui_kit::relative(fill)),
        )
}

/// The info button and, while open, its popover (UX-28). Click, Return, or Space toggles it
/// (GPUI clicks a focused element on both keys); Escape or a click outside closes it.
fn explainer(
    about: About,
    model: &ModelRef,
    open: bool,
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
    .on_click(move |_, window, cx| on_click(None, window, cx))
    .on_action(move |_: &Confirm, window, cx| {
        on_confirm(None, window, cx);
        cx.stop_propagation();
    })
    .on_key_down(move |event: &KeyDownEvent, window, cx| {
        if open && event.keystroke.key == "escape" {
            on_escape(Some(false), window, cx);
            cx.stop_propagation();
        }
    })
    .flex_none()
    .size(px(24.))
    .flex()
    .items_center()
    .justify_center()
    .rounded_full()
    .bg(theme.field_bg)
    .text_size(theme.text(12.))
    .font_weight(FontWeight::SEMIBOLD)
    .text_color(theme.accent_text)
    .child("i");
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
        if let Some(size) = &about.size {
            facts.push(("size", format!("Download size: {size}")));
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
                .snap_to_window_with_margin(px(8.))
                .child(
                    a11y::node(
                        format!("about-panel-{model}"),
                        Role::Group,
                        about.name.clone(),
                    )
                    .occlude()
                    .on_mouse_down_out(move |_, window, cx| on_out(Some(false), window, cx))
                    .mt(px(28.))
                    .w(px(340.))
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .rounded(px(10.))
                    .bg(theme.menu_bg)
                    .border_1()
                    .border_color(theme.menu_border)
                    .shadow_lg()
                    .text_color(theme.text)
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
            let label = match &row.size {
                Some(size) => format!("Download {}, {size}", row.name),
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

/// Every model of one kind with Automatic first, and Off for a kind that can be off: what
/// Settings > Transcription and Settings > Summaries show.
pub struct ModelList<K: ListKind> {
    models: Entity<Models>,
    controls: Controls,
    auto_focus: FocusHandle,
    off_focus: FocusHandle,
    _observe: Subscription,
    kind: PhantomData<K>,
}

impl<K: ListKind> ModelList<K> {
    pub fn new(models: Entity<Models>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&models, |_, _, cx| cx.notify());
        Self {
            models,
            controls: Controls::default(),
            auto_focus: cx.focus_handle(),
            off_focus: cx.focus_handle(),
            _observe: observe,
            kind: PhantomData,
        }
    }
}

impl<K: ListKind> Render for ModelList<K> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let tag = K::TAG.as_str();
        let list = a11y::node(
            format!("{tag}-models"),
            Role::List,
            format!("{} models", K::NOUN),
        )
        .flex()
        .flex_col()
        .gap(px(2.));
        let models = self.models.read(cx);
        let (Some(settings), Some(rows)) = (models.settings.clone(), K::rows(models)) else {
            return list.child(note("Checking models", &theme));
        };
        let rows: Vec<ModelRow<K>> = rows.to_vec();
        let app = models.app.clone();
        let choice = K::choice(&settings).clone();
        let this = cx.entity().downgrade();

        let pick = |choice: Choice<K>| {
            let models = self.models.clone();
            move |_: &mut Window, cx: &mut App| {
                let choice = choice.clone();
                models.update(cx, |m, cx| m.choose(choice, cx))
            }
        };
        let mut list = list.child(option(
            format!("choice-auto-{tag}"),
            Mark::Radio(choice == Choice::Auto),
            "Automatic",
            auto_line(&rows, &settings),
            &self.auto_focus,
            &theme,
            pick(Choice::Auto),
        ));

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
            let choose = (!worded.chosen && worded.usable).then(|| {
                let focus = self.controls.focus(&model, "choose", cx);
                button(
                    format!("choose-{model}"),
                    format!("Choose {}", worded.name),
                    "Choose",
                    false,
                    &focus,
                    &theme,
                    pick(Choice::Pinned(entry.id.clone())),
                )
            });
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
                if !matches!(worded.state, RowState::Downloading(_)) {
                    parts.push(worded.state.words());
                }
                parts.join(" · ")
            };
            list = list.child(
                a11y::node(format!("model-{model}"), Role::ListItem, worded.label())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(6.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(theme.divider)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(theme.text(13.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(worded.name.clone()),
                                    )
                                    .when(worded.chosen, |d| {
                                        d.child(
                                            div()
                                                .text_size(theme.text(12.))
                                                .text_color(theme.accent_text)
                                                .child("✓ In use"),
                                        )
                                    }),
                            )
                            .child(note(detail, &theme))
                            .when_some(worded.fit.clone(), |d, fit| {
                                d.child(
                                    div()
                                        .text_size(theme.text(12.))
                                        .text_color(theme.attention)
                                        .child(format!("⚠︎ {fit}")),
                                )
                            }),
                    )
                    .child(explainer(
                        about(entry, &settings),
                        &model,
                        open,
                        &about_focus,
                        &theme,
                        toggle,
                    ))
                    .children(fetch)
                    .children(choose),
            );
        }

        if K::OPTIONAL {
            list = list.child(option(
                format!("choice-off-{tag}"),
                Mark::Radio(choice == Choice::Off),
                "Off",
                format!("Dory makes no {} at all.", K::NOUN.to_lowercase()),
                &self.off_focus,
                &theme,
                pick(Choice::Off),
            ));
        }
        list
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            let wanted: Vec<String> = Stt::wanted(&settings).iter().map(language_name).collect();
            let text = if wanted.is_empty() {
                "No speech model is available on this Mac.".to_owned()
            } else {
                format!("No speech model covers {}.", name_list(&wanted))
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
                        about(&entry, &settings),
                        &model,
                        open,
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

/// The languages a speech model in the catalog covers, by name. Every known language when a
/// model takes any language, or before the list loads.
fn language_options(models: &Models, selected: &[LangTag]) -> Vec<LangTag> {
    let any = || {
        LANGUAGES
            .iter()
            .filter_map(|(tag, _)| LangTag::parse(tag).ok())
            .collect::<Vec<_>>()
    };
    let mut tags = match &models.speech {
        Some(rows) if !rows.is_empty() => {
            let mut tags = Vec::new();
            for r in rows {
                match Stt::languages(&r.status.entry.caps) {
                    LangSet::Any => return sorted(any(), selected),
                    LangSet::Only(only) => tags.extend(only.iter().cloned()),
                }
            }
            tags
        }
        _ => any(),
    };
    tags.extend(selected.iter().cloned());
    sorted(tags, &[])
}

fn sorted(mut tags: Vec<LangTag>, extra: &[LangTag]) -> Vec<LangTag> {
    tags.extend(extra.iter().cloned());
    tags.sort_by_key(|tag| (language_name(tag), tag.clone()));
    tags.dedup();
    tags
}

/// The meeting languages (UX-29): one checkbox per language, in name order.
pub fn meeting_languages_picker(
    app: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let models = install(app, cx);
    window
        .use_keyed_state("meeting-languages-picker", cx, |_, cx| {
            LanguagePicker::new(models, cx)
        })
        .into_any_element()
}

pub(crate) struct LanguagePicker {
    models: Entity<Models>,
    focus: HashMap<LangTag, FocusHandle>,
    _observe: Subscription,
}

impl LanguagePicker {
    pub(crate) fn new(models: Entity<Models>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&models, |_, _, cx| cx.notify());
        if models.read(cx).settings.is_none() {
            models.update(cx, |m, cx| m.refresh(cx));
        }
        Self {
            models,
            focus: HashMap::new(),
            _observe: observe,
        }
    }
}

impl Render for LanguagePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let group = a11y::node("meeting-languages", Role::Group, "Meeting languages")
            .flex()
            .flex_col()
            .gap(px(6.));
        let models = self.models.read(cx);
        let Some(selected) = models
            .settings
            .as_ref()
            .map(|s| s.meeting_languages.clone())
        else {
            return group.child(note("Loading", &theme));
        };
        let options = language_options(models, &selected);
        let names: Vec<String> = selected.iter().map(language_name).collect();
        let summary = if names.is_empty() {
            "None chosen, so any speech model can be picked.".to_owned()
        } else {
            format!("Meetings are in {}.", name_list(&names))
        };
        let mut list = a11y::node("meeting-languages-list", Role::List, "Languages")
            .flex()
            .flex_col()
            .max_h(px(220.))
            .overflow_y_scroll()
            .rounded(px(8.))
            .border_1()
            .border_color(theme.control_border)
            .p(px(4.));
        for tag in options {
            let on = selected.contains(&tag);
            let focus = self
                .focus
                .entry(tag.clone())
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let models = self.models.clone();
            let toggled = tag.clone();
            list = list.child(option(
                format!("meeting-language-{tag}"),
                Mark::Check(on),
                language_name(&tag),
                "",
                &focus,
                &theme,
                move |_, cx| {
                    let tag = toggled.clone();
                    models.update(cx, |m, cx| {
                        m.edit(
                            move |s| match s.meeting_languages.iter().position(|t| *t == tag) {
                                Some(ix) => {
                                    s.meeting_languages.remove(ix);
                                }
                                None => s.meeting_languages.push(tag.clone()),
                            },
                            cx,
                        )
                    })
                },
            ));
        }
        group
            .child(
                a11y::node("meeting-languages-summary", Role::Label, summary.clone())
                    .text_size(theme.text(13.))
                    .child(summary),
            )
            .child(note(
                "With Language on Automatic, Dory only picks a speech model that covers every language listed here.",
                &theme,
            ))
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    #[test]
    fn names_read_as_plain_english() {
        assert_eq!(language_name(&tag("tl")), "Tagalog");
        assert_eq!(language_name(&tag("en-US")), "English (US)");
        assert_eq!(language_name(&tag("xx")), "xx");
        let names = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(name_list(&names(&["Tagalog"])), "Tagalog");
        assert_eq!(
            name_list(&names(&["Tagalog", "English"])),
            "Tagalog and English"
        );
        assert_eq!(
            name_list(&names(&["Tagalog", "English", "French"])),
            "Tagalog, English, and French"
        );
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

    #[test]
    fn the_table_is_sorted_and_has_no_duplicates() {
        assert!(LANGUAGES.windows(2).all(|w| w[0].0 < w[1].0));
    }
}

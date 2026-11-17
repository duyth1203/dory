//! First run (UX-16, UX-17, UX-29): Welcome, Permissions, Models, then Done. Someone upgrading
//! from the old app sees only [`OnboardingStep::Upgrade`], the permissions again (P7).
//!
//! The step lives in the UI prefs file (`prefs.rs`), so a quit resumes at the same step. The shell
//! shows [`Onboarding`] in the page column while the step is not `Done` and nothing is selected,
//! with the sidebar and the record button still there: setup never blocks recording (UX-17).
//!
//! What a permission shows is one table, [`permission_row`], shared with the start screen's
//! sources card (UX-6). The Models step's language picker and speech models come from
//! `models_view`; the optional summaries setup is `set_up_summaries::sheet`.

use dory_core::domain::LangTag;
use dory_core::ports::capture::{Access, Source};
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, FontWeight, IntoElement, ParentElement as _,
    Render, Role, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
    relative,
};

use crate::a11y;
use crate::app::AppState;
use crate::detail::{button, spoken_button};
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OnboardingStep {
    Welcome,
    Permissions,
    Models,
    /// The permissions alone, for a library that came from the old app. macOS matches a grant to
    /// the app's code signature, so the new app asks again even though the library carries over.
    Upgrade,
    Done,
}

impl OnboardingStep {
    pub fn next(self) -> OnboardingStep {
        match self {
            OnboardingStep::Welcome => OnboardingStep::Permissions,
            OnboardingStep::Permissions => OnboardingStep::Models,
            OnboardingStep::Models | OnboardingStep::Upgrade | OnboardingStep::Done => {
                OnboardingStep::Done
            }
        }
    }

    /// The step to open on. Every first run of this app saves a step, so recordings with no
    /// saved step can only have come from `dory-cli import` of the old app's folder. Their owner
    /// already records and has models, so they see only [`OnboardingStep::Upgrade`].
    pub fn resolve(saved: Option<OnboardingStep>, has_recordings: bool) -> OnboardingStep {
        match saved {
            Some(step) => step,
            None if has_recordings => OnboardingStep::Upgrade,
            None => OnboardingStep::Welcome,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OnboardingStep::Welcome => "welcome",
            OnboardingStep::Permissions => "permissions",
            OnboardingStep::Models => "models",
            OnboardingStep::Upgrade => "upgrade",
            OnboardingStep::Done => "done",
        }
    }

    /// `None` for a name this version does not know, so a newer file never resets the window.
    pub fn parse(s: &str) -> Option<OnboardingStep> {
        [
            OnboardingStep::Welcome,
            OnboardingStep::Permissions,
            OnboardingStep::Models,
            OnboardingStep::Upgrade,
            OnboardingStep::Done,
        ]
        .into_iter()
        .find(|step| step.as_str() == s)
    }

    /// The step's heading.
    pub fn title(self) -> &'static str {
        match self {
            OnboardingStep::Welcome => "Welcome to Dory",
            OnboardingStep::Permissions => "Allow recording",
            OnboardingStep::Models => "Languages and speech",
            OnboardingStep::Upgrade => "Allow recording again",
            OnboardingStep::Done => "",
        }
    }

    /// "Step 2 of 3", or `None` once done.
    fn counter(self) -> Option<String> {
        let n = match self {
            OnboardingStep::Welcome => 1,
            OnboardingStep::Permissions => 2,
            OnboardingStep::Models => 3,
            OnboardingStep::Upgrade | OnboardingStep::Done => return None,
        };
        Some(format!("Step {n} of 3"))
    }
}

/// What the app knows about one permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// The launch probe has not answered yet.
    Checking,
    /// `request_access` is waiting for the user to answer the macOS prompt.
    Asking,
    Known(Access),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionButton {
    /// Calls `request_access`, which shows the macOS prompt when macOS still can.
    Grant,
    /// macOS never asks twice, so a denied permission is changed in System Settings.
    OpenSystemSettings,
}

impl PermissionButton {
    pub fn label(self) -> &'static str {
        match self {
            PermissionButton::Grant => "Grant",
            PermissionButton::OpenSystemSettings => "Open System Settings",
        }
    }
}

/// A permission's status, glyph and word (A11Y-8), and its one button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionRow {
    pub glyph: &'static str,
    pub word: &'static str,
    /// Shown in the attention color.
    pub attention: bool,
    pub button: Option<PermissionButton>,
}

const fn row(
    glyph: &'static str,
    word: &'static str,
    attention: bool,
    button: Option<PermissionButton>,
) -> PermissionRow {
    PermissionRow {
        glyph,
        word,
        attention,
        button,
    }
}

/// The permission state table (UX-16). macOS has no way to read or prompt for System Audio
/// Recording before the first tap, so its `Undetermined` is permanent until a recording runs.
pub fn permission_row(source: Source, permission: Permission) -> PermissionRow {
    use PermissionButton::{Grant, OpenSystemSettings};
    match (source, permission) {
        (_, Permission::Checking) => row("…", "Checking", false, None),
        (_, Permission::Asking) => row("…", "Waiting for your answer", false, None),
        (_, Permission::Known(Access::Granted)) => row("✓", "Allowed", false, None),
        (Source::Microphone, Permission::Known(Access::Undetermined)) => {
            row("○", "Not asked yet", false, Some(Grant))
        }
        (Source::SystemAudio, Permission::Known(Access::Undetermined)) => {
            row("○", "Asked the first time you record", false, Some(Grant))
        }
        (_, Permission::Known(Access::Denied)) => {
            row("▲", "Not allowed", true, Some(OpenSystemSettings))
        }
        (_, Permission::Known(Access::Unsupported)) => {
            row("▲", "Not available on this Mac", true, None)
        }
    }
}

/// After Grant: whether to open System Settings because no prompt could be shown. Only System
/// Audio Recording has no prompt API; a microphone the user just denied stays on this screen.
pub fn opens_settings_after_grant(source: Source, answer: Access) -> bool {
    source == Source::SystemAudio && answer == Access::Undetermined
}

pub fn source_name(source: Source) -> &'static str {
    match source {
        Source::Microphone => "Microphone",
        Source::SystemAudio => "System Audio Recording",
    }
}

/// The sentence shown before macOS asks.
pub fn why(source: Source) -> &'static str {
    match source {
        Source::Microphone => "Dory records your side of the call from the microphone.",
        Source::SystemAudio => {
            "Dory records the other people on the call from your Mac's audio. It never records the screen."
        }
    }
}

pub fn settings_url(source: Source) -> &'static str {
    match source {
        Source::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        Source::SystemAudio => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
    }
}

/// The first preferred language's primary subtag ("en" for "en-GB"): meetings are held in a
/// language, not a region.
pub fn system_language() -> Option<LangTag> {
    #[cfg(target_os = "macos")]
    let preferred = objc2_foundation::NSLocale::preferredLanguages()
        .firstObject()
        .map(|s| s.to_string());
    #[cfg(not(target_os = "macos"))]
    let preferred = std::env::var("LANG").ok();
    let tag = LangTag::parse(preferred?.split('.').next()?).ok()?;
    LangTag::parse(tag.primary()).ok()
}

/// The status and button of `source`, for this screen and the start screen's sources card.
pub fn permission_button(
    app: &Entity<AppState>,
    source: Source,
    focus: &FocusHandle,
    theme: &Theme,
    cx: &App,
) -> Option<a11y::Node> {
    let which = permission_row(source, app.read(cx).permission(source)).button?;
    let id = match (source, which) {
        (Source::Microphone, PermissionButton::Grant) => "grant-microphone",
        (Source::SystemAudio, PermissionButton::Grant) => "grant-system-audio",
        (Source::Microphone, PermissionButton::OpenSystemSettings) => "settings-microphone",
        (Source::SystemAudio, PermissionButton::OpenSystemSettings) => "settings-system-audio",
    };
    let app = app.clone();
    Some(spoken_button(
        id,
        which.label(),
        format!("{} for {}", which.label(), source_name(source)),
        false,
        focus,
        theme,
        move |_, cx| match which {
            PermissionButton::Grant => app.update(cx, |app, cx| app.request_access(source, cx)),
            PermissionButton::OpenSystemSettings => cx.open_url(settings_url(source)),
        },
    ))
}

pub fn permission_status(row: PermissionRow, theme: &Theme) -> impl IntoElement + use<> {
    div()
        .flex()
        .gap(px(4.))
        .text_size(theme.text(12.))
        .text_color(if row.attention {
            theme.attention
        } else {
            theme.text_secondary
        })
        .child(row.glyph)
        .child(row.word)
}

pub struct Onboarding {
    app: Entity<AppState>,
    primary: FocusHandle,
    skip: FocusHandle,
    summaries: FocusHandle,
    grant: [FocusHandle; 2],
    /// The step the last frame showed, so a new step moves focus to its primary button.
    shown: Option<OnboardingStep>,
    _app: Subscription,
}

impl Onboarding {
    pub fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&app, |_, _, cx| cx.notify());
        Self {
            app,
            primary: cx.focus_handle(),
            skip: cx.focus_handle(),
            summaries: cx.focus_handle(),
            grant: [cx.focus_handle(), cx.focus_handle()],
            shown: None,
            _app: observe,
        }
    }

    pub fn primary_focus(&self) -> &FocusHandle {
        &self.primary
    }

    fn welcome(&self, theme: &Theme) -> Vec<gpui_kit::AnyElement> {
        vec![
            heading(OnboardingStep::Welcome.title(), theme),
            paragraph(
                "Dory records your calls, then writes the transcript and summary on this Mac. Nothing leaves it.",
                theme,
            ),
        ]
    }

    fn permissions(&self, theme: &Theme, cx: &App) -> Vec<gpui_kit::AnyElement> {
        self.permission_rows(
            vec![
                heading(OnboardingStep::Permissions.title(), theme),
                paragraph(
                    "macOS asks before Dory can hear anything. Here is what each permission is for.",
                    theme,
                ),
            ],
            theme,
            cx,
        )
    }

    fn upgrade(&self, theme: &Theme, cx: &App) -> Vec<gpui_kit::AnyElement> {
        self.permission_rows(
            vec![
                heading(OnboardingStep::Upgrade.title(), theme),
                paragraph(
                    "Your recordings, transcripts, and summaries are all here. This version of Dory is a new app to macOS, so it asks for the permissions once more.",
                    theme,
                ),
            ],
            theme,
            cx,
        )
    }

    fn permission_rows(
        &self,
        mut out: Vec<gpui_kit::AnyElement>,
        theme: &Theme,
        cx: &App,
    ) -> Vec<gpui_kit::AnyElement> {
        for (source, focus) in [Source::Microphone, Source::SystemAudio]
            .into_iter()
            .zip(&self.grant)
        {
            let state = permission_row(source, self.app.read(cx).permission(source));
            let name = source_name(source);
            out.push(
                a11y::node(
                    format!("permission-{}", source_slug(source)),
                    Role::Group,
                    format!("{name}, {}", state.word),
                )
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(16.))
                .p(px(16.))
                .rounded(px(12.))
                .bg(theme.card_bg)
                .border_1()
                .border_color(theme.card_border)
                .child(
                    // Grows into the row, but asks for 240 pt before the button may share its
                    // line: in a narrower card the button wraps below instead of spilling out.
                    div()
                        .flex_grow(1.)
                        .flex_shrink(1.)
                        .flex_basis(px(240.))
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(theme.text(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name),
                        )
                        .child(
                            div()
                                .text_size(theme.text(13.))
                                .text_color(theme.text_secondary)
                                .line_height(relative(1.4))
                                .child(why(source)),
                        )
                        .child(permission_status(state, theme)),
                )
                .children(permission_button(&self.app, source, focus, theme, cx))
                .into_any_element(),
            );
        }
        out
    }

    fn models(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<gpui_kit::AnyElement> {
        vec![
            heading(OnboardingStep::Models.title(), theme),
            paragraph("Which languages are your meetings in?", theme),
            crate::models_view::meeting_languages_picker(&self.app, window, cx),
            crate::models_view::speech_step(&self.app, window, cx),
            a11y::node("onboarding-queue-note", Role::Label, QUEUE_NOTE)
                .text_size(theme.text(13.))
                .text_color(theme.text_secondary)
                .line_height(relative(1.4))
                .child(QUEUE_NOTE)
                .into_any_element(),
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(theme.text(13.))
                        .text_color(theme.text_secondary)
                        .child("Summaries are optional and need Ollama."),
                )
                .child(button(
                    "onboarding-summaries",
                    "Set up summaries",
                    false,
                    &self.summaries,
                    theme,
                    crate::set_up_summaries::sheet,
                ))
                .into_any_element(),
        ]
    }
}

/// UX-17, said where the user decides whether to wait for the download.
pub const QUEUE_NOTE: &str = "You can record right away. Recordings wait in the queue and transcribe once the speech model is ready.";

fn source_slug(source: Source) -> &'static str {
    match source {
        Source::Microphone => "microphone",
        Source::SystemAudio => "system-audio",
    }
}

fn heading(text: &'static str, theme: &Theme) -> gpui_kit::AnyElement {
    a11y::node("onboarding-heading", Role::Heading, text)
        .aria_level(1)
        .text_size(theme.text(26.))
        .font_weight(FontWeight::BOLD)
        .child(text)
        .into_any_element()
}

fn paragraph(text: &'static str, theme: &Theme) -> gpui_kit::AnyElement {
    div()
        .text_size(theme.text(15.))
        .text_color(theme.text_secondary)
        .line_height(relative(1.4))
        .child(text)
        .into_any_element()
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let step = self.app.read(cx).onboarding();
        if self.shown != Some(step) {
            // Focus lands on Continue, so VoiceOver would otherwise say only "Continue".
            if self.shown.is_some() && step != OnboardingStep::Done {
                a11y::announce(step.title());
            }
            self.shown = Some(step);
            window.focus(&self.primary, cx);
        }
        let body = match step {
            OnboardingStep::Welcome => self.welcome(&theme),
            OnboardingStep::Permissions => self.permissions(&theme, cx),
            OnboardingStep::Models => self.models(&theme, window, cx),
            OnboardingStep::Upgrade => self.upgrade(&theme, cx),
            OnboardingStep::Done => Vec::new(),
        };
        a11y::node("onboarding", Role::Group, "Set up Dory")
            .flex()
            .flex_col()
            .gap(px(20.))
            .pt(px(72.))
            .children(step.counter().map(|counter| {
                div()
                    .text_size(theme.text(12.))
                    .text_color(theme.text_secondary)
                    .child(counter)
            }))
            .children(body)
    }
}

impl Onboarding {
    /// The step's buttons. The shell pins them under the scrolling page, so Continue stays in
    /// view however tall the step grows.
    pub fn actions(&self, cx: &App) -> AnyElement {
        let theme = Theme::get(cx);
        let step = self.app.read(cx).onboarding();
        let app = self.app.clone();
        let primary = button(
            "onboarding-continue",
            "Continue",
            true,
            &self.primary,
            &theme,
            move |_, cx| app.update(cx, |app, cx| app.continue_onboarding(cx)),
        );
        let app = self.app.clone();
        let skip = (step == OnboardingStep::Models).then(|| {
            button(
                "onboarding-skip",
                "Skip for Now",
                false,
                &self.skip,
                &theme,
                move |_, cx| app.update(cx, |app, cx| app.skip_onboarding(cx)),
            )
        });
        a11y::node("onboarding-actions", Role::Group, "Setup actions")
            .flex()
            .justify_end()
            .gap(px(8.))
            .children(skip)
            .child(primary)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_advance_in_order_and_stop_at_done() {
        let mut step = OnboardingStep::Welcome;
        let mut seen = vec![step];
        while step != OnboardingStep::Done {
            step = step.next();
            seen.push(step);
        }
        assert_eq!(
            seen,
            [
                OnboardingStep::Welcome,
                OnboardingStep::Permissions,
                OnboardingStep::Models,
                OnboardingStep::Done
            ]
        );
        assert_eq!(OnboardingStep::Done.next(), OnboardingStep::Done);
    }

    #[test]
    fn step_names_round_trip_and_unknown_names_are_none() {
        for step in [
            OnboardingStep::Welcome,
            OnboardingStep::Permissions,
            OnboardingStep::Models,
            OnboardingStep::Upgrade,
            OnboardingStep::Done,
        ] {
            assert_eq!(OnboardingStep::parse(step.as_str()), Some(step));
        }
        assert_eq!(OnboardingStep::parse("tour"), None);
    }
}

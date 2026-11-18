//! First run (UX-16, UX-17) and Re-transcribe in… (UX-26) through the window.

mod common;

use std::sync::Arc;

use dory_core::domain::{Goal, LangTag, LanguagePref};
use dory_core::ports::StoreRead as _;
use dory_core::ports::capture::{Access, Source};
use dory_core::testing::store::MemStore;
use dory_gpui::actions::RetranscribeIn;
use dory_gpui::app::AppState;
use dory_gpui::library::Selection;
use dory_gpui::onboarding::{
    OnboardingStep, Permission, PermissionButton, opens_settings_after_grant, permission_row,
    system_language,
};
use dory_gpui::prefs::Prefs;
use dory_gpui::processing::StepGlyph;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, TestAppContext, test::TestWindowExt as _,
};

use crate::common::{dory_on, id_of, open_fresh, open_with, open_with_prefs, sample_store};

fn step(app: &Entity<AppState>, cx: &mut TestAppContext) -> OnboardingStep {
    app.read_with(cx, |app, _| app.onboarding())
}

fn click(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    let id = gpui_kit::ElementId::Name(id.to_owned().into());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn label(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<String> {
    let id = gpui_kit::ElementId::Name(id.to_owned().into());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(id)
            .and_then(|e| e.label().map(str::to_owned))
    })
    .unwrap()
}

fn focused(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> bool {
    let id = gpui_kit::ElementId::Name(id.to_owned().into());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).and_then(|e| e.focused()) == Some(true)
    })
    .unwrap()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dory-gpui-onboarding-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[gpui_kit::test]
fn a_new_library_starts_at_welcome_with_continue_focused(cx: &mut TestAppContext) {
    let (window, app) = open_fresh(cx, dory_on(Arc::new(MemStore::new())));
    assert_eq!(step(&app, cx), OnboardingStep::Welcome);
    assert_eq!(
        label(window, "onboarding-heading", cx).as_deref(),
        Some("Welcome to Dory")
    );
    assert!(focused(window, "onboarding-continue", cx));
    assert!(
        label(window, "start-recording", cx).is_none(),
        "onboarding takes the start screen's place"
    );
}

#[gpui_kit::test]
fn an_upgraded_library_sees_only_the_permissions_then_the_start_screen(cx: &mut TestAppContext) {
    let (window, app) = open_fresh(cx, dory_on(sample_store()));
    assert_eq!(step(&app, cx), OnboardingStep::Upgrade);
    assert_eq!(
        label(window, "onboarding-heading", cx).as_deref(),
        Some("Allow recording again")
    );
    assert!(label(window, "permission-microphone", cx).is_some());
    assert!(label(window, "permission-system-audio", cx).is_some());
    assert!(
        label(window, "onboarding-skip", cx).is_none(),
        "no models step to skip"
    );
    assert!(focused(window, "onboarding-continue", cx));

    click(window, "onboarding-continue", cx);
    assert_eq!(step(&app, cx), OnboardingStep::Done, "one step only");
    assert!(label(window, "onboarding", cx).is_none());
    assert!(focused(window, "start-recording", cx));
}

#[gpui_kit::test]
fn a_finished_first_run_never_shows_the_upgrade_step(cx: &mut TestAppContext) {
    let prefs = Prefs {
        onboarding: Some(OnboardingStep::Done),
        ..Prefs::default()
    };
    let (window, app) = open_with_prefs(cx, dory_on(sample_store()), prefs);
    assert_eq!(step(&app, cx), OnboardingStep::Done);
    assert!(label(window, "start-recording", cx).is_some());
}

#[gpui_kit::test]
fn the_step_survives_a_quit_and_reopen(cx: &mut TestAppContext) {
    let (window, app) = open_fresh(cx, dory_on(Arc::new(MemStore::new())));
    click(window, "onboarding-continue", cx);
    assert_eq!(step(&app, cx), OnboardingStep::Permissions);

    let dir = scratch("reopen");
    app.read_with(cx, |app, _| app.prefs.clone())
        .write(&dir)
        .unwrap();
    let reopened = Prefs::read(&dir);
    assert_eq!(reopened.onboarding, Some(OnboardingStep::Permissions));

    let (window, app) = open_with_prefs(cx, dory_on(sample_store()), reopened);
    assert_eq!(
        step(&app, cx),
        OnboardingStep::Permissions,
        "a saved step wins over a library that now has recordings"
    );
    assert_eq!(
        label(window, "onboarding-heading", cx).as_deref(),
        Some("Allow recording")
    );
    assert_eq!(
        label(window, "permission-microphone", cx).as_deref(),
        Some("Microphone, Allowed"),
        "the fake capture reports both permissions granted"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_permission_state_table() {
    use PermissionButton::{Grant, OpenSystemSettings};
    let mic = Source::Microphone;
    let tap = Source::SystemAudio;
    let cases = [
        (
            mic,
            Permission::Checking,
            StepGlyph::Dots,
            "Checking",
            false,
            None,
        ),
        (
            mic,
            Permission::Asking,
            StepGlyph::Dots,
            "Waiting for your answer",
            false,
            None,
        ),
        (
            mic,
            Permission::Known(Access::Granted),
            StepGlyph::Check,
            "Allowed",
            false,
            None,
        ),
        (
            mic,
            Permission::Known(Access::Undetermined),
            StepGlyph::Pending,
            "Not asked yet",
            false,
            Some(Grant),
        ),
        (
            mic,
            Permission::Known(Access::Denied),
            StepGlyph::Warning,
            "Not allowed",
            true,
            Some(OpenSystemSettings),
        ),
        (
            mic,
            Permission::Known(Access::Unsupported),
            StepGlyph::Warning,
            "Not available on this Mac",
            true,
            None,
        ),
        (
            tap,
            Permission::Checking,
            StepGlyph::Dots,
            "Checking",
            false,
            None,
        ),
        (
            tap,
            Permission::Known(Access::Granted),
            StepGlyph::Check,
            "Allowed",
            false,
            None,
        ),
        (
            tap,
            Permission::Known(Access::Undetermined),
            StepGlyph::Pending,
            "Asked the first time you record",
            false,
            Some(Grant),
        ),
        (
            tap,
            Permission::Known(Access::Denied),
            StepGlyph::Warning,
            "Not allowed",
            true,
            Some(OpenSystemSettings),
        ),
        (
            tap,
            Permission::Known(Access::Unsupported),
            StepGlyph::Warning,
            "Not available on this Mac",
            true,
            None,
        ),
    ];
    for (source, permission, glyph, word, attention, button) in cases {
        let row = permission_row(source, permission);
        assert_eq!(
            (row.glyph, row.word, row.attention, row.button),
            (glyph, word, attention, button),
            "{source:?} {permission:?}"
        );
    }
    assert_eq!(Grant.label(), "Grant");
    assert_eq!(OpenSystemSettings.label(), "Open System Settings");
}

#[test]
fn grant_opens_system_settings_only_where_macos_cannot_prompt() {
    assert!(opens_settings_after_grant(
        Source::SystemAudio,
        Access::Undetermined
    ));
    assert!(!opens_settings_after_grant(
        Source::SystemAudio,
        Access::Granted
    ));
    assert!(!opens_settings_after_grant(
        Source::Microphone,
        Access::Undetermined
    ));
    assert!(!opens_settings_after_grant(
        Source::Microphone,
        Access::Denied
    ));
}

#[gpui_kit::test]
fn skip_on_the_models_step_finishes_and_focuses_start_recording(cx: &mut TestAppContext) {
    let store = Arc::new(MemStore::new());
    let (window, app) = open_fresh(cx, dory_on(store.clone()));
    click(window, "onboarding-continue", cx);
    click(window, "onboarding-continue", cx);
    assert_eq!(step(&app, cx), OnboardingStep::Models);
    assert!(label(window, "onboarding-queue-note", cx).is_some());
    if let Some(language) = system_language() {
        assert_eq!(
            store.settings().unwrap().meeting_languages,
            vec![language],
            "the Models step preselects the system language"
        );
    }

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let height = window.viewport_size().height;
        for id in ["onboarding-skip", "onboarding-continue"] {
            let bounds = window
                .try_find(gpui_kit::ElementId::Name(id.into()))
                .expect(id)
                .bounds();
            assert!(
                bounds.bottom() <= height,
                "{id} sits below the window: {bounds:?}, window height {height:?}"
            );
        }
    })
    .unwrap();

    click(window, "onboarding-skip", cx);
    assert_eq!(step(&app, cx), OnboardingStep::Done);
    assert!(label(window, "onboarding", cx).is_none());
    assert!(focused(window, "start-recording", cx));
}

#[gpui_kit::test]
fn the_models_step_keeps_meeting_languages_already_chosen(cx: &mut TestAppContext) {
    let store = Arc::new(MemStore::new());
    let mut settings = store.settings().unwrap();
    settings.meeting_languages = vec![LangTag::parse("tl").unwrap()];
    dory_core::ports::Store::apply(
        &*store,
        vec![dory_core::ports::Change::SettingsReplaced(settings)],
    )
    .unwrap();
    let (window, app) = open_fresh(cx, dory_on(store.clone()));
    click(window, "onboarding-continue", cx);
    click(window, "onboarding-continue", cx);
    assert_eq!(step(&app, cx), OnboardingStep::Models);
    assert_eq!(
        store.settings().unwrap().meeting_languages,
        vec![LangTag::parse("tl").unwrap()]
    );
}

#[gpui_kit::test]
fn retranscribe_in_pins_the_language_and_queues_the_whole_pipeline(cx: &mut TestAppContext) {
    let store = sample_store();
    let (window, app) = open_with(cx, dory_on(store.clone()));
    let id = id_of("Design review");
    assert!(store.jobs_for(&id).unwrap().is_empty());
    app.update(cx, |app, cx| app.select(Selection::One(id.clone()), cx));
    cx.run_until_parked();

    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(
            Box::new(RetranscribeIn {
                language: LanguagePref::Pinned(LangTag::parse("fr").unwrap()),
            }),
            cx,
        )
    })
    .unwrap();
    cx.run_until_parked();

    let recording = store.recording(&id).unwrap().unwrap();
    assert_eq!(
        recording.language,
        LanguagePref::Pinned(LangTag::parse("fr").unwrap())
    );
    let jobs = store.jobs_for(&id).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].goal, Goal::ALL);
    assert_eq!(
        app.read_with(cx, |app, _| app.notice().map(|n| n.text.to_string())),
        Some("Transcribing again in French.".into())
    );

    app.update(cx, |app, cx| {
        app.retranscribe(id.clone(), LanguagePref::Auto, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        store.recording(&id).unwrap().unwrap().language,
        LanguagePref::Auto
    );
    let jobs = store.jobs_for(&id).unwrap();
    assert_eq!(
        jobs.len(),
        2,
        "the first job was cancelled and a new one queued"
    );
    assert!(jobs[0].is_terminal());
    assert!(!jobs[1].is_terminal());
}

/// A library that skipped onboarding still gets a meeting language, and one already chosen is
/// kept.
#[test]
fn a_library_with_no_meeting_languages_gets_the_system_language() {
    let store = sample_store();
    let dory = dory_on(store.clone());
    assert!(store.settings().unwrap().meeting_languages.is_empty());
    let english = LangTag::parse("en").unwrap();
    dory_gpui::seed_meeting_languages(&dory, Some(english.clone())).unwrap();
    let seeded = store.settings().unwrap();
    assert_eq!(seeded.meeting_languages, vec![english]);
    assert!(
        seeded.meeting_languages_guessed,
        "nobody chose it, so summary models must not trust it"
    );
    assert!(seeded.chosen_meeting_languages().is_empty());
    dory_gpui::seed_meeting_languages(&dory, LangTag::parse("fr").ok()).unwrap();
    assert_eq!(
        store.settings().unwrap().meeting_languages,
        vec![LangTag::parse("en").unwrap()],
        "a chosen list is kept"
    );
}

#[gpui_kit::test]
fn the_models_step_turns_a_guessed_language_into_a_chosen_one(cx: &mut TestAppContext) {
    let store = Arc::new(MemStore::new());
    let dory = dory_on(store.clone());
    dory_gpui::seed_meeting_languages(&dory, LangTag::parse("en").ok()).unwrap();
    assert!(store.settings().unwrap().meeting_languages_guessed);
    let (window, app) = open_fresh(cx, dory);
    click(window, "onboarding-continue", cx);
    assert!(
        store.settings().unwrap().meeting_languages_guessed,
        "the Welcome step does not show the languages"
    );
    click(window, "onboarding-continue", cx);
    assert_eq!(step(&app, cx), OnboardingStep::Models);
    let settings = store.settings().unwrap();
    assert_eq!(
        settings.meeting_languages,
        vec![LangTag::parse("en").unwrap()]
    );
    assert!(
        !settings.meeting_languages_guessed,
        "the Models step asks for the languages, so the list it shows is chosen"
    );
}

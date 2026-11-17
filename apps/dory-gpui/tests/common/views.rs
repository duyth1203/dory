//! Every view of the app, driven into place one after another, for the checks that must hold on
//! all of them: the accessibility tree (`a11y_tree.rs`) and the hit targets (`targets.rs`). Each
//! view is taken in Modern, then again in Book, whose names end in ", Book".

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dory_core::app::{Dory, Options, Ports, RecorderStatus};
use dory_core::domain::Timestamp;
use dory_core::recorder::Limits;
use dory_core::testing::playback::FakePlayback;
use dory_core::testing::store::MemStore;
use dory_core::testing::{FakeCapture, FakeTrash, ManualClock, MemTakes, Step};
use dory_gpui::app::{AppState, DetailTab};
use dory_gpui::library::Selection;
use dory_gpui::prefs::Prefs;
use dory_gpui::{about, fixtures, processing_window, quit, set_up_summaries, settings_window};
use gpui_kit::base::test_support::ElementSnapshot;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, InputEvent as _, TestAppContext,
    test::TestWindowExt as _,
};

use dory_core::ports::capture::{Access, CaptureCaps};
use dory_gpui::prefs::MIN_WINDOW_SIZE;
use dory_gpui::theme::{Display, ThemeName};

use super::{
    dory_with, dory_with_capture, id_of, long_winded_model, open_with, open_with_prefs,
    past_first_run, sample_store,
};

/// What a check sees of one view: its name and every observed element of its last frame, each
/// read the way `find` reads it, through the native node a Kit component names as its owner.
pub struct View {
    pub name: String,
    pub elements: Vec<ElementSnapshot>,
    /// The window's content size, for checks that nothing spills past its edges.
    pub width: f32,
    pub height: f32,
}

thread_local! {
    /// The theme the views are being taken in.
    static THEME: std::cell::Cell<ThemeName> = const { std::cell::Cell::new(ThemeName::Modern) };
}

fn frame(name: impl Into<String>, window: AnyWindowHandle, cx: &mut TestAppContext) -> View {
    let theme = THEME.get();
    // Each window's app state applies its own saved theme when it opens; this puts the one under
    // test back before every frame, as choosing it in Settings would.
    if cx.update(|cx| cx.try_global::<Display>().map(|d| d.theme)) != Some(theme) {
        cx.update(|cx| dory_gpui::theme::set_display(|display| display.theme = theme, cx));
    }
    cx.run_until_parked();
    let (elements, width, height) = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let size = window.viewport_size();
            let (width, height) = (f32::from(size.width), f32::from(size.height));
            let elements = gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .map(|raw| {
                    let path = raw.path().to_vec();
                    let Some((id, scope)) = path.split_last() else {
                        return raw;
                    };
                    gpui_kit::base::test_support::find_all(window, scope, id)
                        .into_iter()
                        .find(|resolved| resolved.path() == path.as_slice())
                        .unwrap_or(raw)
                })
                .collect();
            (elements, width, height)
        })
        .unwrap();
    let name: String = name.into();
    View {
        name: match theme {
            ThemeName::Modern => name,
            ThemeName::Book => format!("{name}, Book"),
        },
        elements,
        width,
        height,
    }
}

fn until(cx: &mut TestAppContext, what: &str, mut ok: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if ok(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Scrolls the page holding `id` until it shows, as a user would, from the page's left margin so
/// a nested list never takes the scroll.
fn reveal(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    let target = gpui_kit::ElementId::Name(id.to_owned().into());
    for _ in 0..30 {
        let page = cx
            .update_window(window, |_, window, cx| {
                window.render_frame(cx);
                if window.find(target.clone()).visible() {
                    return None;
                }
                ["detail", "settings-panel"]
                    .into_iter()
                    .find_map(|page| window.try_find(page).map(|p| p.bounds()))
            })
            .unwrap();
        let Some(page) = page else { return };
        cx.update_window(window, |_, window, cx| {
            let position = gpui_kit::point(page.left() + gpui_kit::px(4.), page.center().y);
            window.dispatch_event(
                gpui_kit::MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: window.modifiers(),
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                gpui_kit::ScrollEvent {
                    position,
                    delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                        gpui_kit::px(0.),
                        gpui_kit::px(-120.),
                    )),
                    modifiers: window.modifiers(),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
    }
}

fn click(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) {
    reveal(window, id, cx);
    let id = gpui_kit::ElementId::Name(id.to_owned().into());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// What a Mac shows on a first launch: the microphone refused once, and System Audio Recording,
/// which macOS never reports, unknown. Both permission buttons show.
fn unasked() -> FakeCapture {
    FakeCapture::new().with_caps(CaptureCaps {
        microphone: Access::Denied,
        system_audio: Access::Undetermined,
        unsupported: None,
    })
}

/// First run on an empty library: Welcome, Permissions, Models.
fn first_run(views: &mut Vec<View>, cx: &mut TestAppContext) {
    first_run_at(views, Prefs::default().window_size, cx);
    first_run_at(views, MIN_WINDOW_SIZE, cx);
}

fn first_run_at(views: &mut Vec<View>, window_size: (f32, f32), cx: &mut TestAppContext) {
    let prefs = Prefs {
        window_size,
        ..Prefs::default()
    };
    let (window, _) = open_with_prefs(
        cx,
        dory_with(
            Arc::new(MemStore::new()),
            unasked(),
            vec![long_winded_model()],
        ),
        prefs,
    );
    let at = if window_size == MIN_WINDOW_SIZE {
        ", smallest window"
    } else {
        ""
    };
    let frame =
        |name: &str, window, cx: &mut TestAppContext| frame(format!("{name}{at}"), window, cx);
    views.push(frame("welcome", window, cx));
    click(window, "onboarding-continue", cx);
    views.push(frame("permissions", window, cx));
    click(window, "onboarding-continue", cx);
    views.push(frame("models step", window, cx));
    open_explainer(window, cx);
    views.push(frame("models step explainer", window, cx));
    open_explainer(window, cx);
    cx.update_window(window, |_, window, cx| {
        dory_gpui::set_up_summaries::sheet(window, cx)
    })
    .unwrap();
    views.push(frame("set up summaries", window, cx));
}

/// The library past first run: the start screen, then each kind of detail, then the queue.
fn library(views: &mut Vec<View>, scale: f32, window_size: (f32, f32), cx: &mut TestAppContext) {
    let prefs = Prefs {
        text_scale: scale,
        window_size,
        ..past_first_run()
    };
    let (window, app) = open_with_prefs(cx, dory_with_capture(sample_store(), unasked()), prefs);
    let at = if window_size == MIN_WINDOW_SIZE {
        ", smallest window"
    } else {
        ""
    };
    let frame =
        |name: &str, window, cx: &mut TestAppContext| frame(format!("{name}{at}"), window, cx);
    views.push(frame("start screen", window, cx));
    let select = |title: &str, app: &Entity<AppState>, cx: &mut TestAppContext| {
        app.update(cx, |app, cx| app.select(Selection::One(id_of(title)), cx));
    };
    select("Design review", &app, cx);
    views.push(frame("summary tab", window, cx));
    app.update(cx, |app, cx| app.set_tab(DetailTab::Transcript, cx));
    views.push(frame("transcript tab", window, cx));
    app.update(cx, |app, cx| app.set_tab(DetailTab::Summary, cx));
    select("Hiring debrief", &app, cx);
    views.push(frame("needs attention", window, cx));
    select("Weekly sync", &app, cx);
    views.push(frame("processing", window, cx));
    for step in ["U1", "U2", "U3"] {
        cx.update_window(window, |_, window, cx| {
            set_up_summaries::preview(fixtures::summaries_step(step).unwrap(), window, cx)
        })
        .unwrap();
        views.push(frame(&format!("set up summaries {step}"), window, cx));
        cx.update_window(window, |_, window, cx| {
            dory_gpui::surface::close_dialog(window, cx)
        })
        .unwrap();
    }
    cx.update_window(window, |_, window, cx| {
        let prompt = quit::prompt(&fixtures::busy()).unwrap();
        quit::ask(prompt, app.clone(), window, cx);
    })
    .unwrap();
    views.push(frame("quit dialog", window, cx));
    cx.update_window(window, |_, window, cx| {
        dory_gpui::surface::close_dialog(window, cx)
    })
    .unwrap();
    processing_queue(views, &app, at, cx);
}

/// The Processing window in each preview state, then the busiest one at its shortest.
fn processing_queue(
    views: &mut Vec<View>,
    app: &Entity<AppState>,
    at: &str,
    cx: &mut TestAppContext,
) {
    let before = cx.update(|cx| cx.windows());
    cx.update(|cx| processing_window::open(app, cx));
    let window = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(w))
        .expect("the Processing window opened");
    for state in fixtures::QUEUE_STATES {
        let queue = fixtures::queue_state(state);
        app.update(cx, |app, cx| app.preview_queue(queue, cx));
        views.push(frame(format!("processing window {state}{at}"), window, cx));
    }
    cx.simulate_window_resize(
        window,
        gpui_kit::size(
            gpui_kit::px(processing_window::WIDTH),
            gpui_kit::px(processing_window::MIN_HEIGHT),
        ),
    );
    views.push(frame(
        format!("processing window busy, shortest{at}"),
        window,
        cx,
    ));
    app.update(cx, |app, cx| app.preview_queue(None, cx));
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
}

fn settings(views: &mut Vec<View>, scale: f32, cx: &mut TestAppContext) {
    let prefs = Prefs {
        text_scale: scale,
        ..past_first_run()
    };
    let (_, app) = open_with_prefs(
        cx,
        dory_with(
            sample_store(),
            FakeCapture::new(),
            vec![long_winded_model()],
        ),
        prefs,
    );
    let before = cx.update(|cx| cx.windows());
    cx.update(|cx| settings_window::open(&app, cx));
    let window = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(w))
        .expect("the Settings window opened");
    settings_panes(views, window, "", cx);
    languages_open(views, window, "", cx);
    // The shortest window Settings allows. `Window::resize` only moves the test window's bounds;
    // the simulated resize also lays the view out at the new size.
    cx.simulate_window_resize(
        window,
        gpui_kit::size(
            gpui_kit::px(settings_window::WIDTH),
            gpui_kit::px(settings_window::MIN_HEIGHT),
        ),
    );
    settings_panes(views, window, ", 560 tall", cx);
    languages_open(views, window, ", 560 tall", cx);
}

/// The meeting-languages popover, open over Transcription, then closed with Escape.
fn languages_open(
    views: &mut Vec<View>,
    window: AnyWindowHandle,
    at: &str,
    cx: &mut TestAppContext,
) {
    click(window, &settings_window::Pane::Transcription.id(), cx);
    click(window, "add-language", cx);
    let popover = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("meeting-languages-popover").is_some()
        })
        .unwrap()
    };
    until(cx, "the languages popover opens", popover);
    views.push(frame(
        format!("settings Transcription, languages open{at}"),
        window,
        cx,
    ));
    cx.update_window(window, |_, window, cx| window.press("escape", cx))
        .unwrap();
    until(cx, "the languages popover closes", |cx| !popover(cx));
}

/// About Dory with its Acknowledgements, over the theme the last window set up.
fn about(views: &mut Vec<View>, cx: &mut TestAppContext) {
    let before = cx.update(|cx| cx.windows());
    cx.update(about::open);
    let window = cx
        .update(|cx| cx.windows())
        .into_iter()
        .find(|w| !before.contains(w))
        .expect("the About window opened");
    views.push(frame("about", window, cx));
}

/// Each Settings pane, and its first model explainer where it has one.
fn settings_panes(
    views: &mut Vec<View>,
    window: AnyWindowHandle,
    at: &str,
    cx: &mut TestAppContext,
) {
    for tab in settings_window::Pane::ALL {
        click(window, &tab.id(), cx);
        views.push(frame(format!("settings {}{at}", tab.title()), window, cx));
        if open_explainer(window, cx) {
            views.push(frame(
                format!("settings {} explainer{at}", tab.title()),
                window,
                cx,
            ));
            open_explainer(window, cx);
        }
    }
}

/// Clicks the first model's info button, which opens its explainer or closes an open one.
/// False when the view has no model rows.
fn open_explainer(window: AnyWindowHandle, cx: &mut TestAppContext) -> bool {
    cx.run_until_parked();
    let id = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            gpui_kit::base::test_support::snapshots(window)
                .into_iter()
                .find_map(|e| match e.path().last() {
                    Some(gpui_kit::ElementId::Name(n))
                        if n.starts_with("about-") && e.role() == Some(gpui_kit::Role::Button) =>
                    {
                        Some(n.to_string())
                    }
                    _ => None,
                })
        })
        .unwrap();
    match id {
        Some(id) => {
            click(window, &id, cx);
            true
        }
        None => false,
    }
}

/// A live take, then the Undo toast after Stop. The fake device sends two seconds of silence, and
/// the manual clock holds the grace open.
fn recording(views: &mut Vec<View>, cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!(
        "dory-gpui-views-{}-{}",
        std::process::id(),
        views.len()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let capture = Arc::new(FakeCapture::new());
    capture.push_script((0..20).map(|_| Step::Silence { frames: 800 }).collect());
    let dory = Dory::open(
        Ports {
            store: sample_store(),
            takes: Arc::new(MemTakes::in_dir(&dir)),
            trash: Arc::new(FakeTrash::new(&dir)),
            capture: capture.clone(),
            playback: Arc::new(FakePlayback::new()),
            clock: Arc::new(ManualClock::new(Timestamp(
                fixtures::now().timestamp_millis(),
            ))),
            stt: Vec::new(),
            llm: Vec::new(),
            diar: Vec::new(),
            machine: Arc::new(dory_core::testing::FakeMachine::default()),
            guard: None,
        },
        Options {
            limits: Limits {
                rate_hz: NonZeroU32::new(8_000).unwrap(),
                ..Limits::default()
            },
            clock_poll: Duration::from_millis(2),
            level_every: Duration::from_millis(10),
            ..Options::default()
        },
    )
    .unwrap();
    cx.executor().allow_parking();
    let (window, app) = open_with(cx, dory);
    app.update(cx, |app, cx| app.start_recording(cx));
    until(cx, "two seconds recorded", |cx| {
        app.read_with(cx, |app, cx| {
            matches!(app.recorder(), RecorderStatus::Recording { .. })
                && app.live().read(cx).recorded.0 >= 2_000
        })
    });
    views.push(frame("recording", window, cx));
    cx.update_window(window, |_, window, cx| {
        let prompt = quit::prompt(&app.read(cx).in_progress(cx)).expect("a take is live");
        quit::ask(prompt, app.clone(), window, cx);
    })
    .unwrap();
    views.push(frame("quit while recording", window, cx));
    cx.update_window(window, |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.run_until_parked();
    app.update(cx, |app, cx| app.stop_recording(cx));
    until(cx, "grace", |cx| {
        app.read_with(cx, |app, _| {
            matches!(app.recorder(), RecorderStatus::Grace { .. })
        })
    });
    views.push(frame("undo toast", window, cx));
}

fn upgrade(views: &mut Vec<View>, cx: &mut TestAppContext) {
    for (name, window_size) in [
        ("upgrade permissions", Prefs::default().window_size),
        ("upgrade permissions, smallest window", MIN_WINDOW_SIZE),
    ] {
        let prefs = Prefs {
            window_size,
            ..Prefs::default()
        };
        let (window, _) = open_with_prefs(cx, dory_with_capture(sample_store(), unasked()), prefs);
        views.push(frame(name, window, cx));
    }
}

/// The processing card in each of its states, on Weekly sync.
fn processing(views: &mut Vec<View>, scale: f32, window_size: (f32, f32), cx: &mut TestAppContext) {
    let prefs = Prefs {
        text_scale: scale,
        window_size,
        ..past_first_run()
    };
    let (window, app) = open_with_prefs(cx, dory_with_capture(sample_store(), unasked()), prefs);
    let id = id_of("Weekly sync");
    app.update(cx, |app, cx| app.select(Selection::One(id.clone()), cx));
    let at = format!(", {:.0}x{:.0}", window_size.0, window_size.1);
    for (name, phase, lines) in fixtures::card_phases() {
        app.update(cx, |app, cx| {
            app.preview_phase(id.clone(), phase, lines, cx)
        });
        views.push(frame(format!("{name}{at}"), window, cx));
    }
}

/// Every view at the given text scale, in Modern and then in Book.
pub fn every_view(scale: f32, cx: &mut TestAppContext) -> Vec<View> {
    let mut views = Vec::new();
    for theme in ThemeName::ALL {
        THEME.set(theme);
        views.extend(views_in_theme(scale, cx));
        // The next theme's pass opens Settings, About, and the Processing window anew.
        for window in cx.update(|cx| cx.windows()) {
            let _ = cx.update_window(window, |_, window, _| window.remove_window());
        }
        cx.run_until_parked();
    }
    THEME.set(ThemeName::Modern);
    views
}

fn views_in_theme(scale: f32, cx: &mut TestAppContext) -> Vec<View> {
    // Settings probes models and the recorder captures on their own threads.
    cx.executor().allow_parking();
    let mut views = Vec::new();
    if scale == 1.0 {
        first_run(&mut views, cx);
        upgrade(&mut views, cx);
        recording(&mut views, cx);
    }
    library(&mut views, scale, Prefs::default().window_size, cx);
    if scale == 1.0 {
        library(&mut views, scale, MIN_WINDOW_SIZE, cx);
    }
    processing(&mut views, scale, MIN_WINDOW_SIZE, cx);
    if scale == 2.0 {
        processing(&mut views, scale, Prefs::default().window_size, cx);
    }
    settings(&mut views, scale, cx);
    about(&mut views, cx);
    views
}

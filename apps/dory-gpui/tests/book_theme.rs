//! Book's presentation (`docs/design/book-theme.md` sections 4 and 5): the running head shows from
//! 900 pt, the play-script transcript reads to VoiceOver the same at every width, and Reduce
//! Transparency turns the dialog scrim opaque.

mod common;

use common::{id_of, open_with_prefs, past_first_run, sample_store};
use dory_gpui::app::{AppState, DetailTab};
use dory_gpui::library::Selection;
use dory_gpui::prefs::Prefs;
use dory_gpui::system_prefs::{self, SystemPrefs};
use dory_gpui::theme::Theme;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, Role, TestAppContext, test::TestWindowExt as _,
};

fn sized(size: (f32, f32)) -> Prefs {
    Prefs {
        window_size: size,
        ..past_first_run()
    }
}

fn select(app: &Entity<AppState>, title: &str, cx: &mut TestAppContext) {
    app.update(cx, |app, cx| app.select(Selection::One(id_of(title)), cx));
    cx.run_until_parked();
}

fn has(window: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) -> bool {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some_and(|el| el.visible())
    })
    .unwrap()
}

/// The labels VoiceOver reads for every list item and speaker button, sorted: the snapshot
/// lists elements in no fixed order.
fn spoken_transcript(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<String> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|el| matches!(el.role(), Some(Role::ListItem) | Some(Role::Button)))
            .filter_map(|el| {
                let label = el.label()?.to_owned();
                (label.starts_with("Rename ") || el.role() == Some(Role::ListItem)).then_some(label)
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_running_head_shows_from_900_points(cx: &mut TestAppContext) {
    let (window, app) = open_with_prefs(cx, common::dory_on(sample_store()), sized((1100., 760.)));
    select(&app, "Design review", cx);
    assert!(has(window, "running-head", cx));

    let (narrow, app) = open_with_prefs(cx, common::dory_on(sample_store()), sized((860., 600.)));
    select(&app, "Design review", cx);
    assert!(!has(narrow, "running-head", cx), "hidden below 900 pt");
}

#[gpui_kit::test]
fn the_script_transcript_reads_the_same_wide_and_narrow(cx: &mut TestAppContext) {
    let read = |size: (f32, f32), cx: &mut TestAppContext| {
        let (window, app) = open_with_prefs(cx, common::dory_on(sample_store()), sized(size));
        select(&app, "Design review", cx);
        app.update(cx, |app, cx| app.set_tab(DetailTab::Transcript, cx));
        cx.run_until_parked();
        spoken_transcript(window, cx)
    };
    let wide = read((1100., 760.), cx);
    assert!(
        wide.iter().any(|l| l.starts_with("Rename ")),
        "the speaker buttons are read: {wide:?}"
    );
    assert_eq!(read((860., 600.), cx), wide);
}

#[gpui_kit::test]
fn reduce_transparency_turns_the_dialog_scrim_opaque(cx: &mut TestAppContext) {
    let (_, _) = open_with_prefs(cx, common::dory_on(sample_store()), past_first_run());
    let scrim = |cx: &mut TestAppContext| cx.update(|cx| cx.theme().overlay);
    let theme = cx.update(|cx| Theme::get(cx));
    assert_eq!(scrim(cx), theme.scrim);

    cx.update(|cx| {
        system_prefs::apply(
            SystemPrefs {
                reduce_transparency: true,
                ..SystemPrefs::default()
            },
            cx,
        )
    });
    assert_eq!(scrim(cx), theme.scrim_opaque);

    cx.update(|cx| system_prefs::apply(SystemPrefs::default(), cx));
    assert_eq!(
        scrim(cx),
        theme.scrim,
        "turning it off brings the scrim back"
    );
}

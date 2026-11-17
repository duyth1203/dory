//! Choosing the Book theme (`docs/design/book-theme.md` sections 4 and 5): it is offered only
//! once Settings turns it on, View › Theme switches to it and back, the choice is remembered, the
//! window has no glass, and its flags draw without changing what VoiceOver reads.

mod common;

use common::{id_of, open_with_prefs, past_first_run, sample_store};
use dory_gpui::app::{AppState, DetailTab};
use dory_gpui::library::Selection;
use dory_gpui::prefs::Prefs;
use dory_gpui::theme::{Theme, ThemeName};
use dory_gpui::{glass, menus};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, Role, TestAppContext, test::TestWindowExt as _,
};

fn book_prefs(size: (f32, f32)) -> Prefs {
    Prefs {
        window_size: size,
        book_offered: true,
        theme: ThemeName::Book,
        ..past_first_run()
    }
}

fn shown(cx: &mut TestAppContext) -> ThemeName {
    cx.update(|cx| Theme::get(cx).name)
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
fn book_is_offered_only_once_settings_turns_it_on(cx: &mut TestAppContext) {
    let (_, app) = open_with_prefs(cx, common::dory_on(sample_store()), past_first_run());
    assert_eq!(shown(cx), ThemeName::Modern);
    app.update(cx, |app, cx| app.set_theme(ThemeName::Book, cx));
    cx.run_until_parked();
    assert_eq!(shown(cx), ThemeName::Modern, "Book is not offered yet");
    let menu = cx.update(|cx| menus::menu_state(app.read(cx), cx));
    assert!(!menu.book_offered);

    app.update(cx, |app, cx| app.set_book_offered(true, cx));
    app.update(cx, |app, cx| app.set_theme(ThemeName::Book, cx));
    cx.run_until_parked();
    assert_eq!(shown(cx), ThemeName::Book);
    let menu = cx.update(|cx| menus::menu_state(app.read(cx), cx));
    assert_eq!((menu.theme, menu.book_offered), (ThemeName::Book, true));

    app.update(cx, |app, cx| app.set_theme(ThemeName::Modern, cx));
    cx.run_until_parked();
    assert_eq!(
        shown(cx),
        ThemeName::Modern,
        "View › Theme › Modern switches back"
    );

    app.update(cx, |app, cx| app.set_theme(ThemeName::Book, cx));
    app.update(cx, |app, cx| app.set_book_offered(false, cx));
    cx.run_until_parked();
    assert_eq!(
        shown(cx),
        ThemeName::Modern,
        "withdrawing Book returns to Modern"
    );
    assert_eq!(
        app.read_with(cx, |app, _| app.prefs.theme),
        ThemeName::Modern
    );
}

#[gpui_kit::test]
fn the_chosen_theme_is_remembered_across_launches(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("dory-book-theme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (_, app) = open_with_prefs(cx, common::dory_on(sample_store()), past_first_run());
    app.update(cx, |app, cx| {
        app.set_book_offered(true, cx);
        app.set_theme(ThemeName::Book, cx);
    });
    cx.run_until_parked();
    app.read_with(cx, |app, _| app.prefs.write(&dir)).unwrap();

    let saved = Prefs::read(&dir);
    assert_eq!((saved.book_offered, saved.theme), (true, ThemeName::Book));
    let _ = std::fs::remove_dir_all(&dir);
    let (_, _) = open_with_prefs(cx, common::dory_on(sample_store()), saved);
    assert_eq!(shown(cx), ThemeName::Book, "a relaunch opens in Book");
}

#[gpui_kit::test]
fn book_has_no_glass_even_without_reduce_transparency(cx: &mut TestAppContext) {
    let (_, _) = open_with_prefs(
        cx,
        common::dory_on(sample_store()),
        book_prefs((1100., 760.)),
    );
    let (background, column, panel, theme) = cx.update(|cx| {
        let theme = Theme::get(cx);
        (
            glass::window_background(cx),
            glass::sidebar_column(&theme, cx),
            glass::sidebar_panel(&theme, cx),
            theme,
        )
    });
    assert_eq!(background, gpui_kit::WindowBackgroundAppearance::Opaque);
    assert_eq!(column, theme.window_bg);
    assert_eq!(panel, theme.sidebar_bg);
}

#[gpui_kit::test]
fn the_running_head_shows_in_book_from_900_points(cx: &mut TestAppContext) {
    let (window, app) = open_with_prefs(
        cx,
        common::dory_on(sample_store()),
        book_prefs((1100., 760.)),
    );
    select(&app, "Design review", cx);
    assert!(has(window, "running-head", cx));

    let (narrow, app) = open_with_prefs(
        cx,
        common::dory_on(sample_store()),
        book_prefs((860., 600.)),
    );
    select(&app, "Design review", cx);
    assert!(!has(narrow, "running-head", cx), "hidden below 900 pt");

    let (modern, app) = open_with_prefs(cx, common::dory_on(sample_store()), past_first_run());
    select(&app, "Design review", cx);
    assert!(!has(modern, "running-head", cx), "Modern has none");
}

#[gpui_kit::test]
fn the_script_transcript_reads_to_voiceover_as_modern_does(cx: &mut TestAppContext) {
    let read = |prefs: Prefs, cx: &mut TestAppContext| {
        let (window, app) = open_with_prefs(cx, common::dory_on(sample_store()), prefs);
        select(&app, "Design review", cx);
        app.update(cx, |app, cx| app.set_tab(DetailTab::Transcript, cx));
        cx.run_until_parked();
        spoken_transcript(window, cx)
    };
    let modern = read(past_first_run(), cx);
    assert!(
        modern.iter().any(|l| l.starts_with("Rename ")),
        "the speaker buttons are read: {modern:?}"
    );
    assert_eq!(read(book_prefs((1100., 760.)), cx), modern);
    assert_eq!(read(book_prefs((860., 600.)), cx), modern, "narrow too");
}

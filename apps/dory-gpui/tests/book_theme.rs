//! Choosing the Book theme (`docs/design/book-theme.md` sections 4 and 5): Settings shows Modern
//! and Book as choices from the start, a card or View › Theme switches at once, the choice is
//! remembered, the window has no glass, and its flags draw without changing what VoiceOver reads.

mod common;

use common::{id_of, open_with_prefs, past_first_run, sample_store};
use dory_gpui::app::{AppState, DetailTab};
use dory_gpui::library::Selection;
use dory_gpui::prefs::Prefs;
use dory_gpui::theme::{Theme, ThemeName};
use dory_gpui::{glass, menus, settings_window};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, MenuItem, Role, TestAppContext,
    test::TestWindowExt as _,
};

fn book_prefs(size: (f32, f32)) -> Prefs {
    Prefs {
        window_size: size,
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

fn open_settings(
    app: &Entity<AppState>,
    main: AnyWindowHandle,
    cx: &mut TestAppContext,
) -> AnyWindowHandle {
    cx.update(|cx| settings_window::open_at(app, settings_window::Pane::General, cx));
    cx.run_until_parked();
    cx.update(|cx| cx.windows())
        .into_iter()
        .find(|w| *w != main)
        .expect("the Settings window opened")
}

/// What VoiceOver hears for a theme card: its role, name, and whether it is the chosen one.
fn card(
    window: AnyWindowHandle,
    id: &'static str,
    cx: &mut TestAppContext,
) -> (Option<Role>, Option<String>, Option<bool>) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let el = window.find(id);
        (el.role(), el.label().map(str::to_owned), el.checked())
    })
    .unwrap()
}

fn theme_menu(app: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<(String, bool)> {
    let built = cx.update(|cx| menus::build(menus::menu_state(app.read(cx), cx)));
    let view = built
        .iter()
        .find(|m| m.name == "View")
        .expect("a View menu");
    let items = view
        .items
        .iter()
        .find_map(|item| match item {
            MenuItem::Submenu(sub) if sub.name == "Theme" => Some(&sub.items),
            _ => None,
        })
        .expect("View › Theme");
    items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Action { name, checked, .. } => Some((name.to_string(), *checked)),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn settings_shows_modern_and_book_from_the_start(cx: &mut TestAppContext) {
    let (main, app) = open_with_prefs(cx, common::dory_on(sample_store()), past_first_run());
    let window = open_settings(&app, main, cx);
    assert!(
        has(window, "setting-theme", cx),
        "the Theme choice shows at once"
    );
    assert_eq!(
        cx.update_window(window, |_, window, _| window.find("setting-theme").role())
            .unwrap(),
        Some(Role::RadioGroup)
    );
    assert_eq!(
        card(window, "theme-modern", cx),
        (Some(Role::RadioButton), Some("Modern".into()), Some(true)),
        "a new user gets Modern"
    );
    assert_eq!(
        card(window, "theme-book", cx),
        (Some(Role::RadioButton), Some("Book".into()), Some(false))
    );
    assert!(!has(window, "setting-book-offered", cx), "no Try step");
}

#[gpui_kit::test]
fn picking_a_theme_card_switches_at_once_by_click_and_keyboard(cx: &mut TestAppContext) {
    let (main, app) = open_with_prefs(cx, common::dory_on(sample_store()), past_first_run());
    let window = open_settings(&app, main, cx);
    cx.update_window(window, |_, window, cx| {
        window.click("settings-search", cx);
        window.input("book", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-hit-book-theme", cx);
    })
    .unwrap();
    for _ in 0..20 {
        cx.run_until_parked();
        let on_screen = cx
            .update_window(window, |_, window, cx| {
                window.render_frame(cx);
                let card = window.find("theme-book").bounds().center();
                window.find("settings-panel").bounds().contains(&card)
            })
            .unwrap();
        if on_screen {
            break;
        }
    }

    cx.update_window(window, |_, window, cx| window.click("theme-book", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(shown(cx), ThemeName::Book, "one click on Book switches");
    assert_eq!(card(window, "theme-book", cx).2, Some(true));
    assert_eq!(card(window, "theme-modern", cx).2, Some(false));
    assert_eq!(
        theme_menu(&app, cx),
        [("Modern".into(), false), ("Book".into(), true)]
    );

    cx.update_window(window, |_, window, cx| window.click("theme-modern", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(shown(cx), ThemeName::Modern, "and back to Modern");

    cx.update_window(window, |_, window, cx| {
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("theme-book").focused(),
            Some(true),
            "Tab reaches Book"
        );
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(shown(cx), ThemeName::Book, "Space on Book switches");
    assert_eq!(card(window, "theme-book", cx).2, Some(true));
}

#[gpui_kit::test]
fn view_theme_lists_both_themes_and_switches_between_them(cx: &mut TestAppContext) {
    let (_, app) = open_with_prefs(cx, common::dory_on(sample_store()), past_first_run());
    assert_eq!(shown(cx), ThemeName::Modern);
    assert_eq!(
        theme_menu(&app, cx),
        [("Modern".into(), true), ("Book".into(), false)]
    );

    app.update(cx, |app, cx| app.set_theme(ThemeName::Book, cx));
    cx.run_until_parked();
    assert_eq!(shown(cx), ThemeName::Book);
    assert_eq!(
        theme_menu(&app, cx),
        [("Modern".into(), false), ("Book".into(), true)]
    );

    app.update(cx, |app, cx| app.set_theme(ThemeName::Modern, cx));
    cx.run_until_parked();
    assert_eq!(
        shown(cx),
        ThemeName::Modern,
        "View › Theme › Modern switches back"
    );
}

#[gpui_kit::test]
fn the_chosen_theme_is_remembered_across_launches(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("dory-book-theme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (_, app) = open_with_prefs(cx, common::dory_on(sample_store()), past_first_run());
    app.update(cx, |app, cx| app.set_theme(ThemeName::Book, cx));
    cx.run_until_parked();
    app.read_with(cx, |app, _| app.prefs.write(&dir)).unwrap();

    let saved = Prefs::read(&dir);
    assert_eq!(saved.theme, ThemeName::Book);
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

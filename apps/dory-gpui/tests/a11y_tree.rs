//! A11Y-1, A11Y-2, A11Y-4, and the keyboard table: the accessibility tree of every view, F6
//! landmark navigation, text scaling, and the words VoiceOver hears.
//!
//! `TestAppContext` never starts AccessKit, so the tree is read from the elements each frame
//! registers with Kit's test support: the same role, label, and focus binding the AccessKit node
//! is built from.

mod common;

use dory_gpui::app::DetailTab;
use dory_gpui::library::Selection;
use dory_gpui::menus;
use dory_gpui::theme::Theme;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ElementId, Role, TestAppContext, test::TestWindowExt as _,
};

use common::views::every_view;
use common::{id_of, open};

/// Roles a user acts on, which VoiceOver must be able to name.
const ACTIONABLE: [Role; 12] = [
    Role::Button,
    Role::CheckBox,
    Role::RadioButton,
    Role::Switch,
    Role::Tab,
    Role::Slider,
    Role::Splitter,
    Role::TextInput,
    Role::ComboBox,
    Role::MenuItem,
    Role::Link,
    Role::ListItem,
];

#[gpui_kit::test]
fn every_focusable_or_actionable_element_has_a_role_and_a_label(cx: &mut TestAppContext) {
    let mut problems = Vec::new();
    let mut checked = 0;
    let mut dialogs = Vec::new();
    for view in every_view(1.0, cx) {
        for element in &view.elements {
            if element.role() == Some(Role::Dialog) {
                dialogs.push((view.name.clone(), element.label().map(str::to_owned)));
            }
            let focusable = element.focused().is_some();
            let actionable = element.role().is_some_and(|r| ACTIONABLE.contains(&r));
            if !focusable && !actionable {
                continue;
            }
            checked += 1;
            let labelled = element.label().is_some_and(|l| !l.trim().is_empty());
            if element.role().is_none() || !labelled {
                problems.push(format!(
                    "{}: {:?} role {:?} label {:?}",
                    view.name,
                    element.path().last(),
                    element.role(),
                    element.label()
                ));
            }
        }
    }
    assert!(checked > 100, "only {checked} elements were checked");
    // Set Up Summaries opens on Dory's own dialog host, which names the dialog by its title
    // where Kit 0.9.1 could not (`docs/plans/upstream/kit-dialog-has-no-accessible-name.md`).
    let summaries = dialogs
        .iter()
        .filter(|(_, label)| label.as_deref() == Some("Set Up Summaries"))
        .count();
    assert!(summaries >= 4, "{dialogs:?}");
    let unnamed: Vec<_> = dialogs
        .iter()
        .filter(|(_, label)| label.is_none())
        .collect();
    assert!(unnamed.is_empty(), "unnamed dialogs: {unnamed:?}");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[gpui_kit::test]
fn every_view_is_covered(cx: &mut TestAppContext) {
    let names: Vec<String> = every_view(1.0, cx).into_iter().map(|v| v.name).collect();
    for want in [
        "welcome",
        "permissions",
        "models step",
        "set up summaries",
        "upgrade permissions",
        "recording",
        "undo toast",
        "start screen",
        "summary tab",
        "transcript tab",
        "needs attention",
        "processing",
        "set up summaries U1",
        "set up summaries U2",
        "set up summaries U3",
        "processing window running",
        "processing window light",
        "processing window blocked",
        "processing window finished",
        "processing window empty",
        "processing window busy",
        "processing window busy, shortest",
        "settings General",
        "settings Recording",
        "settings Transcription",
        "settings Summaries",
        "settings Storage",
        "settings Transcription, languages open",
        "settings General, 560 tall",
        "settings Storage, 560 tall",
        "about",
    ] {
        assert!(names.iter().any(|n| n == want), "{want} missing: {names:?}");
    }
    for modern in names.iter().filter(|n| !n.ends_with(", Book")) {
        let book = format!("{modern}, Book");
        assert!(
            names.contains(&book),
            "{book} missing: every view runs in both themes"
        );
    }
}

/// A11Y-1: a button that shows only an icon still says what it does. The icon itself has no node,
/// so VoiceOver never reads it beside its word.
#[gpui_kit::test]
fn icon_only_buttons_keep_their_labels(cx: &mut TestAppContext) {
    let views = every_view(1.0, cx);
    let label = |id: &str| {
        views
            .iter()
            .flat_map(|v| &v.elements)
            .filter(|e| e.role() == Some(Role::Button))
            .find_map(|e| match e.path().last() {
                Some(ElementId::Name(n))
                    if n.as_ref() == id || (id.ends_with('-') && n.starts_with(id)) =>
                {
                    e.label().map(|l| l.to_string())
                }
                _ => None,
            })
    };
    for (id, want) in [
        ("player-back", "Back 5 seconds"),
        ("player-play", "Play"),
        ("player-forward", "Forward 5 seconds"),
        ("more", "More actions"),
        ("record-pause", "Pause recording"),
        ("record-stop", "Stop recording"),
        ("remove-language-", "Remove "),
        ("about-", "About "),
    ] {
        let got = label(id).unwrap_or_else(|| panic!("no {id} button in any view"));
        assert!(got.starts_with(want), "{id} reads {got:?}, want {want:?}");
    }
}

fn focused(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> bool {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(ElementId::Name(id.to_owned().into()))
            .and_then(|e| e.focused())
            == Some(true)
    })
    .unwrap()
}

fn press(window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn f6_and_shift_f6_cycle_sidebar_toolbar_and_detail(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    cx.update(menus::install);
    app.update(cx, |app, cx| {
        app.select(Selection::One(id_of("Design review")), cx)
    });
    cx.run_until_parked();
    assert!(
        focused(window, "recordings", cx),
        "selecting moves focus to the list"
    );
    let forward = ["record", "title", "recordings"];
    for id in forward {
        press(window, "f6", cx);
        assert!(focused(window, id, cx), "F6 should reach {id}");
    }
    let back = ["title", "record", "recordings"];
    for id in back {
        press(window, "shift-f6", cx);
        assert!(focused(window, id, cx), "Shift-F6 should reach {id}");
    }
}

#[gpui_kit::test]
fn f6_reaches_the_start_button_when_nothing_is_selected(cx: &mut TestAppContext) {
    let (window, _) = open(cx);
    cx.update(menus::install);
    press(window, "f6", cx);
    assert!(focused(window, "recordings", cx));
    press(window, "f6", cx);
    assert!(focused(window, "record", cx));
    press(window, "f6", cx);
    assert!(focused(window, "start-recording", cx));
}

#[gpui_kit::test]
fn return_on_a_selected_row_renames_it(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    cx.update(menus::install);
    press(window, "f6", cx);
    assert!(focused(window, "recordings", cx));
    press(window, "down", cx);
    press(window, "enter", cx);
    let editing = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("title-editor").is_some()
        })
        .unwrap();
    assert!(editing, "Return opens the title field");
    assert_eq!(app.read_with(cx, |app, _| app.tab), DetailTab::Summary);
}

#[gpui_kit::test]
fn command_plus_and_minus_scale_text_within_85_and_200_percent(cx: &mut TestAppContext) {
    let (window, app) = open(cx);
    cx.update(menus::install);
    cx.update(|cx| menus::watch(&app, cx));
    let scale = |cx: &mut TestAppContext| cx.update(|cx| Theme::get(cx).text_scale);
    assert_eq!(scale(cx), 1.0);
    for _ in 0..10 {
        press(window, "cmd-=", cx);
    }
    assert_eq!(scale(cx), 2.0, "Bigger stops at 200%");
    assert_eq!(
        app.read_with(cx, |app, _| app.prefs.text_scale),
        2.0,
        "saved"
    );
    for _ in 0..10 {
        press(window, "cmd--", cx);
    }
    assert_eq!(scale(cx), 0.85, "Smaller stops at 85%");
    press(window, "cmd-0", cx);
    assert_eq!(scale(cx), 1.0);
    let title = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.find("start-recording").bounds().size.height
        })
        .unwrap();
    for _ in 0..10 {
        press(window, "cmd-=", cx);
    }
    let grown = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.find("start-recording").bounds().size.height
        })
        .unwrap();
    assert!(
        grown > title,
        "the start button grows with its text: {title:?} to {grown:?}"
    );
}

/// The pattern every hand-built button uses: a click handler alone. GPUI turns Return and Space on
/// a focused element into one click, so a button that also handled the keys acted twice (a
/// checkbox toggled back, Play paused again). This holds GPUI to the one click.
struct Pressable {
    focus: gpui_kit::FocusHandle,
    count: std::rc::Rc<std::cell::Cell<u32>>,
}

impl gpui_kit::Render for Pressable {
    fn render(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{StatefulInteractiveElement as _, Styled as _};
        window.focus(&self.focus, cx);
        let theme = Theme::get(cx);
        let on_click = self.count.clone();
        dory_gpui::a11y::focusable(
            dory_gpui::a11y::node("pressable", Role::Button, "Press"),
            &self.focus,
            &theme,
            false,
        )
        .size(gpui_kit::px(40.))
        .on_click(move |_, _, _| on_click.set(on_click.get() + 1))
    }
}

#[gpui_kit::test]
fn return_or_space_on_a_hand_built_button_acts_once(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(dory_gpui::theme::sync);
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let window = cx
        .update(|cx| {
            let count = count.clone();
            gpui_kit::open_window(Default::default(), cx, move |_, cx| {
                cx.new(|cx| Pressable {
                    focus: cx.focus_handle(),
                    count,
                })
            })
        })
        .unwrap()
        .0;
    cx.run_until_parked();
    press(window, "space", cx);
    assert_eq!(count.get(), 1, "Space acts once");
    press(window, "enter", cx);
    assert_eq!(count.get(), 2, "Return acts once");
}

#[test]
fn the_recorder_and_the_pipeline_speak_the_words_a11y_4_names() {
    use dory_core::app::RecorderStatus;
    use dory_core::domain::{Ms, RecordingId};
    use dory_gpui::a11y::Announcement;
    let id = RecordingId::parse("rec-1").unwrap();
    let recording = RecorderStatus::Recording {
        id: id.clone(),
        recorded: Ms(0),
    };
    let grace = RecorderStatus::Grace {
        id,
        recorded: Ms(0),
    };
    let said = |a: Option<Announcement>| a.map(|a| a.text());
    assert_eq!(
        said(Announcement::of_recorder(&RecorderStatus::Idle, &recording)).as_deref(),
        Some("Recording started")
    );
    assert_eq!(
        said(Announcement::of_recorder(&recording, &grace)).as_deref(),
        Some("Recording saved")
    );
    assert_eq!(
        Announcement::Finished {
            title: "Design review".into()
        }
        .text(),
        "Finished processing Design review"
    );
    assert_eq!(
        Announcement::Failed {
            title: "Hiring debrief".into()
        }
        .text(),
        "Hiring debrief needs attention"
    );
}

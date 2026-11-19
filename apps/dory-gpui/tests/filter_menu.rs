//! The sidebar's status filter: an icon on the button and on each option, and a check on the
//! chosen one, with the option names still what VoiceOver reads.

mod common;

use std::time::Duration;

use dory_gpui::icons::Icon;
use dory_gpui::library::StatusFilter;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ElementId, Role, TestAppContext, test::TestWindowExt as _,
};

struct Item {
    label: String,
    id: ElementId,
    checked: Option<bool>,
}

fn menu_items(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<Item> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let mut els: Vec<_> = gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .filter(|el| el.role() == Some(Role::MenuItem))
            .collect();
        els.sort_by(|a, b| {
            f32::from(a.bounds().origin.y).total_cmp(&f32::from(b.bounds().origin.y))
        });
        els.into_iter()
            .filter_map(|el| {
                Some(Item {
                    label: el.label()?.to_owned(),
                    id: el.path().last()?.clone(),
                    checked: el.checked(),
                })
            })
            .collect()
    })
    .unwrap()
}

/// Kit's menus animate on the wall clock, so draw frames until the menu is open or closed.
fn menu_reaches(window: AnyWindowHandle, open: bool, cx: &mut TestAppContext) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        cx.run_until_parked();
        if menu_items(window, cx).is_empty() != open {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn filter_label(window: AnyWindowHandle, cx: &mut TestAppContext) -> String {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.find("filter").label().map(str::to_owned)
    })
    .unwrap()
    .expect("the filter button has a label")
}

fn open_menu(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<Item> {
    common::views::click(window, "filter", cx);
    assert!(
        menu_reaches(window, true, cx),
        "the filter menu did not open"
    );
    menu_items(window, cx)
}

fn assert_only_checked(items: &[Item], name: &str) {
    for item in items {
        assert_eq!(
            item.checked == Some(true),
            item.label == name,
            "{}: checked is {:?}",
            item.label,
            item.checked
        );
    }
}

#[test]
fn every_filter_has_an_icon_the_assets_serve() {
    let icons: Vec<Icon> = StatusFilter::ALL.iter().map(|f| f.icon()).collect();
    assert_eq!(
        icons,
        [
            Icon::List,
            Icon::Ellipsis,
            Icon::TriangleAlert,
            Icon::CircleCheck
        ]
    );
    for icon in icons.into_iter().chain([Icon::ListFilter]) {
        assert_eq!(Icon::at_path(&format!("icons/{}", icon.file())), Some(icon));
    }
}

#[gpui_kit::test]
fn the_filter_menu_names_each_option_and_checks_the_chosen_one(cx: &mut TestAppContext) {
    let (window, _app) = common::open(cx);
    assert_eq!(filter_label(window, cx), "Filter, All");

    let items = open_menu(window, cx);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["All", "Processing", "Needs attention", "Done"]);
    assert_only_checked(&items, "All");

    let done = items.iter().find(|i| i.label == "Done").unwrap().id.clone();
    cx.update_window(window, |_, window, cx| window.click(done, cx))
        .unwrap();
    assert!(
        menu_reaches(window, false, cx),
        "choosing an option closes the menu"
    );
    assert_eq!(filter_label(window, cx), "Filter, Done");

    assert_only_checked(&open_menu(window, cx), "Done");
}

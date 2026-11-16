//! A11Y-9: every pointer target in every view is at least 24 by 24 points, and the primary
//! actions are at least 28 points tall, at the smallest text size and the largest (A11Y-7).

mod common;

use gpui_kit::{ElementId, Role, TestAppContext};

use common::views::every_view;

/// Roles a pointer acts on.
const TARGETS: [Role; 11] = [
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
];

/// The one main action of each view.
const PRIMARY: [&str; 6] = [
    "start-recording",
    "record",
    "record-stop",
    "onboarding-continue",
    "undo-stop",
    "queue-done",
];

fn id(path: &[ElementId]) -> String {
    match path.last() {
        Some(ElementId::Name(name)) => name.to_string(),
        other => format!("{other:?}"),
    }
}

fn check(scale: f32, cx: &mut TestAppContext) {
    let mut small = Vec::new();
    let mut primaries = 0;
    let mut checked = 0;
    for view in every_view(scale, cx) {
        for element in &view.elements {
            let target =
                element.role().is_some_and(|r| TARGETS.contains(&r)) || element.focused().is_some();
            if !target || !element.visible() {
                continue;
            }
            // A Kit component's frame takes the pointer for the native node inside it (a text
            // field's editable line), so the frame is the target.
            let inside_its_frame = view.elements.iter().any(|frame| {
                frame.path().len() < element.path().len()
                    && element.path().starts_with(frame.path())
                    && frame.role() == element.role()
            });
            if inside_its_frame {
                continue;
            }
            checked += 1;
            let size = element.bounds().size;
            let (w, h) = (f32::from(size.width), f32::from(size.height));
            let name = id(element.path());
            if w < 24. || h < 24. {
                small.push(format!("{} at {scale}: {name} is {w:.0}x{h:.0}", view.name));
            }
            if PRIMARY.contains(&name.as_str()) {
                primaries += 1;
                if h < 28. {
                    small.push(format!(
                        "{} at {scale}: primary {name} is {h:.0} tall",
                        view.name
                    ));
                }
            }
        }
    }
    assert!(checked > 50, "only {checked} targets checked at {scale}");
    assert!(primaries > 0, "no primary action seen at {scale}");
    assert!(small.is_empty(), "{}", small.join("\n"));
}

#[gpui_kit::test]
fn every_target_is_at_least_24_points_at_actual_size(cx: &mut TestAppContext) {
    check(1.0, cx);
}

#[gpui_kit::test]
fn every_target_is_at_least_24_points_at_the_smallest_text(cx: &mut TestAppContext) {
    check(0.85, cx);
}

#[gpui_kit::test]
fn every_target_is_at_least_24_points_at_the_largest_text(cx: &mut TestAppContext) {
    check(2.0, cx);
}

/// No target spills sideways out of the window or out of an accessible container around it,
/// where it would be clipped, as the upgrade step's Grant button once was.
fn contained(scale: f32, cx: &mut TestAppContext) {
    let mut spilled = Vec::new();
    let mut checked = 0;
    for view in every_view(scale, cx) {
        for element in &view.elements {
            let target = element.role().is_some_and(|r| TARGETS.contains(&r));
            if !target || !element.visible() {
                continue;
            }
            checked += 1;
            let b = element.bounds();
            let (left, right) = (f32::from(b.origin.x), f32::from(b.origin.x + b.size.width));
            let name = id(element.path());
            if left < -0.5 || right > view.width + 0.5 {
                spilled.push(format!(
                    "{} at {scale}: {name} spans {left:.0}..{right:.0}, window is {:.0} wide",
                    view.name, view.width
                ));
            }
            for frame in &view.elements {
                let encloses = frame.path().len() < element.path().len()
                    && element.path().starts_with(frame.path());
                if !encloses || !frame.visible() {
                    continue;
                }
                let f = frame.bounds();
                let (fl, fr) = (f32::from(f.origin.x), f32::from(f.origin.x + f.size.width));
                if left < fl - 0.5 || right > fr + 0.5 {
                    spilled.push(format!(
                        "{} at {scale}: {name} spans {left:.0}..{right:.0}, outside {} at {fl:.0}..{fr:.0}",
                        view.name,
                        id(frame.path())
                    ));
                }
            }
        }
    }
    assert!(checked > 50, "only {checked} targets checked at {scale}");
    assert!(spilled.is_empty(), "{}", spilled.join("\n"));
}

#[gpui_kit::test]
fn every_target_fits_its_window_and_containers_at_actual_size(cx: &mut TestAppContext) {
    contained(1.0, cx);
}

#[gpui_kit::test]
fn every_target_fits_its_window_and_containers_at_the_largest_text(cx: &mut TestAppContext) {
    contained(2.0, cx);
}

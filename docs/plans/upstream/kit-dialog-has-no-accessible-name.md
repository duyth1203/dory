# GPUI Kit: a component `Dialog` has no accessible name

Not filed. Filing upstream is the operator's call (P7 brief).

## Versions

- `gpui-kit` 0.9.1
- `gpui-component` 0.9.1
- `gpui-base` 0.9.1
- `gpui-pre` 0.3.12

All are pinned in `Cargo.lock` on `feat/gpui-accessibility`.

## What happens

`gpui_component::dialog::Dialog::title("Processing Queue")` draws the title, but the dialog's accessibility node (`Role::Dialog`, from `gpui_base::Dialog`) has no label. VoiceOver says only "dialog" when focus enters it.

`gpui_base::Dialog` has `accessibility_label`, documented as "Names the dialog for accessibility clients (its title, typically)" (`gpui-base-0.9.1/src/dialog.rs:495`). The component `Dialog` never calls it.

- `title` stores an `AnyElement` (`gpui-component-0.9.1/src/dialog/dialog.rs:402`). That type cannot be turned back into text.
- The `BaseDialogRoot` wrapper forwards eleven builder methods, and `accessibility_label` is not one of them (`dialog.rs:251-292`).
- The base root is private, so an app cannot reach it.

## Minimal repro

```rust
use gpui_kit::{AppContext as _, Role, TestAppContext, WindowExt as _, div};

#[gpui_kit::test]
fn a_titled_dialog_is_named(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = cx
        .update(|cx| gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Empty)))
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.open_dialog(cx, |dialog, _, _| dialog.title("Processing Queue"))
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        use gpui_kit::test::TestWindowExt as _;
        window.render_frame(cx);
        let dialog = gpui_kit::base::test_support::snapshots(window)
            .into_iter()
            .find(|e| e.role() == Some(Role::Dialog))
            .expect("a dialog node");
        // Fails on 0.9.1: the label is None.
        assert_eq!(dialog.label(), Some("Processing Queue"));
    })
    .unwrap();
}

struct Empty;
impl gpui_kit::Render for Empty {
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<Self>) -> impl gpui_kit::IntoElement {
        div()
    }
}
```

Dory's `apps/dory-gpui/tests/a11y_tree.rs` hits the same gap on the Processing Queue popover and the Set up summaries sheet. That test lists this file as the one known exception. When Kit is fixed, the test fails until the exception is removed.

## Suggested fix

Add a `Dialog::accessibility_label(label)` builder to the component `Dialog` that forwards to the base root. Alternatively, have `title` take `impl Into<SharedString>` as well and set the label from it.

## Dory's workaround

None in the dialog node itself. Each dialog's first child is a labelled group or list ("Processing Queue", "Set up summaries"), so VoiceOver reads a name one step in.

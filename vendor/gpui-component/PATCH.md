# Patch against gpui-component 0.9.1

This folder is the published `gpui-component` 0.9.1 crate (the `src/`, `locales/`, `tests/`, `build.rs`, `Cargo.toml`, `README.md`, and `LICENSE-APACHE` that crates.io ships), with one change. The root `Cargo.toml` points `[patch.crates-io]` at this folder.

A popup menu's submenu could run past the window. The row menu's model pickers opened past the right edge in a window just too narrow for them, and past the left edge once they flipped. The submenu sits in an `anchored` element that pulls it back inside the window, but the menu shifted the submenu sideways with `.left()` on the child. `anchored` measures only its children's size at its own origin, so it never saw that shift. It missed overflow by up to the parent menu's width, and on the left side the shift undid the pull-back. The patch hands the same shift to `anchored().offset()`, so the pull-back measures the submenu where it is drawn. Where a submenu already fit, it lands on the same pixel as before.

0.10.0 has the same code. Upstream PR to longbridge/gpui-kit not filed; it needs the operator's go.

Delete this folder and the `[patch.crates-io]` line once a release that carries this fix is the pinned `gpui-kit` version. `apps/dory-gpui/tests/submenu_placement.rs` fails without it.

## Diff against 0.9.1

```diff
--- a/src/menu/popup_menu.rs
+++ b/src/menu/popup_menu.rs
@@ -1514,17 +1514,18 @@
                         let (anchor, left) = self.submenu_anchor;
                         let is_bottom_pos =
                             matches!(anchor, Anchor::BottomLeft | Anchor::BottomRight);
+                        let top = if is_bottom_pos { Pixels::ZERO } else { px(-1.) };
+                        // The shift goes to `anchored`, not the child, so the window snap
+                        // measures the submenu where it is drawn.
                         deferred(
                             anchored()
                                 .anchor(anchor)
+                                .offset(Point::new(left, top))
                                 .child(
                                     div()
                                         .id("submenu")
                                         .test_support()
                                         .occlude()
-                                        .when(is_bottom_pos, |this| this.bottom_0())
-                                        .when(!is_bottom_pos, |this| this.top_neg_1())
-                                        .left(left)
                                         .child(menu.clone()),
                                 )
                                 .snap_to_window_with_margin(Edges::all(EDGE_PADDING)),
```

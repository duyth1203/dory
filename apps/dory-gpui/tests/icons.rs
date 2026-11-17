//! UX-25: every icon comes from the vendored Lucide set in `assets/icons/`, and no view draws a
//! text glyph in its place (`docs/design/settings-icons-blur.md` section 2).

use std::path::{Path, PathBuf};

use dory_gpui::icons::Icon;

fn icons_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons")
}

/// Parsed with usvg, at the version GPUI draws SVGs with.
#[test]
fn every_icon_is_a_plain_lucide_svg_that_parses() {
    let version = std::fs::read_to_string(icons_dir().join("VERSION")).unwrap();
    for icon in Icon::ALL {
        let path = icons_dir().join(icon.file());
        let on_disk = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(on_disk, icon.bytes(), "{icon:?} embeds its file");
        let text = std::str::from_utf8(icon.bytes()).unwrap();
        assert!(
            text.starts_with(&format!(
                "<!-- @license lucide-static v{} - ISC -->",
                version.trim()
            )),
            "{icon:?} comes from the pinned release"
        );
        for banned in [
            "<script",
            "<style",
            "<foreignObject",
            "<image",
            "href",
            "javascript:",
        ] {
            assert!(!text.contains(banned), "{icon:?} contains {banned}");
        }
        let tree = usvg::Tree::from_data(icon.bytes(), &usvg::Options::default())
            .unwrap_or_else(|e| panic!("{icon:?} does not parse: {e}"));
        assert_eq!(tree.size(), usvg::Size::from_wh(24., 24.).unwrap());
        assert!(tree.root().has_children(), "{icon:?} draws something");
        let stroke = if icon.file().starts_with("transport-") {
            "stroke-width=\"1.5\""
        } else {
            "stroke-width=\"2\""
        };
        assert!(text.contains(stroke), "{icon:?} has {stroke}");
    }
}

#[test]
fn the_folder_holds_only_the_icons_dory_draws() {
    let mut files: Vec<String> = std::fs::read_dir(icons_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".svg"))
        .collect();
    files.sort();
    let mut known: Vec<String> = Icon::ALL.iter().map(|i| i.file().to_owned()).collect();
    known.sort();
    assert_eq!(files, known);
    let license = std::fs::read_to_string(icons_dir().join("LICENSE")).unwrap();
    assert!(license.starts_with("ISC License"));
    assert!(license.contains("The MIT License (MIT)"));
}

/// Text glyphs the views drew before Lucide, which the icons replace.
const GLYPHS: &[char] = &[
    '✓', '✔', '✕', '✖', '×', '●', '○', '◌', '◷', '▤', '▭', '▲', '■', '▶', '❚', '⏸', '⏵', '⏹', '↺',
    '↻', '⋯', '⚠', '⌄', '▾', '▸', '›', '‹', '🕒', '🔋',
];

/// Where a glyph is still the right thing to draw, and why.
const ALLOWED: &[(&str, char, &str)] = &[(
    "status_item.rs",
    '●',
    "the menu bar's record dot is the native status item's title text, not a GPUI view",
)];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_view_draws_a_text_glyph() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() > 30, "found {} sources", files.len());
    let mut found = Vec::new();
    for file in files {
        let name = file.file_name().unwrap().to_str().unwrap().to_owned();
        let text = std::fs::read_to_string(&file).unwrap();
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for glyph in line.chars().filter(|c| GLYPHS.contains(c)) {
                if !ALLOWED.iter().any(|(f, g, _)| *f == name && *g == glyph) {
                    found.push(format!("{name}:{}: {glyph} in {}", n + 1, line.trim()));
                }
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

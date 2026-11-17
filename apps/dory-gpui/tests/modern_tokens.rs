//! Modern looks exactly as it did before themes: every token, in every variant the system can
//! ask for, matches `golden/modern-tokens.txt`, which was written from the code before the Book
//! theme existed. `DORY_BLESS=1` rewrites the file; only do that for an intended Modern change.

use dory_gpui::theme::{Display, Theme};
use gpui_kit::{Hsla, Rgba, rgb};

const ACCENTS: [(&str, u32, u32); 8] = [
    ("blue", 0x007AFF, 0x0A84FF),
    ("purple", 0xAF52DE, 0xBF5AF2),
    ("pink", 0xFF2D55, 0xFF375F),
    ("red", 0xFF3B30, 0xFF453A),
    ("orange", 0xFF9500, 0xFF9F0A),
    ("yellow", 0xFFCC00, 0xFFD60A),
    ("green", 0x34C759, 0x30D158),
    ("graphite", 0x8E8E93, 0x98989D),
];

fn hex(color: Hsla) -> String {
    let c: Rgba = color.into();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        byte(c.r),
        byte(c.g),
        byte(c.b),
        byte(c.a)
    )
}

fn listing() -> String {
    let mut out = String::new();
    for (name, light, dark) in ACCENTS {
        for increase_contrast in [false, true] {
            for (appearance, accent) in [("light", light), ("dark", dark)] {
                let display = Display {
                    increase_contrast,
                    ..Display::default()
                };
                let theme = modern(appearance == "dark", rgb(accent).into(), display);
                let variant = format!(
                    "{appearance} {name}{}",
                    if increase_contrast { " ic" } else { "" }
                );
                out.push_str(&format!("{variant} shadows {}\n", theme.shadows));
                for (token, color) in theme.named() {
                    out.push_str(&format!("{variant} {token} {}\n", hex(color)));
                }
            }
        }
    }
    out
}

fn modern(dark: bool, accent: Hsla, display: Display) -> Theme {
    Theme::modern(dark, accent, display)
}

/// Tokens added since the golden file, which only Book's flags paint.
const ADDED: [&str; 1] = ["leader"];

#[test]
fn modern_tokens_match_the_values_from_before_themes() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/modern-tokens.txt");
    let now = listing();
    let kept: Vec<&str> = now
        .lines()
        .filter(|line| !ADDED.iter().any(|t| line.split(' ').rev().nth(1) == Some(t)))
        .collect();
    if std::env::var_os("DORY_BLESS").is_some() {
        std::fs::write(path, kept.join("\n") + "\n").unwrap();
    }
    let before = std::fs::read_to_string(path).expect("golden file");
    let changed: Vec<String> = before
        .lines()
        .zip(&kept)
        .filter(|(b, n)| b != *n)
        .map(|(b, n)| format!("was {b}\nnow {n}"))
        .collect();
    assert_eq!(before.lines().count(), kept.len(), "token count changed");
    assert!(changed.is_empty(), "{}", changed.join("\n"));
}

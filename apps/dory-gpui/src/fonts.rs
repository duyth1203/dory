//! Dory's typeface, Literata (`docs/design/book-theme.md` section 3), embedded in the binary so
//! every Mac renders the same type, and the OpenType features its roles use.
//! `scripts/vendor-literata.sh` fetches the faces from one pinned official release.

use std::borrow::Cow;
use std::sync::Arc;

use gpui_kit::{App, FontFeatures, Global};

/// The one typeface every window and control is set in, through GPUI Kit's root (`theme::sync`).
pub const FAMILY: &str = "Literata";

pub const LITERATA_VERSION: &str = include_str!("../assets/fonts/literata/VERSION");
pub const LITERATA_LICENSE: &str = include_str!("../assets/fonts/literata/OFL.txt");

/// Regular and italic for reading, Medium for the chapter title, SemiBold for bold runs.
pub const FACES: [(&str, &[u8]); 4] = [
    (
        "Literata-Regular",
        include_bytes!("../assets/fonts/literata/Literata-Regular.ttf"),
    ),
    (
        "Literata-Italic",
        include_bytes!("../assets/fonts/literata/Literata-Italic.ttf"),
    ),
    (
        "Literata-Medium",
        include_bytes!("../assets/fonts/literata/Literata-Medium.ttf"),
    ),
    (
        "Literata-SemiBold",
        include_bytes!("../assets/fonts/literata/Literata-SemiBold.ttf"),
    ),
];

struct Loaded;

impl Global for Loaded {}

/// Adds the faces to GPUI's text system once. `theme::sync` calls it, so any window that has a
/// theme can set the Literata family.
pub fn ensure(cx: &mut App) {
    if cx.has_global::<Loaded>() {
        return;
    }
    let faces = FACES
        .iter()
        .map(|(_, bytes)| Cow::Borrowed(*bytes))
        .collect();
    if let Err(error) = cx.text_system().add_fonts(faces) {
        eprintln!("fonts: could not load Literata: {error}");
    }
    cx.set_global(Loaded);
}

fn features(tags: &[&str]) -> FontFeatures {
    FontFeatures(Arc::new(tags.iter().map(|t| (t.to_string(), 1)).collect()))
}

/// Real small caps for capitals and lowercase alike (`c2sc` and `smcp`): headings, tabs, the
/// running head, and the script's speaker names. The text keeps its own letters.
pub fn small_caps() -> FontFeatures {
    features(&["c2sc", "smcp"])
}

/// Oldstyle figures for the recording lengths in the sidebar's contents.
pub fn oldstyle() -> FontFeatures {
    features(&["onum"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vendored_face_is_embedded_and_truetype() {
        let script = include_str!("../../../scripts/vendor-literata.sh");
        let line = script
            .lines()
            .find(|l| l.starts_with("faces=("))
            .expect("faces line");
        let listed: Vec<&str> = line
            .trim_start_matches("faces=(")
            .trim_end_matches(')')
            .split_whitespace()
            .collect();
        let embedded: Vec<&str> = FACES.iter().map(|(name, _)| *name).collect();
        assert_eq!(listed, embedded);
        for (name, bytes) in FACES {
            assert_eq!(&bytes[..4], &[0, 1, 0, 0], "{name}");
        }
    }

    #[test]
    fn the_license_is_the_ofl_with_literatas_copyright() {
        assert!(LITERATA_LICENSE.contains("SIL OPEN FONT LICENSE Version 1.1"));
        assert!(LITERATA_LICENSE.contains("The Literata Project Authors"));
        assert_eq!(LITERATA_VERSION.trim(), "3.103");
    }
}

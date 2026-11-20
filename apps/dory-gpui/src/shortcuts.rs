//! Shortcuts that start, stop, pause, and resume a recording from any app (Settings › Recording ›
//! Keyboard Shortcuts). None is set until the user records one.
//!
//! A [`Shortcut`] is a key and its modifiers, stored as `ctrl-alt-r` in `ui-prefs.json` and drawn
//! as `⌃⌥R`. [`Shortcut::accept`] decides which keystrokes may become one. [`Shortcuts`] holds
//! at most one per [`Bind`], and never the same one twice. `hotkeys` registers them with macOS
//! and runs a [`Bind`]'s command when one is pressed.

use std::collections::BTreeMap;

use gpui_kit::Keystroke;

use crate::status_item::Command;

/// What a global shortcut does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bind {
    StartStop,
    PauseResume,
}

impl Bind {
    pub const ALL: [Bind; 2] = [Bind::StartStop, Bind::PauseResume];

    pub fn label(self) -> &'static str {
        match self {
            Bind::StartStop => "Start or stop recording",
            Bind::PauseResume => "Pause or resume recording",
        }
    }

    /// The row's title in Settings.
    pub fn title(self) -> &'static str {
        match self {
            Bind::StartStop => "Start or Stop Recording",
            Bind::PauseResume => "Pause or Resume Recording",
        }
    }

    /// What the row says while no note is due.
    pub fn hint(self) -> &'static str {
        match self {
            Bind::StartStop => "Works in any app, even with Dory's window closed.",
            Bind::PauseResume => "Works in any app while a recording is under way.",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Bind::StartStop => "start-stop",
            Bind::PauseResume => "pause-resume",
        }
    }

    pub fn parse(slug: &str) -> Option<Bind> {
        Bind::ALL.into_iter().find(|bind| bind.slug() == slug)
    }

    /// The same command the menu-bar extra runs.
    pub fn command(self) -> Command {
        match self {
            Bind::StartStop => Command::Toggle,
            Bind::PauseResume => Command::Pause,
        }
    }
}

/// A key and the modifiers held with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    /// GPUI's name for the key: `r`, `5`, `f6`, `space`, `-`.
    key: String,
    control: bool,
    alt: bool,
    shift: bool,
    command: bool,
}

/// Why a keystroke cannot become a shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A plain key, or one with only Shift or Option, types a character in another app: ⇧R a
    /// capital, ⌥E an accent.
    NeedsModifier,
    /// macOS has no key code for it, so it cannot be registered.
    UnknownKey,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Refusal::NeedsModifier => {
                "Hold ⌘ or ⌃ with the key, so the shortcut never fires while you type."
            }
            Refusal::UnknownKey => "That key can't be a shortcut. Try a letter, number, or F-key.",
        }
    }
}

impl Shortcut {
    /// The shortcut `keystroke` would make, if it may become one.
    pub fn accept(keystroke: &Keystroke) -> Result<Shortcut, Refusal> {
        let shortcut = Shortcut {
            key: keystroke.key.to_ascii_lowercase(),
            control: keystroke.modifiers.control,
            alt: keystroke.modifiers.alt,
            shift: keystroke.modifiers.shift,
            command: keystroke.modifiers.platform,
        };
        if shortcut.key_code().is_none() {
            return Err(Refusal::UnknownKey);
        }
        // F-keys type nothing, so they may stand alone; any other key needs ⌘ or ⌃.
        if shortcut.is_function_key() || shortcut.command || shortcut.control {
            Ok(shortcut)
        } else {
            Err(Refusal::NeedsModifier)
        }
    }

    /// `ctrl-alt-shift-cmd-r`, modifiers in that order.
    pub fn unparse(&self) -> String {
        let mut out = String::new();
        for (held, name) in [
            (self.control, "ctrl-"),
            (self.alt, "alt-"),
            (self.shift, "shift-"),
            (self.command, "cmd-"),
        ] {
            if held {
                out.push_str(name);
            }
        }
        out.push_str(&self.key);
        out
    }

    /// Reads what [`Shortcut::unparse`] wrote. `None` for anything else, so a hand-edited file
    /// drops the shortcut instead of registering a wrong one.
    pub fn parse(text: &str) -> Option<Shortcut> {
        let mut shortcut = Shortcut {
            key: String::new(),
            control: false,
            alt: false,
            shift: false,
            command: false,
        };
        let mut rest = text;
        loop {
            let held = if let Some(after) = rest.strip_prefix("ctrl-") {
                rest = after;
                &mut shortcut.control
            } else if let Some(after) = rest.strip_prefix("alt-") {
                rest = after;
                &mut shortcut.alt
            } else if let Some(after) = rest.strip_prefix("shift-") {
                rest = after;
                &mut shortcut.shift
            } else if let Some(after) = rest.strip_prefix("cmd-") {
                rest = after;
                &mut shortcut.command
            } else {
                break;
            };
            *held = true;
        }
        shortcut.key = rest.to_owned();
        shortcut.key_code().map(|_| shortcut)
    }

    /// The Mac's way of writing it, as menus do: `⌃⌥⇧⌘R`.
    pub fn symbols(&self) -> String {
        let mut out = String::new();
        for (held, symbol) in [
            (self.control, '⌃'),
            (self.alt, '⌥'),
            (self.shift, '⇧'),
            (self.command, '⌘'),
        ] {
            if held {
                out.push(symbol);
            }
        }
        out.push_str(&match self.key.as_str() {
            "space" => "Space".to_owned(),
            "enter" => "↩".to_owned(),
            "tab" => "⇥".to_owned(),
            "backspace" => "⌫".to_owned(),
            "delete" => "⌦".to_owned(),
            "escape" => "⎋".to_owned(),
            "up" => "↑".to_owned(),
            "down" => "↓".to_owned(),
            "left" => "←".to_owned(),
            "right" => "→".to_owned(),
            "home" => "↖".to_owned(),
            "end" => "↘".to_owned(),
            "pageup" => "⇞".to_owned(),
            "pagedown" => "⇟".to_owned(),
            key => key.to_uppercase(),
        });
        out
    }

    /// What VoiceOver reads: "Control Option R".
    pub fn spoken(&self) -> String {
        let mut words: Vec<String> = [
            (self.control, "Control"),
            (self.alt, "Option"),
            (self.shift, "Shift"),
            (self.command, "Command"),
        ]
        .into_iter()
        .filter(|(held, _)| *held)
        .map(|(_, name)| name.to_owned())
        .collect();
        words.push(match self.key.as_str() {
            "pageup" => "Page Up".to_owned(),
            "pagedown" => "Page Down".to_owned(),
            key if key.len() == 1 => key.to_uppercase(),
            key => {
                let mut chars = key.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            }
        });
        words.join(" ")
    }

    pub fn is_function_key(&self) -> bool {
        self.key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=20).contains(&n))
    }

    /// The macOS virtual key code, by the US layout's key positions.
    pub fn key_code(&self) -> Option<u32> {
        key_code(&self.key)
    }

    /// Carbon's modifier mask: `cmdKey`, `shiftKey`, `optionKey`, `controlKey`.
    pub fn carbon_modifiers(&self) -> u32 {
        let mut mask = 0;
        for (held, bit) in [
            (self.command, 1 << 8),
            (self.shift, 1 << 9),
            (self.alt, 1 << 11),
            (self.control, 1 << 12),
        ] {
            if held {
                mask |= bit;
            }
        }
        mask
    }
}

/// The shortcuts the user set, at most one per [`Bind`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shortcuts {
    set: BTreeMap<Bind, Shortcut>,
}

impl Shortcuts {
    pub fn get(&self, bind: Bind) -> Option<&Shortcut> {
        self.set.get(&bind)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Bind, &Shortcut)> {
        self.set.iter().map(|(bind, shortcut)| (*bind, shortcut))
    }

    /// Sets `bind`'s shortcut, unless another bind already has it: then that one is returned and
    /// nothing changes.
    pub fn set(&mut self, bind: Bind, shortcut: Shortcut) -> Result<(), Bind> {
        if let Some((other, _)) = self
            .set
            .iter()
            .find(|(other, held)| **other != bind && **held == shortcut)
        {
            return Err(*other);
        }
        self.set.insert(bind, shortcut);
        Ok(())
    }

    pub fn clear(&mut self, bind: Bind) {
        self.set.remove(&bind);
    }

    /// For `ui-prefs.json`: the bind's slug to the shortcut's text.
    pub fn to_disk(&self) -> BTreeMap<String, String> {
        self.set
            .iter()
            .map(|(bind, shortcut)| (bind.slug().to_owned(), shortcut.unparse()))
            .collect()
    }

    /// Drops what it cannot read, and a second bind with a shortcut already taken.
    pub fn from_disk(disk: &BTreeMap<String, String>) -> Shortcuts {
        let mut shortcuts = Shortcuts::default();
        for (slug, text) in disk {
            if let (Some(bind), Some(shortcut)) = (Bind::parse(slug), Shortcut::parse(text)) {
                let _ = shortcuts.set(bind, shortcut);
            }
        }
        shortcuts
    }
}

fn key_code(key: &str) -> Option<u32> {
    const LETTERS: [(char, u32); 26] = [
        ('a', 0x00),
        ('s', 0x01),
        ('d', 0x02),
        ('f', 0x03),
        ('h', 0x04),
        ('g', 0x05),
        ('z', 0x06),
        ('x', 0x07),
        ('c', 0x08),
        ('v', 0x09),
        ('b', 0x0B),
        ('q', 0x0C),
        ('w', 0x0D),
        ('e', 0x0E),
        ('r', 0x0F),
        ('y', 0x10),
        ('t', 0x11),
        ('o', 0x1F),
        ('u', 0x20),
        ('i', 0x22),
        ('p', 0x23),
        ('l', 0x25),
        ('j', 0x26),
        ('k', 0x28),
        ('n', 0x2D),
        ('m', 0x2E),
    ];
    const FUNCTION: [u32; 20] = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71,
        0x6A, 0x40, 0x4F, 0x50, 0x5A,
    ];
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && let Some((_, code)) = LETTERS.iter().find(|(letter, _)| *letter == c)
    {
        return Some(*code);
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return FUNCTION.get(n.checked_sub(1)?).copied();
    }
    Some(match key {
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "6" => 0x16,
        "5" => 0x17,
        "=" => 0x18,
        "9" => 0x19,
        "7" => 0x1A,
        "-" => 0x1B,
        "8" => 0x1C,
        "0" => 0x1D,
        "]" => 0x1E,
        "[" => 0x21,
        "'" => 0x27,
        ";" => 0x29,
        "\\" => 0x2A,
        "," => 0x2B,
        "/" => 0x2C,
        "." => 0x2F,
        "`" => 0x32,
        "enter" => 0x24,
        "tab" => 0x30,
        "space" => 0x31,
        "backspace" => 0x33,
        "escape" => 0x35,
        "home" => 0x73,
        "pageup" => 0x74,
        "delete" => 0x75,
        "end" => 0x77,
        "pagedown" => 0x79,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use gpui_kit::Modifiers;

    use super::*;

    fn keystroke(key: &str, control: bool, alt: bool, shift: bool, command: bool) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                control,
                alt,
                shift,
                platform: command,
                function: false,
            },
            key: key.to_owned(),
            key_char: None,
        }
    }

    #[test]
    fn a_shortcut_survives_the_file() {
        let shortcut = Shortcut::accept(&keystroke("r", true, true, false, true)).unwrap();
        assert_eq!(shortcut.unparse(), "ctrl-alt-cmd-r");
        assert_eq!(Shortcut::parse("ctrl-alt-cmd-r"), Some(shortcut));
        assert_eq!(Shortcut::parse("ctrl-alt-nonsense"), None);
    }

    #[test]
    fn it_reads_the_way_menus_write_it() {
        let shortcut = Shortcut::parse("ctrl-alt-shift-cmd-f6").unwrap();
        assert_eq!(shortcut.symbols(), "⌃⌥⇧⌘F6");
        assert_eq!(shortcut.spoken(), "Control Option Shift Command F6");
        assert_eq!(Shortcut::parse("alt-space").unwrap().symbols(), "⌥Space");
    }

    #[test]
    fn carbon_gets_the_key_code_and_mask() {
        let shortcut = Shortcut::parse("ctrl-shift-r").unwrap();
        assert_eq!(shortcut.key_code(), Some(0x0F));
        assert_eq!(shortcut.carbon_modifiers(), (1 << 12) | (1 << 9));
        assert_eq!(Shortcut::parse("cmd-f13").unwrap().key_code(), Some(0x69));
    }

    #[test]
    fn a_shortcut_needs_command_or_control_unless_it_is_an_f_key() {
        let accepts = |key, control, alt, shift, command| {
            Shortcut::accept(&keystroke(key, control, alt, shift, command)).is_ok()
        };
        assert!(accepts("r", false, false, false, true), "⌘R");
        assert!(accepts("r", true, false, false, false), "⌃R");
        assert!(accepts("r", true, true, false, false), "⌃⌥R");
        assert!(accepts("f13", false, false, false, false), "F13 alone");
        assert!(accepts("f6", false, false, true, false), "⇧F6");
        for (key, alt, shift, why) in [
            ("r", false, false, "a plain letter"),
            ("r", false, true, "⇧R types a capital"),
            ("e", true, false, "⌥E types an accent"),
            ("5", true, true, "⌥⇧5 types a character"),
            ("space", false, false, "a plain Space"),
        ] {
            assert_eq!(
                Shortcut::accept(&keystroke(key, false, alt, shift, false)),
                Err(Refusal::NeedsModifier),
                "{why}"
            );
        }
    }

    #[test]
    fn a_key_macos_cannot_register_is_refused() {
        assert_eq!(
            Shortcut::accept(&keystroke("é", true, false, false, false)),
            Err(Refusal::UnknownKey)
        );
    }

    #[test]
    fn one_shortcut_belongs_to_one_bind() {
        let shortcut = Shortcut::parse("ctrl-alt-r").unwrap();
        let mut shortcuts = Shortcuts::default();
        shortcuts.set(Bind::StartStop, shortcut.clone()).unwrap();
        assert_eq!(
            shortcuts.set(Bind::PauseResume, shortcut.clone()),
            Err(Bind::StartStop)
        );
        assert_eq!(shortcuts.get(Bind::PauseResume), None);
        let disk = shortcuts.to_disk();
        assert_eq!(
            disk.get("start-stop").map(String::as_str),
            Some("ctrl-alt-r")
        );
        assert_eq!(Shortcuts::from_disk(&disk), shortcuts);
    }
}

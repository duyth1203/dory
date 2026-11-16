use super::{InvalidInput, LangTag, LanguagePref};
use crate::models::{Choice, Diar, Llm, Stt};

/// What the user chose. A model is a [`Choice`], never a path, so a removed model degrades to
/// `Auto` instead of breaking the pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub stt: Choice<Stt>,
    pub llm: Choice<Llm>,
    pub diar: Choice<Diar>,
    /// Default for recordings that do not pin their own.
    pub language: LanguagePref,
    /// The languages meetings are held in. With `language` on Auto, speech models must cover all
    /// of them. Empty means any.
    pub meeting_languages: Vec<LangTag>,
    /// Start processing after the undo grace ends.
    pub auto_process: bool,
    pub processing: Processing,
}

/// How hard processing may work (UX-30). Automatic follows the Mac's power, heat, and the
/// recorder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Processing {
    #[default]
    Auto,
    Fast,
    Light,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            stt: Choice::Auto,
            llm: Choice::Auto,
            diar: Choice::Auto,
            language: LanguagePref::Auto,
            meeting_languages: Vec::new(),
            auto_process: true,
            processing: Processing::Auto,
        }
    }
}

impl Settings {
    /// Call at the boundary where settings are loaded or edited.
    pub fn validate(&self) -> Result<(), InvalidInput> {
        self.stt.validate()?;
        self.llm.validate()?;
        self.diar.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Choice;

    #[test]
    fn defaults_are_valid_and_speech_cannot_be_off() {
        assert_eq!(Settings::default().validate(), Ok(()));
        let optional_off = Settings {
            llm: Choice::Off,
            diar: Choice::Off,
            ..Settings::default()
        };
        assert_eq!(optional_off.validate(), Ok(()));
        let stt_off = Settings {
            stt: Choice::Off,
            ..Settings::default()
        };
        assert!(stt_off.validate().is_err());
    }
}

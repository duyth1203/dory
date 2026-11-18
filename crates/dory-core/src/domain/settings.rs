use super::{Goal, InvalidInput, LangTag, LanguagePref};
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
    /// The language transcripts and summaries are written in. `None` is Same as spoken: nothing
    /// is translated and each summary follows the meeting's own language. Applies to recordings
    /// processed after it is set.
    pub output_language: Option<LangTag>,
    /// Start processing after the undo grace ends.
    pub auto_process: bool,
    pub processing: Processing,
    /// Whether transcription decodes again the speech a window's first pass left without text.
    pub switch_recovery: Recovery,
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

/// Recovering the speech a speech model drops when a window switches language. `Auto` recovers
/// only where it helps: a recording on Automatic, two or more meeting languages, and a model that
/// reports sentence times. It is `Off` by default because it missed the public-audio rule in
/// `docs/plans/bilingual.md` Appendix F (see `docs/plans/decisions.tsv`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Recovery {
    #[default]
    Off,
    On,
    Auto,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            stt: Choice::Auto,
            llm: Choice::Auto,
            diar: Choice::Auto,
            language: LanguagePref::Auto,
            meeting_languages: Vec::new(),
            output_language: None,
            auto_process: true,
            processing: Processing::Auto,
            switch_recovery: Recovery::default(),
        }
    }
}

impl Settings {
    /// The goal a new job gets: `goal`, plus the translate stage when an output language is set.
    /// This is the one place the setting reaches the job, so Same as spoken queues exactly the
    /// jobs it always did.
    pub fn goal_for(&self, goal: Goal) -> Goal {
        goal.translating(self.output_language.is_some())
    }

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
    fn an_output_language_adds_the_translate_stage_and_none_changes_nothing() {
        use crate::domain::Stage;
        let default = Settings::default();
        assert_eq!(default.output_language, None);
        for goal in [Goal::ALL, Goal::TRANSCRIBE, Goal::SUMMARIZE] {
            assert_eq!(default.goal_for(goal), goal);
        }
        let english = Settings {
            output_language: Some(LangTag::parse("en").unwrap()),
            ..Settings::default()
        };
        assert!(english.goal_for(Goal::ALL).wants(Stage::Translate));
        assert!(english.goal_for(Goal::TRANSCRIBE).wants(Stage::Translate));
        assert_eq!(english.goal_for(Goal::SUMMARIZE), Goal::SUMMARIZE);
    }

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

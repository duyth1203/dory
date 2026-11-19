use super::vocabulary::{validate_hidden_names, validate_terms};
use super::{Goal, InvalidInput, LangTag, LanguagePref, VocabularyTerm};
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
    /// The app filled `meeting_languages` from the Mac's language and nobody has chosen them yet.
    /// Speech still follows the guess, but summary models do not trust it: an English Mac says
    /// nothing about a meeting held half in Tagalog.
    pub meeting_languages_guessed: bool,
    /// The language transcripts and summaries are written in. `None` is Same as spoken: nothing
    /// is translated and each summary follows the meeting's own language. Applies to recordings
    /// processed after it is set.
    pub output_language: Option<LangTag>,
    /// Start processing after the undo grace ends.
    pub auto_process: bool,
    pub processing: Processing,
    /// Whether transcription decodes again the speech a window's first pass left without text.
    pub switch_recovery: Recovery,
    /// Names and terms the speech engine should get right. See `speech_vocabulary` for what the
    /// engine is actually given.
    pub vocabulary: Vec<VocabularyTerm>,
    /// Add the names the user gave speakers in any recording to what the engine favors.
    pub vocabulary_from_speakers: bool,
    /// Speaker names left out of that, for a rename that is no name or a name that hurts.
    pub vocabulary_hidden_names: Vec<String>,
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

/// How hard the pipeline works on one speech window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pace {
    Fast,
    /// Two threads at normal priority.
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
            meeting_languages_guessed: false,
            output_language: None,
            auto_process: true,
            processing: Processing::Auto,
            switch_recovery: Recovery::default(),
            vocabulary: Vec::new(),
            vocabulary_from_speakers: true,
            vocabulary_hidden_names: Vec::new(),
        }
    }
}

impl Settings {
    /// The meeting languages the user chose, or none while they are only a guess.
    pub fn chosen_meeting_languages(&self) -> &[LangTag] {
        if self.meeting_languages_guessed {
            &[]
        } else {
            &self.meeting_languages
        }
    }

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
        self.diar.validate()?;
        validate_terms(&self.vocabulary)?;
        validate_hidden_names(&self.vocabulary_hidden_names)
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
    fn a_guessed_meeting_language_is_unknown_to_summaries_but_still_guides_speech() {
        use crate::app::AppKind;
        use crate::models::{Llm, Stt};
        let english = vec![LangTag::parse("en").unwrap()];
        let guessed = Settings {
            meeting_languages: english.clone(),
            meeting_languages_guessed: true,
            ..Settings::default()
        };
        assert_eq!(guessed.chosen_meeting_languages(), &[] as &[LangTag]);
        assert_eq!(Llm::listing_needs(&guessed).spoken, Vec::<LangTag>::new());
        assert_eq!(Stt::listing_needs(&guessed).expected, english);
        let chosen = Settings {
            meeting_languages_guessed: false,
            ..guessed
        };
        assert_eq!(chosen.chosen_meeting_languages(), english.as_slice());
        assert_eq!(Llm::listing_needs(&chosen).spoken, english);
    }

    #[test]
    fn the_vocabulary_is_checked_with_the_rest_of_the_settings() {
        use crate::domain::Boost;
        let default = Settings::default();
        assert!(default.vocabulary.is_empty());
        assert!(default.vocabulary_from_speakers);
        assert!(default.vocabulary_hidden_names.is_empty());
        let bad_term = Settings {
            vocabulary: vec![VocabularyTerm::new("AC/DC", Boost::Normal)],
            ..Settings::default()
        };
        assert!(bad_term.validate().is_err());
        let bad_hidden = Settings {
            vocabulary_hidden_names: vec!["two\nlines".into()],
            ..Settings::default()
        };
        assert!(bad_hidden.validate().is_err());
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

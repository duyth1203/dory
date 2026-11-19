//! The commands that write outside recording and processing: `add`, `models choose`,
//! `models provision`, `languages`, `switch-recovery`, `output-language`, and `vocabulary`. Each
//! opens Dory with the data folder locked, except `vocabulary` with no edit, which only reads.

use dory::Config;
use dory_core::app::{AppKind, Event};
use dory_core::domain::{Boost, Recovery, Settings, VocabularyTerm, folded_words};
use dory_core::models::{Choice, Diar, Llm, ModelId, Stt};

use crate::args::{
    AddArgs, ChooseArgs, KindArg, LanguagesArgs, OutputLanguageArgs, ProvisionArgs,
    SwitchRecoveryArgs, VocabularyArgs,
};
use crate::wait::block_on;
use crate::{open, open_library};

pub fn choose(config: &Config, args: &ChooseArgs) -> Result<(), String> {
    let dory = open(config, args.fake_engines)?;
    match args.kind {
        KindArg::Stt => choose_kind::<Stt>(&dory, &args.choice),
        KindArg::Llm => choose_kind::<Llm>(&dory, &args.choice),
        KindArg::Diar => choose_kind::<Diar>(&dory, &args.choice),
    }
}

fn choose_kind<K: AppKind>(dory: &dory::Dory, word: &str) -> Result<(), String> {
    let choice: Choice<K> = match word {
        "auto" => Choice::Auto,
        "off" => Choice::Off,
        id => {
            let id: ModelId<K> = id.parse().map_err(|e| format!("{e}"))?;
            let rows = block_on(dory.models().list::<K>()).map_err(crate::chain)?;
            if !rows.iter().any(|r| r.status.entry.id == id) {
                return Err(format!("{id} is not in the {} catalog", K::TAG));
            }
            Choice::Pinned(id)
        }
    };
    dory.models().choose(choice).map_err(crate::chain)?;
    println!("{} model set to {word}", K::TAG);
    Ok(())
}

pub fn provision(config: &Config, args: &ProvisionArgs) -> Result<(), String> {
    let dory = open(config, false)?;
    match args.kind {
        KindArg::Stt => provision_kind::<Stt>(&dory, &args.id),
        KindArg::Llm => provision_kind::<Llm>(&dory, &args.id),
        KindArg::Diar => provision_kind::<Diar>(&dory, &args.id),
    }
}

fn provision_kind<K: AppKind>(dory: &dory::Dory, id: &str) -> Result<(), String> {
    let id: ModelId<K> = id.parse().map_err(|e| format!("{e}"))?;
    let events = dory.events();
    let done = dory.models().provision(id.clone());
    let printer = std::thread::spawn(move || {
        let mut last = None;
        while let Some(event) = events.recv_blocking() {
            match event {
                Event::ProvisionProgress { .. } => {
                    let line = crate::view::provision_progress(&event);
                    if last.as_ref() != Some(&line) {
                        println!("  {line}");
                        last = Some(line);
                    }
                }
                Event::ProvisionFinished { .. } => break,
                _ => {}
            }
        }
    });
    let result = block_on(done);
    drop(printer);
    result.map_err(crate::chain)?;
    println!("{id} is ready");
    Ok(())
}

pub fn languages(config: &Config, args: &LanguagesArgs) -> Result<(), String> {
    let dory = open(config, false)?;
    if let LanguagesArgs::Set(tags) = args {
        let tags = tags.clone();
        dory.update_settings(|s| {
            s.meeting_languages = tags;
            s.meeting_languages_guessed = false;
        })
        .map_err(crate::chain)?;
    }
    let settings = dory.settings().map_err(crate::chain)?;
    if settings.meeting_languages.is_empty() {
        println!("meeting languages: any");
    } else {
        let tags: Vec<String> = settings
            .meeting_languages
            .iter()
            .map(ToString::to_string)
            .collect();
        let guessed = if settings.meeting_languages_guessed {
            " (guessed from the Mac's language, so summary models that read only some languages wait for you to set them)"
        } else {
            ""
        };
        println!("meeting languages: {}{guessed}", tags.join(", "));
    }
    Ok(())
}

pub fn switch_recovery(config: &Config, args: &SwitchRecoveryArgs) -> Result<(), String> {
    let dory = open(config, false)?;
    if let SwitchRecoveryArgs::Set(switch) = *args {
        dory.update_settings(|s| s.switch_recovery = switch)
            .map_err(crate::chain)?;
    }
    let word = match dory.settings().map_err(crate::chain)?.switch_recovery {
        Recovery::On => "on",
        Recovery::Off => "off",
        Recovery::Auto => "auto, for meetings in two or more languages",
    };
    println!("switch recovery: {word}");
    Ok(())
}

pub fn output_language(config: &Config, args: &OutputLanguageArgs) -> Result<(), String> {
    let dory = open(config, false)?;
    if let OutputLanguageArgs::Set(tag) = args {
        let tag = tag.clone();
        dory.update_settings(|s| s.output_language = tag)
            .map_err(crate::chain)?;
    }
    match dory.settings().map_err(crate::chain)?.output_language {
        Some(tag) => println!("output language: {tag}"),
        None => println!("output language: same as spoken"),
    }
    Ok(())
}

pub fn vocabulary(config: &Config, args: &VocabularyArgs) -> Result<(), String> {
    if *args == VocabularyArgs::Show {
        let library = open_library(config, false)?;
        let settings = library.settings().map_err(crate::chain)?;
        let merged = library.speech_vocabulary().map_err(crate::chain)?;
        print!("{}", describe_vocabulary(&settings, &merged));
        return Ok(());
    }
    let dory = open(config, false)?;
    let mut outcome = Ok(());
    dory.update_settings(|s| outcome = edit_vocabulary(s, args))
        .map_err(crate::chain)?;
    outcome?;
    let settings = dory.settings().map_err(crate::chain)?;
    let merged = dory.speech_vocabulary().map_err(crate::chain)?;
    print!("{}", describe_vocabulary(&settings, &merged));
    Ok(())
}

/// Applies one edit. A failure leaves `settings` as it was.
fn edit_vocabulary(settings: &mut Settings, args: &VocabularyArgs) -> Result<(), String> {
    let same = |a: &str, b: &str| folded_words(a) == folded_words(b);
    match args {
        VocabularyArgs::Show => {}
        VocabularyArgs::Add { text, strong } => {
            let boost = if *strong {
                Boost::Strong
            } else {
                Boost::Normal
            };
            match settings.vocabulary.iter_mut().find(|t| same(&t.text, text)) {
                Some(term) => term.boost = boost,
                None => settings
                    .vocabulary
                    .push(VocabularyTerm::new(text.clone(), boost)),
            }
        }
        VocabularyArgs::Remove { text } => {
            let before = settings.vocabulary.len();
            settings.vocabulary.retain(|t| !same(&t.text, text));
            if settings.vocabulary.len() == before {
                return Err(format!("{text:?} is not in the vocabulary"));
            }
        }
        VocabularyArgs::Speakers(on) => settings.vocabulary_from_speakers = *on,
        VocabularyArgs::Hide { name } => {
            if !settings
                .vocabulary_hidden_names
                .iter()
                .any(|n| same(n, name))
            {
                settings.vocabulary_hidden_names.push(name.clone());
            }
        }
        VocabularyArgs::Unhide { name } => {
            settings.vocabulary_hidden_names.retain(|n| !same(n, name));
        }
    }
    Ok(())
}

/// `merged` is `speech_vocabulary`: the user's terms, then the names speakers add.
fn describe_vocabulary(settings: &Settings, merged: &[VocabularyTerm]) -> String {
    let mut out = String::new();
    if settings.vocabulary.is_empty() {
        out.push_str("terms: none\n");
    } else {
        out.push_str("terms:\n");
        for term in &settings.vocabulary {
            let strong = if term.boost == Boost::Strong {
                " (strong)"
            } else {
                ""
            };
            out.push_str(&format!("  {}{strong}\n", term.text));
        }
    }
    let on = if settings.vocabulary_from_speakers {
        "on"
    } else {
        "off"
    };
    out.push_str(&format!("From speaker names: {on}\n"));
    for name in merged.iter().skip(settings.vocabulary.len()) {
        out.push_str(&format!("  {}\n", name.text));
    }
    if settings.vocabulary_hidden_names.is_empty() {
        out.push_str("hidden names: none\n");
    } else {
        out.push_str("hidden names:\n");
        for name in &settings.vocabulary_hidden_names {
            out.push_str(&format!("  {name}\n"));
        }
    }
    out
}

pub fn add(config: &Config, args: &AddArgs) -> Result<(), String> {
    let dory = open(config, false)?;
    let id = dory
        .library()
        .import_audio(&args.path, args.title.clone())
        .map_err(crate::chain)?;
    println!("added {id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(text: &str, strong: bool) -> VocabularyArgs {
        VocabularyArgs::Add {
            text: text.into(),
            strong,
        }
    }

    #[test]
    fn adding_an_existing_term_updates_its_boost_and_keeps_its_spelling() {
        let mut s = Settings::default();
        edit_vocabulary(&mut s, &add("Priya", false)).unwrap();
        edit_vocabulary(&mut s, &add("Acme Corp", false)).unwrap();
        edit_vocabulary(&mut s, &add("priya", true)).unwrap();
        assert_eq!(
            s.vocabulary,
            [
                VocabularyTerm::new("Priya", Boost::Strong),
                VocabularyTerm::new("Acme Corp", Boost::Normal)
            ]
        );
    }

    #[test]
    fn remove_matches_folded_words_and_a_missing_term_is_an_error() {
        let mut s = Settings::default();
        edit_vocabulary(&mut s, &add("Zoë Ng", false)).unwrap();
        assert!(
            edit_vocabulary(
                &mut s,
                &VocabularyArgs::Remove {
                    text: "Lina".into()
                }
            )
            .is_err()
        );
        edit_vocabulary(
            &mut s,
            &VocabularyArgs::Remove {
                text: "zoe  ng".into(),
            },
        )
        .unwrap();
        assert!(s.vocabulary.is_empty());
    }

    #[test]
    fn hide_and_show_edit_the_hidden_names_once_each() {
        let mut s = Settings::default();
        let hide = |name: &str| VocabularyArgs::Hide { name: name.into() };
        edit_vocabulary(&mut s, &hide("Me")).unwrap();
        edit_vocabulary(&mut s, &hide("me")).unwrap();
        edit_vocabulary(&mut s, &hide("Omar")).unwrap();
        assert_eq!(s.vocabulary_hidden_names, ["Me", "Omar"]);
        edit_vocabulary(&mut s, &VocabularyArgs::Unhide { name: "ME".into() }).unwrap();
        assert_eq!(s.vocabulary_hidden_names, ["Omar"]);
        edit_vocabulary(&mut s, &VocabularyArgs::Speakers(false)).unwrap();
        assert!(!s.vocabulary_from_speakers);
    }

    #[test]
    fn the_listing_shows_terms_strength_the_speaker_toggle_added_names_and_hidden_names() {
        let s = Settings {
            vocabulary: vec![
                VocabularyTerm::new("Priya", Boost::Strong),
                VocabularyTerm::new("Lina", Boost::Normal),
            ],
            vocabulary_hidden_names: vec!["Me".into()],
            ..Settings::default()
        };
        let merged =
            dory_core::domain::speech_vocabulary(&s, &["Omar".to_owned(), "Me".to_owned()]);
        assert_eq!(
            describe_vocabulary(&s, &merged),
            "terms:\n  Priya (strong)\n  Lina\nFrom speaker names: on\n  Omar\nhidden names:\n  Me\n"
        );
        let bare = Settings {
            vocabulary_from_speakers: false,
            ..Settings::default()
        };
        assert_eq!(
            describe_vocabulary(&bare, &[]),
            "terms: none\nFrom speaker names: off\nhidden names: none\n"
        );
    }
}

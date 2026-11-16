//! The commands that write outside recording and processing: `add`, `models choose`,
//! `models provision`, and `languages`. Each opens Dory with the data folder locked.

use dory::Config;
use dory_core::app::{AppKind, Event};
use dory_core::models::{Choice, Diar, Llm, ModelId, Stt};

use crate::args::{AddArgs, ChooseArgs, KindArg, LanguagesArgs, ProvisionArgs};
use crate::open;
use crate::wait::block_on;

pub fn choose(config: &Config, args: &ChooseArgs) -> Result<(), String> {
    let dory = open(config, false)?;
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
        dory.update_settings(|s| s.meeting_languages = tags)
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
        println!("meeting languages: {}", tags.join(", "));
    }
    Ok(())
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

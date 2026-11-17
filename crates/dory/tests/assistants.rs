use dory::Config;
use dory_core::app::DoryError;
use dory_core::domain::{ClientKey, Exposure, Writes};

#[test]
fn the_owner_adds_changes_and_removes_assistants() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let dory = dory::open(&config).unwrap();
    let assistants = dory.assistants();
    assert!(!assistants.access().unwrap().enabled);
    assistants.set_enabled(true).unwrap();

    let key = dory::new_client_key().unwrap();
    assert!(ClientKey::parse(key.as_str()).is_some());
    assert_ne!(key, dory::new_client_key().unwrap(), "keys are random");
    let added = assistants
        .add(
            "  Claude Code ",
            Exposure::Summaries,
            Writes::Never,
            key.clone(),
        )
        .unwrap();
    assert_eq!(added.label, "Claude Code");

    assert!(matches!(
        assistants.add(
            "Claude Code",
            Exposure::Titles,
            Writes::Never,
            dory::new_client_key().unwrap()
        ),
        Err(DoryError::AssistantExists(label)) if label == "Claude Code"
    ));
    assert!(matches!(
        assistants.add(
            "  ",
            Exposure::Titles,
            Writes::Never,
            dory::new_client_key().unwrap()
        ),
        Err(DoryError::Invalid(_))
    ));

    assistants
        .set_exposure("Claude Code", Exposure::Transcripts)
        .unwrap();
    assistants.set_writes("Claude Code", Writes::Ask).unwrap();
    let reader = dory::open_library(&config).unwrap();
    let access = reader.assistant_access().unwrap();
    assert!(access.enabled);
    assert_eq!(access.clients.len(), 1);
    assert_eq!(access.clients[0].exposure, Exposure::Transcripts);
    assert_eq!(access.clients[0].writes, Writes::Ask);

    assistants.remove("Claude Code").unwrap();
    assert!(assistants.access().unwrap().clients.is_empty());
    assert!(
        assistants
            .set_exposure("Claude Code", Exposure::Titles)
            .is_err()
    );
}

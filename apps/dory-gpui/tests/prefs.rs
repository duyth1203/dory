use dory_core::domain::{Ms, RecordingId, Timestamp};
use dory_gpui::export::srt_time;
use dory_gpui::library::Selection;
use dory_gpui::prefs::{
    DEFAULT_SIDEBAR_WIDTH, DEFAULT_WINDOW_SIZE, Prefs, RESTORE_WITHIN, clamp_sidebar_width,
    restore_selection,
};
use dory_gpui::theme::ThemeName;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dory-gpui-prefs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn prefs_round_trip_through_the_file() {
    let dir = scratch("round-trip");
    let prefs = Prefs {
        window_size: (1280., 800.),
        sidebar_width: 280.,
        last_selection: None,
        quit_at: Some(Timestamp(5)),
        onboarding: Some(dory_gpui::onboarding::OnboardingStep::Models),
        text_scale: 1.5,
        settings_pane: dory_gpui::settings_window::Pane::Storage,
        processing_origin: Some((120., 64.)),
        notifications: {
            let mut notes = dory_gpui::prefs::Notifications::default();
            notes.set(dory_gpui::notify::NoteKind::Transcript, false);
            notes.set(dory_gpui::notify::NoteKind::Download, false);
            notes
        },
        theme: ThemeName::Book,
    };
    prefs.write(&dir).unwrap();
    assert_eq!(Prefs::read(&dir), prefs);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_theme_is_remembered_and_new_users_get_modern() {
    let dir = scratch("theme");
    std::fs::create_dir_all(&dir).unwrap();
    let read = |json: &str| {
        std::fs::write(Prefs::path(&dir), json).unwrap();
        Prefs::read(&dir)
    };
    assert_eq!(read(r#"{"version":1}"#).theme, ThemeName::Modern);
    assert_eq!(
        read(r#"{"version":1,"theme":"book"}"#).theme,
        ThemeName::Book
    );
    let tried = read(r#"{"version":1,"book_offered":true,"theme":"book","text_scale":1.5}"#);
    assert_eq!(
        (tried.theme, tried.text_scale),
        (ThemeName::Book, 1.5),
        "a file from when Book sat behind Try the Book theme still reads"
    );
    assert_eq!(
        read(r#"{"version":1,"theme":"sepia"}"#).theme,
        ThemeName::Modern
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_or_corrupt_file_reads_as_the_defaults() {
    let dir = scratch("corrupt");
    assert_eq!(Prefs::read(&dir), Prefs::default());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(Prefs::path(&dir), b"{ not json").unwrap();
    assert_eq!(Prefs::read(&dir), Prefs::default());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn out_of_range_sizes_fall_back_or_clamp() {
    let dir = scratch("range");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        Prefs::path(&dir),
        br#"{"version":1,"window_width":5,"window_height":99999,"sidebar_width":9000}"#,
    )
    .unwrap();
    let prefs = Prefs::read(&dir);
    assert_eq!(prefs.window_size, DEFAULT_WINDOW_SIZE);
    assert_eq!(prefs.sidebar_width, 380.);
    assert_eq!(
        prefs.text_scale, 1.0,
        "a file from before P7 opens at actual size"
    );
    assert_eq!(clamp_sidebar_width(1.), 240.);
    assert_eq!(clamp_sidebar_width(f32::NAN), DEFAULT_SIDEBAR_WIDTH);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_selection_restores_only_after_a_short_absence() {
    let id = RecordingId::generate(Timestamp(1_000), 0);
    let prefs = Prefs {
        last_selection: Some(id.clone()),
        quit_at: Some(Timestamp(1_000)),
        ..Prefs::default()
    };
    let window = RESTORE_WITHIN.as_millis() as i64;
    assert_eq!(
        restore_selection(&prefs, Timestamp(1_000 + window - 1)),
        Selection::One(id)
    );
    assert_eq!(
        restore_selection(&prefs, Timestamp(1_000 + window)),
        Selection::None
    );
    assert_eq!(
        restore_selection(&prefs, Timestamp(0)),
        Selection::None,
        "clock moved back"
    );
    assert_eq!(
        restore_selection(&Prefs::default(), Timestamp(1_001)),
        Selection::None
    );
}

#[test]
fn srt_times_are_hours_minutes_seconds_comma_millis() {
    assert_eq!(srt_time(Ms(0)), "00:00:00,000");
    assert_eq!(srt_time(Ms(3_723_004)), "01:02:03,004");
}

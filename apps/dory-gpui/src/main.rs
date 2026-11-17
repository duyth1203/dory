use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;

use dory_gpui::{icons, menus, notify, platform, prefs, quit, show, status_item, system_prefs};

fn main() -> ExitCode {
    let config = match dory::Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("dory: {error}");
            return ExitCode::FAILURE;
        }
    };
    let launch = match show::Launch::from_env() {
        Ok(launch) => launch,
        Err(error) => {
            eprintln!("dory: {error}");
            return ExitCode::FAILURE;
        }
    };
    let dory = match dory::open(&config) {
        Ok(dory) => dory,
        Err(error) => {
            eprintln!("dory: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("dory: data folder {}", config.data_dir.display());
    dory::cleanup_legacy_audio(&config);
    if let Err(error) =
        dory_gpui::seed_meeting_languages(&dory, dory_gpui::onboarding::system_language())
    {
        eprintln!("dory: could not set the meeting languages: {error}");
    }
    let prefs = prefs::load(&config);
    let dory = Rc::new(RefCell::new(Some(dory)));
    let application = gpui_kit::application().with_assets(icons::Assets);
    // A click on the Dock icon with no window open brings the main window back.
    application.on_reopen(|cx| {
        dory_gpui::show_main_window(cx);
    });
    application.run(move |cx| {
        if let Some(appearance) = launch.appearance {
            platform::force_appearance(appearance);
        }
        gpui_kit::init(cx);
        menus::install(cx);
        system_prefs::install(launch.system_prefs, cx);
        let core = dory.borrow_mut().take().expect("the app starts once");
        let state = dory_gpui::new_state(core, prefs.clone(), cx);
        prefs::watch(&state, &config, cx);
        menus::watch(&state, cx);
        quit::install(&state, cx);
        status_item::install(&state, cx);
        notify::install(&state, cx);
        let window = match dory_gpui::open_main_window(cx) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("dory: could not open the window: {error}");
                cx.quit();
                return;
            }
        };
        if let Some(view) = launch.view.clone() {
            let _ = window.update(cx, |_, window, cx| show::run(view, &state, window, cx));
        }
        cx.activate(true);
        eprintln!("first-frame: window opened, pid {}", std::process::id());
    });
    ExitCode::SUCCESS
}

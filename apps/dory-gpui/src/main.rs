use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;

use dory_gpui::{
    consent, glass, icons, menus, notify, platform, prefs, show, status_item, system_prefs,
};
use gpui_kit::{Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size};

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
    let (asker, consent_requests) = dory_core::app::consent_channel();
    let assistants = match dory::serve_assistants(&config, &dory, std::sync::Arc::new(asker)) {
        Ok(server) => Some(server),
        Err(error) => {
            eprintln!("dory: {error}");
            None
        }
    };
    let prefs = prefs::load(&config);
    let dory = Rc::new(RefCell::new(Some(dory)));
    gpui_kit::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            consent::install(assistants, consent_requests, cx);
            if let Some(appearance) = launch.appearance {
                platform::force_appearance(appearance);
            }
            gpui_kit::init(cx);
            menus::install(cx);
            system_prefs::install(launch.system_prefs, cx);
            let (width, height) = prefs.window_size;
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Dory".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(18.), px(18.))),
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(width), px(height)),
                    cx,
                ))),
                window_background: glass::window_background(cx),
                window_min_size: Some(size(
                    px(dory_gpui::prefs::MIN_WINDOW_SIZE.0),
                    px(dory_gpui::prefs::MIN_WINDOW_SIZE.1),
                )),
                ..Default::default()
            };
            let core = dory.borrow_mut().take().expect("the app opens one window");
            let prefs = prefs.clone();
            let config = config.clone();
            let view = launch.view.clone();
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                let (state, shell) = dory_gpui::build(core, prefs, window, cx);
                glass::follow(window, cx);
                platform::print_window_id(window);
                prefs::watch(&state, &config, window, cx);
                menus::watch(&state, cx);
                status_item::install(&state, cx);
                notify::install(&state, cx);
                if let Some(view) = view {
                    show::run(view, &state, window, cx);
                }
                shell
            });
            if let Err(error) = opened {
                eprintln!("dory: could not open the window: {error}");
                cx.quit();
                return;
            }
            cx.activate(true);
            eprintln!("first-frame: window opened, pid {}", std::process::id());
        });
    ExitCode::SUCCESS
}

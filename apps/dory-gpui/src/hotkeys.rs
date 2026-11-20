//! Registers the global shortcuts (`crate::shortcuts`) with macOS, and runs a shortcut's command
//! when it is pressed in any app.
//!
//! Carbon's `RegisterEventHotKey` needs no Accessibility permission, and macOS keeps the keystroke
//! from the app in front. It refuses a shortcut another app already registered; [`taken`] tells
//! Settings which ones it refused. While Settings records a new shortcut, [`suspend`] lifts them
//! all, so pressing a shortcut already set records it instead of starting a recording.
//!
//! The registrations follow `AppState.prefs.shortcuts`. All of the `unsafe` stays in [`carbon`].

use std::collections::BTreeSet;

use gpui_kit::{App, AsyncApp, Entity, Global};

use crate::app::AppState;
use crate::shortcuts::{Bind, Shortcuts};

struct Hotkeys {
    wanted: Shortcuts,
    suspended: bool,
    taken: BTreeSet<Bind>,
    registered: Vec<carbon::Registration>,
}

impl Global for Hotkeys {}

/// Called once by `main`. Registers the saved shortcuts and follows later changes.
pub fn install(app: &Entity<AppState>, cx: &mut App) {
    let (pressed, inbox) = async_channel::unbounded();
    if !carbon::listen(pressed) {
        eprintln!("hotkeys: could not listen for shortcuts");
        return;
    }
    cx.set_global(Hotkeys {
        wanted: app.read(cx).prefs.shortcuts.clone(),
        suspended: false,
        taken: BTreeSet::new(),
        registered: Vec::new(),
    });
    apply(cx);
    cx.observe(app, |app, cx| {
        let wanted = &app.read(cx).prefs.shortcuts;
        if *wanted != cx.global::<Hotkeys>().wanted {
            cx.global_mut::<Hotkeys>().wanted = wanted.clone();
            apply(cx);
        }
    })
    .detach();
    let app = app.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(bind) = inbox.recv().await {
            eprintln!("hotkey: {}", bind.slug());
            cx.update(|cx| {
                if let Some(app) = app.upgrade() {
                    crate::status_item::run(bind.command(), &app, cx);
                }
            });
        }
    })
    .detach();
}

/// Lifts every shortcut while Settings records one, and puts them back after.
pub fn suspend(suspended: bool, cx: &mut App) {
    if let Some(hotkeys) = cx.try_global::<Hotkeys>()
        && hotkeys.suspended != suspended
    {
        cx.global_mut::<Hotkeys>().suspended = suspended;
        apply(cx);
    }
}

/// Whether macOS refused `bind`'s shortcut, because another app holds it.
pub fn taken(bind: Bind, cx: &App) -> bool {
    cx.try_global::<Hotkeys>()
        .is_some_and(|hotkeys| hotkeys.taken.contains(&bind))
}

fn apply(cx: &mut App) {
    let hotkeys = cx.global_mut::<Hotkeys>();
    hotkeys.registered.clear();
    hotkeys.taken.clear();
    if !hotkeys.suspended {
        for (bind, shortcut) in hotkeys.wanted.iter() {
            match carbon::register(bind, shortcut) {
                Some(registration) => hotkeys.registered.push(registration),
                None => {
                    eprintln!("hotkey taken: {}", shortcut.unparse());
                    hotkeys.taken.insert(bind);
                }
            }
        }
    }
    cx.refresh_windows();
}

#[cfg(target_os = "macos")]
mod carbon {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use crate::shortcuts::{Bind, Shortcut};

    type OSStatus = i32;
    type EventTargetRef = *mut c_void;
    type EventHandlerRef = *mut c_void;
    type EventHandlerCallRef = *mut c_void;
    type EventRef = *mut c_void;
    type EventHotKeyRef = *mut c_void;
    type EventHandler = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

    #[repr(C)]
    struct EventTypeSpec {
        event_class: u32,
        event_kind: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct EventHotKeyID {
        signature: u32,
        id: u32,
    }

    const fn four_cc(code: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*code)
    }

    const SIGNATURE: u32 = four_cc(b"DORY");
    const EVENT_CLASS_KEYBOARD: u32 = four_cc(b"keyb");
    const EVENT_HOT_KEY_PRESSED: u32 = 5;
    const PARAM_DIRECT_OBJECT: u32 = four_cc(b"----");
    const TYPE_EVENT_HOT_KEY_ID: u32 = four_cc(b"hkid");
    const NO_ERR: OSStatus = 0;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> EventTargetRef;
        fn InstallEventHandler(
            target: EventTargetRef,
            handler: EventHandler,
            num_types: u32,
            list: *const EventTypeSpec,
            user_data: *mut c_void,
            out_ref: *mut EventHandlerRef,
        ) -> OSStatus;
        fn RegisterEventHotKey(
            key_code: u32,
            modifiers: u32,
            id: EventHotKeyID,
            target: EventTargetRef,
            options: u32,
            out_ref: *mut EventHotKeyRef,
        ) -> OSStatus;
        fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OSStatus;
        fn GetEventParameter(
            event: EventRef,
            name: u32,
            desired_type: u32,
            actual_type: *mut u32,
            buffer_size: usize,
            actual_size: *mut usize,
            data: *mut c_void,
        ) -> OSStatus;
    }

    static PRESSED: OnceLock<async_channel::Sender<Bind>> = OnceLock::new();

    /// One registered shortcut. Dropping it unregisters it.
    pub struct Registration(EventHotKeyRef);

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY: the ref came from a successful RegisterEventHotKey and is dropped once.
            unsafe { UnregisterEventHotKey(self.0) };
        }
    }

    extern "C" fn on_hot_key(_: EventHandlerCallRef, event: EventRef, _: *mut c_void) -> OSStatus {
        let mut id = EventHotKeyID::default();
        // SAFETY: `event` is the hot key event Carbon passes in, and `id` is a buffer of the
        // size asked for.
        let status = unsafe {
            GetEventParameter(
                event,
                PARAM_DIRECT_OBJECT,
                TYPE_EVENT_HOT_KEY_ID,
                std::ptr::null_mut(),
                std::mem::size_of::<EventHotKeyID>(),
                std::ptr::null_mut(),
                (&raw mut id).cast(),
            )
        };
        if status == NO_ERR
            && id.signature == SIGNATURE
            && let Some(bind) = Bind::ALL.get(id.id as usize)
            && let Some(pressed) = PRESSED.get()
        {
            let _ = pressed.try_send(*bind);
        }
        NO_ERR
    }

    /// Installs the one handler for every Dory hot key. Runs once per process.
    pub fn listen(pressed: async_channel::Sender<Bind>) -> bool {
        if PRESSED.set(pressed).is_err() {
            return true;
        }
        let spec = EventTypeSpec {
            event_class: EVENT_CLASS_KEYBOARD,
            event_kind: EVENT_HOT_KEY_PRESSED,
        };
        let mut handler: EventHandlerRef = std::ptr::null_mut();
        // SAFETY: called on the main thread with a handler that lives for the whole process.
        let status = unsafe {
            InstallEventHandler(
                GetApplicationEventTarget(),
                on_hot_key,
                1,
                &spec,
                std::ptr::null_mut(),
                &mut handler,
            )
        };
        status == NO_ERR
    }

    /// `None` when macOS refuses it, most often because another app registered it first.
    pub fn register(bind: Bind, shortcut: &Shortcut) -> Option<Registration> {
        let key_code = shortcut.key_code()?;
        let id = EventHotKeyID {
            signature: SIGNATURE,
            id: Bind::ALL.iter().position(|b| *b == bind)? as u32,
        };
        let mut hot_key: EventHotKeyRef = std::ptr::null_mut();
        // SAFETY: called on the main thread; `hot_key` receives the new ref.
        let status = unsafe {
            RegisterEventHotKey(
                key_code,
                shortcut.carbon_modifiers(),
                id,
                GetApplicationEventTarget(),
                0,
                &mut hot_key,
            )
        };
        (status == NO_ERR && !hot_key.is_null()).then_some(Registration(hot_key))
    }
}

#[cfg(not(target_os = "macos"))]
mod carbon {
    use crate::shortcuts::{Bind, Shortcut};

    pub struct Registration;

    pub fn listen(_: async_channel::Sender<Bind>) -> bool {
        false
    }

    pub fn register(_: Bind, _: &Shortcut) -> Option<Registration> {
        None
    }
}

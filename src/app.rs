use std::cell::RefCell;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWindow};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

use crate::selection;

thread_local! {
    static SELECTION_WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("snipsy must start on the macOS main thread");
    let application = NSApplication::sharedApplication(mtm);
    application.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    application.finishLaunching();

    let manager = GlobalHotKeyManager::new().expect("could not create global hotkey manager");
    let hotkey = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
    manager
        .register(hotkey)
        .expect("could not register Command+Shift+L");

    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id != hotkey.id() || event.state != HotKeyState::Pressed {
            return;
        }

        let mtm = MainThreadMarker::new().expect("hotkey event must run on the macOS main thread");
        if !CGPreflightScreenCaptureAccess() {
            if !CGRequestScreenCaptureAccess() {
                eprintln!(
                    "Screen Recording permission is required; enable the app running Snipsy in System Settings, then try again."
                );
            }
            return;
        }
        SELECTION_WINDOW.with(|selection_window| {
            let mut selection_window = selection_window.borrow_mut();
            if selection_window
                .as_ref()
                .is_some_and(|window| window.isVisible())
            {
                return;
            }
            *selection_window = selection::begin_selection(mtm);
        });
    }));

    println!("Snipsy is ready — press Command+Shift+L to select an area.");
    application.run();
}

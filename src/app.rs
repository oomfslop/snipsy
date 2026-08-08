use std::sync::{Arc, Mutex};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use nekoframe::catui::UiDelegate;
use nekoframe::prelude::*;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEvent};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

use crate::selection;

pub struct SharedState {
    /// The hotkey press's pointer position, in AppKit's bottom-left global
    /// points — the ONE platform query left here, because the winit shell
    /// has no eventless pointer lookup (`get_pointer_position` is None).
    activation: Mutex<Option<LogicalPosition>>,
    /// The monitor chosen for the pending activation, consumed by
    /// `SelectionDelegate::before_frame`.
    pub(crate) pending_overlay: Mutex<Option<Monitor>>,
    event_waker: Mutex<Option<Box<dyn Fn() + Send>>>,
    pub(crate) window_id: Mutex<Option<WindowId>>,
}

pub struct App {
    pub(crate) shared: Arc<SharedState>,
    alloc: &'static IdAlloc,
    _hotkey_manager: GlobalHotKeyManager,
}

impl Application for App {
    fn init(&self, app: &dyn ApplicationController<AppCtx = Self>) {
        let mtm = MainThreadMarker::new().expect("Snipsy must run on the macOS main thread");
        NSApplication::sharedApplication(mtm)
            .setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let waker = app
            .create_event_waker()
            .expect("the shell must provide an application event waker");
        let pending_activation = self.shared.activation.lock().unwrap().is_some();
        *self.shared.event_waker.lock().unwrap() = Some(waker);
        if pending_activation {
            self.shared.event_waker.lock().unwrap().as_ref().unwrap()();
        }
    }

    fn awakened(&self, app: &dyn ApplicationController<AppCtx = Self>) {
        let Some(mouse) = self.shared.activation.lock().unwrap().take() else {
            return;
        };

        let monitors = app.list_monitors();
        let Some(primary_height) = monitors
            .iter()
            .find(|monitor| monitor.primary)
            .map(|monitor| monitor.size.height)
        else {
            return;
        };
        let pointer = LogicalPosition::new(mouse.x, primary_height - mouse.y);
        let Some(monitor) = monitors
            .iter()
            .find(|monitor| monitor.contains(pointer))
            .or_else(|| monitors.iter().find(|monitor| monitor.primary))
            .cloned()
        else {
            return;
        };

        let attrs_position = monitor.position;
        let attrs_size = monitor.size;
        *self.shared.pending_overlay.lock().unwrap() = Some(monitor);

        if let Some(window_id) = *self.shared.window_id.lock().unwrap() {
            app.request_redraw(window_id);
            return;
        }

        let delegate = UiDelegate::with_delegate(
            selection::SelectionState::new(),
            selection::build_root(self.alloc),
            self.alloc,
            selection::SelectionDelegate,
        )
        .with_graphics_opts(GraphicsOpts {
            transparent: true,
            ..GraphicsOpts::default()
        });
        app.spawn_window(
            // Visible from birth (nothing flashes: the surface is transparent
            // until the first present) so the window server's orderFront
            // round trip overlaps gpu init instead of gating the first
            // drawable afterwards.
            WindowAttrs::new()
                .with_title("Snipsy")
                .with_inner_size(attrs_size)
                .with_position(attrs_position)
                .with_resizable(false)
                .with_decorations(false)
                .with_transparent(true)
                .with_always_on_top(true)
                .with_shadow(false),
            Box::new(delegate),
        );
    }

    fn should_exit_after_last_window_closes(&self) -> bool {
        return false;
    }
}

pub fn run() {
    init_logger(LoggerOpts {
        directives: "snipsy=debug,nekoframe=debug,catui_core=debug,catui_components=debug,catui_backends=debug",
        ..LoggerOpts::default()
    });

    std::thread::spawn(TextEngine::preload_system_fonts);

    let manager = GlobalHotKeyManager::new().expect("could not create global hotkey manager");
    let hotkey = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
    manager
        .register(hotkey)
        .expect("could not register Command+Shift+L");

    let shared = Arc::new(SharedState {
        activation: Mutex::new(None),
        pending_overlay: Mutex::new(None),
        event_waker: Mutex::new(None),
        window_id: Mutex::new(None),
    });
    let handler_shared = Arc::clone(&shared);
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id != hotkey.id() || event.state != HotKeyState::Pressed {
            return;
        }
        if !CGPreflightScreenCaptureAccess() {
            if !CGRequestScreenCaptureAccess() {
                eprintln!(
                    "Screen Recording permission is required; enable the app running Snipsy in System Settings, then try again."
                );
            }
            return;
        }
        let mouse = NSEvent::mouseLocation();
        *handler_shared.activation.lock().unwrap() =
            Some(LogicalPosition::new(mouse.x, mouse.y));
        if let Some(waker) = handler_shared.event_waker.lock().unwrap().as_ref() {
            waker();
        }
    }));

    println!("Snipsy is ready — press Command+Shift+L to select an area.");
    nekoframe::app::run(App {
        shared,
        alloc: Box::leak(Box::new(IdAlloc::new())),
        _hotkey_manager: manager,
    });
}

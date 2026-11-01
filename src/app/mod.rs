pub mod cli;
pub mod config;
pub mod content;
pub mod export;
pub mod hotkeys;
pub mod recording;
pub mod storage;
pub mod toolbar_layout;
mod tray;

use crate::{capture, pin, platform, ui};
use anyhow::Result;
use cli::Command;
use gpui_kit::*;
use resvg::usvg::fontdb;
use std::{os::unix::net::UnixListener, path::Path, sync::Arc};

#[derive(Default)]
pub struct AppState {
    pub fonts: Arc<fontdb::Database>,
    pub capture_cursor: bool,
    windows: Vec<(&'static str, AnyWindowHandle)>,
}

impl Global for AppState {}

actions!(
    snipsy,
    [Quit, Hide, ShowPreferences, CloseWindow, MinimizeWindow]
);

pub fn run(command: Option<Command>, data_dir: &Path, listener: UnixListener) {
    let data_dir = data_dir.to_path_buf();
    let application = gpui_kit::application().with_assets(ui::Assets);
    application.on_reopen(|cx| capture::start(cli::SnipOptions::default(), cx));
    application.run(move |cx| {
        gpui_kit::init(cx);
        ui::init_theme(cx);
        cx.set_activation_policy(ActivationPolicy::Accessory);
        platform::use_window_resize_cursors();
        let mut fonts = fontdb::Database::new();
        fonts.load_system_fonts();
        cx.set_global(AppState {
            fonts: Arc::new(fonts),
            capture_cursor: false,
            windows: Vec::new(),
        });
        let (sender, commands) = async_channel::unbounded();
        cx.set_global(recording::Recording::Idle);
        capture::init(cx);
        let errors = [
            config::init(&data_dir, cx),
            cli::listen(listener, sender.clone()),
            hotkeys::init(sender.clone(), cx),
            tray::init(sender, cx),
        ]
        .into_iter()
        .filter_map(|result| result.err().map(|error| format!("{error:#}")))
        .collect::<Vec<_>>();
        pin::init(&data_dir, cx);
        init_actions(cx);
        cx.spawn(async move |cx| {
            while let Ok(command) = commands.recv().await {
                cx.update(|cx| dispatch(command, cx));
            }
        })
        .detach();
        if let Some(command) = command {
            cx.defer(move |cx| dispatch(command, cx));
        }
        if !errors.is_empty() {
            show_error(anyhow::anyhow!(errors.join("\n")));
        }
    });
}

fn init_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| recording::quit(cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &ShowPreferences, cx| dispatch(Command::Preferences, cx));
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-,", ShowPreferences, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
    ]);
    use component::input;
    cx.set_menus([
        Menu::new("Snipsy").items([
            MenuItem::action("Preferences…", ShowPreferences),
            MenuItem::separator(),
            MenuItem::action("Hide Snipsy", Hide),
            MenuItem::action("Quit Snipsy", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", MinimizeWindow),
            MenuItem::action("Close", CloseWindow),
        ]),
    ]);
    cx.on_window_closed(|cx, id| {
        let state = cx.global_mut::<AppState>();
        state.windows.retain(|(_, handle)| handle.window_id() != id);
        if state.windows.is_empty() {
            cx.set_activation_policy(ActivationPolicy::Accessory);
        }
    })
    .detach();
}

fn dispatch(command: Command, cx: &mut App) {
    let result = match command {
        Command::Snip(options) => options.validate().map(|()| capture::start(options, cx)),
        Command::CustomSnip => show_custom_snip(ui::custom_snip::Target::Capture, cx),
        Command::Record => return recording::toggle(cx),
        Command::Paste {
            plain: Some(words), ..
        } => pin::open_text(words.join(" "), cx),
        Command::Paste {
            files: Some(files), ..
        } => return pin::open_paths(files, cx),
        Command::Paste { .. } => pin::paste(cx),
        Command::Whiteboard { color } => return capture::start_whiteboard(color, cx),
        Command::TogglePins => return pin::toggle_hidden(cx),
        Command::ToggleClickThrough => return pin::toggle_click_through(cx),
        Command::Preferences => show_preferences(cx),
        Command::Quit => return recording::quit(cx),
    };
    if let Err(error) = result {
        show_error(error);
    }
}

fn show_preferences(cx: &mut App) -> Result<()> {
    const KEY: &str = "preferences";
    if let Some((_, handle)) = cx
        .global::<AppState>()
        .windows
        .iter()
        .find(|(existing, _)| *existing == KEY)
        && handle
            .update(cx, |_, window, _| {
                platform::make_key_window(window);
                window.activate_window();
            })
            .is_ok()
    {
        cx.activate(true);
        return Ok(());
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(900.), px(600.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Preferences".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(18.), px(20.))),
        }),
        window_background: WindowBackgroundAppearance::Blurred,
        window_min_size: Some(gpui_kit::size(px(280.), px(220.))),
        ..Default::default()
    };
    open_window(KEY, options, cx, |window, cx| {
        cx.new(|cx| ui::preferences::PreferencesView::new(window, cx))
    })?;
    return Ok(());
}

pub fn show_custom_snip(target: ui::custom_snip::Target, cx: &mut App) -> Result<()> {
    const KEY: &str = "custom-snip";
    if let Some((_, handle)) = cx
        .global::<AppState>()
        .windows
        .iter()
        .find(|(existing, _)| *existing == KEY)
        .copied()
    {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(480.), px(230.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Custom Snip".into()),
            ..Default::default()
        }),
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };
    open_window(KEY, options, cx, |window, cx| {
        cx.new(|cx| ui::custom_snip::CustomSnipView::new(target, window, cx))
    })?;
    return Ok(());
}

pub fn open_window<V: Render>(
    key: &'static str,
    options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Result<WindowHandle<base::Root>> {
    cx.set_activation_policy(ActivationPolicy::Regular);
    let handle = cx.open_window(options, |window, cx| {
        let view = build(window, cx);
        cx.new(|cx| base::Root::new(view, window, cx).bg(transparent_black()))
    })?;
    cx.global_mut::<AppState>()
        .windows
        .push((key, handle.into()));
    cx.activate(true);
    return Ok(handle);
}

pub fn show_error(error: anyhow::Error) {
    let message = format!("{error:#}");
    eprintln!("{message}");
    platform::show_error(&message);
}

#[cfg(test)]
mod tests {
    use super::{AppState, init_actions};
    use crate::ui;
    use gpui_kit::{
        Context, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, Window, WindowHandle, div,
    };

    struct TestView {
        focus: FocusHandle,
        standard: bool,
    }

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let root = div().id("root").track_focus(&self.focus).size_full();
            return if self.standard {
                ui::handle_window_actions(root)
            } else {
                root
            }
            .child("window");
        }
    }

    fn open_window(standard: bool, cx: &mut TestAppContext) -> WindowHandle<TestView> {
        let window = cx.add_window(|window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            TestView { focus, standard }
        });
        cx.run_until_parked();
        return window;
    }

    #[gpui_kit::test]
    fn command_w_closes_only_standard_windows(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(AppState::default());
            init_actions(cx);
        });
        let overlay = open_window(false, cx);
        cx.simulate_keystrokes(overlay.into(), "cmd-w");
        cx.run_until_parked();
        assert!(overlay.update(cx, |_, _, _| ()).is_ok());
        let standard = open_window(true, cx);
        cx.simulate_keystrokes(standard.into(), "cmd-w");
        cx.run_until_parked();
        assert!(standard.update(cx, |_, _, _| ()).is_err());
    }
}

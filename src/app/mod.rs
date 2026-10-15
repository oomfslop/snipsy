pub mod cli;
pub mod content;
pub mod export;
pub mod groups;
pub mod history;
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
use std::{collections::HashMap, os::unix::net::UnixListener, path::Path, sync::Arc};

#[derive(Default)]
pub struct AppState {
    pub fonts: Arc<fontdb::Database>,
    pub capture_cursor: bool,
    windows: HashMap<&'static str, AnyWindowHandle>,
}

impl Global for AppState {}

actions!(snipsy, [Quit]);

pub fn run(command: Option<Command>, data_dir: &Path, listener: UnixListener) {
    let data_dir = data_dir.to_path_buf();
    let application = gpui_kit::application().with_assets(ui::Assets);
    application.on_reopen(capture::start);
    application.run(move |cx| {
        gpui_kit::init(cx);
        ui::init_theme(cx);
        platform::initialize_app();
        let mut fonts = fontdb::Database::new();
        fonts.load_system_fonts();
        cx.set_global(AppState {
            fonts: Arc::new(fonts),
            capture_cursor: false,
            windows: HashMap::new(),
        });
        let (sender, commands) = async_channel::unbounded();
        cx.set_global(recording::Recording::Idle);
        history::init(&data_dir, cx);
        capture::init(cx);
        let errors = [
            cli::listen(listener, sender.clone()),
            groups::init(&data_dir, cx),
            toolbar_layout::init(&data_dir, cx),
            hotkeys::init(&data_dir, sender.clone(), cx),
            tray::init(sender, cx),
        ]
        .into_iter()
        .filter_map(|result| result.err().map(|error| format!("{error:#}")))
        .collect::<Vec<_>>();
        pin::init(&data_dir, cx);
        cx.on_action(|_: &Quit, cx| recording::quit(cx));
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
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

fn dispatch(command: Command, cx: &mut App) {
    let result = match command {
        Command::Snip => return capture::start(cx),
        Command::Record => return recording::toggle(cx),
        Command::Paste {
            plain: Some(words), ..
        } => pin::open_text(words.join(" "), cx),
        Command::Paste {
            files: Some(files), ..
        } => return pin::open_paths(files, cx),
        Command::Paste { .. } => pin::paste(cx),
        Command::History => show_window(
            "history",
            "History",
            size(px(760.), px(520.)),
            false,
            cx,
            |window, cx| cx.new(|cx| ui::history::HistoryView::new(window, cx)),
        ),
        Command::ClearHistory => return history::clear(cx),
        Command::Whiteboard { color } => return capture::start_whiteboard(color, cx),
        Command::TogglePins => return pin::toggle_hidden(cx),
        Command::ToggleClickThrough => return pin::toggle_click_through(cx),
        Command::CreateGroup { name } => {
            groups::create((!name.is_empty()).then(|| name.join(" ")), cx)
        }
        Command::SwitchGroup { name } => {
            groups::switch_by_name((!name.is_empty()).then(|| name.join(" ")), cx)
        }
        Command::EmptyGroup => return pin::empty_active_group(cx),
        Command::Groups => show_window(
            "groups",
            "Image Groups",
            size(px(360.), px(420.)),
            false,
            cx,
            |window, cx| cx.new(|cx| ui::groups::GroupsView::new(window, cx)),
        ),
        Command::Preferences => show_window(
            "preferences",
            "Preferences",
            size(px(900.), px(600.)),
            true,
            cx,
            |window, cx| cx.new(|cx| ui::preferences::PreferencesView::new(window, cx)),
        ),
        Command::Quit => return recording::quit(cx),
    };
    if let Err(error) = result {
        show_error(error);
    }
}

fn show_window<V: Render>(
    key: &'static str,
    title: &'static str,
    size: Size<Pixels>,
    translucent: bool,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Result<()> {
    cx.activate(true);
    if let Some(handle) = cx.global::<AppState>().windows.get(key)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: translucent,
            traffic_light_position: translucent.then(|| point(px(18.), px(20.))),
        }),
        window_background: if translucent {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Opaque
        },
        window_min_size: Some(gpui_kit::size(px(280.), px(220.))),
        ..Default::default()
    };
    let handle = cx.open_window(options, |window, cx| {
        let view = build(window, cx);
        cx.new(|cx| base::Root::new(view, window, cx).bg(transparent_black()))
    })?;
    cx.global_mut::<AppState>()
        .windows
        .insert(key, handle.into());
    return Ok(());
}

pub fn show_error(error: anyhow::Error) {
    let message = format!("{error:#}");
    eprintln!("{message}");
    platform::show_error(&message);
}

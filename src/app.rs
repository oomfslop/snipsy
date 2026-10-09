use crate::{
    editor::Screenshot, history::History, icons::Icons, pin, platform, raster,
    selection::SelectionState,
};
use base64::Engine;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui::{prelude::*, *};
use image::{Rgba, RgbaImage};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct AppState {
    _hotkeys: GlobalHotKeyManager,
    pub fonts: Arc<resvg::usvg::fontdb::Database>,
    pub selection: Option<WindowHandle<SelectionState>>,
    pub cached_selection: Option<WindowHandle<SelectionState>>,
    pub history_window: Option<WindowHandle<History>>,
    pub pins: Vec<WindowHandle<pin::Pin>>,
    pub closed_pins: Vec<pin::ClosedPin>,
    pub pins_hidden: bool,
    pub capture_history: Vec<Screenshot>,
    pub history: Vec<(Arc<RgbaImage>, Arc<RenderImage>)>,
    history_persist: async_channel::Sender<Arc<RgbaImage>>,
    pin_persist: async_channel::Sender<pin::PinBackupCommand>,
    pub error: Option<String>,
}
impl Global for AppState {}
#[derive(Clone, Copy, Debug)]
enum Command {
    Capture,
    Paste,
    ToggleClickThrough,
    TogglePins,
}

#[derive(Debug, Default, PartialEq)]
pub enum StartupAction {
    #[default]
    None,
    Capture,
    Paste,
    Plain(String),
    Files(Vec<PathBuf>),
    History,
}

actions!(
    snipsy,
    [
        CaptureScreen,
        PinClipboard,
        ShowHistory,
        TogglePinnedImages,
        QuitSnipsy
    ]
);

pub fn parse_startup_action(
    args: impl IntoIterator<Item = OsString>,
) -> Result<StartupAction, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    let Some(command) = args.first().and_then(|arg| arg.to_str()) else {
        return Ok(StartupAction::None);
    };
    return match command {
        "snip" => (args.len() == 1)
            .then_some(StartupAction::Capture)
            .ok_or("Usage: snipsy snip".into()),
        "paste"
            if args.len() == 1
                || args.get(1).and_then(|arg| arg.to_str()) == Some("--clipboard") =>
        {
            (args.len() <= 2)
                .then_some(StartupAction::Paste)
                .ok_or("Usage: snipsy paste [--clipboard]".into())
        }
        "paste" if args.get(1).and_then(|arg| arg.to_str()) == Some("--plain") => (!args[2..]
            .is_empty())
        .then(|| {
            StartupAction::Plain(
                args[2..]
                    .iter()
                    .map(|arg| arg.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        })
        .ok_or("Usage: snipsy paste --plain TEXT".into()),
        "paste" if args.get(1).and_then(|arg| arg.to_str()) == Some("--files") => (!args[2..]
            .is_empty())
        .then(|| StartupAction::Files(args[2..].iter().map(PathBuf::from).collect()))
        .ok_or("Usage: snipsy paste --files FILE...".into()),
        "history" if args.len() == 1 => Ok(StartupAction::History),
        _ => Err(
            "Usage: snipsy [snip | paste [--clipboard | --plain TEXT | --files FILE...] | history]"
                .into(),
        ),
    };
}

pub fn run(startup: StartupAction) {
    let application = Application::new().with_assets(Icons);
    application.on_reopen(start_capture);
    application.run(move |cx| {
        platform::initialize_app();
        let mut fonts = resvg::usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        let (data_directory, mut hotkey_error) = match platform::get_app_data_dir() {
            Ok(directory) => (Some(directory), None),
            Err(error) => (None, Some(error)),
        };
        let (history_persist, history_receiver) = async_channel::unbounded();
        let (pin_persist, pin_receiver) = async_channel::unbounded();
        let hotkeys = GlobalHotKeyManager::new().expect("could not create global shortcuts");
        let capture = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyA);
        let lens = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
        let paste = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyP);
        let click_through = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyX);
        let toggle_pins_hotkey = HotKey::new(Some(Modifiers::SHIFT), Code::F3);
        if startup == StartupAction::None {
            for hotkey in [&capture, &lens, &paste, &click_through, &toggle_pins_hotkey] {
                if let Err(error) = hotkeys.register(*hotkey) {
                    eprintln!("could not register shortcut: {error}");
                    hotkey_error = Some(format!("Could not register a shortcut: {error}"));
                }
            }
        }
        let (sender, receiver) = async_channel::unbounded();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state != HotKeyState::Pressed {
                return;
            }
            let command = if event.id == capture.id() || event.id == lens.id() {
                Command::Capture
            } else if event.id == paste.id() {
                Command::Paste
            } else if event.id == click_through.id() {
                Command::ToggleClickThrough
            } else if event.id == toggle_pins_hotkey.id() {
                Command::TogglePins
            } else {
                return;
            };
            #[cfg(debug_assertions)]
            eprintln!("Snipsy hotkey: {command:?}");
            if let Err(error) = sender.try_send(command) {
                eprintln!("could not queue shortcut: {error}");
            }
        }));
        cx.set_global(AppState {
            _hotkeys: hotkeys,
            fonts: Arc::new(fonts),
            selection: None,
            cached_selection: None,
            history_window: None,
            pins: Vec::new(),
            closed_pins: Vec::new(),
            pins_hidden: false,
            capture_history: Vec::new(),
            history: Vec::new(),
            history_persist,
            pin_persist,
            error: hotkey_error,
        });
        if let Some(directory) = data_directory {
            let history_directory = directory.join("history");
            let persist_directory = history_directory.clone();
            cx.background_executor()
                .spawn(async move {
                    while let Ok(image) = history_receiver.recv().await {
                        if let Err(error) = write_history(&persist_directory, &image) {
                            eprintln!("could not persist history: {error}");
                        }
                    }
                })
                .detach();
            let load = cx
                .background_executor()
                .spawn(async move { load_history(&history_directory) });
            cx.spawn(async move |cx| {
                let loaded = load.await;
                let _ = cx.update(|cx| {
                    let state = cx.global_mut::<AppState>();
                    match loaded {
                        Ok(images) => {
                            for image in images {
                                if state.history.len() == 10 {
                                    break;
                                }
                                let preview = raster::create_render_image(
                                    &image::imageops::thumbnail(&*image, 220, 140),
                                );
                                state.history.push((image, preview));
                            }
                        }
                        Err(error) => eprintln!("could not load history: {error}"),
                    }
                    cx.refresh_windows();
                });
            })
            .detach();
            let pin_directory = directory.join("pins");
            let persist_directory = pin_directory.clone();
            cx.background_executor()
                .spawn(async move {
                    let mut images = HashMap::new();
                    while let Ok(command) = pin_receiver.recv().await {
                        let result = match command {
                            pin::PinBackupCommand::Save(pin) => {
                                write_pin_backup(&persist_directory, &pin, &mut images)
                            }
                            pin::PinBackupCommand::Remove(id) => {
                                images.remove(&id);
                                remove_pin_backup(&persist_directory, id)
                            }
                        };
                        if let Err(error) = result {
                            eprintln!("could not persist pin: {error}");
                        }
                    }
                })
                .detach();
            let load = cx
                .background_executor()
                .spawn(async move { load_pin_backups(&pin_directory) });
            cx.spawn(async move |cx| {
                let loaded = load.await;
                let _ = cx.update(|cx| match loaded {
                    Ok(pins) => {
                        for pin in pins {
                            pin::restore_backup(pin, cx);
                        }
                    }
                    Err(error) => eprintln!("could not load pins: {error}"),
                });
            })
            .detach();
        }
        cx.on_action(|_: &CaptureScreen, cx| start_capture(cx));
        cx.on_action(|_: &PinClipboard, cx| paste_image(cx));
        cx.on_action(|_: &ShowHistory, cx| show_history(cx));
        cx.on_action(|_: &TogglePinnedImages, cx| toggle_pins(cx));
        cx.on_action(|_: &QuitSnipsy, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", QuitSnipsy, None)]);
        cx.set_menus(vec![Menu {
            name: "Snipsy".into(),
            items: vec![
                MenuItem::action("New screenshot", CaptureScreen),
                MenuItem::action("Pin clipboard", PinClipboard),
                MenuItem::action("History", ShowHistory),
                MenuItem::action("Show/hide pins", TogglePinnedImages),
                MenuItem::separator(),
                MenuItem::action("Quit Snipsy", QuitSnipsy),
            ],
        }]);
        cx.spawn(async move |cx| {
            while let Ok(command) = receiver.recv().await {
                if let Err(error) = cx.update(|cx| match command {
                    Command::Capture => start_capture(cx),
                    Command::Paste => paste_image(cx),
                    Command::ToggleClickThrough => toggle_pin_click_through(cx),
                    Command::TogglePins => toggle_pins(cx),
                }) {
                    eprintln!("could not dispatch shortcut: {error}");
                }
            }
        })
        .detach();
        cx.defer(move |cx| match startup {
            StartupAction::None => {}
            StartupAction::Capture => start_capture(cx),
            StartupAction::Paste => paste_image(cx),
            StartupAction::Plain(text) => pin_text(text, cx),
            StartupAction::Files(paths) => open_paths(paths, cx),
            StartupAction::History => show_history(cx),
        });
        if let Some(error) = cx.global::<AppState>().error.clone() {
            cx.defer(move |cx| show_error(error, cx));
        }
    });
}

fn show_history(cx: &mut App) {
    if let Some(handle) = cx.global::<AppState>().history_window {
        if handle
            .update(cx, |_, window, cx| {
                cx.activate(true);
                window.activate_window();
            })
            .is_ok()
        {
            return;
        }
    }
    cx.activate(true);
    match cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(760.0), px(520.0)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("History".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(280.0), px(220.0))),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| History::new(window, cx)),
    ) {
        Ok(handle) => cx.global_mut::<AppState>().history_window = Some(handle),
        Err(error) => show_error(error.to_string(), cx),
    }
}

fn toggle_pins(cx: &mut App) {
    let visible = cx.global::<AppState>().pins_hidden;
    set_pins_visible(visible, cx);
}

pub fn set_pins_visible(visible: bool, cx: &mut App) {
    let pins = std::mem::take(&mut cx.global_mut::<AppState>().pins);
    let live = pins
        .into_iter()
        .filter(|pin| {
            pin.update(cx, |_, window, _| {
                platform::set_window_visible(window, visible)
            })
            .is_ok()
        })
        .collect();
    let state = cx.global_mut::<AppState>();
    state.pins = live;
    state.pins_hidden = !visible;
}

pub fn persist_pin(command: pin::PinBackupCommand, cx: &mut App) {
    let _ = cx.global::<AppState>().pin_persist.try_send(command);
}

fn toggle_pin_click_through(cx: &mut App) {
    let pins = std::mem::take(&mut cx.global_mut::<AppState>().pins);
    let mut live = Vec::with_capacity(pins.len());
    let mut target = None;
    for pin in pins {
        match pin.update(cx, |_, window, _| platform::is_window_under_pointer(window)) {
            Ok(true) => target = Some(pin),
            Ok(false) => live.push(pin),
            Err(_) => {}
        }
    }
    if let Some(pin) = target {
        let _ = pin.update(cx, |state, window, cx| {
            state.toggle_click_through(window, cx)
        });
        live.push(pin);
    } else {
        for pin in &live {
            let _ = pin.update(cx, |state, window, cx| {
                state.disable_click_through(window, cx)
            });
        }
    }
    cx.global_mut::<AppState>().pins = live;
}

pub fn start_capture(cx: &mut App) {
    if let Some(handle) = cx.global::<AppState>().selection {
        if handle
            .update(cx, |state, window, cx| state.activate(window, cx))
            .is_ok()
        {
            return;
        }
        cx.global_mut::<AppState>().selection = None;
    }
    let bounds = match platform::get_capture_bounds() {
        Ok(bounds) => bounds,
        Err(error) => {
            show_error(error, cx);
            return;
        }
    };
    let (sender, receiver) = async_channel::bounded(1);
    platform::capture_screen(bounds, sender);
    let capture = cx.background_executor().spawn(async move {
        match receiver.recv().await {
            Ok(Ok(screenshot)) => {
                let background = raster::create_render_image(&screenshot.image);
                Ok(Ok((screenshot, background)))
            }
            Ok(Err(error)) => Ok(Err(error)),
            Err(error) => Err(error),
        }
    });
    let cached = cx.global::<AppState>().cached_selection.filter(|handle| {
        handle
            .update(cx, |state, window, cx| {
                platform::configure_overlay(window, bounds, false);
                state.reset_for_capture(bounds, window, cx);
            })
            .is_ok()
    });
    let handle = if let Some(handle) = cached {
        handle
    } else {
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(bounds.pos.x), px(bounds.pos.y)),
                    size(px(bounds.size.x), px(bounds.size.y)),
                ))),
                titlebar: None,
                kind: WindowKind::Normal,
                show: false,
                focus: false,
                window_background: WindowBackgroundAppearance::Transparent,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            },
            |window, cx| {
                platform::configure_overlay(window, bounds, false);
                cx.new(|cx| SelectionState::new(bounds, window, cx))
            },
        ) {
            Ok(handle) => handle,
            Err(error) => {
                show_error(error.to_string(), cx);
                return;
            }
        }
    };
    cx.global_mut::<AppState>().selection = Some(handle);
    cx.global_mut::<AppState>().cached_selection = Some(handle);
    cx.spawn(async move |cx| {
        let result = capture.await;
        let _ = cx.update(|cx| match result {
            Ok(Ok((screenshot, background))) => {
                let _ = handle.update(cx, |state, window, cx| {
                    state.complete_capture(screenshot, background, window, cx);
                });
            }
            Ok(Err(error)) => {
                if handle
                    .update(cx, |state, window, cx| state.dismiss(window, cx))
                    .is_ok()
                {
                    show_error(error, cx);
                }
            }
            Err(error) => {
                if handle
                    .update(cx, |state, window, cx| state.dismiss(window, cx))
                    .is_ok()
                {
                    show_error(error.to_string(), cx);
                }
            }
        });
    })
    .detach();
}

pub fn paste_image(cx: &mut App) {
    let clipboard_change_count = platform::get_clipboard_change_count();
    let closed = {
        let state = cx.global_mut::<AppState>();
        should_restore_pin(
            state
                .closed_pins
                .last()
                .map(pin::ClosedPin::clipboard_change_count),
            clipboard_change_count,
        )
        .then(|| state.closed_pins.pop())
        .flatten()
    };
    if let Some(closed) = closed {
        pin::restore_pin(closed, cx);
        return;
    }
    let paths = platform::get_clipboard_paths();
    if !paths.is_empty() {
        open_paths(paths, cx);
        return;
    }
    let Some(item) = cx.read_from_clipboard() else {
        show_error("Clipboard is empty".into(), cx);
        return;
    };
    let image = item.entries().iter().find_map(|entry| {
        if let ClipboardEntry::Image(image) = entry {
            Some(image.clone())
        } else {
            None
        }
    });
    match image {
        Some(image) => match image::load_from_memory(&image.bytes) {
            Ok(image) => pin::open_pin(Arc::new(image.to_rgba8()), None, cx),
            Err(error) => show_error(error.to_string(), cx),
        },
        None => {
            let Some(text) = item.text() else {
                show_error("Clipboard has no supported content".into(), cx);
                return;
            };
            pin_text(text, cx);
        }
    }
}

pub struct PinContent {
    pub image: Arc<RgbaImage>,
    pub text: Option<String>,
}

pub fn load_clipboard_content(cx: &mut App) -> Result<Task<Result<PinContent, String>>, String> {
    let fonts = cx.global::<AppState>().fonts.clone();
    if let Some(path) = platform::get_clipboard_paths().into_iter().next() {
        return Ok(cx.background_executor().spawn(async move {
            return load_path(&path, &fonts).map(|(image, text)| PinContent { image, text });
        }));
    }
    let item = cx.read_from_clipboard().ok_or("Clipboard is empty")?;
    if let Some(image) = item.entries().iter().find_map(|entry| {
        if let ClipboardEntry::Image(image) = entry {
            Some(image.clone())
        } else {
            None
        }
    }) {
        return Ok(cx.background_executor().spawn(async move {
            return image::load_from_memory(&image.bytes)
                .map(|image| PinContent {
                    image: Arc::new(image.to_rgba8()),
                    text: None,
                })
                .map_err(|error| error.to_string());
        }));
    }
    let text = item.text().ok_or("Clipboard has no supported content")?;
    return Ok(cx.background_executor().spawn(async move {
        if let Some(color) = parse_color(&text) {
            return Ok(PinContent {
                image: Arc::new(RgbaImage::from_pixel(240, 160, Rgba(color))),
                text: None,
            });
        }
        return raster::render_text_image(&text, &fonts).map(|image| PinContent {
            image: Arc::new(image),
            text: Some(text),
        });
    }));
}

fn pin_text(text: String, cx: &mut App) {
    if let Some(color) = parse_color(&text) {
        pin::open_pin(
            Arc::new(RgbaImage::from_pixel(240, 160, Rgba(color))),
            None,
            cx,
        );
        return;
    }
    let fonts = cx.global::<AppState>().fonts.clone();
    match raster::render_text_image(&text, &fonts) {
        Ok(image) => pin::open_text_pin(Arc::new(image), text, cx),
        Err(error) => show_error(error, cx),
    }
}

fn should_restore_pin(closed_change_count: Option<i64>, clipboard_change_count: i64) -> bool {
    return closed_change_count == Some(clipboard_change_count);
}

pub fn open_paths(paths: Vec<PathBuf>, cx: &mut App) {
    let fonts = cx.global::<AppState>().fonts.clone();
    let load = cx.background_executor().spawn(async move {
        return paths
            .into_iter()
            .map(|path| load_path(&path, &fonts))
            .collect::<Vec<_>>();
    });
    cx.spawn(async move |cx| {
        let results = load.await;
        let _ = cx.update(|cx| {
            let mut errors = Vec::new();
            for result in results {
                match result {
                    Ok((image, Some(text))) => pin::open_text_pin(image, text, cx),
                    Ok((image, None)) => pin::open_pin(image, None, cx),
                    Err(error) => errors.push(error),
                }
            }
            if !errors.is_empty() {
                show_error(errors.join("\n"), cx);
            }
        });
    })
    .detach();
}

fn load_path(
    path: &Path,
    fonts: &Arc<resvg::usvg::fontdb::Database>,
) -> Result<(Arc<RgbaImage>, Option<String>), String> {
    if let Ok(image) = image::open(path) {
        return Ok((Arc::new(image.to_rgba8()), None));
    }
    let text = path.to_string_lossy().into_owned();
    return raster::render_text_image(&text, fonts)
        .map(|image| (Arc::new(image), Some(text)))
        .map_err(|error| format!("{}: {error}", path.display()));
}

fn parse_color(text: &str) -> Option<[u8; 4]> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        let mut color = [0, 0, 0, 255];
        match hex.len() {
            3 | 4 => {
                for (index, value) in hex.bytes().enumerate() {
                    color[index] = (value as char).to_digit(16)? as u8 * 17;
                }
            }
            6 | 8 => {
                for (index, value) in hex.as_bytes().chunks_exact(2).enumerate() {
                    color[index] = u8::from_str_radix(std::str::from_utf8(value).ok()?, 16).ok()?;
                }
            }
            _ => return None,
        }
        return Some(color);
    }
    let values = text
        .strip_prefix("rgb(")
        .and_then(|text| text.strip_suffix(')'))
        .unwrap_or(text)
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.len() != 3 {
        return None;
    }
    let mut color = [0, 0, 0, 255];
    if values.iter().any(|value| value.contains(['.', 'e', 'E'])) {
        for (channel, value) in color.iter_mut().zip(values) {
            let value = value.parse::<f32>().ok()?;
            if !(0.0..=1.0).contains(&value) {
                return None;
            }
            *channel = (value * 255.0).round() as u8;
        }
    } else {
        for (channel, value) in color.iter_mut().zip(values) {
            *channel = value.parse().ok()?;
        }
    }
    return Some(color);
}

pub fn copy_image(image: &RgbaImage, cx: &mut App) -> Result<(), String> {
    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
        ImageFormat::Png,
        raster::encode_png(image)?,
    )));
    return Ok(());
}

pub fn remember_capture(image: Arc<RgbaImage>, cx: &mut App) {
    let preview = raster::create_render_image(&image::imageops::thumbnail(&*image, 220, 140));
    let state = cx.global_mut::<AppState>();
    let _ = state.history_persist.try_send(image.clone());
    state.history.insert(0, (image, preview));
    let removed = if state.history.len() > 10 {
        state.history.pop()
    } else {
        None
    };
    if let Some((_, preview)) = removed {
        cx.drop_image(preview, None);
    }
    cx.refresh_windows();
}

pub fn remember_capture_frame(screenshot: Screenshot, cx: &mut App) {
    insert_capture_history(&mut cx.global_mut::<AppState>().capture_history, screenshot);
}

fn insert_capture_history(history: &mut Vec<Screenshot>, screenshot: Screenshot) {
    history.retain(|entry| !Arc::ptr_eq(&entry.image, &screenshot.image));
    history.insert(0, screenshot);
    history.truncate(10);
}

pub fn save_image(image: Arc<RgbaImage>, capture: Option<Screenshot>, cx: &mut App) {
    let suggested = format!(
        "Snipsy-{}.png",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );
    let receiver = platform::prompt_save_image(&suggested);
    cx.spawn(async move |cx| match receiver.recv().await {
        Ok(Ok(Some(path))) => {
            let saved_image = image.clone();
            let result = cx
                .background_executor()
                .spawn(async move { write_image(&image, &path) })
                .await;
            if let Err(error) = result {
                let _ = cx.update(|cx| show_error(error, cx));
            } else {
                let _ = cx.update(|cx| {
                    if let Some(capture) = capture {
                        remember_capture_frame(capture, cx);
                    }
                    remember_capture(saved_image, cx);
                });
            }
        }
        Ok(Ok(None)) => {}
        Ok(Err(error)) => {
            let _ = cx.update(|cx| show_error(error.to_string(), cx));
        }
        Err(error) => {
            let _ = cx.update(|cx| show_error(error.to_string(), cx));
        }
    })
    .detach();
}

fn write_image(image: &RgbaImage, path: &Path) -> Result<(), String> {
    let path = if path.extension().is_none() {
        path.with_extension("png")
    } else {
        path.to_path_buf()
    };
    let format = image::ImageFormat::from_path(&path).map_err(|error| error.to_string())?;
    if !matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg) {
        return Err("Choose a .png or .jpg filename".into());
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))
        .map_err(|error| error.to_string())?;
    if format == image::ImageFormat::Jpeg {
        image::DynamicImage::ImageRgba8(image.clone())
            .to_rgb8()
            .write_to(file.as_file_mut(), format)
            .map_err(|error| error.to_string())?;
    } else {
        image
            .write_to(file.as_file_mut(), format)
            .map_err(|error| error.to_string())?;
    }
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    file.persist(path).map_err(|error| error.to_string())?;
    return Ok(());
}

fn write_history(directory: &Path, image: &RgbaImage) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let mut id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    while directory.join(format!("{id}.png")).exists() {
        id += 1;
    }
    write_image(image, &directory.join(format!("{id}.png")))?;
    for (_, path) in get_history_paths(directory)?.into_iter().skip(10) {
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    return Ok(());
}

fn load_history(directory: &Path) -> Result<Vec<Arc<RgbaImage>>, String> {
    let mut images = Vec::new();
    for (_, path) in get_history_paths(directory)?.into_iter().take(10) {
        match image::open(&path) {
            Ok(image) => images.push(Arc::new(image.to_rgba8())),
            Err(error) => eprintln!("could not load history entry {}: {error}", path.display()),
        }
    }
    return Ok(images);
}

fn get_history_paths(directory: &Path) -> Result<Vec<(u128, PathBuf)>, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_stem()?.to_str()?.parse::<u128>().ok()?;
            (path.extension().and_then(|extension| extension.to_str()) == Some("png"))
                .then_some((id, path))
        })
        .collect::<Vec<_>>();
    paths.sort_unstable_by(|left, right| right.0.cmp(&left.0));
    return Ok(paths);
}

fn write_pin_backup(
    directory: &Path,
    pin: &pin::PinBackup,
    images: &mut HashMap<u128, Arc<RgbaImage>>,
) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    if images
        .get(&pin.id)
        .is_none_or(|image| !Arc::ptr_eq(image, &pin.image))
    {
        write_image(&pin.image, &directory.join(format!("{}.png", pin.id)))?;
        images.insert(pin.id, pin.image.clone());
    }
    let thumbnail = pin
        .thumbnail_size
        .map(|size| (f32::from(size.width), f32::from(size.height)))
        .unwrap_or_default();
    let text = pin
        .text
        .as_ref()
        .map(|text| base64::engine::general_purpose::STANDARD.encode(text.as_bytes()))
        .unwrap_or_default();
    let metadata = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        f32::from(pin.bounds.origin.x),
        f32::from(pin.bounds.origin.y),
        f32::from(pin.bounds.size.width),
        f32::from(pin.bounds.size.height),
        pin.opacity,
        f32::from(pin.original_size.width),
        f32::from(pin.original_size.height),
        thumbnail.0,
        thumbnail.1,
        text,
    );
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    file.write_all(metadata.as_bytes())
        .map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    file.persist(directory.join(format!("{}.meta", pin.id)))
        .map_err(|error| error.to_string())?;
    return Ok(());
}

fn remove_pin_backup(directory: &Path, id: u128) -> Result<(), String> {
    for extension in ["png", "meta"] {
        match std::fs::remove_file(directory.join(format!("{id}.{extension}"))) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    return Ok(());
}

fn load_pin_backups(directory: &Path) -> Result<Vec<pin::PinBackup>, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_stem()?.to_str()?.parse::<u128>().ok()?;
            (path.extension().and_then(|extension| extension.to_str()) == Some("meta"))
                .then_some((id, path))
        })
        .collect::<Vec<_>>();
    paths.sort_unstable_by_key(|(id, _)| *id);
    let mut pins = Vec::new();
    for (id, path) in paths {
        let result = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|metadata| parse_pin_backup(id, &metadata, directory));
        match result {
            Ok(pin) => pins.push(pin),
            Err(error) => eprintln!("could not load pin {id}: {error}"),
        }
    }
    return Ok(pins);
}

fn parse_pin_backup(id: u128, metadata: &str, directory: &Path) -> Result<pin::PinBackup, String> {
    let fields = metadata.splitn(10, '\t').collect::<Vec<_>>();
    if fields.len() != 10 {
        return Err("invalid metadata".into());
    }
    let numbers = fields[..9]
        .iter()
        .map(|field| field.parse::<f32>().map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    if numbers.iter().any(|number| !number.is_finite())
        || numbers[2] < 40.0
        || numbers[3] < 40.0
        || !(0.2..=1.0).contains(&numbers[4])
        || numbers[5] <= 0.0
        || numbers[6] <= 0.0
        || !((numbers[7] == 0.0 && numbers[8] == 0.0) || (numbers[7] > 0.0 && numbers[8] > 0.0))
    {
        return Err("invalid dimensions or opacity".into());
    }
    let image = image::open(directory.join(format!("{id}.png")))
        .map_err(|error| error.to_string())?
        .to_rgba8();
    if image.width() == 0 || image.height() == 0 {
        return Err("empty image".into());
    }
    let text = if fields[9].is_empty() {
        None
    } else {
        Some(
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(fields[9])
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?
            .into(),
        )
    };
    return Ok(pin::PinBackup {
        id,
        image: Arc::new(image),
        bounds: Bounds::new(
            point(px(numbers[0]), px(numbers[1])),
            size(px(numbers[2]), px(numbers[3])),
        ),
        opacity: numbers[4],
        original_size: size(px(numbers[5]), px(numbers[6])),
        thumbnail_size: (numbers[7] > 0.0).then_some(size(px(numbers[7]), px(numbers[8]))),
        text,
    });
}

pub fn show_error(error: String, cx: &mut App) {
    eprintln!("{error}");
    cx.global_mut::<AppState>().error = Some(error.clone());
    platform::show_error(&error);
}

#[cfg(test)]
mod tests {
    use super::{
        StartupAction, get_history_paths, insert_capture_history, load_history, load_path,
        load_pin_backups, parse_color, parse_pin_backup, parse_startup_action, remove_pin_backup,
        should_restore_pin, write_history, write_image, write_pin_backup,
    };
    use crate::{editor::Screenshot, geometry::Rect, pin::PinBackup};
    use glam::{Vec2, vec2};
    use gpui::{Bounds, point, px, size};
    use image::{Rgba, RgbaImage};
    use std::path::PathBuf;

    #[test]
    fn save_replaces_png_and_rejects_unsupported_formats_without_truncating() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("capture.png");
        std::fs::write(&path, b"old file").unwrap();
        let image = RgbaImage::from_pixel(3, 2, Rgba([255, 50, 80, 128]));
        write_image(&image, &path).unwrap();
        assert_eq!(image::open(&path).unwrap().to_rgba8(), image);
        let unsupported = directory.path().join("capture.txt");
        std::fs::write(&unsupported, b"keep this").unwrap();
        assert!(write_image(&image, &unsupported).is_err());
        assert_eq!(std::fs::read(&unsupported).unwrap(), b"keep this");
        let jpeg = directory.path().join("capture.jpg");
        write_image(&image, &jpeg).unwrap();
        assert_eq!(image::open(jpeg).unwrap().width(), 3);
    }

    #[test]
    fn clipboard_colors_accept_hex_integer_and_unit_rgb() {
        assert_eq!(parse_color("#0f8"), Some([0, 255, 136, 255]));
        assert_eq!(parse_color("#0102ff80"), Some([1, 2, 255, 128]));
        assert_eq!(parse_color("rgb(255, 2, 3)"), Some([255, 2, 3, 255]));
        assert_eq!(parse_color("1.0 0 0.5"), Some([255, 0, 128, 255]));
        assert_eq!(parse_color("1 0 0"), Some([1, 0, 0, 255]));
        assert_eq!(parse_color("256, 0, 0"), None);
        assert_eq!(parse_color("1.1 0.0 0.0"), None);
        assert_eq!(parse_color("not a color"), None);
    }

    #[test]
    fn paths_load_images_or_render_their_names() {
        let directory = tempfile::tempdir().unwrap();
        let image_path = directory.path().join("image.png");
        RgbaImage::from_pixel(2, 3, Rgba([1, 2, 3, 255]))
            .save(&image_path)
            .unwrap();
        let fonts = std::sync::Arc::new(resvg::usvg::fontdb::Database::new());
        let (image, text) = load_path(&image_path, &fonts).unwrap();
        assert_eq!(image.dimensions(), (2, 3));
        assert_eq!(text, None);

        let file_path = directory.path().join("notes.txt");
        std::fs::write(&file_path, "notes").unwrap();
        let mut fonts = resvg::usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        let (_, text) = load_path(&file_path, &std::sync::Arc::new(fonts)).unwrap();
        assert_eq!(text.as_deref(), Some(file_path.to_str().unwrap()));
    }

    #[test]
    fn closed_pins_restore_only_before_the_clipboard_changes() {
        assert!(should_restore_pin(Some(7), 7));
        assert!(!should_restore_pin(Some(7), 8));
        assert!(!should_restore_pin(None, 7));
    }

    #[test]
    fn capture_history_deduplicates_frames_and_stays_bounded() {
        let image = std::sync::Arc::new(RgbaImage::new(1, 1));
        let mut history = Vec::new();
        for index in 0..12 {
            insert_capture_history(
                &mut history,
                Screenshot {
                    image: if index == 11 {
                        image.clone()
                    } else {
                        std::sync::Arc::new(RgbaImage::new(1, 1))
                    },
                    bounds: Rect {
                        pos: Vec2::ZERO,
                        size: vec2(1.0, 1.0),
                    },
                    windows: Vec::new(),
                },
            );
        }
        insert_capture_history(
            &mut history,
            Screenshot {
                image: image.clone(),
                bounds: Rect {
                    pos: Vec2::ZERO,
                    size: vec2(1.0, 1.0),
                },
                windows: Vec::new(),
            },
        );
        assert_eq!(history.len(), 10);
        assert!(std::sync::Arc::ptr_eq(&history[0].image, &image));
        assert_eq!(
            history
                .iter()
                .filter(|entry| std::sync::Arc::ptr_eq(&entry.image, &image))
                .count(),
            1
        );
    }

    #[test]
    fn persistent_history_round_trips_and_removes_stale_entries() {
        let directory = tempfile::tempdir().unwrap();
        let first = RgbaImage::from_pixel(2, 1, Rgba([1, 2, 3, 255]));
        let second = RgbaImage::from_pixel(1, 2, Rgba([4, 5, 6, 255]));
        write_history(directory.path(), &first).unwrap();
        write_history(directory.path(), &second).unwrap();
        let loaded = load_history(directory.path()).unwrap();
        assert_eq!(&*loaded[0], &second);
        assert_eq!(&*loaded[1], &first);

        for value in 0..10 {
            write_history(
                directory.path(),
                &RgbaImage::from_pixel(1, 1, Rgba([value, 0, 0, 255])),
            )
            .unwrap();
        }
        assert_eq!(get_history_paths(directory.path()).unwrap().len(), 10);
    }

    #[test]
    fn pin_backups_round_trip_text_geometry_and_validation() {
        let directory = tempfile::tempdir().unwrap();
        let image = std::sync::Arc::new(RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 255])));
        let pin = PinBackup {
            id: 42,
            image: image.clone(),
            bounds: Bounds::new(point(px(-20.0), px(30.0)), size(px(320.0), px(180.0))),
            opacity: 0.6,
            original_size: size(px(640.0), px(360.0)),
            thumbnail_size: Some(size(px(160.0), px(90.0))),
            text: Some(std::sync::Arc::from("héllo")),
        };
        let mut images = std::collections::HashMap::new();
        write_pin_backup(directory.path(), &pin, &mut images).unwrap();
        let loaded = load_pin_backups(directory.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, 42);
        assert_eq!(&*loaded[0].image, &*image);
        assert_eq!(loaded[0].text.as_deref(), Some("héllo"));
        assert_eq!(f32::from(loaded[0].bounds.origin.x), -20.0);
        assert_eq!(loaded[0].opacity, 0.6);
        assert!(parse_pin_backup(42, "0\t0\t0\t0\t2\t0\t0\t0\t0\t", directory.path()).is_err());
        remove_pin_backup(directory.path(), 42).unwrap();
        assert!(load_pin_backups(directory.path()).unwrap().is_empty());
    }

    #[test]
    fn startup_commands_parse_without_losing_file_paths() {
        assert_eq!(
            parse_startup_action(Vec::<std::ffi::OsString>::new()).unwrap(),
            StartupAction::None
        );
        assert_eq!(
            parse_startup_action(["snip".into()]).unwrap(),
            StartupAction::Capture
        );
        assert_eq!(
            parse_startup_action([
                "paste".into(),
                "--plain".into(),
                "hello".into(),
                "world".into()
            ])
            .unwrap(),
            StartupAction::Plain("hello world".into())
        );
        assert_eq!(
            parse_startup_action(["paste".into(), "--files".into(), "image.png".into()]).unwrap(),
            StartupAction::Files(vec![PathBuf::from("image.png")])
        );
        assert!(parse_startup_action(["paste".into(), "--files".into()]).is_err());
    }
}

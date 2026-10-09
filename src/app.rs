use crate::{editor::Screenshot, icons::Icons, pin, platform, raster, selection::SelectionState};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui::{prelude::*, *};
use image::{Rgba, RgbaImage};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct AppState {
    _hotkeys: GlobalHotKeyManager,
    pub fonts: Arc<resvg::usvg::fontdb::Database>,
    pub selection: Option<WindowHandle<SelectionState>>,
    pub cached_selection: Option<WindowHandle<SelectionState>>,
    pub pins: Vec<WindowHandle<pin::Pin>>,
    pub closed_pins: Vec<pin::ClosedPin>,
    pub pins_hidden: bool,
    pub capture_history: Vec<Screenshot>,
    pub history: Vec<(Arc<RgbaImage>, Arc<RenderImage>)>,
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

actions!(
    snipsy,
    [CaptureScreen, PinClipboard, TogglePinnedImages, QuitSnipsy]
);

pub fn run() {
    let application = Application::new().with_assets(Icons);
    application.on_reopen(start_capture);
    application.run(|cx| {
        platform::initialize_app();
        let mut fonts = resvg::usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        let hotkeys = GlobalHotKeyManager::new().expect("could not create global shortcuts");
        let mut hotkey_error = None;
        let capture = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyA);
        let lens = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
        let paste = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyP);
        let click_through = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyX);
        let toggle_pins_hotkey = HotKey::new(Some(Modifiers::SHIFT), Code::F3);
        for hotkey in [&capture, &lens, &paste, &click_through, &toggle_pins_hotkey] {
            if let Err(error) = hotkeys.register(*hotkey) {
                eprintln!("could not register shortcut: {error}");
                hotkey_error = Some(format!("Could not register a shortcut: {error}"));
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
            pins: Vec::new(),
            closed_pins: Vec::new(),
            pins_hidden: false,
            capture_history: Vec::new(),
            history: Vec::new(),
            error: hotkey_error,
        });
        cx.on_action(|_: &CaptureScreen, cx| start_capture(cx));
        cx.on_action(|_: &PinClipboard, cx| paste_image(cx));
        cx.on_action(|_: &TogglePinnedImages, cx| toggle_pins(cx));
        cx.on_action(|_: &QuitSnipsy, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", QuitSnipsy, None)]);
        cx.set_menus(vec![Menu {
            name: "Snipsy".into(),
            items: vec![
                MenuItem::action("New screenshot", CaptureScreen),
                MenuItem::action("Pin clipboard", PinClipboard),
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
        if let Some(error) = cx.global::<AppState>().error.clone() {
            cx.defer(move |cx| show_error(error, cx));
        }
    });
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

pub fn save_image(image: Arc<RgbaImage>, cx: &mut App) {
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
                let _ = cx.update(|cx| remember_capture(saved_image, cx));
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

pub fn show_error(error: String, cx: &mut App) {
    eprintln!("{error}");
    cx.global_mut::<AppState>().error = Some(error.clone());
    platform::show_error(&error);
}

#[cfg(test)]
mod tests {
    use super::{load_path, parse_color, should_restore_pin, write_image};
    use image::{Rgba, RgbaImage};

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
}

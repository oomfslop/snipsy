use crate::{
    editor::Screenshot, geometry::Rect, groups::Groups, history::History, icons::Icons, pin,
    platform, preferences::Preferences, raster, selection::SelectionState,
};
use base64::Engine;
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui::{prelude::*, *};
use image::{
    AnimationDecoder, Frame, Rgba, RgbaImage,
    codecs::gif::{GifDecoder, GifEncoder, Repeat},
};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::{BufReader, Cursor, Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct AppState {
    _hotkeys: GlobalHotKeyManager,
    status_item: platform::StatusItem,
    recorder: Option<platform::ScreenRecorder>,
    recording_pending: bool,
    recording_stopping: bool,
    quit_after_recording: bool,
    pub fonts: Arc<resvg::usvg::fontdb::Database>,
    pub selection: Option<WindowHandle<SelectionState>>,
    pub cached_selection: Option<WindowHandle<SelectionState>>,
    pub history_window: Option<WindowHandle<History>>,
    pub pin_editor: Option<WindowHandle<SelectionState>>,
    pub preferences_window: Option<WindowHandle<Preferences>>,
    pub groups_window: Option<WindowHandle<Groups>>,
    pub pins: Vec<WindowHandle<pin::Pin>>,
    pub closed_pins: Vec<pin::ClosedPin>,
    pub pins_hidden: bool,
    pub capture_history: Vec<Screenshot>,
    pub capture_cursor: bool,
    pub last_capture_selection: Option<(Rect, Rect)>,
    pub history: Vec<(Arc<RgbaImage>, Arc<RenderImage>)>,
    history_persist: async_channel::Sender<HistoryCommand>,
    pin_persist: async_channel::Sender<pin::PinBackupCommand>,
    hotkeys: HotkeySettings,
    hotkey_commands: Arc<Mutex<HashMap<u32, Command>>>,
    settings_path: Option<PathBuf>,
    pub groups: Vec<String>,
    pub active_group: usize,
    groups_path: Option<PathBuf>,
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

enum HistoryCommand {
    Save(Arc<RgbaImage>),
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyKind {
    Capture,
    Paste,
    ClickThrough,
    TogglePins,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotkeySettings {
    pub capture: HotKey,
    pub paste: HotKey,
    pub click_through: HotKey,
    pub toggle_pins: HotKey,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        return Self {
            capture: HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyA),
            paste: HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyP),
            click_through: HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyX),
            toggle_pins: HotKey::new(Some(Modifiers::SHIFT), Code::F3),
        };
    }
}

impl HotkeySettings {
    pub fn get(self, kind: HotkeyKind) -> HotKey {
        return match kind {
            HotkeyKind::Capture => self.capture,
            HotkeyKind::Paste => self.paste,
            HotkeyKind::ClickThrough => self.click_through,
            HotkeyKind::TogglePins => self.toggle_pins,
        };
    }

    fn set(&mut self, kind: HotkeyKind, hotkey: HotKey) {
        match kind {
            HotkeyKind::Capture => self.capture = hotkey,
            HotkeyKind::Paste => self.paste = hotkey,
            HotkeyKind::ClickThrough => self.click_through = hotkey,
            HotkeyKind::TogglePins => self.toggle_pins = hotkey,
        }
    }
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
    Whiteboard([u8; 4]),
    CreateGroup(Option<String>),
    SwitchGroup(Option<String>),
    Groups,
    EmptyGroup,
}

actions!(
    snipsy,
    [
        CaptureScreen,
        PinClipboard,
        ShowHistory,
        ClearHistory,
        ShowWhiteboard,
        ShowPreferences,
        ShowGroups,
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
        "whiteboard" if args.len() == 1 => Ok(StartupAction::Whiteboard([255; 4])),
        "whiteboard"
            if args.len() == 3
                && args.get(1).and_then(|arg| arg.to_str()) == Some("--color") =>
        {
            args[2]
                .to_str()
                .and_then(parse_color)
                .map(StartupAction::Whiteboard)
                .ok_or("Invalid whiteboard color".into())
        }
        "transparent-whiteboard" if args.len() == 1 => {
            Ok(StartupAction::Whiteboard([0, 0, 0, 0]))
        }
        "create-group" => Ok(StartupAction::CreateGroup((args.len() > 1).then(|| {
            args[1..]
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        }))),
        "switch-group" => Ok(StartupAction::SwitchGroup((args.len() > 1).then(|| {
            args[1..]
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        }))),
        "switch-groups" | "show-group-manager" if args.len() == 1 => Ok(StartupAction::Groups),
        "empty-group" if args.len() == 1 => Ok(StartupAction::EmptyGroup),
        _ => Err(
            "Usage: snipsy [snip | paste [--clipboard | --plain TEXT | --files FILE...] | history | whiteboard [--color COLOR] | transparent-whiteboard | create-group [NAME] | switch-group [NAME] | switch-groups | empty-group]".into(),
        ),
    };
}

fn encode_startup_action(action: &StartupAction) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    match action {
        StartupAction::None | StartupAction::Capture => output.push(0),
        StartupAction::Paste => output.push(1),
        StartupAction::Plain(text) => {
            output.push(2);
            append_startup_bytes(&mut output, text.as_bytes())?;
        }
        StartupAction::Files(paths) => {
            output.push(3);
            output.extend_from_slice(
                &u32::try_from(paths.len())
                    .map_err(|_| "Too many file paths")?
                    .to_be_bytes(),
            );
            for path in paths {
                append_startup_bytes(&mut output, path.as_os_str().as_bytes())?;
            }
        }
        StartupAction::History => output.push(4),
        StartupAction::Whiteboard(color) => {
            output.push(5);
            output.extend_from_slice(color);
        }
        StartupAction::CreateGroup(name) => {
            output.push(6);
            append_startup_bytes(&mut output, name.as_deref().unwrap_or_default().as_bytes())?;
        }
        StartupAction::SwitchGroup(name) => {
            output.push(7);
            append_startup_bytes(&mut output, name.as_deref().unwrap_or_default().as_bytes())?;
        }
        StartupAction::Groups => output.push(8),
        StartupAction::EmptyGroup => output.push(9),
    }
    return Ok(output);
}

fn append_startup_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<(), String> {
    output.extend_from_slice(
        &u32::try_from(value.len())
            .map_err(|_| "Command value is too large")?
            .to_be_bytes(),
    );
    output.extend_from_slice(value);
    return Ok(());
}

fn decode_startup_action(input: &[u8]) -> Result<StartupAction, String> {
    let (&tag, mut rest) = input.split_first().ok_or("Empty command")?;
    let action = match tag {
        0 => StartupAction::Capture,
        1 => StartupAction::Paste,
        2 => StartupAction::Plain(
            String::from_utf8(read_startup_bytes(&mut rest)?.to_vec())
                .map_err(|error| error.to_string())?,
        ),
        3 => {
            let count = read_startup_u32(&mut rest)? as usize;
            if count > 10_000 {
                return Err("Too many file paths".into());
            }
            let mut paths = Vec::with_capacity(count);
            for _ in 0..count {
                paths.push(PathBuf::from(OsString::from_vec(
                    read_startup_bytes(&mut rest)?.to_vec(),
                )));
            }
            StartupAction::Files(paths)
        }
        4 => StartupAction::History,
        5 if rest.len() == 4 => StartupAction::Whiteboard(rest.try_into().unwrap()),
        6 => {
            let name = String::from_utf8(read_startup_bytes(&mut rest)?.to_vec())
                .map_err(|error| error.to_string())?;
            StartupAction::CreateGroup((!name.is_empty()).then_some(name))
        }
        7 => {
            let name = String::from_utf8(read_startup_bytes(&mut rest)?.to_vec())
                .map_err(|error| error.to_string())?;
            StartupAction::SwitchGroup((!name.is_empty()).then_some(name))
        }
        8 => StartupAction::Groups,
        9 => StartupAction::EmptyGroup,
        _ => return Err("Invalid command".into()),
    };
    if !rest.is_empty() && tag != 5 {
        return Err("Unexpected command data".into());
    }
    return Ok(action);
}

fn read_startup_u32(input: &mut &[u8]) -> Result<u32, String> {
    let bytes = input.get(..4).ok_or("Truncated command")?;
    *input = &input[4..];
    return Ok(u32::from_be_bytes(bytes.try_into().unwrap()));
}

fn read_startup_bytes<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], String> {
    let length = read_startup_u32(input)? as usize;
    let bytes = input.get(..length).ok_or("Truncated command")?;
    *input = &input[length..];
    return Ok(bytes);
}

fn connect_instance(
    startup: &StartupAction,
) -> Result<Option<async_channel::Receiver<StartupAction>>, String> {
    let socket = platform::get_app_data_dir()?.join("snipsy.sock");
    let command = encode_startup_action(startup)?;
    match UnixStream::connect(&socket) {
        Ok(mut stream) => {
            stream
                .write_all(&command)
                .map_err(|error| error.to_string())?;
            return Ok(None);
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) => {}
        Err(error) => return Err(error.to_string()),
    }
    match std::fs::remove_file(&socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let listener = match UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            let mut stream = UnixStream::connect(&socket).map_err(|error| error.to_string())?;
            stream
                .write_all(&command)
                .map_err(|error| error.to_string())?;
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };
    let (sender, receiver) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("snipsy-command-listener".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let result = stream
                    .map_err(|error| error.to_string())
                    .and_then(|stream| {
                        let mut input = Vec::new();
                        stream
                            .take(16 * 1024 * 1024)
                            .read_to_end(&mut input)
                            .map_err(|error| error.to_string())?;
                        decode_startup_action(&input)
                    });
                match result {
                    Ok(action) => {
                        if sender.send_blocking(action).is_err() {
                            break;
                        }
                    }
                    Err(error) => eprintln!("invalid startup command: {error}"),
                }
            }
        })
        .map_err(|error| error.to_string())?;
    return Ok(Some(receiver));
}

fn load_hotkey_settings(path: &Path) -> Result<HotkeySettings, String> {
    let input = match std::fs::read_to_string(path) {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HotkeySettings::default());
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut settings = HotkeySettings::default();
    for line in input.lines() {
        let Some((key, value)) = line.split_once('=') else {
            return Err("Invalid hotkey settings".into());
        };
        let hotkey = value.parse::<HotKey>().map_err(|error| error.to_string())?;
        match key {
            "capture" => settings.capture = hotkey,
            "paste" => settings.paste = hotkey,
            "click_through" => settings.click_through = hotkey,
            "toggle_pins" => settings.toggle_pins = hotkey,
            _ => return Err("Unknown hotkey setting".into()),
        }
    }
    validate_hotkey_settings(settings)?;
    return Ok(settings);
}

fn validate_hotkey_settings(settings: HotkeySettings) -> Result<(), String> {
    let hotkeys = [
        settings.capture,
        settings.paste,
        settings.click_through,
        settings.toggle_pins,
    ];
    if hotkeys
        .iter()
        .enumerate()
        .any(|(index, hotkey)| hotkeys[index + 1..].contains(hotkey))
    {
        return Err("Global shortcuts must be unique".into());
    }
    let lens = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
    if [settings.paste, settings.click_through, settings.toggle_pins].contains(&lens) {
        return Err("⌘⇧L is reserved for capture".into());
    }
    return Ok(());
}

fn write_hotkey_settings(path: &Path, settings: HotkeySettings) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))
        .map_err(|error| error.to_string())?;
    write!(
        file,
        "capture={}\npaste={}\nclick_through={}\ntoggle_pins={}\n",
        settings.capture, settings.paste, settings.click_through, settings.toggle_pins
    )
    .map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    file.persist(path).map_err(|error| error.to_string())?;
    return Ok(());
}

fn load_groups(path: &Path) -> Result<(Vec<String>, usize), String> {
    let input = match std::fs::read_to_string(path) {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((vec!["Default".into()], 0));
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut groups = Vec::new();
    let mut active = 0;
    for line in input.lines() {
        if let Some(value) = line.strip_prefix("active=") {
            active = value.parse::<usize>().map_err(|error| error.to_string())?;
        } else if let Some(value) = line.strip_prefix("group=") {
            let name = String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(value)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            if name.is_empty() || name.len() > 128 || groups.len() == 64 {
                return Err("Invalid image group".into());
            }
            groups.push(name);
        } else {
            return Err("Invalid image-group settings".into());
        }
    }
    if groups.is_empty() || active >= groups.len() {
        return Err("Invalid active image group".into());
    }
    return Ok((groups, active));
}

fn write_groups(path: &Path, groups: &[String], active: usize) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))
        .map_err(|error| error.to_string())?;
    writeln!(file, "active={active}").map_err(|error| error.to_string())?;
    for group in groups {
        writeln!(
            file,
            "group={}",
            base64::engine::general_purpose::STANDARD.encode(group.as_bytes())
        )
        .map_err(|error| error.to_string())?;
    }
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    file.persist(path).map_err(|error| error.to_string())?;
    return Ok(());
}

fn get_hotkey_entries(settings: HotkeySettings) -> [(HotKey, Command); 4] {
    return [
        (settings.capture, Command::Capture),
        (settings.paste, Command::Paste),
        (settings.click_through, Command::ToggleClickThrough),
        (settings.toggle_pins, Command::TogglePins),
    ];
}

pub fn get_hotkey_settings(cx: &App) -> HotkeySettings {
    return cx.global::<AppState>().hotkeys;
}

pub fn set_hotkey(kind: HotkeyKind, hotkey: HotKey, cx: &mut App) -> Result<(), String> {
    let state = cx.global_mut::<AppState>();
    let old = state.hotkeys.get(kind);
    if old == hotkey {
        return Ok(());
    }
    let mut settings = state.hotkeys;
    settings.set(kind, hotkey);
    validate_hotkey_settings(settings)?;
    let lens = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
    if old != lens {
        state
            ._hotkeys
            .unregister(old)
            .map_err(|error| error.to_string())?;
    }
    if hotkey != lens {
        if let Err(error) = state._hotkeys.register(hotkey) {
            if old != lens {
                let _ = state._hotkeys.register(old);
            }
            return Err(error.to_string());
        }
    }
    if let Some(path) = &state.settings_path {
        if let Err(error) = write_hotkey_settings(path, settings) {
            if hotkey != lens {
                let _ = state._hotkeys.unregister(hotkey);
            }
            if old != lens {
                let _ = state._hotkeys.register(old);
            }
            return Err(error);
        }
    }
    let mut commands = state.hotkey_commands.lock().unwrap();
    if old != lens {
        commands.remove(&old.id());
    }
    commands.insert(
        hotkey.id(),
        match kind {
            HotkeyKind::Capture => Command::Capture,
            HotkeyKind::Paste => Command::Paste,
            HotkeyKind::ClickThrough => Command::ToggleClickThrough,
            HotkeyKind::TogglePins => Command::TogglePins,
        },
    );
    drop(commands);
    state.hotkeys = settings;
    return Ok(());
}

pub fn run(startup: StartupAction) -> Result<(), String> {
    let Some(instance_commands) = connect_instance(&startup)? else {
        return Ok(());
    };
    let application = Application::new().with_assets(Icons);
    application.on_reopen(start_capture);
    application.run(move |cx| {
        platform::initialize_app();
        let (status_sender, status_receiver) = async_channel::unbounded();
        let status_item = platform::create_status_item(status_sender);
        let mut fonts = resvg::usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        let (data_directory, mut hotkey_error) = match platform::get_app_data_dir() {
            Ok(directory) => (Some(directory), None),
            Err(error) => (None, Some(error)),
        };
        let settings_path = data_directory
            .as_ref()
            .map(|directory| directory.join("hotkeys.conf"));
        let hotkey_settings = match settings_path.as_deref().map(load_hotkey_settings) {
            Some(Ok(settings)) => settings,
            Some(Err(error)) => {
                hotkey_error = Some(format!("Could not load shortcuts: {error}"));
                HotkeySettings::default()
            }
            None => HotkeySettings::default(),
        };
        let groups_path = data_directory
            .as_ref()
            .map(|directory| directory.join("groups.conf"));
        let (groups, active_group) = match groups_path.as_deref().map(load_groups) {
            Some(Ok(groups)) => groups,
            Some(Err(error)) => {
                hotkey_error = Some(format!("Could not load image groups: {error}"));
                (vec!["Default".into()], 0)
            }
            None => (vec!["Default".into()], 0),
        };
        let (history_persist, history_receiver) = async_channel::unbounded();
        let (pin_persist, pin_receiver) = async_channel::unbounded();
        let hotkeys = GlobalHotKeyManager::new().expect("could not create global shortcuts");
        let lens = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
        let hotkey_commands = Arc::new(Mutex::new(HashMap::new()));
        for (hotkey, command) in get_hotkey_entries(hotkey_settings)
            .into_iter()
            .chain((lens != hotkey_settings.capture).then_some((lens, Command::Capture)))
        {
            if let Err(error) = hotkeys.register(hotkey) {
                eprintln!("could not register shortcut: {error}");
                hotkey_error = Some(format!("Could not register a shortcut: {error}"));
            } else {
                hotkey_commands.lock().unwrap().insert(hotkey.id(), command);
            }
        }
        let (sender, receiver) = async_channel::unbounded();
        let event_hotkeys = hotkey_commands.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state != HotKeyState::Pressed {
                return;
            }
            let Some(command) = event_hotkeys.lock().unwrap().get(&event.id).copied() else {
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
            status_item,
            recorder: None,
            recording_pending: false,
            recording_stopping: false,
            quit_after_recording: false,
            fonts: Arc::new(fonts),
            selection: None,
            cached_selection: None,
            history_window: None,
            pin_editor: None,
            preferences_window: None,
            groups_window: None,
            pins: Vec::new(),
            closed_pins: Vec::new(),
            pins_hidden: false,
            capture_history: Vec::new(),
            capture_cursor: false,
            last_capture_selection: None,
            history: Vec::new(),
            history_persist,
            pin_persist,
            hotkeys: hotkey_settings,
            hotkey_commands,
            settings_path,
            groups,
            active_group,
            groups_path,
            error: hotkey_error,
        });
        if let Some(directory) = data_directory {
            let history_directory = directory.join("history");
            let persist_directory = history_directory.clone();
            cx.background_executor()
                .spawn(async move {
                    while let Ok(command) = history_receiver.recv().await {
                        let result = match command {
                            HistoryCommand::Save(image) => {
                                write_history(&persist_directory, &image)
                            }
                            HistoryCommand::Clear => clear_history_files(&persist_directory),
                        };
                        if let Err(error) = result {
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
                        if let Some(max_group) = pins.iter().map(|pin| pin.group).max() {
                            let state = cx.global_mut::<AppState>();
                            while state.groups.len() <= max_group && state.groups.len() < 64 {
                                state
                                    .groups
                                    .push(format!("Group {}", state.groups.len() + 1));
                            }
                            if let Some(path) = &state.groups_path {
                                let _ = write_groups(path, &state.groups, state.active_group);
                            }
                        }
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
        cx.on_action(|_: &ClearHistory, cx| clear_history(cx));
        cx.on_action(|_: &ShowWhiteboard, cx| start_whiteboard([255; 4], cx));
        cx.on_action(|_: &ShowPreferences, cx| show_preferences(cx));
        cx.on_action(|_: &ShowGroups, cx| show_groups(cx));
        cx.on_action(|_: &TogglePinnedImages, cx| toggle_pins(cx));
        cx.on_action(|_: &QuitSnipsy, cx| quit(cx));
        cx.bind_keys([KeyBinding::new("cmd-q", QuitSnipsy, None)]);
        cx.set_menus(vec![Menu {
            name: "Snipsy".into(),
            items: vec![
                MenuItem::action("New screenshot", CaptureScreen),
                MenuItem::action("Pin clipboard", PinClipboard),
                MenuItem::action("History", ShowHistory),
                MenuItem::action("Clear History…", ClearHistory),
                MenuItem::action("Whiteboard", ShowWhiteboard),
                MenuItem::action("Preferences…", ShowPreferences),
                MenuItem::action("Image Groups…", ShowGroups),
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
        cx.spawn(async move |cx| {
            while let Ok(action) = instance_commands.recv().await {
                let _ = cx.update(|cx| dispatch_startup(action, cx));
            }
        })
        .detach();
        cx.spawn(async move |cx| {
            while let Ok(command) = status_receiver.recv().await {
                let _ = cx.update(|cx| match command {
                    platform::StatusCommand::Capture => start_capture(cx),
                    platform::StatusCommand::ToggleRecording => toggle_recording(cx),
                    platform::StatusCommand::Paste => paste_image(cx),
                    platform::StatusCommand::History => show_history(cx),
                    platform::StatusCommand::ClearHistory => clear_history(cx),
                    platform::StatusCommand::Whiteboard => start_whiteboard([255; 4], cx),
                    platform::StatusCommand::Preferences => show_preferences(cx),
                    platform::StatusCommand::Groups => show_groups(cx),
                    platform::StatusCommand::TogglePins => toggle_pins(cx),
                    platform::StatusCommand::Quit => quit(cx),
                });
            }
        })
        .detach();
        cx.defer(move |cx| dispatch_startup(startup, cx));
        if let Some(error) = cx.global::<AppState>().error.clone() {
            cx.defer(move |cx| show_error(error, cx));
        }
    });
    return Ok(());
}

fn dispatch_startup(action: StartupAction, cx: &mut App) {
    match action {
        StartupAction::None => {}
        StartupAction::Capture => start_capture(cx),
        StartupAction::Paste => paste_image(cx),
        StartupAction::Plain(text) => pin_text(text, cx),
        StartupAction::Files(paths) => open_paths(paths, cx),
        StartupAction::History => show_history(cx),
        StartupAction::Whiteboard(color) => start_whiteboard(color, cx),
        StartupAction::CreateGroup(name) => {
            if let Err(error) = create_group(name, cx) {
                show_error(error, cx);
            }
        }
        StartupAction::SwitchGroup(name) => {
            let result = if let Some(name) = name {
                let (groups, _) = get_groups(cx);
                groups
                    .iter()
                    .position(|group| group == &name)
                    .ok_or_else(|| "Image group does not exist".into())
                    .and_then(|index| switch_group(index, cx))
            } else {
                let (groups, active) = get_groups(cx);
                switch_group((active + 1) % groups.len(), cx)
            };
            if let Err(error) = result {
                show_error(error, cx);
            }
        }
        StartupAction::Groups => show_groups(cx),
        StartupAction::EmptyGroup => empty_active_group(cx),
    }
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

pub fn clear_history(cx: &mut App) {
    if !platform::confirm("Clear Snipsy history?") {
        return;
    }
    let state = cx.global_mut::<AppState>();
    let history = std::mem::take(&mut state.history);
    state.capture_history.clear();
    let _ = state.history_persist.try_send(HistoryCommand::Clear);
    for (_, preview) in history {
        cx.drop_image(preview, None);
    }
    cx.refresh_windows();
}

fn show_preferences(cx: &mut App) {
    if let Some(handle) = cx.global::<AppState>().preferences_window {
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
                size(px(430.0), px(300.0)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Preferences".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(360.0), px(260.0))),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| Preferences::new(window, cx)),
    ) {
        Ok(handle) => cx.global_mut::<AppState>().preferences_window = Some(handle),
        Err(error) => show_error(error.to_string(), cx),
    }
}

fn show_groups(cx: &mut App) {
    if let Some(handle) = cx.global::<AppState>().groups_window {
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
                size(px(360.0), px(420.0)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Image Groups".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(280.0), px(240.0))),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| Groups::new(window, cx)),
    ) {
        Ok(handle) => cx.global_mut::<AppState>().groups_window = Some(handle),
        Err(error) => show_error(error.to_string(), cx),
    }
}

fn start_whiteboard(color: [u8; 4], cx: &mut App) {
    let bounds = match platform::get_desktop_bounds() {
        Ok(bounds) => bounds,
        Err(error) => {
            show_error(error, cx);
            return;
        }
    };
    let scale = platform::get_desktop_scale();
    if let Some(handle) = cx.global::<AppState>().selection {
        let _ = handle.update(cx, |state, window, cx| state.dismiss(window, cx));
    }
    let cached = cx.global::<AppState>().cached_selection.filter(|handle| {
        handle
            .update(cx, |state, window, cx| {
                platform::configure_overlay(window, bounds, false);
                *state = SelectionState::new_whiteboard(bounds, color, scale, window, cx);
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
                cx.new(|cx| SelectionState::new_whiteboard(bounds, color, scale, window, cx))
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
    let _ = handle.update(cx, |state, window, cx| state.activate(window, cx));
}

fn toggle_pins(cx: &mut App) {
    let visible = cx.global::<AppState>().pins_hidden;
    set_pins_visible(visible, cx);
}

pub fn set_pins_visible(visible: bool, cx: &mut App) {
    let active_group = cx.global::<AppState>().active_group;
    let pins = std::mem::take(&mut cx.global_mut::<AppState>().pins);
    let live = pins
        .into_iter()
        .filter(|pin| {
            pin.update(cx, |state, window, _| {
                platform::set_window_visible(window, visible && state.group() == active_group)
            })
            .is_ok()
        })
        .collect();
    let state = cx.global_mut::<AppState>();
    state.pins = live;
    state.pins_hidden = !visible;
}

pub fn get_groups(cx: &App) -> (Vec<String>, usize) {
    let state = cx.global::<AppState>();
    return (state.groups.clone(), state.active_group);
}

pub fn create_group(name: Option<String>, cx: &mut App) -> Result<(), String> {
    let state = cx.global_mut::<AppState>();
    if state.groups.len() == 64 {
        return Err("Maximum image-group count reached".into());
    }
    let name = name.unwrap_or_else(|| format!("Group {}", state.groups.len() + 1));
    if name.trim().is_empty() || name.len() > 128 || state.groups.contains(&name) {
        return Err("Image-group name must be unique and 1–128 bytes".into());
    }
    state.groups.push(name);
    let index = state.groups.len() - 1;
    if let Some(path) = &state.groups_path {
        write_groups(path, &state.groups, index)?;
    }
    state.active_group = index;
    set_active_group_visibility(cx);
    return Ok(());
}

pub fn switch_group(index: usize, cx: &mut App) -> Result<(), String> {
    let state = cx.global_mut::<AppState>();
    if index >= state.groups.len() {
        return Err("Image group does not exist".into());
    }
    if let Some(path) = &state.groups_path {
        write_groups(path, &state.groups, index)?;
    }
    state.active_group = index;
    set_active_group_visibility(cx);
    return Ok(());
}

fn set_active_group_visibility(cx: &mut App) {
    let active = cx.global::<AppState>().active_group;
    let pins = std::mem::take(&mut cx.global_mut::<AppState>().pins);
    let live = pins
        .into_iter()
        .filter(|pin| {
            pin.update(cx, |state, window, _| {
                platform::set_window_visible(window, state.group() == active)
            })
            .is_ok()
        })
        .collect();
    let state = cx.global_mut::<AppState>();
    state.pins = live;
    state.pins_hidden = false;
}

fn empty_active_group(cx: &mut App) {
    let active = cx.global::<AppState>().active_group;
    let pins = std::mem::take(&mut cx.global_mut::<AppState>().pins);
    let mut live = Vec::new();
    for pin in pins {
        match pin.update(cx, |state, window, cx| {
            if state.group() == active {
                state.destroy(window, cx);
                return false;
            }
            return true;
        }) {
            Ok(true) => live.push(pin),
            Ok(false) | Err(_) => {}
        }
    }
    cx.global_mut::<AppState>().pins = live;
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

pub fn toggle_recording(cx: &mut App) {
    if cx.global::<AppState>().recorder.is_some() {
        stop_recording(None, false, cx);
        return;
    }
    if cx.global::<AppState>().recording_pending {
        return;
    }
    start_capture(cx);
}

fn cancel_recording(cx: &mut App) {
    if cx.global::<AppState>().recorder.is_none() {
        cx.global_mut::<AppState>().recording_pending = false;
    }
}

pub fn start_recording(bounds: Rect, cx: &mut App) {
    if cx.global::<AppState>().recorder.is_some() {
        show_error("A screen recording is already active".into(), cx);
        return;
    }
    if cx.global::<AppState>().recording_pending {
        return;
    }
    cx.global_mut::<AppState>().recording_pending = true;
    let path = platform::prompt_save_recording();
    cx.spawn(async move |cx| {
        let result = match path.recv().await {
            Ok(Ok(Some(path))) => {
                let (sender, receiver) = async_channel::bounded(1);
                let (error_sender, error_receiver) = async_channel::bounded(1);
                platform::start_screen_recording(bounds, path, sender, error_sender);
                match receiver.recv().await {
                    Ok(result) => result.map(|recorder| (recorder, error_receiver)),
                    Err(error) => Err(error.to_string()),
                }
            }
            Ok(Ok(None)) => {
                let _ = cx.update(cancel_recording);
                return;
            }
            Ok(Err(error)) => Err(error),
            Err(error) => Err(error.to_string()),
        };
        let _ = cx.update(|cx| match result {
            Ok((recorder, errors)) => {
                let state = cx.global_mut::<AppState>();
                state.recording_pending = false;
                state.status_item.set_recording(true);
                state.recorder = Some(recorder);
                cx.spawn(async move |cx| {
                    let Ok(error) = errors.recv().await else {
                        return;
                    };
                    let _ = cx.update(|cx| {
                        if cx.global::<AppState>().recorder.is_some() {
                            stop_recording(Some(error), false, cx);
                        }
                    });
                })
                .detach();
            }
            Err(error) => {
                cancel_recording(cx);
                show_error(error, cx);
            }
        });
    })
    .detach();
}

fn stop_recording(error: Option<String>, quit_after: bool, cx: &mut App) {
    let Some(recorder) = cx.global_mut::<AppState>().recorder.take() else {
        if quit_after {
            if cx.global::<AppState>().recording_stopping {
                cx.global_mut::<AppState>().quit_after_recording = true;
            } else {
                cx.quit();
            }
        }
        return;
    };
    let state = cx.global_mut::<AppState>();
    state.recording_pending = true;
    state.recording_stopping = true;
    let (sender, receiver) = async_channel::bounded(1);
    platform::stop_screen_recording(recorder, sender);
    cx.spawn(async move |cx| {
        let result = receiver
            .recv()
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
        let _ = cx.update(|cx| {
            let state = cx.global_mut::<AppState>();
            state.recording_pending = false;
            state.recording_stopping = false;
            state.status_item.set_recording(false);
            let quit_after = quit_after || state.quit_after_recording;
            state.quit_after_recording = false;
            if let Some(error) = error.or_else(|| result.err()) {
                if quit_after {
                    eprintln!("could not finish recording: {error}");
                } else {
                    show_error(error, cx);
                }
            }
            if quit_after {
                cx.quit();
            }
        });
    })
    .detach();
}

fn quit(cx: &mut App) {
    if cx.global::<AppState>().recorder.is_some() {
        stop_recording(None, true, cx);
    } else if cx.global::<AppState>().recording_stopping {
        cx.global_mut::<AppState>().quit_after_recording = true;
    } else {
        cx.quit();
    }
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
    platform::capture_screen(bounds, cx.global::<AppState>().capture_cursor, sender);
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
    let load = match load_clipboard_content(cx) {
        Ok(load) => load,
        Err(error) => {
            show_error(error, cx);
            return;
        }
    };
    cx.spawn(async move |cx| {
        let result = load.await;
        let _ = cx.update(|cx| match result {
            Ok(content) => pin::open_content(content, cx),
            Err(error) => show_error(error, cx),
        });
    })
    .detach();
}

pub struct PinContent {
    pub image: Arc<RgbaImage>,
    pub text: Option<String>,
    pub frames: Option<Arc<Vec<Frame>>>,
}

pub fn load_clipboard_content(cx: &mut App) -> Result<Task<Result<PinContent, String>>, String> {
    let fonts = cx.global::<AppState>().fonts.clone();
    if let Some(path) = platform::get_clipboard_paths().into_iter().next() {
        return Ok(cx.background_executor().spawn(async move {
            return load_path(&path, &fonts);
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
            return load_image_bytes(&image.bytes);
        }));
    }
    let text = item.text().ok_or("Clipboard has no supported content")?;
    return Ok(cx.background_executor().spawn(async move {
        if let Some(color) = parse_color(&text) {
            return Ok(PinContent {
                image: Arc::new(RgbaImage::from_pixel(240, 160, Rgba(color))),
                text: None,
                frames: None,
            });
        }
        return raster::render_text_image(&text, &fonts).map(|image| PinContent {
            image: Arc::new(image),
            text: Some(text),
            frames: None,
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
                    Ok(content) => pin::open_content(content, cx),
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

pub fn load_path_content(path: PathBuf, cx: &mut App) -> Task<Result<PinContent, String>> {
    let fonts = cx.global::<AppState>().fonts.clone();
    return cx
        .background_executor()
        .spawn(async move { load_path(&path, &fonts) });
}

fn load_path(
    path: &Path,
    fonts: &Arc<resvg::usvg::fontdb::Database>,
) -> Result<PinContent, String> {
    if let Ok(content) = load_image_path(path) {
        return Ok(content);
    }
    let text = path.to_string_lossy().into_owned();
    return raster::render_text_image(&text, fonts)
        .map(|image| PinContent {
            image: Arc::new(image),
            text: Some(text),
            frames: None,
        })
        .map_err(|error| format!("{}: {error}", path.display()));
}

fn load_image_path(path: &Path) -> Result<PinContent, String> {
    let reader = image::ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    if reader.format() == Some(image::ImageFormat::Gif) {
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        let decoder = GifDecoder::new(BufReader::new(file)).map_err(|error| error.to_string())?;
        return collect_animation_frames(decoder.into_frames());
    }
    return reader
        .decode()
        .map(|image| PinContent {
            image: Arc::new(image.to_rgba8()),
            text: None,
            frames: None,
        })
        .map_err(|error| error.to_string());
}

fn load_image_bytes(bytes: &[u8]) -> Result<PinContent, String> {
    if image::guess_format(bytes).map_err(|error| error.to_string())? == image::ImageFormat::Gif {
        let decoder = GifDecoder::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
        return collect_animation_frames(decoder.into_frames());
    }
    return image::load_from_memory(bytes)
        .map(|image| PinContent {
            image: Arc::new(image.to_rgba8()),
            text: None,
            frames: None,
        })
        .map_err(|error| error.to_string());
}

fn collect_animation_frames(
    frames: impl Iterator<Item = image::ImageResult<Frame>>,
) -> Result<PinContent, String> {
    let mut decoded = Vec::new();
    let mut bytes = 0_u64;
    for frame in frames {
        if decoded.len() == 500 {
            return Err("Animation has too many frames".into());
        }
        let frame = frame.map_err(|error| error.to_string())?;
        bytes = bytes
            .checked_add(frame.buffer().len() as u64)
            .ok_or("Animation is too large")?;
        if bytes > 256 * 1024 * 1024 {
            return Err("Animation is too large".into());
        }
        decoded.push(frame);
    }
    let image = decoded
        .first()
        .map(|frame| Arc::new(frame.buffer().clone()))
        .ok_or("Animation has no frames")?;
    let frames = (decoded.len() > 1).then(|| Arc::new(decoded));
    return Ok(PinContent {
        image,
        text: None,
        frames,
    });
}

fn parse_color(text: &str) -> Option<[u8; 4]> {
    let text = text.trim();
    match text.to_ascii_lowercase().as_str() {
        "black" => return Some([0, 0, 0, 255]),
        "white" => return Some([255, 255, 255, 255]),
        "red" => return Some([255, 0, 0, 255]),
        "green" => return Some([0, 128, 0, 255]),
        "blue" => return Some([0, 0, 255, 255]),
        "transparent" => return Some([0, 0, 0, 0]),
        _ => {}
    }
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

pub fn print_image(image: &RgbaImage) -> Result<(), String> {
    return platform::print_png(&raster::encode_png(image)?);
}

pub fn remember_capture(image: Arc<RgbaImage>, cx: &mut App) {
    let preview = raster::create_render_image(&image::imageops::thumbnail(&*image, 220, 140));
    let state = cx.global_mut::<AppState>();
    let _ = state
        .history_persist
        .try_send(HistoryCommand::Save(image.clone()));
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

pub fn remember_capture_selection(bounds: Rect, selection: Rect, cx: &mut App) {
    cx.global_mut::<AppState>().last_capture_selection = Some((bounds, selection));
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

pub fn quick_save_image(image: Arc<RgbaImage>, capture: Option<Screenshot>, cx: &mut App) {
    let directory = match platform::get_quick_save_dir() {
        Ok(directory) => directory,
        Err(error) => {
            show_error(error, cx);
            return;
        }
    };
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = create_quick_save_path(&directory, id);
    let saved_image = image.clone();
    let save = cx
        .background_executor()
        .spawn(async move { write_image(&image, &path) });
    cx.spawn(async move |cx| {
        let result = save.await;
        let _ = cx.update(|cx| match result {
            Ok(()) => {
                if let Some(capture) = capture {
                    remember_capture_frame(capture, cx);
                }
                remember_capture(saved_image, cx);
            }
            Err(error) => show_error(error, cx),
        });
    })
    .detach();
}

fn create_quick_save_path(directory: &Path, mut id: u128) -> PathBuf {
    while directory.join(format!("Snipsy-{id}.png")).exists() {
        id += 1;
    }
    return directory.join(format!("Snipsy-{id}.png"));
}

fn write_image(image: &RgbaImage, path: &Path) -> Result<(), String> {
    let path = if path.extension().is_none() {
        path.with_extension("png")
    } else {
        path.to_path_buf()
    };
    let format = image::ImageFormat::from_path(&path).map_err(|error| error.to_string())?;
    if !matches!(
        format,
        image::ImageFormat::Png
            | image::ImageFormat::Jpeg
            | image::ImageFormat::WebP
            | image::ImageFormat::Bmp
            | image::ImageFormat::Tiff
            | image::ImageFormat::Tga
            | image::ImageFormat::Ico
            | image::ImageFormat::Gif
    ) {
        return Err("Choose a PNG, JPEG, WebP, BMP, TIFF, TGA, ICO, or GIF filename".into());
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

fn write_animation(frames: &[Frame], path: &Path) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))
        .map_err(|error| error.to_string())?;
    let mut encoder = GifEncoder::new(file.as_file_mut());
    encoder
        .set_repeat(Repeat::Infinite)
        .map_err(|error| error.to_string())?;
    encoder
        .encode_frames(frames.iter().cloned())
        .map_err(|error| error.to_string())?;
    drop(encoder);
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

fn clear_history_files(directory: &Path) -> Result<(), String> {
    for (_, path) in get_history_paths(directory)? {
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    return Ok(());
}

fn write_pin_backup(
    directory: &Path,
    pin: &pin::PinBackup,
    images: &mut HashMap<u128, (Arc<RgbaImage>, Option<Arc<Vec<Frame>>>)>,
) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    if images.get(&pin.id).is_none_or(|(image, frames)| {
        !Arc::ptr_eq(image, &pin.image)
            || match (frames, &pin.frames) {
                (Some(previous), Some(current)) => !Arc::ptr_eq(previous, current),
                (None, None) => false,
                _ => true,
            }
    }) {
        if let Some(frames) = &pin.frames {
            write_animation(frames, &directory.join(format!("{}.gif", pin.id)))?;
            let _ = std::fs::remove_file(directory.join(format!("{}.png", pin.id)));
        } else {
            write_image(&pin.image, &directory.join(format!("{}.png", pin.id)))?;
            let _ = std::fs::remove_file(directory.join(format!("{}.gif", pin.id)));
        }
        images.insert(pin.id, (pin.image.clone(), pin.frames.clone()));
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
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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
        u8::from(pin.frames.is_some()),
        pin.group,
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
    for extension in ["png", "gif", "meta"] {
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
    let fields = metadata.splitn(12, '\t').collect::<Vec<_>>();
    if !matches!(fields.len(), 10 | 11 | 12) {
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
    let animated = fields.get(10).is_some_and(|value| *value == "1");
    if fields
        .get(10)
        .is_some_and(|value| !matches!(*value, "0" | "1"))
    {
        return Err("invalid animation flag".into());
    }
    let group = fields
        .get(11)
        .map(|value| value.parse::<usize>().map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or(0);
    if group >= 64 {
        return Err("invalid image group".into());
    }
    let content =
        load_image_path(&directory.join(format!("{id}.{}", if animated { "gif" } else { "png" })))?;
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
        image: content.image,
        bounds: Bounds::new(
            point(px(numbers[0]), px(numbers[1])),
            size(px(numbers[2]), px(numbers[3])),
        ),
        opacity: numbers[4],
        original_size: size(px(numbers[5]), px(numbers[6])),
        thumbnail_size: (numbers[7] > 0.0).then_some(size(px(numbers[7]), px(numbers[8]))),
        text,
        frames: content.frames,
        group,
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
        HotkeySettings, StartupAction, clear_history_files, collect_animation_frames,
        create_quick_save_path, decode_startup_action, encode_startup_action, get_history_paths,
        insert_capture_history, load_groups, load_history, load_hotkey_settings, load_path,
        load_pin_backups, parse_color, parse_pin_backup, parse_startup_action, remove_pin_backup,
        should_restore_pin, validate_hotkey_settings, write_groups, write_history,
        write_hotkey_settings, write_image, write_pin_backup,
    };
    use crate::{editor::Screenshot, geometry::Rect, pin::PinBackup};
    use glam::{Vec2, vec2};
    use gpui::{Bounds, point, px, size};
    use image::{Delay, Frame, Rgba, RgbaImage};
    use std::path::PathBuf;
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

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
        for extension in ["webp", "bmp", "tiff", "tga", "ico", "gif"] {
            let path = directory.path().join(format!("capture.{extension}"));
            write_image(&image, &path).unwrap();
            let saved = image::open(path).unwrap();
            assert_eq!((saved.width(), saved.height()), (3, 2));
        }
    }

    #[test]
    fn clipboard_colors_accept_hex_integer_and_unit_rgb() {
        assert_eq!(parse_color("transparent"), Some([0, 0, 0, 0]));
        assert_eq!(parse_color("red"), Some([255, 0, 0, 255]));
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
        let content = load_path(&image_path, &fonts).unwrap();
        assert_eq!(content.image.dimensions(), (2, 3));
        assert_eq!(content.text, None);

        let file_path = directory.path().join("notes.txt");
        std::fs::write(&file_path, "notes").unwrap();
        let mut fonts = resvg::usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        let content = load_path(&file_path, &std::sync::Arc::new(fonts)).unwrap();
        assert_eq!(content.text.as_deref(), Some(file_path.to_str().unwrap()));
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
        clear_history_files(directory.path()).unwrap();
        assert!(get_history_paths(directory.path()).unwrap().is_empty());
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
            frames: None,
            group: 1,
        };
        let mut images = std::collections::HashMap::new();
        write_pin_backup(directory.path(), &pin, &mut images).unwrap();
        let loaded = load_pin_backups(directory.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, 42);
        assert_eq!(&*loaded[0].image, &*image);
        assert_eq!(loaded[0].text.as_deref(), Some("héllo"));
        assert_eq!(loaded[0].group, 1);
        assert_eq!(f32::from(loaded[0].bounds.origin.x), -20.0);
        assert_eq!(loaded[0].opacity, 0.6);
        assert!(parse_pin_backup(42, "0\t0\t0\t0\t2\t0\t0\t0\t0\t", directory.path()).is_err());
        remove_pin_backup(directory.path(), 42).unwrap();
        assert!(load_pin_backups(directory.path()).unwrap().is_empty());
    }

    #[test]
    fn animated_pins_decode_and_survive_backup() {
        let delay = Delay::from_numer_denom_ms(80, 1);
        let content = collect_animation_frames(
            [
                Ok(Frame::from_parts(
                    RgbaImage::from_pixel(2, 1, Rgba([1, 2, 3, 255])),
                    0,
                    0,
                    delay,
                )),
                Ok(Frame::from_parts(
                    RgbaImage::from_pixel(2, 1, Rgba([4, 5, 6, 255])),
                    0,
                    0,
                    delay,
                )),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(content.frames.as_ref().unwrap().len(), 2);
        let directory = tempfile::tempdir().unwrap();
        let pin = PinBackup {
            id: 43,
            image: content.image,
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(50.0))),
            opacity: 1.0,
            original_size: size(px(100.0), px(50.0)),
            thumbnail_size: None,
            text: None,
            frames: content.frames,
            group: 0,
        };
        write_pin_backup(
            directory.path(),
            &pin,
            &mut std::collections::HashMap::new(),
        )
        .unwrap();
        let loaded = load_pin_backups(directory.path()).unwrap();
        assert_eq!(loaded[0].frames.as_ref().unwrap().len(), 2);
        assert_eq!(loaded[0].frames.as_ref().unwrap()[1].delay(), delay);
        remove_pin_backup(directory.path(), 43).unwrap();
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
        assert_eq!(
            parse_startup_action(["whiteboard".into(), "--color".into(), "#01020380".into()])
                .unwrap(),
            StartupAction::Whiteboard([1, 2, 3, 128])
        );
        assert_eq!(
            parse_startup_action(["transparent-whiteboard".into()]).unwrap(),
            StartupAction::Whiteboard([0, 0, 0, 0])
        );
    }

    #[test]
    fn startup_commands_round_trip_over_instance_socket() {
        let actions = [
            StartupAction::Capture,
            StartupAction::Paste,
            StartupAction::Plain("héllo".into()),
            StartupAction::Files(vec![PathBuf::from(OsString::from_vec(vec![0xff, b'a']))]),
            StartupAction::History,
            StartupAction::Whiteboard([1, 2, 3, 4]),
            StartupAction::CreateGroup(Some("Work".into())),
            StartupAction::SwitchGroup(None),
            StartupAction::Groups,
            StartupAction::EmptyGroup,
        ];
        for action in actions {
            assert_eq!(
                decode_startup_action(&encode_startup_action(&action).unwrap()).unwrap(),
                action
            );
        }
        assert!(decode_startup_action(&[2, 0, 0, 0, 4, b'a']).is_err());
    }

    #[test]
    fn quick_save_paths_do_not_replace_existing_captures() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("Snipsy-7.png"), b"existing").unwrap();
        assert_eq!(
            create_quick_save_path(directory.path(), 7),
            directory.path().join("Snipsy-8.png")
        );
    }

    #[test]
    fn hotkey_settings_round_trip_and_reject_conflicts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hotkeys.conf");
        let settings = HotkeySettings::default();
        write_hotkey_settings(&path, settings).unwrap();
        assert_eq!(load_hotkey_settings(&path).unwrap(), settings);
        let mut duplicate = settings;
        duplicate.paste = duplicate.capture;
        assert!(validate_hotkey_settings(duplicate).is_err());
        let mut reserved = settings;
        reserved.paste = "super+shift+KeyL".parse().unwrap();
        assert!(validate_hotkey_settings(reserved).is_err());
    }

    #[test]
    fn image_groups_round_trip_names_and_active_group() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("groups.conf");
        let groups = vec!["Default".into(), "レビュー".into()];
        write_groups(&path, &groups, 1).unwrap();
        assert_eq!(load_groups(&path).unwrap(), (groups, 1));
        std::fs::write(&path, "active=2\ngroup=RGVmYXVsdA==\n").unwrap();
        assert!(load_groups(&path).is_err());
    }
}

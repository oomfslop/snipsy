use crate::app::cli::Command;
use anyhow::Result;
use gpui_kit::{App, Global};
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

pub struct Tray {
    _icon: TrayIcon,
    recording_item: MenuItem,
    stop_icon: Option<TrayIcon>,
}

impl Global for Tray {}

const RECORD_LABEL: &str = "Start Recording…";
const STOP_ICON_ID: &str = "stop-recording";

pub fn init(sender: async_channel::Sender<Command>, cx: &mut App) -> Result<()> {
    let create_item = |label| MenuItem::new(label, true, None);
    let recording_item = create_item(RECORD_LABEL);
    let items = [
        (create_item("New Screenshot"), Command::Snip),
        (recording_item.clone(), Command::Record),
        (create_item("Pin Clipboard"), Command::PASTE_CLIPBOARD),
        (
            create_item("Whiteboard"),
            Command::Whiteboard { color: [255; 4] },
        ),
        (create_item("Show or Hide Pins"), Command::TogglePins),
        (create_item("Preferences…"), Command::Preferences),
    ];
    let quit = create_item("Quit Snipsy");
    let menu = Menu::new();
    for (item, _) in &items {
        menu.append(item)?;
    }
    menu.append_items(&[&PredefinedMenuItem::separator(), &quit])?;
    let commands = items
        .map(|(item, command)| (item.id().clone(), command))
        .into_iter()
        .chain([(quit.id().clone(), Command::Quit)])
        .collect::<Vec<_>>();
    let menu_sender = sender.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some((_, command)) = commands.iter().find(|(id, _)| *id == event.id) {
            let _ = menu_sender.try_send(command.clone());
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            id,
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
            && id.0 == STOP_ICON_ID
        {
            let _ = sender.try_send(Command::Record);
        }
    }));
    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_title("✂︎")
        .build()?;
    cx.set_global(Tray {
        _icon: icon,
        recording_item,
        stop_icon: None,
    });
    return Ok(());
}

pub fn set_recording(recording: bool, cx: &mut App) {
    if !cx.has_global::<Tray>() {
        return;
    }
    let tray = cx.global_mut::<Tray>();
    tray.recording_item.set_text(if recording {
        "Stop Recording"
    } else {
        RECORD_LABEL
    });
    tray.stop_icon = None;
    if !recording {
        return;
    }
    match TrayIconBuilder::new()
        .with_id(STOP_ICON_ID)
        .with_title("⏹︎")
        .with_tooltip("Stop Recording")
        .build()
    {
        Ok(icon) => tray.stop_icon = Some(icon),
        Err(error) => super::show_error(error.into()),
    }
}

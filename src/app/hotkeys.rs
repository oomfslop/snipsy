use crate::app::{
    cli::Command,
    config::{self, Config},
};
use anyhow::{Result, ensure};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeySettings {
    #[serde(default)]
    pub capture: Option<HotKey>,
    #[serde(default)]
    pub paste: Option<HotKey>,
    #[serde(default)]
    pub click_through: Option<HotKey>,
    #[serde(default)]
    pub toggle_pins: Option<HotKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyKind {
    Capture,
    Paste,
    ClickThrough,
    TogglePins,
}

impl HotkeyKind {
    pub const ALL: [Self; 4] = [
        Self::Capture,
        Self::Paste,
        Self::ClickThrough,
        Self::TogglePins,
    ];

    pub fn get_label(self) -> &'static str {
        return match self {
            Self::Capture => "Capture",
            Self::Paste => "Pin clipboard",
            Self::ClickThrough => "Toggle click-through",
            Self::TogglePins => "Show or hide pins",
        };
    }

    fn get_command(self) -> Command {
        return match self {
            Self::Capture => Command::Snip,
            Self::Paste => Command::PASTE_CLIPBOARD,
            Self::ClickThrough => Command::ToggleClickThrough,
            Self::TogglePins => Command::TogglePins,
        };
    }
}

impl HotkeySettings {
    pub fn get(self, kind: HotkeyKind) -> Option<HotKey> {
        return match kind {
            HotkeyKind::Capture => self.capture,
            HotkeyKind::Paste => self.paste,
            HotkeyKind::ClickThrough => self.click_through,
            HotkeyKind::TogglePins => self.toggle_pins,
        };
    }

    fn get_mut(&mut self, kind: HotkeyKind) -> &mut Option<HotKey> {
        return match kind {
            HotkeyKind::Capture => &mut self.capture,
            HotkeyKind::Paste => &mut self.paste,
            HotkeyKind::ClickThrough => &mut self.click_through,
            HotkeyKind::TogglePins => &mut self.toggle_pins,
        };
    }

    fn get_bindings(self) -> Vec<(HotKey, Command)> {
        return HotkeyKind::ALL
            .into_iter()
            .filter_map(|kind| Some((self.get(kind)?, kind.get_command())))
            .collect();
    }

    pub fn validate(self) -> Result<()> {
        let hotkeys = HotkeyKind::ALL.map(|kind| self.get(kind));
        ensure!(
            !hotkeys.iter().enumerate().any(|(index, hotkey)| {
                hotkey.is_some() && hotkeys[index + 1..].contains(hotkey)
            }),
            "Global shortcuts must be unique"
        );
        return Ok(());
    }
}

pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    commands: Arc<Mutex<HashMap<u32, Command>>>,
}

impl Global for Hotkeys {}

pub fn init(sender: async_channel::Sender<Command>, cx: &mut App) -> Result<()> {
    let manager = GlobalHotKeyManager::new()?;
    let commands = Arc::new(Mutex::new(HashMap::new()));
    let event_commands = commands.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state != HotKeyState::Pressed {
            return;
        }
        if let Some(command) = event_commands.lock().unwrap().get(&event.id).cloned() {
            let _ = sender.try_send(command);
        }
    }));
    let mut hotkeys = Hotkeys {
        manager,
        registered: Vec::new(),
        commands,
    };
    let registered = hotkeys.apply(cx.global::<Config>().hotkeys);
    cx.set_global(hotkeys);
    return registered;
}

pub fn get_settings(cx: &App) -> Option<HotkeySettings> {
    return cx
        .has_global::<Hotkeys>()
        .then(|| cx.global::<Config>().hotkeys);
}

pub fn set_hotkey(kind: HotkeyKind, hotkey: Option<HotKey>, cx: &mut App) -> Result<()> {
    ensure!(
        cx.has_global::<Hotkeys>(),
        "Global shortcuts are unavailable"
    );
    let previous = cx.global::<Config>().hotkeys;
    let mut settings = previous;
    *settings.get_mut(kind) = hotkey;
    if settings == previous {
        return Ok(());
    }
    settings.validate()?;
    let result = cx
        .global_mut::<Hotkeys>()
        .apply(settings)
        .and_then(|()| config::update(cx, |config| config.hotkeys = settings));
    if let Err(error) = result {
        return Err(match cx.global_mut::<Hotkeys>().apply(previous) {
            Ok(()) => error,
            Err(rollback) => error.context(format!("{rollback:#}")),
        });
    }
    return Ok(());
}

impl Hotkeys {
    fn apply(&mut self, settings: HotkeySettings) -> Result<()> {
        let bindings = settings.get_bindings();
        let removed = self
            .registered
            .iter()
            .copied()
            .filter(|hotkey| !bindings.iter().any(|(next, _)| next == hotkey))
            .collect::<Vec<_>>();
        let mut errors = Vec::new();
        for hotkey in removed {
            match self.manager.unregister(hotkey) {
                Ok(()) => self.registered.retain(|registered| *registered != hotkey),
                Err(error) => errors.push(format!("{hotkey}: {error}")),
            }
        }
        for (hotkey, _) in &bindings {
            if self.registered.contains(hotkey) {
                continue;
            }
            match self.manager.register(*hotkey) {
                Ok(()) => self.registered.push(*hotkey),
                Err(error) => errors.push(format!("{hotkey}: {error}")),
            }
        }
        *self.commands.lock().unwrap() = bindings
            .into_iter()
            .filter(|(hotkey, _)| self.registered.contains(hotkey))
            .map(|(hotkey, command)| (hotkey.id(), command))
            .collect();
        ensure!(
            errors.is_empty(),
            "Could not register shortcuts: {}",
            errors.join(", ")
        );
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::HotkeySettings;

    #[test]
    fn duplicate_shortcuts_are_rejected() {
        assert!(HotkeySettings::default().validate().is_ok());
        let capture = Some("super+shift+KeyA".parse().unwrap());
        let duplicate = HotkeySettings {
            capture,
            paste: capture,
            ..Default::default()
        };
        assert!(duplicate.validate().is_err());
    }
}

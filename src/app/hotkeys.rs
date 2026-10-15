use crate::app::{cli::Command, storage};
use anyhow::{Result, ensure};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    pub capture: HotKey,
    pub paste: HotKey,
    pub click_through: HotKey,
    pub toggle_pins: HotKey,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        let command_shift = Some(Modifiers::SUPER | Modifiers::SHIFT);
        return Self {
            capture: HotKey::new(command_shift, Code::KeyA),
            paste: HotKey::new(command_shift, Code::KeyP),
            click_through: HotKey::new(command_shift, Code::KeyX),
            toggle_pins: HotKey::new(Some(Modifiers::SHIFT), Code::F3),
        };
    }
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
    pub fn get(self, kind: HotkeyKind) -> HotKey {
        return match kind {
            HotkeyKind::Capture => self.capture,
            HotkeyKind::Paste => self.paste,
            HotkeyKind::ClickThrough => self.click_through,
            HotkeyKind::TogglePins => self.toggle_pins,
        };
    }

    fn get_mut(&mut self, kind: HotkeyKind) -> &mut HotKey {
        return match kind {
            HotkeyKind::Capture => &mut self.capture,
            HotkeyKind::Paste => &mut self.paste,
            HotkeyKind::ClickThrough => &mut self.click_through,
            HotkeyKind::TogglePins => &mut self.toggle_pins,
        };
    }

    fn get_bindings(self) -> Vec<(HotKey, Command)> {
        let mut bindings = HotkeyKind::ALL
            .map(|kind| (self.get(kind), kind.get_command()))
            .to_vec();
        let alias = get_capture_alias();
        if self.capture != alias {
            bindings.push((alias, Command::Snip));
        }
        return bindings;
    }

    fn validate(self) -> Result<()> {
        let hotkeys = HotkeyKind::ALL.map(|kind| self.get(kind));
        ensure!(
            !hotkeys
                .iter()
                .enumerate()
                .any(|(index, hotkey)| hotkeys[index + 1..].contains(hotkey)),
            "Global shortcuts must be unique"
        );
        ensure!(
            !hotkeys[1..].contains(&get_capture_alias()),
            "⌘⇧L is reserved for capture"
        );
        return Ok(());
    }
}

fn get_capture_alias() -> HotKey {
    return HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyL);
}

fn load_settings(path: &Path) -> Result<HotkeySettings> {
    let settings: HotkeySettings = storage::read_toml(path)?;
    settings.validate()?;
    return Ok(settings);
}

pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    settings: HotkeySettings,
    registered: Vec<HotKey>,
    commands: Arc<Mutex<HashMap<u32, Command>>>,
    path: PathBuf,
}

impl Global for Hotkeys {}

pub fn init(data_dir: &Path, sender: async_channel::Sender<Command>, cx: &mut App) -> Result<()> {
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
    let path = data_dir.join("hotkeys.toml");
    let loaded = load_settings(&path);
    let mut hotkeys = Hotkeys {
        manager,
        settings: HotkeySettings::default(),
        registered: Vec::new(),
        commands,
        path,
    };
    let registered = hotkeys.apply(loaded.as_ref().copied().unwrap_or_default());
    cx.set_global(hotkeys);
    let errors = [loaded.map(|_| ()), registered]
        .into_iter()
        .filter_map(|result| result.err().map(|error| format!("{error:#}")))
        .collect::<Vec<_>>();
    ensure!(errors.is_empty(), "{}", errors.join("\n"));
    return Ok(());
}

pub fn get_settings(cx: &App) -> Option<HotkeySettings> {
    return cx.try_global::<Hotkeys>().map(|hotkeys| hotkeys.settings);
}

pub fn set_hotkey(kind: HotkeyKind, hotkey: HotKey, cx: &mut App) -> Result<()> {
    ensure!(
        cx.has_global::<Hotkeys>(),
        "Global shortcuts are unavailable"
    );
    let hotkeys = cx.global_mut::<Hotkeys>();
    let mut settings = hotkeys.settings;
    *settings.get_mut(kind) = hotkey;
    if settings == hotkeys.settings {
        return Ok(());
    }
    settings.validate()?;
    let previous = hotkeys.settings;
    let result = hotkeys
        .apply(settings)
        .and_then(|()| storage::write_toml(&hotkeys.path, &settings));
    if let Err(error) = result {
        return Err(match hotkeys.apply(previous) {
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
        self.settings = settings;
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
    use super::{HotkeySettings, load_settings};
    use crate::app::storage;

    #[test]
    fn settings_round_trip_and_reject_conflicts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hotkeys.toml");
        assert_eq!(load_settings(&path).unwrap(), HotkeySettings::default());
        let settings = HotkeySettings {
            paste: "control+KeyV".parse().unwrap(),
            ..Default::default()
        };
        storage::write_toml(&path, &settings).unwrap();
        assert_eq!(load_settings(&path).unwrap(), settings);
        std::fs::write(&path, "paste = \"shift+KeyQ\"\n").unwrap();
        assert_eq!(
            load_settings(&path).unwrap().capture,
            HotkeySettings::default().capture
        );
        let mut duplicate = settings;
        duplicate.paste = duplicate.capture;
        assert!(duplicate.validate().is_err());
        let mut reserved = settings;
        reserved.paste = "super+shift+KeyL".parse().unwrap();
        assert!(reserved.validate().is_err());
    }
}

use crate::app::{
    cli::SnipOptions,
    hotkeys::HotkeySettings,
    recording::RecordingSettings,
    storage,
    toolbar_layout::{self, DEFAULT_ITEMS, ToolbarItem},
};
use anyhow::{Context as _, Result};
use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub toolbar: Vec<ToolbarItem>,
    pub pin_frame: bool,
    pub hotkeys: HotkeySettings,
    pub recording: RecordingSettings,
    pub custom_snip: SnipOptions,
    #[serde(skip)]
    path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        return Self {
            toolbar: DEFAULT_ITEMS.to_vec(),
            pin_frame: true,
            hotkeys: HotkeySettings::default(),
            recording: RecordingSettings::default(),
            custom_snip: SnipOptions::default(),
            path: PathBuf::new(),
        };
    }
}

impl Global for Config {}

fn load(path: &Path) -> Result<Config> {
    let config: Config = storage::read_toml(path)?;
    toolbar_layout::validate(&config.toolbar)?;
    config.hotkeys.validate()?;
    config.recording.validate()?;
    config.custom_snip.validate()?;
    return Ok(config);
}

pub fn init(data_dir: &Path, cx: &mut App) -> Result<()> {
    let path = data_dir.join("config.toml");
    let loaded = load(&path);
    cx.set_global(Config {
        path,
        ..loaded.as_ref().ok().cloned().unwrap_or_default()
    });
    return loaded.map(|_| ()).context("Could not load config.toml");
}

pub fn update(cx: &mut App, change: impl FnOnce(&mut Config)) -> Result<()> {
    let mut config = cx.global::<Config>().clone();
    change(&mut config);
    storage::write_toml(&config.path, &config)?;
    cx.set_global(config);
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::{Config, load};
    use crate::app::{
        storage,
        toolbar_layout::{DEFAULT_ITEMS, ToolbarItem},
    };

    #[test]
    fn config_round_trips_and_rejects_invalid_sections() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.toolbar, DEFAULT_ITEMS);
        assert_eq!(loaded.hotkeys, Default::default());

        let mut config = Config {
            toolbar: vec![
                ToolbarItem::Copy,
                ToolbarItem::Separator,
                ToolbarItem::Separator,
                ToolbarItem::Pen,
            ],
            ..Default::default()
        };
        config.hotkeys.capture = Some("super+shift+KeyA".parse().unwrap());
        storage::write_toml(&path, &config).unwrap();
        assert_eq!(load(&path).unwrap(), config);

        config.hotkeys.capture = None;
        storage::write_toml(&path, &config).unwrap();
        assert_eq!(load(&path).unwrap(), config);

        std::fs::write(&path, "[hotkeys]\ncapture = \"super+KeyA\"\n").unwrap();
        let partial = load(&path).unwrap();
        assert_eq!(partial.toolbar, DEFAULT_ITEMS);
        assert!(partial.hotkeys.capture.is_some());

        std::fs::write(&path, "toolbar = [\"pen\", \"pen\"]\n").unwrap();
        assert!(load(&path).is_err());
        std::fs::write(
            &path,
            "[hotkeys]\ncapture = \"super+KeyA\"\npaste = \"super+KeyA\"\n",
        )
        .unwrap();
        assert!(load(&path).is_err());
        std::fs::write(&path, "[recording]\nfps = 0\n").unwrap();
        assert!(load(&path).is_err());
        std::fs::write(&path, "[recording]\ncodec = \"hevc\"\n").unwrap();
        let recording = load(&path).unwrap().recording;
        assert_eq!(recording.codec, crate::app::recording::VideoCodec::Hevc);
        assert_eq!(recording.fps, 60);
    }
}

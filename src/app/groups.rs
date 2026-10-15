use crate::{app::storage, pin};
use anyhow::{Context as _, Result, ensure};
use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_GROUPS: usize = 64;
const MAX_NAME_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Groups {
    pub names: Vec<String>,
    pub active: usize,
    #[serde(skip)]
    path: PathBuf,
}

impl Default for Groups {
    fn default() -> Self {
        return Self {
            names: vec!["Default".into()],
            active: 0,
            path: PathBuf::new(),
        };
    }
}

impl Global for Groups {}

impl Groups {
    fn load(path: &Path) -> Result<Self> {
        let groups: Self = storage::read_toml(path)?;
        ensure!(
            !groups.names.is_empty()
                && groups.names.len() <= MAX_GROUPS
                && groups.active < groups.names.len()
                && groups.names.iter().enumerate().all(|(index, name)| {
                    !name.trim().is_empty()
                        && name.len() <= MAX_NAME_BYTES
                        && !groups.names[index + 1..].contains(name)
                }),
            "Invalid image groups"
        );
        return Ok(Self {
            path: path.to_path_buf(),
            ..groups
        });
    }

    fn save(&self) -> Result<()> {
        return storage::write_toml(&self.path, self);
    }

    fn get_default_name(&self) -> String {
        let mut index = self.names.len() + 1;
        loop {
            let name = format!("Group {index}");
            if !self.names.contains(&name) {
                return name;
            }
            index += 1;
        }
    }
}

pub fn init(data_dir: &Path, cx: &mut App) -> Result<()> {
    let path = data_dir.join("groups.toml");
    let loaded = Groups::load(&path);
    cx.set_global(Groups {
        path,
        ..loaded.as_ref().ok().cloned().unwrap_or_default()
    });
    return loaded.map(|_| ()).context("Could not load image groups");
}

pub fn get_active(cx: &App) -> usize {
    return cx.global::<Groups>().active;
}

fn update(cx: &mut App, change: impl FnOnce(&mut Groups) -> Result<()>) -> Result<()> {
    let mut groups = cx.global::<Groups>().clone();
    change(&mut groups)?;
    groups.save()?;
    cx.set_global(groups);
    pin::refresh_visibility(cx);
    return Ok(());
}

pub fn create(name: Option<String>, cx: &mut App) -> Result<()> {
    return update(cx, |groups| {
        ensure!(
            groups.names.len() < MAX_GROUPS,
            "Maximum image-group count reached"
        );
        let name = name.unwrap_or_else(|| groups.get_default_name());
        ensure!(
            !name.trim().is_empty()
                && name.len() <= MAX_NAME_BYTES
                && !groups.names.contains(&name),
            "Image-group names must be unique and 1–{MAX_NAME_BYTES} bytes"
        );
        groups.names.push(name);
        groups.active = groups.names.len() - 1;
        return Ok(());
    });
}

pub fn switch(index: usize, cx: &mut App) -> Result<()> {
    return update(cx, |groups| {
        ensure!(index < groups.names.len(), "Image group does not exist");
        groups.active = index;
        return Ok(());
    });
}

pub fn switch_by_name(name: Option<String>, cx: &mut App) -> Result<()> {
    let groups = cx.global::<Groups>();
    let index = match name {
        Some(name) => groups
            .names
            .iter()
            .position(|group| *group == name)
            .context("Image group does not exist")?,
        None => (groups.active + 1) % groups.names.len(),
    };
    return switch(index, cx);
}

/// Restored pins may reference groups created by a previous session.
pub fn ensure_count(count: usize, cx: &mut App) -> Result<()> {
    if cx.global::<Groups>().names.len() >= count {
        return Ok(());
    }
    return update(cx, |groups| {
        while groups.names.len() < count.min(MAX_GROUPS) {
            let name = groups.get_default_name();
            groups.names.push(name);
        }
        return Ok(());
    });
}

#[cfg(test)]
mod tests {
    use super::Groups;
    use crate::app::storage;

    #[test]
    fn default_names_skip_existing_custom_names() {
        let groups = Groups {
            names: vec!["Default".into(), "Group 3".into(), "Group 4".into()],
            ..Default::default()
        };
        assert_eq!(groups.get_default_name(), "Group 5");
    }

    #[test]
    fn loading_groups_rejects_invalid_names() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("groups.toml");
        for names in [
            vec!["Default".into(), "Default".into()],
            vec![" ".into()],
            vec!["x".repeat(129)],
        ] {
            storage::write_toml(
                &path,
                &Groups {
                    names,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(Groups::load(&path).is_err());
        }
    }

    #[test]
    fn groups_round_trip_and_reject_invalid_active_index() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("groups.toml");
        assert_eq!(Groups::load(&path).unwrap().names, vec!["Default"]);
        let groups = Groups {
            names: vec!["Default".into(), "レビュー".into()],
            active: 1,
            path: path.clone(),
        };
        storage::write_toml(&path, &groups).unwrap();
        assert_eq!(Groups::load(&path).unwrap(), groups);
        std::fs::write(&path, "active = 2\nnames = [\"Default\"]\n").unwrap();
        assert!(Groups::load(&path).is_err());
    }
}

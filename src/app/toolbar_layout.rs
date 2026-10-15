use crate::app::storage;
use anyhow::{Context as _, Result, ensure};
use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolbarItem {
    Record,
    Select,
    Shape,
    Arrow,
    Pen,
    Highlighter,
    Text,
    Blur,
    Eraser,
    Undo,
    Redo,
    Layers,
    Search,
    Print,
    Save,
    Pin,
    Cancel,
    Copy,
    Separator,
}

impl ToolbarItem {
    pub const ALL: [Self; 19] = [
        Self::Record,
        Self::Select,
        Self::Shape,
        Self::Arrow,
        Self::Pen,
        Self::Highlighter,
        Self::Text,
        Self::Blur,
        Self::Eraser,
        Self::Undo,
        Self::Redo,
        Self::Layers,
        Self::Search,
        Self::Print,
        Self::Save,
        Self::Pin,
        Self::Cancel,
        Self::Copy,
        Self::Separator,
    ];
}

const DEFAULT_ITEMS: [ToolbarItem; 21] = [
    ToolbarItem::Record,
    ToolbarItem::Separator,
    ToolbarItem::Select,
    ToolbarItem::Shape,
    ToolbarItem::Arrow,
    ToolbarItem::Pen,
    ToolbarItem::Highlighter,
    ToolbarItem::Text,
    ToolbarItem::Blur,
    ToolbarItem::Eraser,
    ToolbarItem::Separator,
    ToolbarItem::Undo,
    ToolbarItem::Redo,
    ToolbarItem::Layers,
    ToolbarItem::Separator,
    ToolbarItem::Search,
    ToolbarItem::Print,
    ToolbarItem::Save,
    ToolbarItem::Pin,
    ToolbarItem::Cancel,
    ToolbarItem::Copy,
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolbarLayout {
    pub items: Vec<ToolbarItem>,
    #[serde(skip)]
    path: PathBuf,
}

impl Default for ToolbarLayout {
    fn default() -> Self {
        return Self {
            items: DEFAULT_ITEMS.to_vec(),
            path: PathBuf::new(),
        };
    }
}

impl Global for ToolbarLayout {}

impl ToolbarLayout {
    fn load(path: &Path) -> Result<Self> {
        let layout: Self = storage::read_toml(path)?;
        validate(&layout.items)?;
        return Ok(Self {
            path: path.to_path_buf(),
            ..layout
        });
    }

    pub fn get_is_default(&self) -> bool {
        return self.items == DEFAULT_ITEMS;
    }
}

fn validate(items: &[ToolbarItem]) -> Result<()> {
    ensure!(
        items.iter().enumerate().all(|(index, item)| {
            *item == ToolbarItem::Separator || !items[index + 1..].contains(item)
        }),
        "Toolbar items must be unique"
    );
    return Ok(());
}

pub fn init(data_dir: &Path, cx: &mut App) -> Result<()> {
    let path = data_dir.join("toolbar.toml");
    let loaded = ToolbarLayout::load(&path);
    cx.set_global(ToolbarLayout {
        path,
        ..loaded.as_ref().ok().cloned().unwrap_or_default()
    });
    return loaded
        .map(|_| ())
        .context("Could not load the toolbar layout");
}

pub fn set_items(items: Vec<ToolbarItem>, cx: &mut App) -> Result<()> {
    validate(&items)?;
    let layout = ToolbarLayout {
        items,
        path: cx.global::<ToolbarLayout>().path.clone(),
    };
    storage::write_toml(&layout.path, &layout)?;
    cx.set_global(layout);
    return Ok(());
}

pub fn reset(cx: &mut App) -> Result<()> {
    return set_items(DEFAULT_ITEMS.to_vec(), cx);
}

/// Drops separators that would sit at either end or next to another separator once hidden
/// items are removed.
pub fn get_visible_items(
    items: &[ToolbarItem],
    is_hidden: impl Fn(ToolbarItem) -> bool,
) -> Vec<ToolbarItem> {
    let mut visible = Vec::new();
    for item in items.iter().copied().filter(|item| !is_hidden(*item)) {
        if item == ToolbarItem::Separator
            && visible
                .last()
                .is_none_or(|last| *last == ToolbarItem::Separator)
        {
            continue;
        }
        visible.push(item);
    }
    if visible.last() == Some(&ToolbarItem::Separator) {
        visible.pop();
    }
    return visible;
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_ITEMS, ToolbarItem, ToolbarLayout, get_visible_items};
    use crate::app::storage;

    #[test]
    fn layouts_round_trip_and_reject_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("toolbar.toml");
        assert_eq!(ToolbarLayout::load(&path).unwrap().items, DEFAULT_ITEMS);
        let layout = ToolbarLayout {
            items: vec![
                ToolbarItem::Copy,
                ToolbarItem::Separator,
                ToolbarItem::Separator,
                ToolbarItem::Pen,
            ],
            path: path.clone(),
        };
        storage::write_toml(&path, &layout).unwrap();
        assert_eq!(ToolbarLayout::load(&path).unwrap(), layout);
        std::fs::write(&path, "items = [\"pen\", \"pen\"]\n").unwrap();
        assert!(ToolbarLayout::load(&path).is_err());
    }

    #[test]
    fn hidden_items_do_not_leave_stray_separators() {
        use ToolbarItem::*;
        assert_eq!(
            get_visible_items(&[Record, Separator, Pen, Separator], |item| item == Record),
            [Pen]
        );
        assert_eq!(
            get_visible_items(&[Pen, Separator, Record, Separator, Copy], |item| {
                item == Record
            }),
            [Pen, Separator, Copy]
        );
        assert_eq!(
            get_visible_items(&DEFAULT_ITEMS, |_| false),
            DEFAULT_ITEMS.to_vec()
        );
    }
}
